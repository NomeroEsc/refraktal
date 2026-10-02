// SPDX-License-Identifier: GPL-3.0-or-later
// Shared definitions, prepended to every shader in this folder.

struct Globals {
    screen: vec4<f32>,     // width, height, time, scale
    sequencer: vec4<f32>,  // panel rect: x, y, w, h
    transport: vec4<f32>,  // pill rect: x, y, w, h
    radii: vec4<f32>,      // panel radius, pill radius, -, -
    play: vec4<f32>,       // center x, center y, radius, playing (0/1)
    grid: vec4<f32>,       // origin x, origin y, cell, gap
    grid2: vec4<f32>,      // beat gap, row gap, label x, current step (-1 = none)
    dots: vec4<f32>,       // first x, last x, y, radius
    hover: vec4<f32>,      // track, step, play hovered (0/1), add hovered (0/1)
    pattern: array<vec4<u32>, 2>, // one 16-bit step mask per track slot
    tracks: vec4<u32>,     // selected track, sample-loaded mask, file hover (0/1), track count
    colors: vec4<u32>,     // x: 4-bit palette index per track
    add: vec4<f32>,        // "+" button: center x, center y, radius, visible (0/1)
};

// Linear-light neon palette.
const INDIGO = vec3<f32>(0.0065, 0.0037, 0.023);
const MAGENTA = vec3<f32>(1.0, 0.027, 0.24);
const CYAN = vec3<f32>(0.025, 0.76, 1.0);
const VIOLET = vec3<f32>(0.26, 0.10, 1.0);
const AMBER = vec3<f32>(1.0, 0.45, 0.06);
const MINT = vec3<f32>(0.08, 1.0, 0.5);
const CORAL = vec3<f32>(1.0, 0.22, 0.10);
const SKY = vec3<f32>(0.18, 0.42, 1.0);
const ROSE = vec3<f32>(1.0, 0.30, 0.65);

fn palette(index: u32) -> vec3<f32> {
    switch index {
        case 0u: { return MAGENTA; }
        case 1u: { return CYAN; }
        case 2u: { return AMBER; }
        case 3u: { return VIOLET * 1.3; }
        case 4u: { return MINT; }
        case 5u: { return CORAL; }
        case 6u: { return SKY; }
        default: { return ROSE; }
    }
}

struct VsOut {
    @builtin(position) pos: vec4<f32>,
};

// One triangle that covers the whole screen; no vertex buffer needed.
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VsOut {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var out: VsOut;
    out.pos = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    return out;
}

fn sd_round_box(p: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let q = abs(p) - half_size + radius;
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

fn sd_box(p: vec2<f32>, half_size: vec2<f32>) -> f32 {
    let q = abs(p) - half_size;
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0);
}
