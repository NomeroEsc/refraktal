
// Animated neon backdrop: drifting color fields, light streaks and a
// perspective grid floor. Rendered in linear HDR.

@group(0) @binding(0) var<uniform> g: Globals;

fn blob(p: vec2<f32>, center: vec2<f32>, radius: f32, color: vec3<f32>) -> vec3<f32> {
    let d = p - center;
    return color * exp(-dot(d, d) / (radius * radius));
}

fn grid_line(v: f32) -> f32 {
    let w = max(fwidth(v), 1e-4);
    let f = abs(fract(v - 0.5) - 0.5) / w;
    return 1.0 - min(f, 1.0);
}

fn streak(p: vec2<f32>, dir: vec2<f32>, offset: f32, width_px: f32, res_y: f32) -> f32 {
    let n = vec2<f32>(-dir.y, dir.x);
    let d = abs(dot(p, n) - offset) * res_y;
    return exp(-d / width_px);
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let res = g.screen.xy;
    let t = g.screen.z;
    let uv = frag.xy / res;
    let p = (frag.xy - 0.5 * res) / res.y;
    let horizon = 0.66;

    // Sky.
    var col = mix(INDIGO, vec3<f32>(0.045, 0.012, 0.085), smoothstep(0.0, horizon, uv.y));

    // Slowly drifting color fields.
    col += blob(p, vec2<f32>(-0.55 + 0.08 * sin(t * 0.07), -0.16 + 0.05 * cos(t * 0.05)), 0.45, MAGENTA * 0.20);
    col += blob(p, vec2<f32>(0.62 + 0.07 * cos(t * 0.06), -0.24 + 0.05 * sin(t * 0.08)), 0.40, CYAN * 0.14);
    col += blob(p, vec2<f32>(0.05 + 0.10 * sin(t * 0.04), 0.02), 0.55, VIOLET * 0.12);

    // Thin diagonal light streaks; they make refraction easy to see.
    let dir = normalize(vec2<f32>(1.0, -0.32));
    col += CYAN * 0.55 * streak(p, dir, -0.10 + 0.01 * sin(t * 0.3), 1.6, res.y);
    col += MAGENTA * 0.45 * streak(p, dir, 0.05, 1.2, res.y);
    col += VIOLET * 0.35 * streak(p, normalize(vec2<f32>(1.0, 0.55)), 0.12, 1.4, res.y);

    // Horizon glow.
    let hy = uv.y - horizon;
    col += MAGENTA * 0.8 * exp(-abs(hy) * 60.0) + VIOLET * 0.12 * exp(-abs(hy) * 7.0);

    // Perspective floor grid, scrolling towards the viewer. Derivatives must
    // run in uniform control flow, so everything is computed unconditionally
    // and masked afterwards.
    let depth_y = max(hy, 1e-3);
    let z = 0.12 / depth_y;
    let x = (uv.x - 0.5) * (res.x / res.y) * z * 2.2;
    let lines = max(grid_line(x), grid_line(z + t * 0.25));
    let floor_mask = step(0.0, hy);
    let fade = smoothstep(0.0, 0.3, hy);
    let line_color = mix(MAGENTA, CYAN, smoothstep(0.0, 0.34, hy));
    col = mix(col, vec3<f32>(0.006, 0.003, 0.018), floor_mask * 0.85);
    col += line_color * lines * fade * floor_mask * 0.9;

    // Vignette.
    let v = uv - 0.5;
    col *= 1.0 - dot(v, v) * 0.9;

    return vec4<f32>(col, 1.0);
}
