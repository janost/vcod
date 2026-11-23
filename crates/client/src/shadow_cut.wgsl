// vc_shadows' alpha-tested world casters (crate::shadows): soups and props
// whose texture cuts, discarding as their main pass does. `fs_cut` is the
// implicit path's test (fs_main, fs_prop, the overlays' coverage at half);
// `fs_cut_stage` a scripted surface's first depth-writing stage.

@group(0) @binding(0) var<uniform> light_vp: mat4x4<f32>;

// the world's material layout; only bundle 0 and its sampler are read
@group(1) @binding(0) var t_diffuse: texture_2d<f32>;
// vc_shadows' own sampler (shadows::ShadowPass), not r_textureMode's
@group(0) @binding(1) var s_diffuse: sampler;

// The stage's `renderer::StageParams`, as shader.wgsl mirrors it.
struct StageParams {
    uv0: mat3x2<f32>,
    uv1: mat3x2<f32>,
    turb01: vec4<f32>,
    tint: vec4<f32>,
    flags: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
    vec0_s: vec4<f32>,
    vec0_t: vec4<f32>,
    vec1_s: vec4<f32>,
    vec1_t: vec4<f32>,
    eye_off: vec4<f32>,
    // the deform records and their kinds (deform.wgsl)
    deform: array<vec4<f32>, 3>,
    deform_kinds: vec4<u32>,
    // the falloff and the CoD4 world fx, unread here
    tail: array<vec4<f32>, 3>,
    // the deforms' clock terms (deform.wgsl)
    deform_ext: array<vec4<f32>, 3>,
};
@group(2) @binding(0) var<uniform> stage: StageParams;

const F_VERTEX_ALPHA: u32 = 2u;
const F_AF_GT0: u32 = 16u;
const F_AF_LT128: u32 = 32u;
const F_AF_GE128: u32 = 48u;
const ATEST128: f32 = 0.5019607843137255;

fn alphafunc_pass(a: f32) -> bool {
    if ((stage.flags & F_AF_GE128) == F_AF_GE128) {
        return a >= ATEST128;
    }
    if ((stage.flags & F_AF_LT128) != 0u) {
        return a < ATEST128;
    }
    if ((stage.flags & F_AF_GT0) != 0u) {
        return a > 0.0;
    }
    return true;
}

struct CutOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) alpha: f32,
};

@vertex
fn vs_cut(@location(0) pos: vec3<f32>, @location(1) uv: vec2<f32>) -> CutOut {
    var out: CutOut;
    out.clip = light_vp * vec4<f32>(pos, 1.0);
    out.uv = uv;
    out.alpha = 1.0;
    return out;
}

@fragment
fn fs_cut(in: CutOut) {
    if (textureSample(t_diffuse, s_diffuse, in.uv).a < 0.5) {
        discard;
    }
}

@vertex
fn vs_cut_stage(
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(3) normal: vec3<f32>,
    @location(4) color: vec4<f32>,
) -> CutOut {
    var out: CutOut;
    // the flags (mp_ship's and mp_powcamp's) cast as they wave
    out.clip = light_vp * vec4<f32>(deform_vertex(pos, normal, uv, 0.0).pos, 1.0);
    out.uv = stage.uv0 * vec3<f32>(uv, 1.0);
    out.alpha = select(1.0, color.a, (stage.flags & F_VERTEX_ALPHA) != 0u);
    return out;
}

@fragment
fn fs_cut_stage(in: CutOut) {
    let a = textureSample(t_diffuse, s_diffuse, in.uv).a * stage.tint.a * in.alpha;
    if (!alphafunc_pass(a)) {
        discard;
    }
}
