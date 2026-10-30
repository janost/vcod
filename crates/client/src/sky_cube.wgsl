// CoD2's sky (docs/research/cod2-materials.md 9, "Sky"): the sky material's
// cube colour map sampled along the direction from the eye, unfogged. Drawn
// on the farbox geometry (sky.rs), whose vertices are that direction; the
// colour leaves scaled by identityLight, as the scene target holds it, and
// alpha 0 as sky.hlsl writes it: the glow's setup reads that as sky
// (crate::cod2_glow).
// tex_samplers.wgsl is appended.

struct Camera {
    view_proj: mat4x4<f32>,
    // .y identityLight
    time_pad: vec4<f32>,
    eye_fog_mode: vec4<f32>,
    fog_color_density: vec4<f32>,
    fog_range: vec4<f32>,
    view_fwd: vec4<f32>,
};
@group(0) @binding(0) var<uniform> camera: Camera;

@group(1) @binding(0) var t_sky: texture_cube<f32>;

struct SkyEye {
    // xyz view origin
    eye: vec4<f32>,
};
@group(2) @binding(0) var<uniform> sky_eye: SkyEye;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) dir: vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = camera.view_proj * vec4<f32>(in.pos + sky_eye.eye.xyz, 1.0);
    out.dir = in.pos;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(t_sky, s_tex_clamp, in.dir);
    return vec4<f32>(c.rgb * camera.time_pad.y, 0.0);
}
