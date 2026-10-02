
// Final pass: glass panels refract and frost the backdrop, then the
// controls are drawn on top.

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var scene: texture_2d<f32>;
@group(0) @binding(2) var blurred: texture_2d<f32>;
@group(0) @binding(3) var samp: sampler;

fn sample_scene(px: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(scene, samp, px / g.screen.xy, 0.0).rgb;
}

fn sample_blur(px: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(blurred, samp, px / g.screen.xy, 0.0).rgb;
}

// Coverage of a shape with signed distance `d`, antialiased over one pixel.
fn fill(d: f32) -> f32 {
    return clamp(0.5 - d, 0.0, 1.0);
}

fn rect_sdf(p: vec2<f32>, rect: vec4<f32>, radius: f32) -> f32 {
    let half_size = rect.zw * 0.5;
    return sd_round_box(p - rect.xy - half_size, half_size, radius);
}

fn rect_normal(p: vec2<f32>, rect: vec4<f32>, radius: f32) -> vec2<f32> {
    let e = vec2<f32>(1.0, 0.0);
    let n = vec2<f32>(
        rect_sdf(p + e.xy, rect, radius) - rect_sdf(p - e.xy, rect, radius),
        rect_sdf(p + e.yx, rect, radius) - rect_sdf(p - e.yx, rect, radius),
    );
    return n / max(length(n), 1e-5);
}

fn shadow(col: vec3<f32>, p: vec2<f32>, rect: vec4<f32>, radius: f32) -> vec3<f32> {
    let s = g.screen.w;
    let d = rect_sdf(p - vec2<f32>(0.0, 14.0 * s), rect, radius);
    let amount = 1.0 - smoothstep(-10.0 * s, 46.0 * s, d);
    return col * (1.0 - amount * 0.6);
}

// Liquid glass: a clear, strongly refracting rim around a frosted body.
fn glass(col: vec3<f32>, p: vec2<f32>, rect: vec4<f32>, radius: f32) -> vec3<f32> {
    let d = rect_sdf(p, rect, radius);
    let mask = fill(d);
    if (mask <= 0.0) {
        return col;
    }
    let s = g.screen.w;
    let n = rect_normal(p, rect, radius);
    let depth = clamp(-d / (24.0 * s), 0.0, 1.0); // 0 at the edge, 1 inside
    let rim = pow(1.0 - depth, 2.4);

    // Refraction: near the edge the glass bends light from outside the
    // panel inwards. Each color channel bends slightly differently.
    let shift = n * rim * 40.0 * s;
    let sharp = vec3<f32>(
        sample_scene(p + shift * 1.00).r,
        sample_scene(p + shift * 1.10).g,
        sample_scene(p + shift * 1.22).b,
    );
    let soft = vec3<f32>(
        sample_blur(p + shift * 1.00).r,
        sample_blur(p + shift * 1.10).g,
        sample_blur(p + shift * 1.22).b,
    );
    let frost = mix(0.2, 0.9, depth);
    var body = mix(sharp, soft, frost);

    // Slight tint and lift so content on top stays legible.
    body = body * 0.8 + vec3<f32>(0.010, 0.009, 0.026);

    // Specular rim lit from the top left, darker edge on the bottom right.
    let light = normalize(vec2<f32>(-0.55, -0.85));
    let edge = pow(1.0 - depth, 7.0);
    let facing = dot(n, light);
    body += vec3<f32>(0.95, 0.92, 1.0) * (max(facing, 0.0) * 1.4 + 0.18) * edge;
    body *= 1.0 - max(-facing, 0.0) * edge * 0.35;

    return mix(col, body, mask);
}

fn track_color(track: u32) -> vec3<f32> {
    return palette((g.colors.x >> (track * 4u)) & 15u);
}

fn draw_add_button(col_in: vec3<f32>, p: vec2<f32>) -> vec3<f32> {
    if (g.add.w < 0.5) {
        return col_in;
    }
    let s = g.screen.w;
    let rel = p - g.add.xy;
    let d = length(rel) - g.add.z;
    let hovered = g.hover.w > 0.5;
    var col = mix(col_in, col_in + vec3<f32>(select(0.04, 0.10, hovered)), fill(d));
    col += vec3<f32>(1.0) * fill(abs(d + 0.5 * s) - 0.5 * s) * select(0.22, 0.5, hovered);
    let arm = g.add.z * 0.45;
    let plus = min(sd_box(rel, vec2<f32>(arm, 0.9 * s)), sd_box(rel, vec2<f32>(0.9 * s, arm)));
    col = mix(col, vec3<f32>(select(0.7, 1.2, hovered)), fill(plus));
    return col;
}

fn draw_cells(col_in: vec3<f32>, p: vec2<f32>) -> vec3<f32> {
    let s = g.screen.w;
    let origin = g.grid.xy;
    let cell = g.grid.z;
    let gap = g.grid.w;
    let beat_gap = g.grid2.x;
    let row_gap = g.grid2.y;
    let beat_stride = 4.0 * cell + 3.0 * gap + beat_gap;

    // Find the nearest cell arithmetically instead of looping over all 48.
    let bx = p.x - origin.x;
    let beat = clamp(floor((bx + beat_gap * 0.5) / beat_stride), 0.0, 3.0);
    let in_beat = bx - beat * beat_stride;
    let col_in_beat = clamp(floor((in_beat + gap * 0.5) / (cell + gap)), 0.0, 3.0);
    let step_f = beat * 4.0 + col_in_beat;
    let last_track = f32(max(g.tracks.w, 1u) - 1u);
    let track_f = clamp(floor((p.y - origin.y + row_gap * 0.5) / (cell + row_gap)), 0.0, last_track);

    let center = vec2<f32>(
        origin.x + beat * beat_stride + col_in_beat * (cell + gap) + cell * 0.5,
        origin.y + track_f * (cell + row_gap) + cell * 0.5,
    );
    let d = sd_round_box(p - center, vec2<f32>(cell * 0.5), cell * 0.24);

    let track = u32(track_f);
    let step_i = u32(step_f);
    let on = ((g.pattern[track / 4u][track % 4u] >> step_i) & 1u) == 1u;
    let color = track_color(track);
    let current = g.grid2.w >= 0.0 && abs(g.grid2.w - step_f) < 0.5;
    let hovered = abs(g.hover.x - track_f) < 0.5 && abs(g.hover.y - step_f) < 0.5;

    var col = col_in;

    // Neon glow around active cells, brighter when the playhead passes.
    if (on) {
        // Fade out before the gap midpoint so neighbouring cells don't show seams.
        let glow = exp(-max(d, 0.0) / (3.0 * s)) * step(0.0, d) * (1.0 - smoothstep(0.0, gap * 0.5, d));
        col += color * glow * select(0.30, 0.95, current);
    }

    // Cell body.
    let top = (p.y - (center.y - cell * 0.5)) / cell;
    var body: vec3<f32>;
    if (on) {
        body = color * select(1.0, 2.4, current) * (1.15 - 0.3 * top);
    } else {
        body = mix(col, vec3<f32>(1.0), select(0.05, 0.16, current));
    }
    col = mix(col, body, fill(d));

    // Thin inner outline.
    let ring = clamp(1.0 - abs(d + 0.75 * s) / (0.75 * s), 0.0, 1.0);
    var ring_strength = select(0.12, 0.30, on);
    if (hovered) {
        ring_strength = 0.6;
    }
    col += vec3<f32>(1.0) * ring * ring_strength;

    // Track markers to the left of the grid: a dot for the built-in synth,
    // a rounded square when a sample is loaded, a ring around the selected one.
    let marker = vec2<f32>(g.grid2.z, center.y);
    let selected = g.tracks.x == track;
    let has_sample = ((g.tracks.y >> track) & 1u) == 1u;
    let marker_hovered = abs(g.hover.x - track_f) < 0.5 && g.hover.y < 0.0;
    var md: f32;
    if (has_sample) {
        md = sd_round_box(p - marker, vec2<f32>(5.0 * s), 1.5 * s);
    } else {
        md = length(p - marker) - 4.5 * s;
    }
    col += color * exp(-max(md, 0.0) / (6.0 * s)) * 0.35;
    col = mix(col, color * 1.4, fill(md));
    let ring_d = abs(length(p - marker) - 10.0 * s) - 0.75 * s;
    let ring_alpha = select(select(0.0, 0.35, marker_hovered), 1.0, selected);
    col = mix(col, color * 1.2, fill(ring_d) * ring_alpha);

    // While a file is dragged over the window, light up the target row.
    if (selected && g.tracks.z == 1u) {
        let row = vec4<f32>(g.grid2.z - 16.0 * s, center.y - cell * 0.5 - 6.0 * s,
                            g.sequencer.x + g.sequencer.z - g.grid2.z, cell + 12.0 * s);
        let rd = rect_sdf(p, row, 12.0 * s);
        col += color * 0.12 * fill(rd);
        col += color * 0.8 * fill(abs(rd) - 0.75 * s);
    }

    return col;
}

fn sd_triangle_right(p: vec2<f32>, r: f32) -> f32 {
    // Equilateral triangle pointing towards +x.
    let k = sqrt(3.0);
    var q = vec2<f32>(abs(p.y) - r, p.x + r / k);
    if (q.x + k * q.y > 0.0) {
        q = vec2<f32>(q.x - k * q.y, -k * q.x - q.y) / 2.0;
    }
    q.x -= clamp(q.x, -2.0 * r, 0.0);
    return -length(q) * sign(q.y);
}

fn draw_transport(col_in: vec3<f32>, p: vec2<f32>) -> vec3<f32> {
    let s = g.screen.w;
    var col = col_in;
    let playing = g.play.w > 0.5;
    let hovered = abs(g.hover.z - 1.0) < 0.5;

    // Play / stop button.
    let rel = p - g.play.xy;
    let bd = length(rel) - g.play.z;
    col = mix(col, col + vec3<f32>(select(0.05, 0.11, hovered)), fill(bd));
    let ring = clamp(1.0 - abs(bd + 0.75 * s) / (0.75 * s), 0.0, 1.0);
    if (playing) {
        col += CYAN * (ring * 1.2 + exp(-abs(bd) / (7.0 * s)) * 0.35);
    } else {
        col += vec3<f32>(1.0) * ring * select(0.22, 0.45, hovered);
    }
    var icon: f32;
    if (playing) {
        icon = sd_round_box(rel, vec2<f32>(g.play.z * 0.30), 2.0 * s);
    } else {
        icon = sd_triangle_right(rel - vec2<f32>(g.play.z * 0.08, 0.0), g.play.z * 0.36);
    }
    col = mix(col, vec3<f32>(1.3, 1.3, 1.4), fill(icon));

    // Playhead: 16 dots, larger on each beat, the current one lit.
    let first = g.dots.x;
    let last = g.dots.y;
    let stride = (last - first) / 15.0;
    let i = clamp(round((p.x - first) / stride), 0.0, 15.0);
    let c = vec2<f32>(first + i * stride, g.dots.z);
    let on_beat = (u32(i) % 4u) == 0u;
    let r = g.dots.w * select(1.0, 1.5, on_beat);
    let dd = length(p - c) - r;
    let current = g.grid2.w >= 0.0 && abs(g.grid2.w - i) < 0.5;
    if (current) {
        col += CYAN * exp(-max(dd, 0.0) / (5.0 * s)) * 0.9;
        col = mix(col, CYAN * 3.0, fill(dd));
    } else {
        col = mix(col, col + vec3<f32>(select(0.16, 0.32, on_beat)), fill(dd));
    }
    // Tempo buttons and the help button.
    col = small_button(col, p, vec2<f32>(g.tempo_buttons.x, g.tempo_buttons.z), g.tempo_buttons.w, 2.0, 1);
    col = small_button(col, p, vec2<f32>(g.tempo_buttons.y, g.tempo_buttons.z), g.tempo_buttons.w, 3.0, 2);
    col = small_button(col, p, g.help_button.xy, g.help_button.z, 4.0, 0);
    return col;
}

// A round button; `icon` 1 draws a minus, 2 a plus, 0 nothing (text goes on top).
fn small_button(col_in: vec3<f32>, p: vec2<f32>, center: vec2<f32>, r: f32, id: f32, icon: i32) -> vec3<f32> {
    let s = g.screen.w;
    let rel = p - center;
    let d = length(rel) - r;
    let hovered = abs(g.hover.z - id) < 0.5;
    var col = mix(col_in, col_in + vec3<f32>(select(0.04, 0.11, hovered)), fill(d));
    col += vec3<f32>(1.0) * fill(abs(d + 0.5 * s) - 0.5 * s) * select(0.2, 0.45, hovered);
    let arm = r * 0.42;
    var shape = 1e5;
    if (icon >= 1) {
        shape = sd_box(rel, vec2<f32>(arm, 0.9 * s));
    }
    if (icon == 2) {
        shape = min(shape, sd_box(rel, vec2<f32>(0.9 * s, arm)));
    }
    col = mix(col, vec3<f32>(select(0.75, 1.2, hovered)), fill(shape));
    return col;
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let p = frag.xy;
    var col = sample_scene(p);

    col = shadow(col, p, g.sequencer, g.radii.x);
    col = shadow(col, p, g.transport, g.radii.y);
    col = glass(col, p, g.sequencer, g.radii.x);
    col = glass(col, p, g.transport, g.radii.y);

    if (rect_sdf(p, g.sequencer, g.radii.x) < 24.0 * g.screen.w) {
        col = draw_cells(col, p);
        col = draw_add_button(col, p);
    }
    if (rect_sdf(p, g.transport, g.radii.y) < 0.0) {
        col = draw_transport(col, p);
    }

    // Help overlay: dim everything, then a large frosted panel on top.
    if (g.overlay_info.x > 0.5) {
        col *= 0.35;
        col = shadow(col, p, g.overlay, g.overlay_info.y);
        col = glass(col, p, g.overlay, g.overlay_info.y);
    }

    // Soft highlight roll-off instead of hard clipping.
    col = vec3<f32>(1.0) - exp(-col * 1.25);
    return vec4<f32>(col, 1.0);
}
