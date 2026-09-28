struct VertexIn { @location(0) pos: vec2f, @location(1) uv: vec2f, @location(2) color: vec4f };
struct VertexOut { @builtin(position) pos: vec4f, @location(0) uv: vec2f, @location(1) color: vec4f };
@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;
@vertex fn vs(v: VertexIn) -> VertexOut { var o: VertexOut; o.pos = vec4f(v.pos, 0.0, 1.0); o.uv = v.uv; o.color = v.color; return o; }
@fragment fn fs(v: VertexOut) -> @location(0) vec4f { return textureSample(image, image_sampler, v.uv) * v.color; }
@group(1) @binding(0) var target_image: texture_2d<f32>;
@group(1) @binding(1) var target_sampler: sampler;
@group(2) @binding(0) var mask_image: texture_2d<f32>;
@group(2) @binding(1) var mask_sampler: sampler;
@fragment fn fs_mix(v: VertexOut) -> @location(0) vec4f {
    let source=textureSample(image,image_sampler,v.uv);
    let target_color=textureSample(target_image,target_sampler,v.uv);
    let progress=v.color.a;
    var coverage=progress;
    if v.color.r>=1.0 {
        var coordinate=v.uv.x;
        if v.color.r==2.0 {coordinate=1.0-v.uv.x;}
        if v.color.r==3.0 {coordinate=v.uv.y;}
        if v.color.r==4.0 {coordinate=1.0-v.uv.y;}
        if v.color.r==5.0 {
            let size=textureDimensions(mask_image);
            let texel=clamp(vec2i(floor(v.uv*vec2f(size))),vec2i(0),vec2i(size)-vec2i(1));
            coordinate=textureLoad(mask_image,texel,0).a;
            if v.color.b>0.5 {coordinate=1.0-coordinate;}
        }
        let softness=v.color.g;
        if softness>0.0 {
            let threshold=progress*(1.0+softness)-softness/2.0;
            coverage=1.0-smoothstep(threshold-softness/2.0,threshold+softness/2.0,coordinate);
        } else {coverage=select(0.0,1.0,coordinate<=progress);}
    }
    if progress<=0.0 {coverage=0.0;}
    if progress>=1.0 {coverage=1.0;}
    return mix(source,target_color,coverage);
}
