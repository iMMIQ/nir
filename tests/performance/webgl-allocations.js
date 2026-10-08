// Optional browser/Worker diagnostic. Serialize this function into the target
// before creating its renderer; keep the returned API in the test harness.
// Resource identities are weak: the probe never keeps a GL object alive.
export function installWebGlAllocationProbe() {
  if (globalThis.__nirGlAllocations) throw new Error('GL allocation probe already installed');
  const prototype = globalThis.WebGL2RenderingContext?.prototype;
  if (!prototype) return { supported: false };
  const objects = new WeakMap(), contexts = new WeakMap(), live = new Map(), originals = [];
  const unsupported = new Map();
  let next = 0, peakBytes = 0, storageCalls = 0, deleted = 0, overflow = false, closed = false;
  const formats = new Map([
    [0x8229, 1], [0x8232, 1], [0x8231, 1], // R8, R8UI, R8I
    [0x822b, 2], [0x822d, 2], [0x8234, 2], [0x8233, 2], // RG8, R16F, R16UI/I
    [0x8051, 3], [0x8c41, 3], // RGB8, SRGB8
    [0x8058, 4], [0x8c43, 4], [0x822e, 4], [0x822f, 4], [0x81a6, 4], [0x88f0, 4],
    [0x881a, 8], [0x8230, 8], [0x8cad, 8], // RGBA16F, RG32F, DEPTH32F_STENCIL8
    [0x8814, 16], // RGBA32F
  ]);
  const note = name => unsupported.set(name, (unsupported.get(name) || 0) + 1);
  const identify = (object, kind) => {
    if (!object) return null;
    let id = objects.get(object);
    if (!id) {
      id = ++next; objects.set(object, id);
      if (live.size < 4096) live.set(id, { id, kind, levels: new Map(), bytes: 0 });
      else overflow = true;
    }
    return id;
  };
  const state = gl => {
    let value = contexts.get(gl);
    if (!value) {
      value = { unit: gl.getParameter(gl.ACTIVE_TEXTURE), textures: new Map(), buffers: new Map() };
      contexts.set(gl, value);
    }
    return value;
  };
  const total = () => [...live.values()].reduce((sum, row) => sum + row.bytes, 0);
  const account = (id, levels) => {
    storageCalls++;
    const row = live.get(id);
    if (!row) { note('untracked-storage'); return; }
    row.levels = levels; row.bytes = [...levels.values()].reduce((sum, level) => sum + level.bytes, 0);
    peakBytes = Math.max(peakBytes, total());
  };
  function wrap(name, observe) {
    const original = prototype[name];
    if (typeof original !== 'function') return;
    const wrapped = function (...args) {
      const result = Reflect.apply(original, this, args);
      if (!closed) observe(this, args, result);
      return result;
    };
    prototype[name] = wrapped; originals.push({ name, original, wrapped });
  }
  for (const [suffix, kind] of [['Texture', 'texture'], ['Buffer', 'buffer']]) {
    wrap('create' + suffix, (_gl, _args, result) => identify(result, kind));
    wrap('delete' + suffix, (_gl, [object]) => { const id = objects.get(object); if (live.delete(id)) deleted++; });
  }
  wrap('activeTexture', (gl, [unit]) => { state(gl).unit = unit; });
  wrap('bindTexture', (gl, [target, object]) => { const s = state(gl); s.textures.set(s.unit + ':' + target, identify(object, 'texture')); });
  wrap('bindBuffer', (gl, [target, object]) => { state(gl).buffers.set(target, identify(object, 'buffer')); });
  // ELEMENT_ARRAY_BUFFER bindings belong to a VAO. A numeric identity from
  // getParameter avoids retaining the VAO or its buffer in probe state.
  wrap('bindVertexArray', gl => { state(gl).buffers.set(gl.ELEMENT_ARRAY_BUFFER, identify(gl.getParameter(gl.ELEMENT_ARRAY_BUFFER_BINDING), 'buffer')); });
  wrap('bufferData', (gl, [target, data, _usage, offset = 0, length = 0]) => {
    let bytes;
    if (typeof data === 'number') bytes = data;
    else if (data instanceof ArrayBuffer) bytes = data.byteLength;
    else if (ArrayBuffer.isView(data)) {
      const elementBytes = data.BYTES_PER_ELEMENT || 1;
      bytes = length ? length * elementBytes : data.byteLength - offset * elementBytes;
    } else { note('bufferData-source'); return; }
    if (!Number.isSafeInteger(bytes) || bytes < 0) { note('bufferData-size'); return; }
    account(state(gl).buffers.get(target), new Map([[0, { bytes }]]));
  });
  const texture = (gl, target) => { const s = state(gl); return s.textures.get(s.unit + ':' + target); };
  wrap('texStorage2D', (gl, [target, count, format, width, height]) => {
    const bpp = formats.get(format);
    if (target !== gl.TEXTURE_2D || !bpp) { note('texStorage2D-format-or-target'); return; }
    const levels = new Map();
    for (let level = 0; level < count; level++) {
      const w = Math.max(1, Math.floor(width / 2 ** level)), h = Math.max(1, Math.floor(height / 2 ** level));
      levels.set(level, { width: w, height: h, format, bytes: w * h * bpp });
    }
    account(texture(gl, target), levels);
  });
  wrap('texImage2D', (gl, args) => {
    const [target, level, format, width, height] = args;
    const bpp = formats.get(format);
    if (target !== gl.TEXTURE_2D || args.length < 9 || !bpp) { note('texImage2D-overload-format-or-target'); return; }
    const id = texture(gl, target), levels = new Map(live.get(id)?.levels || []);
    levels.set(level, { width, height, format, bytes: width * height * bpp }); account(id, levels);
  });
  for (const name of ['texStorage3D', 'texImage3D', 'compressedTexImage2D', 'compressedTexImage3D', 'renderbufferStorage', 'renderbufferStorageMultisample'])
    wrap(name, () => note(name));
  const snapshot = () => ({
    scope: 'Issued WebGL2 texture/buffer storage, including glyph atlas calls; excludes swapchain, renderbuffers, driver memory and physical GPU usage. Invalid GL calls are not identified by this observer.',
    supported: true, closed, completeObservedStorage: !overflow && unsupported.size === 0,
    requestedLogicalBytes: total(), peakRequestedLogicalBytes: peakBytes, storageCalls, deleted,
    textures: [...live.values()].filter(row => row.kind === 'texture').length,
    buffers: [...live.values()].filter(row => row.kind === 'buffer').length,
    unsupported: Object.fromEntries(unsupported), overflow,
    resources: [...live.values()].map(({ id, kind, bytes, levels }) => ({ id, kind, bytes, levels: [...levels.entries()] })),
  });
  globalThis.__nirGlAllocations = {
    snapshot,
    close() { closed = true; for (const { name, original, wrapped } of originals) if (prototype[name] === wrapped) prototype[name] = original; live.clear(); unsupported.clear(); delete globalThis.__nirGlAllocations; },
  };
  return { supported: true, beforeEngineCreation: !globalThis.__nirWorker && !globalThis.__nir, scope: snapshot().scope };
}
