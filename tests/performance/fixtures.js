/**
 * Compiler-built fixture for serial module residency benchmarks.
 *
 * `route` lists chapter module ids in visit order, ending with the first module
 * again. `steps` alternates the driver's reusable lead-in (`kind: 'lead-in'`)
 * and each chapter dialogue (`kind: 'chapter'`). Advance the browser with
 * Space after each ready dialogue; a chapter function returns to the driver,
 * which shows its lead-in before making the next direct module call. This
 * leaves completed chapter calls off the stack so their packages can be
 * evicted. The lead-in is repeated and has one stable text id.
 */
import fs from 'node:fs/promises';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { gzipSync } from 'node:zlib';
import { promisify } from 'node:util';
import { execFile, spawn } from 'node:child_process';

const run = promisify(execFile);
const driverTextLocalId = 'lead';
const capacityPaddingBytes = 640 * 1024;
const mib = 1024 * 1024;

function chapterId(index) {
  return `ch${String(index + 1).padStart(2, '0')}`;
}

function opIncrement() {
  return {
    id: 'enter.increment',
    operation: {
      type: 'assign',
      target: 'visit_count',
      value: {
        type: 'binary',
        op: 'add',
        left: { type: 'var', name: 'visit_count' },
        right: { type: 'const', value: { type: 'i32', value: 1 } },
      },
    },
  };
}

function driverFunction(route) {
  const blocks = {
    end: { ops: [], terminator: { type: 'end', outcome: 'scale-route-complete' } },
  };
  route.forEach((id, index) => {
    const lead = `lead_${index}`;
    const wait = `wait_${index}`;
    const call = `call_${index}`;
    const after = index + 1 < route.length ? `lead_${index + 1}` : 'end';
    blocks[lead] = {
      ops: [],
      terminator: { type: 'activate', cue: driverTextLocalId, next: wait },
    };
    blocks[wait] = {
      ops: [],
      terminator: {
        type: 'await',
        conditions: [{ task: driverTextLocalId, milestone: { type: 'finished' } }],
        next: call,
        on_cancelled: 'end',
        on_failed: 'end',
      },
    };
    // The call follows the await directly so the runtime can prefetch this
    // chapter while the driver lead-in is on screen.
    blocks[call] = {
      ops: [],
      terminator: {
        type: 'call',
        function: `${id}.start`,
        args: {},
        next: after,
      },
    };
  });
  return { params: {}, locals: {}, returns: null, entry: 'lead_0', blocks };
}

function chapterFunction() {
  return {
    params: {},
    locals: {},
    returns: null,
    entry: 'enter',
    blocks: {
      enter: {
        ops: [opIncrement()],
        terminator: { type: 'activate', cue: 'line', next: 'wait_line' },
      },
      wait_line: {
        ops: [],
        terminator: {
          type: 'await',
          conditions: [{ task: 'line', milestone: { type: 'finished' } }],
          next: 'return',
          on_cancelled: 'return',
          on_failed: 'return',
        },
      },
      return: { ops: [], terminator: { type: 'return', value: null } },
    },
  };
}

function chapterProgram() {
  return {
    fragment_format: 1,
    functions: { visit: chapterFunction() },
    scenes: {
      station: [{ id: 'background', asset: 'bg.station', x: 0, y: 0, width: 1280, height: 720 }],
    },
    cues: {
      line: {
        effects: [
          {
            id: 'stage',
            scope: 'scene',
            effect: { type: 'stage_present', scene: 'station', duration_us: '0' },
          },
          {
            id: 'line',
            scope: 'interaction',
            effect: { type: 'dialogue', text: 'line', speaker: '', reveal_us: '0' },
          },
        ],
      },
    },
  };
}

function driverProgram(route) {
  return {
    fragment_format: 1,
    functions: { main: driverFunction(route) },
    scenes: {
      station: [{ id: 'background', asset: 'bg.station', x: 0, y: 0, width: 1280, height: 720 }],
    },
    cues: {
      lead: {
        effects: [{
          id: 'lead',
          scope: 'interaction',
          effect: { type: 'dialogue', text: driverTextLocalId, speaker: '', reveal_us: '0' },
        }],
      },
    },
  };
}

function moduleToml(id, startFunction) {
  return `id = "${id}"
module_format = 1
sources = ["story.nir.json"]
text_contracts = "texts/contracts.json"
text_revisions = "texts/revisions.json"

[exports]
start = "${startFunction}"

[text_bundles]
en = "texts/en.json"
zh-Hans = "texts/zh-Hans.json"
`;
}

function textSource(locale, kind, index) {
  const text = kind === 'lead'
    ? (locale === 'en' ? `Ready for chapter ${index + 1}?` : `第${index + 1}章 雨`)
    : (locale === 'en' ? `Chapter ${index + 1}: the road continues.` : `第${index + 1}章 雨`);
  return {
    source_revision: 1,
    contract_revision: 1,
    spans: [{ type: 'text', id: 'body', text, emphasis: false }],
  };
}

async function writeJson(file, value) {
  await fs.mkdir(path.dirname(file), { recursive: true });
  await fs.writeFile(file, `${JSON.stringify(value, null, 2)}\n`);
}

async function copyProject(project) {
  const example = path.resolve('examples/rain-letters');
  await fs.mkdir(path.dirname(project), { recursive: true });
  await fs.cp(example, project, {
    recursive: true,
    filter(source) {
      const relative = path.relative(example, source);
      return !relative.split(path.sep).some(part => ['dist', 'reports', '.nir', 'game.lock'].includes(part));
    },
  });
}

async function setInputs(project, moduleIds, driverId, prefetchContent) {
  const file = path.join(project, 'game.toml');
  const content = await fs.readFile(file, 'utf8');
  const replacement = `shared = ["content/shared/story.nir.json"]\nmodules = [${moduleIds.map(id => `"content/${id}/module.toml"`).join(', ')}]`;
  const updated = content
    .replace(/^modules\s*=\s*\[[^\n]*\]/m, replacement)
    .replace('title_scene = "station"', `title_scene = "${driverId}.station"`);
  if (updated === content) throw new Error('could not update scale fixture game.toml');
  await fs.writeFile(file, updated);

  const playerFile = path.join(project, 'config/player.toml');
  const player = await fs.readFile(playerFile, 'utf8');
  const configured = player.replace(
    /^prefetch_content\s*=\s*(?:true|false)\s*$/m,
    `prefetch_content = ${prefetchContent}`,
  );
  if (configured === player && !/^prefetch_content\s*=\s*(?:true|false)\s*$/m.test(player)) {
    throw new Error('could not set config/player.toml prefetch_content');
  }
  await fs.writeFile(playerFile, configured);
}

function stableJson(value) {
  if (Array.isArray(value)) return value.map(stableJson);
  if (value && typeof value === 'object') {
    return Object.fromEntries(Object.keys(value).sort().map(key => [key, stableJson(value[key])]));
  }
  return value;
}

function digest(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

async function patchCapacityObjects(web, manifest, executable, program, chapterIds) {
  const moduleIndexes = program.modules;
  for (const id of chapterIds) {
    const index = moduleIndexes[id];
    const oldHash = index.code;
    const oldDescriptor = manifest.objects[oldHash];
    if (!oldDescriptor) throw new Error(`compiled release is missing code object for ${id}`);
    const oldPath = path.join(web, oldDescriptor.path);
    const source = await fs.readFile(oldPath);
    const targetLength = Math.max(capacityPaddingBytes, source.length);
    if (targetLength === source.length) continue;
    const padded = Buffer.concat([source, Buffer.alloc(targetLength - source.length, 0x20)]);
    JSON.parse(padded.toString('utf8'));
    const newHash = digest(padded);
    const relative = `objects/${newHash}.json`;
    await fs.writeFile(path.join(web, relative), padded);
    await fs.writeFile(path.join(web, `${relative}.gz`), gzipSync(padded,{level:6}));
    index.code = newHash;
    manifest.objects[newHash] = { ...oldDescriptor, path: relative, bytes: padded.byteLength };
    delete manifest.objects[oldHash];
  }

  const oldRootHash = manifest.program;
  const oldRootDescriptor = manifest.objects[oldRootHash];
  const rootBytes = Buffer.from(JSON.stringify(executable));
  const rootHash = digest(rootBytes);
  const rootPath = `objects/${rootHash}.json`;
  await fs.writeFile(path.join(web, rootPath), rootBytes);
  const compressedRoot=gzipSync(rootBytes,{level:6});
  if(compressedRoot.length<rootBytes.length)await fs.writeFile(path.join(web, `${rootPath}.gz`),compressedRoot);
  manifest.program = rootHash;
  manifest.objects[rootHash] = { ...oldRootDescriptor, path: rootPath, bytes: rootBytes.byteLength };
  delete manifest.objects[oldRootHash];

  const releaseBytes = Buffer.from(JSON.stringify(manifest));
  const releaseHash = digest(releaseBytes);
  await fs.mkdir(path.join(web, 'releases'), { recursive: true });
  await fs.writeFile(path.join(web, `releases/${releaseHash}.json`), releaseBytes);
  await fs.writeFile(
    path.join(web, 'channels/stable.json'),
    JSON.stringify({ format: 1, release: releaseHash }),
  );
  return { release: releaseHash };
}

async function readRelease(web) {
  const channel = JSON.parse(await fs.readFile(path.join(web, 'channels/stable.json'), 'utf8'));
  const manifest = JSON.parse(await fs.readFile(path.join(web, `releases/${channel.release}.json`), 'utf8'));
  const rootBytes = await fs.readFile(path.join(web, manifest.objects[manifest.program].path));
  const executable = JSON.parse(rootBytes.toString('utf8'));
  return { channel, manifest, executable, program: executable.program };
}

async function validateCompiledRelease(cli, project, web) {
  // The CLI owns source check/build validation; the independent verifier also
  // re-hashes the final post-processed release objects below.
  await run(cli, ['-p', project, 'check', '--locked'], { maxBuffer: 8 * 1024 * 1024 });
  await run('python3', ['scripts/verify_release.py', web], { maxBuffer: 8 * 1024 * 1024 });
}

export async function buildScaleFixture({
  moduleCount = 3,
  prefetchContent = false,
  capacity = false,
  port = 4192,
} = {}) {
  if (!Number.isInteger(moduleCount) || moduleCount < 1) {
    throw new Error(`moduleCount must be a positive integer, got ${moduleCount}`);
  }
  if (capacity && moduleCount * capacityPaddingBytes <= 16 * mib) {
    throw new Error(`capacity fixture needs at least ${Math.floor(16 * mib / capacityPaddingBytes) + 1} modules at 640 KiB each`);
  }

  const cli = path.resolve('dist/novelc');
  await fs.mkdir(path.resolve('target/tmp'), { recursive: true });
  const temp = await fs.mkdtemp(path.resolve('target/tmp/nir-scale-'));
  const project = path.join(temp, 'story');
  const web = path.join(temp, 'web');
  let server;
  try {
    const ids = Array.from({ length: moduleCount }, (_, index) => chapterId(index));
    const route = [...ids, ids[0]];
    const chapters = ids.map((id, index) => ({ id, textId: `${id}.line`, index }));
    const driverId = 'driver';
    const moduleIds = [driverId, ...ids];
    const leadTextId = `${driverId}.${driverTextLocalId}`;
    await copyProject(project);
    await setInputs(project, moduleIds, driverId, prefetchContent);
    await writeJson(path.join(project, 'content/shared/story.nir.json'), {
      fragment_format: 1,
      variables: { visit_count: { type: 'i32', value: 0 } },
    });

    {
      const base = path.join(project, 'content', driverId);
      await fs.mkdir(base, { recursive: true });
      await fs.writeFile(path.join(base, 'module.toml'), moduleToml(driverId, 'main'));
      await writeJson(path.join(base, 'story.nir.json'), driverProgram(route));
      await writeJson(path.join(base, 'texts/contracts.json'), {
        [driverTextLocalId]: { source_revision: 1, contract_revision: 1, meaning_revision: 1, gates: [], params: {} },
      });
      await writeJson(path.join(base, 'texts/revisions.json'), {
        format: 1,
        source_locale: 'zh-Hans',
        texts: {},
      });
      for (const locale of ['zh-Hans', 'en']) {
        await writeJson(path.join(base, `texts/${locale}.json`), {
          [driverTextLocalId]: textSource(locale, 'lead', 0),
        });
      }
    }

    for (const [index, id] of ids.entries()) {
      const base = path.join(project, 'content', id);
      await fs.mkdir(base, { recursive: true });
      await fs.writeFile(path.join(base, 'module.toml'), moduleToml(id, 'visit'));
      await writeJson(path.join(base, 'story.nir.json'), chapterProgram());
      const contracts = {
        line: { source_revision: 1, contract_revision: 1, meaning_revision: 1, gates: [], params: {} },
      };
      await writeJson(path.join(base, 'texts/contracts.json'), contracts);
      await writeJson(path.join(base, 'texts/revisions.json'), {
        format: 1,
        source_locale: 'zh-Hans',
        texts: {},
      });
      for (const locale of ['zh-Hans', 'en']) {
        await writeJson(path.join(base, `texts/${locale}.json`), {
          line: textSource(locale, 'chapter', index),
        });
      }
    }

    // Track/review each authored text once, keeping CLI revision work linear
    // in the chapter count.
    const authoredTextIds = chapters.map(chapter => chapter.textId);
    authoredTextIds.unshift(leadTextId);
    for (const id of authoredTextIds) {
      await run(cli, ['-p', project, 'text', 'update', '--id', id, '--meaning', 'preserve'], { maxBuffer: 8 * 1024 * 1024 });
      await run(cli, ['-p', project, 'text', 'review', '--id', id, '--locale', 'en'], { maxBuffer: 8 * 1024 * 1024 });
    }
    await run(cli, ['-p', project, 'resolve'], { maxBuffer: 8 * 1024 * 1024 });
    await run(cli, ['-p', project, 'check', '--locked'], { maxBuffer: 8 * 1024 * 1024 });
    await run(cli, ['-p', project, 'build', '--locked', '--out', web], { maxBuffer: 16 * 1024 * 1024 });

    let release = await readRelease(web);
    if (capacity) {
      await patchCapacityObjects(web, release.manifest, release.executable, release.program, ids);
      release = await readRelease(web);
    }
    await validateCompiledRelease(cli, project, web);

    const moduleContentHashes = new Set(Object.values(release.program.modules).flatMap(index => [
      index.code,
      index.static_content,
      ...Object.values(index.locales || {}),
    ]));
    const totalContentBytes = [...moduleContentHashes].reduce((total, hash) =>
      total + release.manifest.objects[hash].bytes, 0);
    if (capacity && totalContentBytes <= 16 * mib) {
      throw new Error(`padded module payloads total only ${totalContentBytes} bytes`);
    }
    if (capacity) {
      for (const id of ids) {
        const index = release.program.modules[id];
        const locale = index.locales['zh-Hans'];
        const closureBytes = [index.code, index.static_content, locale]
          .filter(Boolean)
          .reduce((total, hash) => total + release.manifest.objects[hash].bytes, 0);
        if (closureBytes >= 2 * mib) {
          throw new Error(`${id} chapter fetch closure is ${closureBytes} bytes (must be under 2 MiB)`);
        }
      }
    }

    const semantic = JSON.parse(JSON.stringify(release.program));
    // The compiler's source revision includes resolved player settings, so the
    // only expected differences between these builds are that revision and
    // the operational prefetch switch itself. Normalize both away to digest
    // the actual route and module semantics.
    delete semantic.revision;
    if (semantic.player) semantic.player.prefetch_content = false;
    const semanticDigest = digest(Buffer.from(JSON.stringify(stableJson(semantic))));

    server = spawn(cli, ['serve', web, '--port', String(port)], { stdio: 'ignore' });
    const origin = `http://127.0.0.1:${port}`;
    try {
      let ready = false;
      for (let attempt = 0; attempt < 100; attempt++) {
        try {
          const response = await fetch(`${origin}/channels/stable.json`);
          if (response.ok) {
            const servedChannel = await response.json();
            if (servedChannel.release !== release.channel.release) {
              throw new Error('scale fixture server returned a different release channel');
            }
            ready = true;
            break;
          }
        } catch (error) {
          if (error.message.includes('different release')) throw error;
        }
        await new Promise(resolve => setTimeout(resolve, 100));
      }
      if (!ready) throw new Error(`scale fixture server failed to start at ${origin}`);
    } catch (error) {
      if (server.exitCode === null && server.signalCode === null) server.kill('SIGTERM');
      throw error;
    }

    const routeSteps = route.flatMap((id, routeIndex) => [
      { textId: leadTextId, moduleId: driverId, kind: 'lead-in', routeIndex },
      { textId: `${id}.line`, moduleId: id, kind: 'chapter', routeIndex },
    ]);
    return {
      cli,
      project,
      temp,
      web,
      origin,
      server,
      ...release,
      chapters: chapters.map(({ id, textId }) => ({ id, textId })),
      route,
      steps: routeSteps,
      moduleCount,
      prefetchContent,
      capacity,
      totalContentBytes,
      semanticDigest,
    };
  } catch (error) {
    if (server && server.exitCode === null && server.signalCode === null) {
      server.kill('SIGTERM');
    }
    await fs.rm(temp, { recursive: true, force: true });
    throw error;
  }
}

export async function closeScaleFixture(fixture) {
  if (fixture.server?.exitCode === null && fixture.server?.signalCode === null) {
    const exited = new Promise(resolve => fixture.server.once('exit', resolve));
    fixture.server.kill('SIGTERM');
    await exited;
  }
  await fs.rm(fixture.temp, { recursive: true, force: true });
}
