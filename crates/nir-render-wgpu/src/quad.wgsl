struct VertexIn { @location(0) pos: vec2f, @location(1) uv: vec2f, @location(2) color: vec4f };
struct VertexOut { @builtin(position) pos: vec4f, @location(0) uv: vec2f, @location(1) color: vec4f };
@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;
@vertex fn vs(v: VertexIn) -> VertexOut { var o: VertexOut; o.pos = vec4f(v.pos, 0.0, 1.0); o.uv = v.uv; o.color = v.color; return o; }
@fragment fn fs(v: VertexOut) -> @location(0) vec4f { return textureSample(image, image_sampler, v.uv) * v.color; }
@group(1) @binding(0) var target_image: texture_2d<f32>;
@group(1) @binding(1) var target_sampler: sampler;
@fragment fn fs_mix(v: VertexOut) -> @location(0) vec4f {
    return mix(textureSample(image, image_sampler, v.uv), textureSample(target_image, target_sampler, v.uv), v.color.a);
}
