// Farbox pass. The cube is axis-aligned in world space, centred on the view
// origin each frame (never rotated); sides sample their own env texture with
// clamp-to-edge.

struct Camera {
    view_proj: mat4x4<f32>,
    time_pad: vec4<f32>, // .x = seconds since start; yzw reserved
    // xyz view origin; w fog mode: 0 off, 1 GL_EXP, 2 GL_LINEAR (configstring 12)
    eye_fog_mode: vec4<f32>,
    // rgb fog colour, a density (GL_EXP)
    fog_color_density: vec4<f32>,
    // x near, y far (GL_LINEAR)
    fog_range: vec4<f32>,
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
    @location(1) world_pos: vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let world = in.pos + sky_eye.eye.xyz;
    out.clip = camera.view_proj * vec4<f32>(world, 1.0);
    out.uv = in.uv;
    out.world_pos = world;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var c = textureSample(t_side, s_side, in.uv);
    // Linear farclip fog never touches the sky (RTCW drawsky=false); exp does.
    if (camera.eye_fog_mode.w == 1.0) {
        let d = distance(in.world_pos, camera.eye_fog_mode.xyz);
        let f = 1.0 - exp(-camera.fog_color_density.a * d);
        c = vec4<f32>(mix(c.rgb, camera.fog_color_density.rgb, f), c.a);
    }
    return c;
}

// The sunfile sprite: vertices arrive already in world space (rebuilt per
// frame on the CPU around the view origin).
@vertex
fn vs_sun(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = camera.view_proj * vec4<f32>(in.pos, 1.0);
    out.uv = in.uv;
    out.world_pos = in.pos;
    return out;
}

@fragment
fn fs_sun(in: VsOut) -> @location(0) vec4<f32> {
    var c = textureSample(t_side, s_side, in.uv);
    // Additive glow; fogged exactly like the farbox.
    var rgb = c.rgb * c.a;
    if (camera.eye_fog_mode.w == 1.0) {
        let d = distance(in.world_pos, camera.eye_fog_mode.xyz);
        let f = 1.0 - exp(-camera.fog_color_density.a * d);
        rgb = mix(rgb, camera.fog_color_density.rgb, f);
    }
    return vec4<f32>(rgb, 1.0);
}
