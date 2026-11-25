struct VertexIn {
    @location(0) pos: vec2f,
    @location(1) uv: vec2f,
    @location(2) color: vec4f,
    @location(3) rect_params: vec4f,
    @location(4) border_color: vec4f,
}

struct VertexOut {
    @builtin(position) pos: vec4f,
    @location(0) uv: vec2f,
    @location(1) color: vec4f,
    @location(2) @interpolate(flat) rect_params: vec4f,
    @location(3) @interpolate(flat) border_color: vec4f,
}

@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;

@vertex
fn vs(v: VertexIn) -> VertexOut {
    var o: VertexOut;
    o.pos = vec4f(v.pos, 0.0, 1.0);
    o.uv = v.uv;
    o.color = v.color;
    o.rect_params = v.rect_params;
    o.border_color = v.border_color;
    return o;
}

@fragment
fn fs(v: VertexOut) -> @location(0) vec4f {
    // Always evaluate textureSample & screen-space derivatives in uniform control flow!
    let base_sample = textureSample(image, image_sampler, v.uv);
    let size = max(v.rect_params.xy, vec2f(1.0));
    let local_p = v.uv * size;
    let px_w = max(length(dpdx(local_p)), length(dpdy(local_p)));
    let aa = clamp(px_w * 0.72, 0.25, 1.25);

    var tex_col = base_sample;

    // Optional CAS-lite contrast-adaptive edge sharpening for upscaled classic CGs
    // (sentinel: border_color.a < -0.5, strength in border_color.r).
    if v.border_color.a < -0.5 {
        let strength = clamp(v.border_color.r, 0.0, 1.0);
        let dims = vec2f(textureDimensions(image));
        let inv = vec2f(1.0) / max(dims, vec2f(1.0));
        let n = textureSampleLevel(image, image_sampler, v.uv + vec2f(0.0, -inv.y), 0.0);
        let s = textureSampleLevel(image, image_sampler, v.uv + vec2f(0.0, inv.y), 0.0);
        let w = textureSampleLevel(image, image_sampler, v.uv + vec2f(-inv.x, 0.0), 0.0);
        let e = textureSampleLevel(image, image_sampler, v.uv + vec2f(inv.x, 0.0), 0.0);
        // Only sharpen opaque interior texels so transparent sprite edges never develop halos.
        let min_a = min(base_sample.a, min(min(n.a, s.a), min(w.a, e.a)));
        if min_a > 0.96 {
            let mn = min(base_sample.rgb, min(min(n.rgb, s.rgb), min(w.rgb, e.rgb)));
            let mx = max(base_sample.rgb, max(max(n.rgb, s.rgb), max(w.rgb, e.rgb)));
            let amp = sqrt(clamp(min(mn, vec3f(1.0) - mx) / max(mx, vec3f(0.0001)), vec3f(0.0), vec3f(1.0)));
            let peak = -1.0 / mix(10.0, 5.5, strength);
            let w_rgb = amp * peak;
            let sharp = (base_sample.rgb + (n.rgb + s.rgb + w.rgb + e.rgb) * w_rgb) / (vec3f(1.0) + 4.0 * w_rgb);
            // Clamp to local neighborhood [mn, mx] -> strictly zero ringing / zero white halo!
            tex_col = vec4f(clamp(sharp, mn, mx), base_sample.a);
        }
    }

    var col = tex_col * v.color;

    // Single-pass DPI-adaptive SDF rounded rectangle + crisp non-overlapping border stroke!
    let r = v.rect_params.z;
    let bw = v.rect_params.w;
    if r > 0.0 || bw > 0.0 {
        let cr = min(max(r, 0.0), min(size.x, size.y) * 0.5);
        let half = size * 0.5;
        let p = abs(local_p - half) - (half - vec2f(cr));
        let d = length(max(p, vec2f(0.0))) + min(max(p.x, p.y), 0.0) - cr;
        let outer = 1.0 - smoothstep(-aa, aa, d);
        if bw > 0.0 && v.border_color.a > 0.0 {
            let inner = 1.0 - smoothstep(-aa, aa, d + bw);
            let stroke = max(outer - inner, 0.0);
            col = col * inner + v.border_color * stroke;
        } else {
            col = col * outer;
        }
    }
    return col;
}

@fragment
fn fs_blur(v: VertexOut) -> @location(0) vec4f {
    // 4-tap Dual/Kawase blur kernel (leverages hardware bilinear filtering = 16-tap equivalent!)
    let dims = vec2f(textureDimensions(image));
    let step = v.rect_params.xy / max(dims, vec2f(1.0));
    let c0 = textureSampleLevel(image, image_sampler, clamp(v.uv + vec2f(-step.x, -step.y), vec2f(0.002), vec2f(0.998)), 0.0);
    let c1 = textureSampleLevel(image, image_sampler, clamp(v.uv + vec2f(step.x, -step.y), vec2f(0.002), vec2f(0.998)), 0.0);
    let c2 = textureSampleLevel(image, image_sampler, clamp(v.uv + vec2f(-step.x, step.y), vec2f(0.002), vec2f(0.998)), 0.0);
    let c3 = textureSampleLevel(image, image_sampler, clamp(v.uv + vec2f(step.x, step.y), vec2f(0.002), vec2f(0.998)), 0.0);
    return (c0 + c1 + c2 + c3) * 0.25;
}

@group(1) @binding(0) var target_image: texture_2d<f32>;
@group(1) @binding(1) var target_sampler: sampler;
@group(2) @binding(0) var mask_image: texture_2d<f32>;
@group(2) @binding(1) var mask_sampler: sampler;

@fragment
fn fs_mix(v: VertexOut) -> @location(0) vec4f {
    let source = textureSample(image, image_sampler, v.uv);
    let target_color = textureSample(target_image, target_sampler, v.uv);
    let progress = v.color.a;
    var coverage = progress;
    if v.color.r >= 1.0 {
        var coordinate = v.uv.x;
        if v.color.r == 2.0 { coordinate = 1.0 - v.uv.x; }
        if v.color.r == 3.0 { coordinate = v.uv.y; }
        if v.color.r == 4.0 { coordinate = 1.0 - v.uv.y; }
        if v.color.r == 5.0 {
            // Hardware bilinear mask sampling eliminates jagged blocks on upscaled rule images!
            let m = textureSampleLevel(mask_image, mask_sampler, v.uv, 0.0);
            coordinate = select(m.a, dot(m.rgb, vec3f(0.299, 0.587, 0.114)), m.a > 0.99);
            if v.color.b > 0.5 { coordinate = 1.0 - coordinate; }
        }
        let softness = v.color.g;
        if softness > 0.0 {
            let threshold = progress * (1.0 + softness) - softness * 0.5;
            coverage = 1.0 - smoothstep(threshold - softness * 0.5, threshold + softness * 0.5, coordinate);
        } else {
            coverage = select(0.0, 1.0, coordinate <= progress);
        }
    }
    if progress <= 0.0 { coverage = 0.0; }
    if progress >= 1.0 { coverage = 1.0; }
    return mix(source, target_color, coverage);
}
