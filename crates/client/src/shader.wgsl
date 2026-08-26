struct Camera {
    view_proj: mat4x4<f32>,
    time_pad: vec4<f32>, // .x = seconds since start; yzw reserved
};
@group(0) @binding(0) var<uniform> camera: Camera;

struct FxLights {
    pos_radius: array<vec4<f32>, 8>,
    color: array<vec4<f32>, 8>,
}
@group(0) @binding(1) var<uniform> fx_lights: FxLights;

@group(1) @binding(0) var t_diffuse: texture_2d<f32>;
@group(1) @binding(1) var t_lightmap: texture_2d<f32>;
@group(1) @binding(2) var s_diffuse: sampler;
@group(1) @binding(3) var s_lightmap: sampler;

// Per-stage shader-script parameters; std140-mirrors renderer::StageParams.
struct StageParams {
    uv0: mat3x2<f32>,
    uv1: mat3x2<f32>,
    // [amp0, now0, amp1, now1]; VS adds amp*sin(worldpos_axis/1024 + now)
    turb01: vec4<f32>,
    tint: vec4<f32>,
    flags: u32,
    // scalars, not an array: naga wants uniform array strides at 16
    pad0: u32,
    pad1: u32,
    pad2: u32,
    vec0_s: vec4<f32>,
    vec0_t: vec4<f32>,
    vec1_s: vec4<f32>,
    vec1_t: vec4<f32>,
};
@group(2) @binding(0) var<uniform> stage: StageParams;
// Bundle 1's image when it is neither lightmap nor absent (white is bound then).
@group(2) @binding(1) var t_bundle1: texture_2d<f32>;

const F_VERTEX_RGB: u32 = 1u;
const F_VERTEX_ALPHA: u32 = 2u;
const F_BUNDLE1_LIGHTMAP: u32 = 4u;
const F_BUNDLE0_LIGHTMAP: u32 = 8u;
const F_AF_GT0: u32 = 16u;
const F_AF_LT128: u32 = 32u;
const F_AF_GE128: u32 = 48u;
const F_BUNDLE0_VECTOR: u32 = 64u;
const F_BUNDLE1_VECTOR: u32 = 128u;
// glAlphaFunc thresholds are 128/255 for both LT128 and GE128.
const ATEST128: f32 = 0.5019607843137255;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) lm_uv: vec2<f32>,
    @location(3) normal: vec3<f32>,
    @location(4) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) lm_uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    // BSP vertices are already world space.
    @location(3) world_pos: vec3<f32>,
    @location(4) uv1: vec2<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = camera.view_proj * vec4<f32>(in.pos, 1.0);
    out.uv = in.uv;
    out.lm_uv = in.lm_uv;
    out.color = in.color;
    out.world_pos = in.pos;
    return out;
}

// Bundle UV per the stage flags: lightmap coords, tcGen vector dots, or the
// time-varying affine; then turb on top.
fn bundle_uv(
    base: vec2<f32>,
    lm_uv: vec2<f32>,
    world_pos: vec3<f32>,
    lm: u32,
    vec_flag: u32,
    affine: mat3x2<f32>,
    basis_s: vec4<f32>,
    basis_t: vec4<f32>,
    turb: vec2<f32>,
) -> vec2<f32> {
    var uv: vec2<f32>;
    if (lm != 0u) {
        uv = lm_uv;
    } else if (vec_flag != 0u) {
        uv = vec2<f32>(dot(world_pos, basis_s.xyz), dot(world_pos, basis_t.xyz));
    } else {
        // column-major affine: s' = col0*s + col1*t + col2
        uv = affine * vec3<f32>(base, 1.0);
    }
    if (turb.x != 0.0 || turb.y != 0.0) {
        uv.x += turb.x * sin(world_pos.x / 1024.0 + turb.y);
        uv.y += turb.x * sin(world_pos.y / 1024.0 + turb.y);
    }
    return uv;
}

@vertex
fn vs_stage(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = camera.view_proj * vec4<f32>(in.pos, 1.0);
    out.color = in.color;
    out.world_pos = in.pos;
    out.lm_uv = in.lm_uv;
    out.uv = bundle_uv(
        in.uv,
        in.lm_uv,
        in.pos,
        stage.flags & F_BUNDLE0_LIGHTMAP,
        stage.flags & F_BUNDLE0_VECTOR,
        stage.uv0,
        stage.vec0_s,
        stage.vec0_t,
        stage.turb01.xy,
    );
    out.uv1 = bundle_uv(
        in.uv,
        in.lm_uv,
        in.pos,
        stage.flags & F_BUNDLE1_LIGHTMAP,
        stage.flags & F_BUNDLE1_VECTOR,
        stage.uv1,
        stage.vec1_s,
        stage.vec1_t,
        stage.turb01.zw,
    );
    return out;
}

// Quadratic falloff to zero at the radius; zero-radius slots are unused.
fn fx_light_term(world_pos: vec3<f32>) -> vec3<f32> {
    var sum = vec3(0.0);
    for (var i = 0u; i < 8u; i++) {
        let l = fx_lights.pos_radius[i];
        if (l.w <= 0.0) { continue; }
        let d = distance(world_pos, l.xyz);
        let a = clamp(1.0 - d / l.w, 0.0, 1.0);
        sum += fx_lights.color[i].rgb * a * a;
    }
    return sum;
}

fn shade(in: VsOut, albedo: vec4<f32>) -> vec3<f32> {
    // Lightmapped surfaces have white vertex color; vertex-lit surfaces get a
    // white 1x1 lightmap bound instead. Multiplying all three covers both.
    let light = textureSample(t_lightmap, s_lightmap, in.lm_uv).rgb;
    return albedo.rgb * (light * in.color.rgb * 2.0 + fx_light_term(in.world_pos));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let albedo = textureSample(t_diffuse, s_diffuse, in.uv);
    if (albedo.a < 0.5) { discard; }
    return vec4<f32>(shade(in, albedo), 1.0);
}

// Alpha-to-coverage: sampled alpha drives MSAA coverage for antialiased cutout edges.
@fragment
fn fs_overlay(in: VsOut) -> @location(0) vec4<f32> {
    let albedo = textureSample(t_diffuse, s_diffuse, in.uv);
    return vec4<f32>(shade(in, albedo), albedo.a);
}

// polygonOffset terrain blends, weighted by vertex alpha (`alphagen vertex`).
@fragment
fn fs_layer(in: VsOut) -> @location(0) vec4<f32> {
    let albedo = textureSample(t_diffuse, s_diffuse, in.uv);
    return vec4<f32>(shade(in, albedo), albedo.a * in.color.a);
}

// alphaFunc decode; GE128 carries both low bits so test the pair first.
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

// Authored shader-script stage: multiply up to two bundles, tint and vertex
// colour per flags, discard by alphaFunc. The lightmap is just another bundle
// here, so no implicit shade() boost or fx lights.
@fragment
fn fs_stage(in: VsOut) -> @location(0) vec4<f32> {
    var c0: vec4<f32>;
    if ((stage.flags & F_BUNDLE0_LIGHTMAP) != 0u) {
        c0 = textureSample(t_lightmap, s_lightmap, in.uv);
    } else {
        c0 = textureSample(t_diffuse, s_diffuse, in.uv);
    }
    var c1: vec4<f32>;
    if ((stage.flags & F_BUNDLE1_LIGHTMAP) != 0u) {
        c1 = textureSample(t_lightmap, s_lightmap, in.uv1);
    } else {
        // white (or the lightmap page) is bound when bundle 1 has no image
        c1 = textureSample(t_bundle1, s_diffuse, in.uv1);
    }

    var col = c0 * c1 * vec4<f32>(stage.tint.rgb, 1.0);
    if ((stage.flags & F_VERTEX_RGB) != 0u) {
        col = vec4<f32>(col.rgb * in.color.rgb, col.a);
    }
    col.a = c0.a * c1.a * stage.tint.a;
    if ((stage.flags & F_VERTEX_ALPHA) != 0u) {
        col.a *= in.color.a;
    }
    if (!alphafunc_pass(col.a)) {
        discard;
    }
    return vec4<f32>(col.rgb, col.a);
}
