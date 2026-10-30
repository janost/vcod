// vc_shadows mask (crate::shadows): a full-screen pass over the opaque
// scene that multiplies each pixel by one minus the sun's share of its
// light where a dynamic caster hides the sun and the static world does
// not. The static layer stands in for the lightmap's own shadow, so a
// point the bake already shadowed is not darkened twice.

struct Mask {
    inv_view_proj: mat4x4<f32>,
    light_vp: mat4x4<f32>,
    // xyz unit vector towards the sun
    sun_dir: vec4<f32>,
    // x the sun's luminance over sky plus ambient (shadows::sun_ratio),
    // y 1, z depth bias in light depth units, w normal offset in world units
    share: vec4<f32>,
    // x one shadow-map texel in uv, y the fade's start in light NDC,
    // zw the viewport size
    map: vec4<f32>,
    // the Camera tail of shader.wgsl: fog mode, colour, range, forward
    eye_fog_mode: vec4<f32>,
    fog_color_density: vec4<f32>,
    fog_range: vec4<f32>,
    view_fwd: vec4<f32>,
    // the static layer, which wraps and holds a margin past the window:
    // xy the window's corner in its uv, z the window's side in its uv,
    // w one texel
    static_at: vec4<f32>,
    // the texels the static layer holds, in the window's uv: x0 y0 x1 y1;
    // what it does not hold yet (filling in after a start-over) is left
    // unshadowed
    static_valid: vec4<f32>,
    // world xy to the regional keep's uv: xy * x + yz (shadows::keep_texels)
    keep_at: vec4<f32>,
};
@group(0) @binding(0) var<uniform> m: Mask;
@group(0) @binding(1) var t_depth: texture_depth_multisampled_2d;
// the dynamic casters over the window
@group(0) @binding(2) var t_shadow: texture_depth_2d;
@group(0) @binding(3) var s_shadow: sampler_comparison;
// the static world, repeating
@group(0) @binding(4) var s_static: sampler_comparison;
@group(0) @binding(5) var t_static: texture_depth_2d;
// the part of its light a sunlit texel keeps in the shade, per 1024-unit
// square; 0 where the whole map's share.x stands
@group(0) @binding(6) var t_keep: texture_2d<f32>;
@group(0) @binding(7) var s_keep: sampler;

// The sun's luminance over the rest of the light at `p` (shadows::sun_ratio).
fn sun_ratio_at(p: vec3<f32>) -> f32 {
    let keep = textureSampleLevel(t_keep, s_keep, p.xy * m.keep_at.x + m.keep_at.yz, 0.0).r;
    if (keep <= 0.0) {
        return m.share.x;
    }
    return (1.0 - keep) / (keep * m.sun_dir.z);
}

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

fn world_at(px: vec2<i32>, d: f32) -> vec3<f32> {
    let uv = (vec2<f32>(px) + 0.5) / m.map.zw;
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, d, 1.0);
    let w = m.inv_view_proj * ndc;
    return w.xyz / w.w;
}

fn depth_at(px: vec2<i32>) -> f32 {
    let size = vec2<i32>(textureDimensions(t_depth));
    return textureLoad(t_depth, clamp(px, vec2<i32>(0), size - 1), 0);
}

// The neighbour step along one axis whose depth is closer to the centre's,
// so an edge pixel takes its normal from its own surface.
fn step_along(px: vec2<i32>, axis: vec2<i32>, d: f32, p: vec3<f32>) -> vec3<f32> {
    let da = depth_at(px + axis);
    let db = depth_at(px - axis);
    if (abs(da - d) <= abs(db - d)) {
        return world_at(px + axis, da) - p;
    }
    return p - world_at(px - axis, db);
}

// Percentage-closer filtering over (2r+1)^2 bilinear compare taps: 1 lit.
// `uv` is in the window; the static layer is read through its own.
fn pcf(uv: vec2<f32>, z: f32, is_static: bool, r: i32) -> f32 {
    var sum = 0.0;
    let suv = m.static_at.xy + uv * m.static_at.z;
    for (var y = -r; y <= r; y++) {
        for (var x = -r; x <= r; x++) {
            let o = vec2<f32>(f32(x), f32(y));
            if (is_static) {
                sum += textureSampleCompareLevel(t_static, s_static, suv + o * m.static_at.w, z);
            } else {
                sum += textureSampleCompareLevel(t_shadow, s_shadow, uv + o * m.map.x, z);
            }
        }
    }
    let n = f32(2 * r + 1);
    return sum / (n * n);
}

// glFog GL_EXP / GL_LINEAR, as shader.wgsl's fog_amount.
fn fog_amount(world_pos: vec3<f32>) -> f32 {
    let mode = m.eye_fog_mode.w;
    if (mode == 0.0) {
        return 0.0;
    }
    let d = max(dot(world_pos - m.eye_fog_mode.xyz, m.view_fwd.xyz), 0.0);
    if (mode == 1.0) {
        return 1.0 - exp(-m.fog_color_density.a * max(d - m.fog_range.w, 0.0));
    }
    let span = max(m.fog_range.y - m.fog_range.x, 0.0001);
    return clamp((d - m.fog_range.x) / span, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let px = vec2<i32>(pos.xy);
    let d = depth_at(px);
    if (d >= 1.0) {
        return vec4<f32>(1.0);
    }
    let p = world_at(px, d);
    var n = normalize(cross(step_along(px, vec2<i32>(1, 0), d, p), step_along(px, vec2<i32>(0, 1), d, p)));
    if (dot(n, m.eye_fog_mode.xyz - p) < 0.0) {
        n = -n;
    }
    let ndl = dot(n, m.sun_dir.xyz);
    if (ndl <= 0.0) {
        return vec4<f32>(1.0);
    }
    let lp = m.light_vp * vec4<f32>(p + n * m.share.w, 1.0);
    let edge = max(abs(lp.x), abs(lp.y));
    if (edge >= 1.0) {
        return vec4<f32>(1.0);
    }
    let uv = vec2<f32>(lp.x * 0.5 + 0.5, 0.5 - lp.y * 0.5);
    let pad = 1.5 * m.map.x;
    if (any(uv < m.static_valid.xy + pad) || any(uv > m.static_valid.zw - pad)) {
        return vec4<f32>(1.0);
    }
    let z = lp.z - m.share.z;
    // most pixels have no dynamic caster near: the filter's centre and
    // corners all lit
    let t = 2.0 * m.map.x;
    var probe = 0.0;
    for (var i = 0; i < 4; i++) {
        let o = vec2<f32>(select(-t, t, (i & 1) != 0), select(-t, t, (i & 2) != 0));
        probe += textureSampleCompareLevel(t_shadow, s_shadow, uv + o, z);
    }
    probe += textureSampleCompareLevel(t_shadow, s_shadow, uv, z);
    if (probe >= 5.0) {
        return vec4<f32>(1.0);
    }
    let lit_static = pcf(uv, z, true, 1);
    if (lit_static <= 0.0) {
        return vec4<f32>(1.0);
    }
    let hidden = 1.0 - pcf(uv, z, false, 2);
    let sun = sun_ratio_at(p) * ndl;
    let k = sun / (sun + m.share.y);
    let fade = 1.0 - smoothstep(m.map.y, 1.0, edge);
    let s = 1.0 - k * lit_static * hidden * fade * (1.0 - fog_amount(p));
    return vec4<f32>(s, s, s, 1.0);
}
