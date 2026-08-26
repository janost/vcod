// World-space fx quads: depth-tested, never writing depth. Per-vertex rgba
// multiplies the texture. `fs_main` is straight alpha, `fs_additive` is
// `GL_ONE GL_ONE` for glow materials (`assets::Shaders::additive`).

// Same uniform as shader.wgsl; the fog fields are documented there.
struct Camera {
    view_proj: mat4x4<f32>,
    time_pad: vec4<f32>,
    eye_fog_mode: vec4<f32>,
    fog_color_density: vec4<f32>,
    fog_range: vec4<f32>,
    view_fwd: vec4<f32>,
}
@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) rgba: vec4<f32>,
}
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) rgba: vec4<f32>,
    @location(2) world_pos: vec3<f32>,
}
@vertex fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = camera.view_proj * vec4(in.pos, 1.0);
    out.uv = in.uv;
    out.rgba = in.rgba;
    out.world_pos = in.pos;
    return out;
}

// glFog GL_EXP / GL_LINEAR factors (see shader.wgsl); depth along the view
// forward, the fixed-function fog coordinate without GL_NV_fog_distance.
fn fog_amount(world_pos: vec3<f32>) -> f32 {
    let mode = camera.eye_fog_mode.w;
    if (mode == 0.0) {
        return 0.0;
    }
    let d = max(dot(world_pos - camera.eye_fog_mode.xyz, camera.view_fwd.xyz), 0.0);
    if (mode == 1.0) {
        return 1.0 - exp(-camera.fog_color_density.a * d);
    }
    let span = max(camera.fog_range.y - camera.fog_range.x, 0.0001);
    return clamp((d - camera.fog_range.x) / span, 0.0, 1.0);
}

// Fogged like the surface a decal sits on, so bullet holes and smoke
// take the same colour as the wall behind them.
@fragment fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let t = textureSample(tex, samp, in.uv);
    let rgb = mix(t.rgb * in.rgba.rgb, camera.fog_color_density.rgb, fog_amount(in.world_pos));
    return vec4(rgb, t.a * in.rgba.a);
}

// GL_ONE GL_ONE ignores source alpha, so the fade scales the colour. The
// engine likewise bakes the alpha curve into vertex RGB for emitters without
// `useAlpha` (docs/research/efx-grammar.md, section 3). Texture alpha is never read.
// Additive fog fades toward black, not the fog colour (Q3 fogColorMask): adding
// fog colour would brighten glows the further into the fog they sit.
@fragment fn fs_additive(in: VsOut) -> @location(0) vec4<f32> {
    let t = textureSample(tex, samp, in.uv);
    let rgb = t.rgb * in.rgba.rgb * in.rgba.a * (1.0 - fog_amount(in.world_pos));
    return vec4(rgb, 0.0);
}
