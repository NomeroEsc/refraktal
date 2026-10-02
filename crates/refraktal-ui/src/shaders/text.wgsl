// SPDX-License-Identifier: GPL-3.0-or-later
// Glyph quads sampled from a coverage atlas.

struct Screen {
    size: vec4<f32>, // width, height, -, -
};

@group(0) @binding(0) var<uniform> screen: Screen;
@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

struct Glyph {
    @location(0) rect: vec4<f32>,  // x, y, w, h in pixels
    @location(1) uv: vec4<f32>,    // u0, v0, u1, v1
    @location(2) color: vec4<f32>, // linear rgb, alpha
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_text(@builtin(vertex_index) index: u32, glyph: Glyph) -> VsOut {
    let corner = vec2<f32>(f32(index & 1u), f32((index >> 1u) & 1u));
    let px = glyph.rect.xy + corner * glyph.rect.zw;
    var out: VsOut;
    out.pos = vec4<f32>(px.x / screen.size.x * 2.0 - 1.0, 1.0 - px.y / screen.size.y * 2.0, 0.0, 1.0);
    out.uv = mix(glyph.uv.xy, glyph.uv.zw, corner);
    out.color = glyph.color;
    return out;
}

@fragment
fn fs_text(in: VsOut) -> @location(0) vec4<f32> {
    let coverage = textureSampleLevel(atlas, samp, in.uv, 0.0).r;
    // A slight boost keeps thin strokes readable on bright glass.
    return vec4<f32>(in.color.rgb, in.color.a * pow(coverage, 0.85));
}
