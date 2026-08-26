// Farbox pass. The cube is axis-aligned in world space, centred on the view
// origin each frame (never rotated); sides sample their own env texture with
// clamp-to-edge.

struct Camera {
    view_proj: mat4x4<f32>,
    time_pad: vec4<f32>, // .x = seconds since start; yzw reserved
};
@group(0) @binding(0) var<uniform> camera: Camera;

@group(1) @binding(0) var t_side: texture_2d<f32>;
@group(1) @binding(1) var s_side: sampler;

struct SkyEye {
    // xyz = view origin, w unused
    eye: vec4<f32>,
};
@group(2) @binding(0) var<uniform> sky_eye: SkyEye;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = camera.view_proj * vec4<f32>(in.pos + sky_eye.eye.xyz, 1.0);
    out.uv = in.uv;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(t_side, s_side, in.uv);
}
