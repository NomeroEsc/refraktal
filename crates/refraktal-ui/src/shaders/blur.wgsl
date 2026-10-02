
// Dual Kawase blur: a chain of downsample passes followed by upsample passes.

struct BlurParams {
    texel: vec2<f32>,  // 1 / destination size
    offset: f32,
    _pad: f32,
};

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<uniform> params: BlurParams;

fn tap(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(src, samp, uv, 0.0).rgb;
}

@fragment
fn fs_down(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = frag.xy * params.texel;
    let h = params.texel * 0.5 * params.offset;
    var sum = tap(uv) * 4.0;
    sum += tap(uv - h);
    sum += tap(uv + h);
    sum += tap(uv + vec2<f32>(h.x, -h.y));
    sum += tap(uv - vec2<f32>(h.x, -h.y));
    return vec4<f32>(sum / 8.0, 1.0);
}

@fragment
fn fs_up(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = frag.xy * params.texel;
    let h = params.texel * 0.5 * params.offset;
    var sum = tap(uv + vec2<f32>(-h.x * 2.0, 0.0));
    sum += tap(uv + vec2<f32>(-h.x, h.y)) * 2.0;
    sum += tap(uv + vec2<f32>(0.0, h.y * 2.0));
    sum += tap(uv + vec2<f32>(h.x, h.y)) * 2.0;
    sum += tap(uv + vec2<f32>(h.x * 2.0, 0.0));
    sum += tap(uv + vec2<f32>(h.x, -h.y)) * 2.0;
    sum += tap(uv + vec2<f32>(0.0, -h.y * 2.0));
    sum += tap(uv + vec2<f32>(-h.x, -h.y)) * 2.0;
    return vec4<f32>(sum / 12.0, 1.0);
}
