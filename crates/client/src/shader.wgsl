struct Camera {
    view_proj: mat4x4<f32>,
    // .x seconds since start; .y identityLight (0.5 full screen, 1 windowed);
    // .z 1 with vc_bicubic
    time_pad: vec4<f32>,
    // xyz view origin; w fog mode: 0 off, 1 GL_EXP, 2 GL_LINEAR (configstring 12)
    eye_fog_mode: vec4<f32>,
    // rgb fog colour, a density (GL_EXP)
    fog_color_density: vec4<f32>,
    // x near, y far (GL_LINEAR); z 1: radial distance (CoD2); w the exp
    // fog's start distance (CoD4, 0 before it)
    fog_range: vec4<f32>,
    // xyz unit view forward; fog depth rides along it
    view_fwd: vec4<f32>,
    // A moved brush model's pose; identity for the world's own draw.
    model: mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> camera: Camera;

struct FxLights {
    pos_radius: array<vec4<f32>, 8>,
    color: array<vec4<f32>, 8>,
}
@group(0) @binding(1) var<uniform> fx_lights: FxLights;

@group(1) @binding(0) var t_diffuse: texture_2d<f32>;
@group(1) @binding(1) var t_lightmap: texture_2d<f32>;
@group(1) @binding(3) var s_lightmap: sampler;
// Per lightmap texel, how far its block runs [left, up, right, down]
// (lightmap_blocks.rs); vc_bicubic clamps its taps to them.
@group(1) @binding(4) var t_lm_blocks: texture_2d<u32>;

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
    // sky dome draws: view origin added to the vertex when F_EYE_OFFSET is set
    eye_off: vec4<f32>,
    // the deform records and their kinds (deform.wgsl)
    deform: array<vec4<f32>, 3>,
    deform_kinds: vec4<u32>,
    // a CoD4 falloff stage: [begin rgb, scale], [end rgb, bias]; its kind
    // in eye_off.w (0 distance, 1 angle), a CoD4 eye offset in eye_off.z
    falloff: array<vec4<f32>, 2>,
    // a CoD4 world fx draw (renderer::cod4_worldfx): x featherParms.x,
    // yz distortionScale
    fx: vec4<f32>,
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
const F_EYE_OFFSET: u32 = 256u;
const F_SKY: u32 = 512u;
// rgbGen vertex: vertex rgb scaled by identityLight
const F_VERTEX_HALF: u32 = 1024u;
// blended onto GL_ONE: fog towards black (0x4d2ad0)
const F_FOG_BLACK: u32 = 2048u;
// identityLighting, constLighting, wave: the tint rgb scales by identityLight
const F_TINT_LIGHT: u32 = 4096u;
// tcGen environment on bundle 0 / 1
const F_BUNDLE0_ENV: u32 = 16384u;
const F_BUNDLE1_ENV: u32 = 32768u;
// bundle 1's texture environment (a nextbundle's blendFunc): GL_ADD, GL_DECAL
const F_ENV1_ADD: u32 = 65536u;
const F_ENV1_DECAL: u32 = 131072u;
// lightingDiffuse: the world has no light set to give it; a prop's baked
// vertex colour stands in, rescaled as vs_prop does
const F_LIT: u32 = 8192u;
// the sprite deforms over vertices vcod_common::deform encoded at load
const F_AUTOSPRITE: u32 = 1048576u;
const F_AUTOSPRITE2: u32 = 2097152u;
// `nofog` shaders (markShadow)
const F_NOFOG: u32 = 4194304u;
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
    // invariant: the dlight pass redraws these surfaces depth-equal
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) lm_uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    // BSP vertices are world space at the brush model's spawn pose.
    @location(3) world_pos: vec3<f32>,
    @location(4) uv1: vec2<f32>,
    // CoD2's fog amount, per vertex as its techniques' `Fog_VertexCalc`
    // writes it (cod2-client-perf.md 9; `pixel_fog`); 0 elsewhere
    @location(5) fog: f32,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    return map_vertex(in);
}

fn map_vertex(in: VsIn) -> VsOut {
    var out: VsOut;
    let world = camera.model * vec4<f32>(in.pos, 1.0);
    out.clip = camera.view_proj * world;
    out.uv = in.uv;
    out.lm_uv = in.lm_uv;
    out.color = in.color;
    out.world_pos = world.xyz;
    out.fog = vertex_fog(world.xyz);
    // legacy draws have no second bundle
    out.uv1 = vec2<f32>(0.0);
    return out;
}

// tcGen environment (RB_CalcEnvironmentTexCoords, CoDMP.exe 0x514d40): the
// unit vector to the eye reflected about the normal, both in the model's
// frame; s from its y, t from its z. The axes are the model's y and z in
// the frame given (StageParams' vec*_s / vec*_t, a static prop's own axes).
fn env_uv(to_eye: vec3<f32>, n: vec3<f32>, s_axis: vec3<f32>, t_axis: vec3<f32>) -> vec2<f32> {
    let v = normalize(to_eye);
    let r = 2.0 * dot(n, v) * n - v;
    return vec2<f32>(0.5 + 0.5 * dot(r, s_axis), 0.5 - 0.5 * dot(r, t_axis));
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
    // RB_CalcTurbulentTexCoords (tr_shade_calc.c:917): s rides x+z, t rides y
    if (turb.x != 0.0 || turb.y != 0.0) {
        uv.x += turb.x * sin((world_pos.x + world_pos.z) / 1024.0 + turb.y);
        uv.y += turb.x * sin(world_pos.y / 1024.0 + turb.y);
    }
    return uv;
}

@vertex
fn vs_stage(in: VsIn) -> VsOut {
    var out: VsOut;
    // sky dome vertices are box-space; the view origin rides in StageParams
    let deformed = deform_vertex(in.pos, in.normal, in.uv);
    var normal = deformed.normal;
    var pos = deformed.pos;
    // autosprite (0x514080): the stamp corner in uv, the half-width in
    // lm_uv.x; left and up are the view's, read off view_proj's rows (a
    // symmetric frustum keeps x and y free of z)
    if ((stage.flags & F_AUTOSPRITE) != 0u) {
        let vp = camera.view_proj;
        let right = normalize(vec3<f32>(vp[0].x, vp[1].x, vp[2].x));
        let up = normalize(vec3<f32>(vp[0].y, vp[1].y, vp[2].y));
        let r = in.lm_uv.x;
        pos = pos + right * ((2.0 * in.uv.x - 1.0) * r) + up * ((1.0 - 2.0 * in.uv.y) * r);
        normal = -camera.view_fwd.xyz;
    }
    // autosprite2 (0x514330): out from the end's midpoint across the long
    // axis (in.normal) and the view
    if ((stage.flags & F_AUTOSPRITE2) != 0u) {
        let c = cross(in.normal, camera.view_fwd.xyz);
        if (dot(c, c) > 0.0) {
            pos = pos + normalize(c) * in.lm_uv.x;
        }
        normal = -camera.view_fwd.xyz;
    }
    if ((stage.flags & F_EYE_OFFSET) != 0u) {
        pos = pos + stage.eye_off.xyz;
    }
    let world = camera.model * vec4<f32>(pos, 1.0);
    out.clip = camera.view_proj * world;
    // CoD4's eyeOffsetParms.x in eye_off.z (the sky's flag unset): the
    // vertex that many units nearer along its view ray, the same pixel
    // nearer in depth (`*_eo*` vertex shaders, cod4-world.md 9.5)
    let eye_pull = stage.eye_off.z;
    if ((stage.flags & F_EYE_OFFSET) == 0u && eye_pull != 0.0 && out.clip.w > eye_pull) {
        let eye = camera.eye_fog_mode.xyz;
        let near = eye + (world.xyz - eye) * ((out.clip.w - eye_pull) / out.clip.w);
        out.clip = camera.view_proj * vec4<f32>(near, 1.0);
    }
    out.color = in.color;
    if (stage.falloff[0].w != 0.0) {
        if (stage.eye_off.w == 0.0) {
            // vertcol_simple_fog_df: the falloff over the xy distance from
            // the eye to the texture coordinates (cod4-world.md 9.4)
            let d = distance(camera.eye_fog_mode.xy, in.uv);
            let w = saturate(stage.falloff[0].w * d + stage.falloff[1].w);
            out.color = vec4<f32>(
                in.color.rgb * mix(stage.falloff[1].rgb, stage.falloff[0].rgb, w),
                in.color.a * w,
            );
        } else {
            // vertcol_simple_fog_foa: the falloff over the squared cosine
            // between the view ray and the normal (cod4-world.md 9.5)
            let m = mat3x3<f32>(camera.model[0].xyz, camera.model[1].xyz, camera.model[2].xyz);
            let c = dot(normalize(world.xyz - camera.eye_fog_mode.xyz), normalize(m * normal));
            let w = saturate(stage.falloff[0].w * c * c + stage.falloff[1].w);
            out.color = vec4<f32>(
                in.color.rgb * w * mix(stage.falloff[1].rgb, stage.falloff[0].rgb, w),
                in.color.a,
            );
        }
    }
    out.world_pos = world.xyz;
    out.fog = vertex_fog(world.xyz);
    out.lm_uv = in.lm_uv;
    // the eye in the brush model's frame; `camera.model` is a rigid pose
    let m3 = mat3x3<f32>(camera.model[0].xyz, camera.model[1].xyz, camera.model[2].xyz);
    let to_eye = transpose(m3) * (camera.eye_fog_mode.xyz - world.xyz);
    let env0 = env_uv(to_eye, normal, stage.vec0_s.xyz, stage.vec0_t.xyz);
    let env1 = env_uv(to_eye, normal, stage.vec1_s.xyz, stage.vec1_t.xyz);
    out.uv = bundle_uv(
        select(in.uv, env0, (stage.flags & F_BUNDLE0_ENV) != 0u),
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
        select(in.uv, env1, (stage.flags & F_BUNDLE1_ENV) != 0u),
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

// The lightmap at `uv`: bilinear as retail, or with vc_bicubic a cubic
// B-spline over the 4x4 texels around it. The B-spline is smooth and never
// overshoots, so a shadow edge softens without the dark and bright halos
// Catmull-Rom rings with; it keeps flat and linear gradients as they are.
// Taps clamp to the block of the texel under `uv`, as ClampToEdge does for
// a whole page, since blocks sit flush with no gutter. The B-spline does
// not pass through its texels, so across the block's outer texel it fades
// back to bilinear: at the edge texel's centre it reads that texel, as
// bilinear does, and a seam between two blocks stays where retail has it.
fn lightmap(uv: vec2<f32>) -> vec4<f32> {
    if (camera.time_pad.z == 0.0) {
        return textureSample(t_lightmap, s_lightmap, uv);
    }
    let dims = vec2<i32>(textureDimensions(t_lightmap));
    if (dims.x == 1) {
        // a vertex-lit surface's white texel
        return textureLoad(t_lightmap, vec2(0), 0);
    }
    let p = uv * vec2<f32>(dims) - 0.5;
    let base = floor(p);
    let f = p - base;
    let i = vec2<i32>(base);
    let own = clamp(vec2<i32>(floor(uv * vec2<f32>(dims))), vec2(0), dims - 1);
    let run = vec4<i32>(textureLoad(t_lm_blocks, own, 0));
    let lo = own - run.xy;
    let hi = own + run.zw;
    // uniform cubic B-spline weights for taps i-1 .. i+2, blended per axis
    // with bilinear's (0, 1-f, f, 0) by the distance to the block's edge
    let f2 = f * f;
    let f3 = f2 * f;
    let edge = min(p - vec2<f32>(lo), vec2<f32>(hi) - p);
    let b = smoothstep(vec2(0.0), vec2(1.0), edge);
    let w0 = b * (1.0 - 3.0 * f + 3.0 * f2 - f3) / 6.0;
    let w1 = mix(1.0 - f, (4.0 - 6.0 * f2 + 3.0 * f3) / 6.0, b);
    let w2 = mix(f, (1.0 + 3.0 * f + 3.0 * f2 - 3.0 * f3) / 6.0, b);
    let w3 = b * f3 / 6.0;
    // Four bilinear taps, one per pair of texels on each axis, placed so
    // the hardware's split matches the pair's weights. Clamping two
    // neighbours keeps them neighbours or folds them onto one texel, whose
    // centre the tap then hits, so a tap never reads past the block.
    let a0 = clamp(i - 1, lo, hi);
    let a1 = clamp(i + 1, lo, hi);
    let g0 = w0 + w1;
    let g1 = max(w2 + w3, vec2(1e-6));
    let step0 = vec2<f32>(clamp(i, lo, hi) - a0);
    let step1 = vec2<f32>(clamp(i + 2, lo, hi) - a1);
    let inv = 1.0 / vec2<f32>(dims);
    let t0 = (vec2<f32>(a0) + 0.5 + step0 * w1 / g0) * inv;
    let t1 = (vec2<f32>(a1) + 0.5 + step1 * w3 / g1) * inv;
    return g0.y * (g0.x * textureSampleLevel(t_lightmap, s_lightmap, t0, 0.0)
            + g1.x * textureSampleLevel(t_lightmap, s_lightmap, vec2(t1.x, t0.y), 0.0))
        + g1.y * (g0.x * textureSampleLevel(t_lightmap, s_lightmap, vec2(t0.x, t1.y), 0.0)
            + g1.x * textureSampleLevel(t_lightmap, s_lightmap, t1, 0.0));
}

// Retail's lightmap bytes (0x4d9af0 at load): raw with an overbright bit;
// without one, doubled and scaled back by the brightest channel, keeping hue.
fn lightmap_shift(c: vec3<f32>) -> vec3<f32> {
    if (camera.time_pad.y < 1.0) {
        return c;
    }
    let v = c * 2.0;
    return v / max(1.0, max(v.r, max(v.g, v.b)));
}

// Retail's dynamic lights on a lit model vertex, in framebuffer units:
// RE_AddLightToScene (0x4e9b00) builds a point light with diffuse
// identityLight * r^2 / 32 * rgb, no ambient and attenuation 1 / (d^2 + 0.001),
// and the light pick (0x4b69f0) takes it within twice its radius
// (cod11-light-grid-and-leaf-lights.md, section 11).
fn dlight_term(p: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    var sum = vec3(0.0);
    for (var i = 0u; i < 8u; i++) {
        let l = fx_lights.pos_radius[i];
        if (l.w <= 0.0) { continue; }
        let to = l.xyz - p;
        let d2 = dot(to, to);
        if (d2 > 4.0 * l.w * l.w) { continue; }
        let lambert = max(dot(n, to * inverseSqrt(max(d2, 1e-6))), 0.0);
        sum += fx_lights.color[i].rgb
            * (camera.time_pad.y * l.w * l.w / 32.0 * lambert / (d2 + 0.001));
    }
    return sum;
}

// Static props: the baked vertex colour is a framebuffer value at
// identityLight 0.5, rescaled to the current one. Vertex alpha marks a
// `lightingDiffuse` skin, the only kind retail relights while a dynamic
// light is near (0x505260); the sum clamps like GL lighting does.
@vertex
fn vs_prop(in: VsIn) -> VsOut {
    var out = map_vertex(in);
    var rgb = min(in.color.rgb * (2.0 * camera.time_pad.y), vec3(1.0));
    if (in.color.a > 0.5) {
        rgb = min(rgb + dlight_term(in.pos, normalize(in.normal)), vec3(1.0));
    }
    out.color = vec4<f32>(rgb, 1.0);
    return out;
}

// glFog GL_EXP / GL_LINEAR factors; retail's two fog modes from
// configstring 12 (RTCW-MP tr_main.c R_SetFog picks the GL mode). The fog
// coordinate is eye-space Z (depth along the view forward), the fixed-
// function default without GL_NV_fog_distance - retail on non-NVIDIA GPUs.
fn fog_amount(world_pos: vec3<f32>) -> f32 {
    let mode = camera.eye_fog_mode.w;
    if (mode == 0.0) {
        return 0.0;
    }
    var d = max(dot(world_pos - camera.eye_fog_mode.xyz, camera.view_fwd.xyz), 0.0);
    // CoD2's fog runs on the radial distance (fog_range.z 1; fogcalc.hlsl)
    if (camera.fog_range.z > 0.5) {
        d = length(world_pos - camera.eye_fog_mode.xyz);
    }
    if (mode == 1.0) {
        return 1.0 - exp(-camera.fog_color_density.a * max(d - camera.fog_range.w, 0.0));
    }
    let span = max(camera.fog_range.y - camera.fog_range.x, 0.0001);
    return clamp((d - camera.fog_range.x) / span, 0.0, 1.0);
}

// CoD2 (fog_range.z 1) fogs per vertex: `VsOut::fog`.
fn vertex_fog(world_pos: vec3<f32>) -> f32 {
    if (camera.fog_range.z > 0.5) {
        return fog_amount(world_pos);
    }
    return 0.0;
}

// The fragment's fog amount: the vertex's on CoD2, else at the pixel.
fn pixel_fog(in: VsOut) -> f32 {
    if (camera.fog_range.z > 0.5) {
        return in.fog;
    }
    return fog_amount(in.world_pos);
}

fn apply_fog(rgb: vec3<f32>, in: VsOut) -> vec3<f32> {
    return mix(rgb, camera.fog_color_density.rgb, pixel_fog(in));
}

fn shade(in: VsOut, albedo: vec4<f32>) -> vec3<f32> {
    // Lightmapped surfaces have white vertex color; vertex-lit surfaces get a
    // white 1x1 lightmap bound instead. Multiplying all three covers both.
    let light = lightmap_shift(lightmap(in.lm_uv).rgb);
    return albedo.rgb * light * in.color.rgb;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let albedo = sample_image(t_diffuse, in.uv, 0u);
    if (albedo.a < 0.5) { discard; }
    return vec4<f32>(apply_fog(shade(in, albedo), in), 1.0);
}

// fs_main without the cut, for an image no texel of which it drops: a
// shader with no discard keeps early depth writes (cod11-client-perf.md 5).
@fragment
fn fs_main_solid(in: VsOut) -> @location(0) vec4<f32> {
    let albedo = sample_image(t_diffuse, in.uv, 0u);
    return vec4<f32>(apply_fog(shade(in, albedo), in), 1.0);
}

// Props: texture times the vertex colour, no lightmap. Only an alpha-testing
// template keeps the alpha this cuts on (renderer::prop_skin_draw).
@fragment
fn fs_prop(in: VsOut) -> @location(0) vec4<f32> {
    let albedo = sample_image(t_diffuse, in.uv, 0u);
    if (albedo.a < 0.5) { discard; }
    return vec4<f32>(apply_fog(albedo.rgb * in.color.rgb, in), 1.0);
}

// fs_prop without the cut, as fs_main_solid.
@fragment
fn fs_prop_solid(in: VsOut) -> @location(0) vec4<f32> {
    let albedo = sample_image(t_diffuse, in.uv, 0u);
    return vec4<f32>(apply_fog(albedo.rgb * in.color.rgb, in), 1.0);
}

// Alpha-to-coverage: sampled alpha drives MSAA coverage for antialiased cutout edges.
@fragment
fn fs_overlay(in: VsOut) -> @location(0) vec4<f32> {
    let albedo = sample_image(t_diffuse, in.uv, 0u);
    return vec4<f32>(apply_fog(shade(in, albedo), in), albedo.a);
}

// polygonOffset terrain blends, weighted by vertex alpha (`alphagen vertex`).
@fragment
fn fs_layer(in: VsOut) -> @location(0) vec4<f32> {
    let albedo = sample_image(t_diffuse, in.uv, 0u);
    return vec4<f32>(apply_fog(shade(in, albedo), in), albedo.a * in.color.a);
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

// Authored shader-script stage: up to two bundles through bundle 1's
// texture environment, tint and vertex colour per flags, discard by
// alphaFunc. A $lightmap bundle carries the same shift as shade(), so a
// staged surface matches the implicit-path surface it is coplanar with
// (terrain overlays over base ground).
@fragment
fn fs_stage(in: VsOut) -> @location(0) vec4<f32> {
    return stage_shade(in);
}

// fs_stage's colour times its alpha: a CoD4 premultiplying pixel shader
// under (OneMinusDstColor, One), which no blend factor can scale by alpha
// (StageVariant::AdditiveInvDst, cod4-world.md 8.1).
@fragment
fn fs_stage_premul(in: VsOut) -> @location(0) vec4<f32> {
    let c = stage_shade(in);
    return vec4<f32>(c.rgb * c.a, c.a);
}

fn stage_shade(in: VsOut) -> vec4<f32> {
    // derivatives before any branch; only vc_shadows' blends read it
    let face_n = cross(dpdx(in.world_pos), dpdy(in.world_pos));
    var c0: vec4<f32>;
    if ((stage.flags & F_BUNDLE0_LIGHTMAP) != 0u) {
        let lm = lightmap(in.uv);
        c0 = vec4<f32>(lightmap_shift(lm.rgb), lm.a);
    } else {
        c0 = sample_image(t_diffuse, in.uv, stage.flags);
    }
    var c1: vec4<f32>;
    if ((stage.flags & F_BUNDLE1_LIGHTMAP) != 0u) {
        let lm = lightmap(in.uv1);
        c1 = vec4<f32>(lightmap_shift(lm.rgb), lm.a);
    } else {
        // white (or the lightmap page) is bound when bundle 1 has no image
        c1 = sample_image(t_bundle1, in.uv1, stage.flags >> 2u);
    }
    let col = stage_colour(c0, c1, in.color);
    if (!alphafunc_pass(col.a)) {
        discard;
    }
    // Linear fog leaves the sky alone (RTCW drawsky=false); exp fogs it.
    let sky_unfogged = (stage.flags & F_SKY) != 0u && camera.eye_fog_mode.w != 1.0;
    var rgb = col.rgb;
    if ((stage.flags & F_SUN_SHADOW) != 0u) {
        rgb *= sun_shadow(in.world_pos, face_n, camera.eye_fog_mode.xyz);
    }
    if (!sky_unfogged && (stage.flags & F_NOFOG) == 0u) {
        if ((stage.flags & F_FOG_BLACK) != 0u) {
            rgb *= 1.0 - pixel_fog(in);
        } else {
            rgb = apply_fog(rgb, in);
        }
    }
    return vec4<f32>(rgb, col.a);
}

// A CoD2 multiply decal (`vertcol_mul*`, mul.hlsl): the colour map times
// the vertex rgb, faded to white by the vertex alpha (the map's alpha is
// unread), blended Zero/SrcColor so the framebuffer is multiplied by it. A
// fogged one fades to white with the fog: one pass in place of retail's
// additive fix-up pass (cod2-materials.md 9, "Multiply").
@fragment
fn fs_multiply(in: VsOut) -> @location(0) vec4<f32> {
    let c0 = sample_image(t_diffuse, in.uv, stage.flags);
    var m = mix(vec3<f32>(1.0), c0.rgb * in.color.rgb, in.color.a);
    if ((stage.flags & F_NOFOG) == 0u) {
        m = mix(m, vec3<f32>(1.0), pixel_fog(in));
    }
    return vec4<f32>(m, 1.0);
}

// Fixed-function texturing: unit 0 modulates bundle 0 by the stage colour
// (tint, then vertex colour per flags); unit 1 takes bundle 1 through its
// texture environment.
fn stage_colour(c0: vec4<f32>, c1: vec4<f32>, vcol: vec4<f32>) -> vec4<f32> {
    var tint = stage.tint.rgb;
    // clamped after the scale: a table rgbGen wave arrives unclamped
    // above and retail clamps its scaled bytes (0x514c00)
    if ((stage.flags & F_TINT_LIGHT) != 0u) {
        tint = min(tint * camera.time_pad.y, vec3(1.0));
    }
    var rgb = tint;
    if ((stage.flags & F_LIT) != 0u) {
        rgb = min(vcol.rgb * (2.0 * camera.time_pad.y), vec3(1.0));
    }
    if ((stage.flags & F_VERTEX_RGB) != 0u) {
        var vrgb = vcol.rgb;
        if ((stage.flags & F_VERTEX_HALF) != 0u) {
            vrgb = vrgb * camera.time_pad.y;
        }
        rgb = rgb * vrgb;
    }
    var a = stage.tint.a;
    if ((stage.flags & F_VERTEX_ALPHA) != 0u) {
        a *= vcol.a;
    }
    let u0 = vec4<f32>(c0.rgb * rgb, c0.a * a);
    if ((stage.flags & F_ENV1_ADD) != 0u) {
        return vec4<f32>(u0.rgb + c1.rgb, u0.a * c1.a);
    }
    if ((stage.flags & F_ENV1_DECAL) != 0u) {
        return vec4<f32>(mix(u0.rgb, c1.rgb, c1.a), u0.a);
    }
    return u0 * c1;
}

// ---- the dlight pass (CoDMP.exe 0x500a10) ----

// One light per draw, a dynamic-offset slot each (dlight::DlightUniform).
struct Dlight {
    // xyz origin; w the intensity, which is the projection's radius
    origin_radius: vec4<f32>,
    // rgb the raw colour; w 1 with r_dlightQuality on
    color: vec4<f32>,
};
@group(3) @binding(0) var<uniform> dlight: Dlight;
// the $dlight image (assets::dlight_blob), clamped
@group(3) @binding(1) var t_dlight: texture_2d<f32>;
@group(3) @binding(2) var s_dlight: sampler;

struct DlVsIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) lm_uv: vec2<f32>,
    @location(3) normal: vec3<f32>,
    @location(4) color: vec4<f32>,
    // the soup's tangent s axis (dlight::soup_s_axes)
    @location(5) s_axis: vec3<f32>,
};

struct DlVsOut {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) uv1: vec2<f32>,
    // the light's colour by the depth falloff, the pass's vertex colour
    @location(2) color: vec4<f32>,
    // per-vertex outcodes, 1 past a plane: s < 0, t < 0, depth < -r ...
    @location(3) out_lo: vec3<f32>,
    // ... and s > 1, t > 1, depth > r
    @location(4) out_hi: vec3<f32>,
    @location(5) world_pos: vec3<f32>,
};

// 0x4ff690 per vertex: the light's offset projected on the vertex's tangent
// frame (r_dlightQuality 1; with 0 the model's x, y and z axes) gives the
// dlight image's coordinates, which replace the lightmap's; along the normal
// it fades the colour, full within half the radius, to zero at the radius,
// written as colour bytes.
@vertex
fn vs_dlight(in: DlVsIn) -> DlVsOut {
    var out: DlVsOut;
    let deformed = deform_vertex(in.pos, in.normal, in.uv);
    let normal = deformed.normal;
    let pos = deformed.pos;
    let world = camera.model * vec4<f32>(pos, 1.0);
    out.clip = camera.view_proj * world;
    out.world_pos = world.xyz;
    let m3 = mat3x3<f32>(camera.model[0].xyz, camera.model[1].xyz, camera.model[2].xyz);
    var n = m3 * normal;
    var s_axis = m3 * in.s_axis;
    let c = cross(n, s_axis);
    var t_axis = vec3<f32>(0.0);
    if (dot(c, c) > 0.0) {
        t_axis = normalize(c);
    }
    if (dlight.color.w == 0.0) {
        s_axis = m3[0];
        t_axis = m3[1];
        n = m3[2];
    }
    let r = dlight.origin_radius.w;
    let d = dlight.origin_radius.xyz - world.xyz;
    let st = vec2<f32>(dot(d, s_axis), dot(d, t_axis)) / r + 0.5;
    let z = dot(d, n);
    out.out_lo = vec3<f32>(f32(st.x < 0.0), f32(st.y < 0.0), f32(z < -r));
    out.out_hi = vec3<f32>(f32(st.x > 1.0), f32(st.y > 1.0), f32(z > r));
    var m = 0.0;
    if (abs(z) <= r) {
        m = select(1.0, 2.0 * (r - abs(z)) / r, abs(z) >= 0.5 * r);
    }
    out.color = vec4<f32>(clamp(round(m * dlight.color.rgb * 255.0), vec3(0.0), vec3(255.0)) / 255.0, 1.0);
    let to_eye = transpose(m3) * (camera.eye_fog_mode.xyz - world.xyz);
    let env0 = env_uv(to_eye, normal, stage.vec0_s.xyz, stage.vec0_t.xyz);
    let env1 = env_uv(to_eye, normal, stage.vec1_s.xyz, stage.vec1_t.xyz);
    out.uv = bundle_uv(
        select(in.uv, env0, (stage.flags & F_BUNDLE0_ENV) != 0u),
        st,
        in.pos,
        stage.flags & F_BUNDLE0_LIGHTMAP,
        stage.flags & F_BUNDLE0_VECTOR,
        stage.uv0,
        stage.vec0_s,
        stage.vec0_t,
        stage.turb01.xy,
    );
    out.uv1 = bundle_uv(
        select(in.uv, env1, (stage.flags & F_BUNDLE1_ENV) != 0u),
        st,
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

// A bundle on the lightmap's coordinates samples the dlight image there.
// A fogged stage (no `nofog`) fogs towards black, as GL_State gives every
// stage blended onto GL_ONE (0x4d2ad0), so the light fades with the fog;
// every dlight stage blends onto GL_ONE (shader doc 3).
@fragment
fn fs_dlight(in: DlVsOut) -> @location(0) vec4<f32> {
    var c0: vec4<f32>;
    if ((stage.flags & F_BUNDLE0_LIGHTMAP) != 0u) {
        c0 = textureSample(t_dlight, s_dlight, in.uv);
    } else {
        c0 = sample_image(t_diffuse, in.uv, stage.flags);
    }
    var c1: vec4<f32>;
    if ((stage.flags & F_BUNDLE1_LIGHTMAP) != 0u) {
        c1 = textureSample(t_dlight, s_dlight, in.uv1);
    } else {
        c1 = sample_image(t_bundle1, in.uv1, stage.flags >> 2u);
    }
    // 0x4ff690 keeps a triangle unless all three vertices are past one
    // plane, the only case where an interpolated outcode reaches 1
    if (any(in.out_lo > vec3(0.999)) || any(in.out_hi > vec3(0.999))) {
        discard;
    }
    let col = stage_colour(c0, c1, in.color);
    if (!alphafunc_pass(col.a)) {
        discard;
    }
    var rgb = col.rgb;
    if ((stage.flags & F_NOFOG) == 0u) {
        rgb *= 1.0 - fog_amount(in.world_pos);
    }
    return vec4<f32>(rgb, col.a);
}

// ---- CoD4 world fx (renderer::cod4_worldfx) ----

// Group 3 of the world fx pipelines (crate::scene_copy): the soft
// particles' linear view depth (retail's floatZ), the frame's colour copy
// (resolvedPostSun) and its sampler, clear of the dlight pass's bindings.
@group(3) @binding(5) var wfx_linz: texture_2d<f32>;
@group(3) @binding(6) var wfx_copy: texture_2d<f32>;
@group(3) @binding(7) var wfx_samp: sampler;

// A world or static model zfeather set's two fades (zfeather_*.hlsl,
// cod4-effects.md 14): the near fade saturate(w k), which retail folds into
// the vertex alpha, times the depth feather saturate(|floatZ - w| k), both
// on the view depth w and featherParms.x k.
fn world_feather(in: VsOut) -> f32 {
    let k = stage.fx.x;
    let w = dot(in.world_pos - camera.eye_fog_mode.xyz, camera.view_fwd.xyz);
    let scene = textureLoad(wfx_linz, vec2<i32>(in.clip.xy), 0).r;
    return saturate(w * k) * saturate(abs(scene - w) * k);
}

// Blends that read the source alpha: the fade scales it.
@fragment
fn fs_stage_feather(in: VsOut) -> @location(0) vec4<f32> {
    let c = stage_shade(in);
    return vec4<f32>(c.rgb, c.a * world_feather(in));
}

// (One, One) and the colour multiplies, which read no source alpha:
// zfeather_add*.hlsl scales the colour by the fade too.
@fragment
fn fs_stage_feather_rgb(in: VsOut) -> @location(0) vec4<f32> {
    let c = stage_shade(in);
    let f = world_feather(in);
    return vec4<f32>(c.rgb * f, c.a * f);
}

// fs_stage_premul with the fade in its alpha.
@fragment
fn fs_stage_premul_feather(in: VsOut) -> @location(0) vec4<f32> {
    let c = stage_shade(in);
    let a = c.a * world_feather(in);
    return vec4<f32>(c.rgb * a, a);
}

struct DistortIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) lm_uv: vec2<f32>,
    @location(3) normal: vec3<f32>,
    @location(4) color: vec4<f32>,
    @location(5) tangent: vec3<f32>,
    @location(6) binormal: vec3<f32>,
};

struct DistortOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    // the screen lookup and its two bends, projected (o5, o3, o4)
    @location(2) look: vec4<f32>,
    @location(3) bend_t: vec4<f32>,
    @location(4) bend_b: vec4<f32>,
};

// distortion_scale_zfeather.hlsl, the world set's technique 5 (cod4-world.md
// 9.5, cod4-effects.md 17): the screen lookup (clip x and y times
// clipSpaceLookupScale (0.5, -0.5) plus w times its offset (0.5, 0.5)),
// and the tangent and binormal (tangent x normal) turned into clip space
// without the translation, scaled by distortionScale and the vertex red
// and green; the bytecode's tangent terms keep +0.5 on y.
@vertex
fn vs_world_distort(in: DistortIn) -> DistortOut {
    var out: DistortOut;
    let world = camera.model * vec4<f32>(in.pos, 1.0);
    let c = camera.view_proj * world;
    out.clip = c;
    out.uv = in.uv;
    out.color = in.color;
    let m3 = mat3x3<f32>(camera.model[0].xyz, camera.model[1].xyz, camera.model[2].xyz);
    let t = m3 * in.tangent;
    let b = cross(t, m3 * in.normal);
    let tc = camera.view_proj * vec4<f32>(t, 0.0);
    let bc = camera.view_proj * vec4<f32>(b, 0.0);
    out.look = vec4<f32>(c.x * 0.5 + c.w * 0.5, -c.y * 0.5 + c.w * 0.5, 0.0, c.w);
    out.bend_t = stage.fx.y * in.color.r * vec4<f32>(tc.x * 0.5, tc.y * 0.5, 0.0, tc.w);
    out.bend_b = -stage.fx.z * in.color.g * vec4<f32>(bc.x * 0.5, bc.y * 0.5, 0.0, bc.w);
    return out;
}

// CoD2's clip z on retail's projection, as fx.wgsl's FEATHER_K and
// FEATHER_NEAR: the distortion shader's depth test reads it.
const WFX_CLIP_K: f32 = 0.99950027;
const WFX_CLIP_NEAR: f32 = 4.0;

// distortion_zfeather.hlsl: the normal map (the material's colour map)
// as 2 n - 1 bends the lookup into the frame's copy; where the copy's
// depth there is nearer than this surface the plain lookup is read. The
// colour is the copy times the vertex blue.
@fragment
fn fs_world_distort(in: DistortOut) -> @location(0) vec4<f32> {
    let n = sample_image(t_diffuse, in.uv, stage.flags);
    let off = n.xy * 2.0 - 1.0;
    let q = in.look + off.x * in.bend_t + off.y * in.bend_b;
    let bent = q.xy / q.w;
    let flat_uv = in.look.xy / in.look.w;
    let dims = vec2<f32>(textureDimensions(wfx_linz));
    let texel = clamp(vec2<i32>(bent * dims), vec2(0), vec2<i32>(dims) - 1);
    let z = textureLoad(wfx_linz, texel, 0).r;
    let bent_rgb = textureSampleLevel(wfx_copy, wfx_samp, bent, 0.0).rgb;
    let flat_rgb = textureSampleLevel(wfx_copy, wfx_samp, flat_uv, 0.0).rgb;
    let rgb = select(flat_rgb, bent_rgb, abs(z) - WFX_CLIP_K * (in.look.w - WFX_CLIP_NEAR) >= 0.0);
    return vec4<f32>(rgb * in.color.b, n.a * in.color.a);
}
