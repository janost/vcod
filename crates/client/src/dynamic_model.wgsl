// Snapshot entities: GPU-skinned xmodel instances in world space. A
// material stage is lit per vertex by its light set as retail's GL lighting
// does, or draws its constant or wave colour
// (cod11-light-grid-and-leaf-lights.md, section 13).

struct Camera {
    view_proj: mat4x4<f32>,
    time_pad: vec4<f32>, // .x seconds since start; .y identityLight
    // xyz view origin; w fog mode: 0 off, 1 GL_EXP, 2 GL_LINEAR (configstring 12)
    eye_fog_mode: vec4<f32>,
    // rgb fog colour, a density (GL_EXP)
    fog_color_density: vec4<f32>,
    // x near, y far (GL_LINEAR); z 1: radial distance (CoD2); w the exp
    // fog's start distance (CoD4, 0 before it)
    fog_range: vec4<f32>,
    // xyz unit view forward; fog depth rides along it
    view_fwd: vec4<f32>,
};
@group(0) @binding(0) var<uniform> camera: Camera;
// CoD2's screen dynamic shadow (renderer::cod2_cookie::DynamicShadows), the
// share of the sun the shadow cookies leave, 1 where none reaches; a CoD2
// probe stage's sun takes it at the pixel (lib/lightprobe.hlsl's
// dynamicShadowSampler).
@group(0) @binding(17) var t_dyn_shadow: texture_2d<f32>;

// `tex2Dproj( dynamicShadowSampler, screenCoords )`: `sc` the vertex's
// [`screen_coords`].
fn dyn_shadow(sc: vec4<f32>) -> f32 {
    return textureSampleLevel(t_dyn_shadow, s_lin_clamp, sc.xy / sc.w, 0.0).r;
}

// `Transform_ClipSpacePosToTexCoords` (lib/transform.hlsl): the clip
// position to the screen's texture coordinates times w, for a projective
// read in the pixel.
fn screen_coords(clip: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(0.5 * (clip.x + clip.w), 0.5 * (clip.w - clip.y), 0.0, clip.w);
}

@group(1) @binding(0) var t_diffuse: texture_2d<f32>;
// All instances' bone matrices, world space. Slot 0 is the shared identity
// block (bind pose).
@group(2) @binding(0) var<storage, read> bones: array<mat4x4<f32>>;

// One GL light; `entity_light::GpuLight`.
struct EntLight {
    pos: vec4<f32>,      // w 0: xyz is the unit vector towards the light
    diffuse: vec4<f32>,  // w spot exponent
    ambient: vec4<f32>,  // w cosine of the spot cutoff, below -1 for none
    atten: vec4<f32>,    // constant, linear, quadratic
    spot_dir: vec4<f32>,
};
struct LightSet {
    ambient: vec4<f32>,  // light model ambient; w the light count, -1 a CoD2 probe
    lights: array<EntLight, 8>,
};
// The frame's light sets; an instance names its own.
@group(2) @binding(1) var<storage, read> light_sets: array<LightSet>;

// The stage's `renderer::StageParams`; only the fields read here are named.
struct StageParams {
    uv0: mat3x2<f32>,
    uv1: mat3x2<f32>,
    turb01: vec4<f32>,
    // rgbGen colour and alphaGen value
    tint: vec4<f32>,
    flags: u32,
    // bundle 0's lone scroll (s, t per second) or rotate (degrees per
    // second), for an instance whose material clock runs its own time
    rate_s: f32,
    rate_t: f32,
    rate_rot: f32,
    // tcGen vector axes and the sky's eye offset, unread here
    skip: array<vec4<f32>, 5>,
    // the deform records and their kinds (deform.wgsl)
    deform: array<vec4<f32>, 3>,
    deform_kinds: vec4<u32>,
};
@group(3) @binding(0) var<uniform> stage: StageParams;

const F_AF_GT0: u32 = 16u;
const F_AF_LT128: u32 = 32u;
const F_AF_GE128: u32 = 48u;
// identityLighting, constLighting, wave: the tint scales by identityLight
const F_TINT_LIGHT: u32 = 4096u;
// lightingDiffuse: GL lighting from the light set
const F_LIT: u32 = 8192u;
// tcGen environment on bundle 0
const F_BUNDLE0_ENV: u32 = 16384u;
// renderer::STAGE_FLAG_VERTEX_RGB, _ALPHA, _RGB_HALF
const F_VERTEX_RGB: u32 = 1u;
const F_VERTEX_ALPHA: u32 = 2u;
const F_VERTEX_HALF: u32 = 1024u;
// renderer::STAGE_FLAG_FOG_BLACK, STAGE_FLAG_NOFOG
const F_FOG_BLACK: u32 = 2048u;
const F_NOFOG: u32 = 4194304u;
const ATEST128: f32 = 0.5019607843137255;
// renderer::DEFORM_COD2_FLAG in `deform_kinds.w`, which deform.wgsl never
// reads: a CoD2 `lprobe_flag*` stage (cod2_model.wgsl's `cod2_flap`)
const COD2_FLAG: u32 = 1u;

// GL_LIGHTING's vertex colour: the light model ambient plus each light's
// attenuated ambient and diffuse, clamped to 1. White material, no
// specular, one-sided. A CoD2 probe set takes probe_lighting instead.
fn gl_lighting(ls: LightSet, p: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    if (ls.ambient.w < 0.0) {
        return probe_lighting(ls, n);
    }
    var c = ls.ambient.rgb;
    let count = min(u32(ls.ambient.w), 8u);
    for (var i = 0u; i < count; i++) {
        let l = ls.lights[i];
        var to_l = l.pos.xyz;
        var att = 1.0;
        if (l.pos.w != 0.0) {
            let v = l.pos.xyz - p;
            let d = length(v);
            to_l = v / max(d, 1e-6);
            att = 1.0 / (l.atten.x + (l.atten.y + l.atten.z * d) * d);
            if (l.ambient.w >= -1.0) {
                let s = dot(-to_l, normalize(l.spot_dir.xyz));
                if (s < l.ambient.w) {
                    att = 0.0;
                } else if (l.diffuse.w > 0.0) {
                    att *= pow(max(s, 0.0), l.diffuse.w);
                }
            }
        }
        c += att * (l.ambient.rgb + max(dot(n, to_l), 0.0) * l.diffuse.rgb);
    }
    return clamp(c, vec3(0.0), vec3(1.0));
}

// tcGen environment (RB_CalcEnvironmentTexCoords, CoDMP.exe 0x514d40): the
// unit vector to the eye reflected about the normal, both in the model's
// frame; s from its y, t from its z.
fn env_uv(to_eye: vec3<f32>, n: vec3<f32>) -> vec2<f32> {
    let v = normalize(to_eye);
    let r = 2.0 * dot(n, v) * n - v;
    return vec2<f32>(0.5 + 0.5 * r.y, 0.5 - 0.5 * r.z);
}

struct VsOut {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) world_pos: vec3<f32>,
    // GL's vertex colour, framebuffer units
    @location(3) light: vec3<f32>,
    // the colour the model stores (CoD2); white on CoD1's models
    @location(4) vcol: vec4<f32>,
    // a CoD2 probe stage's tangent frame and half vector (cod2_model.wgsl)
    @location(5) n0: vec3<f32>,
    @location(6) t2: vec3<f32>,
    @location(7) b2: vec3<f32>,
    @location(8) half_angle: vec3<f32>,
    @location(9) @interpolate(flat) light_set: u32,
    // the material clock, seconds (CoD2's gameTime.w)
    @location(10) game_time: f32,
    // CoD2's fog amount, per vertex as `Fog_VertexCalc` writes it
    // (cod2-client-perf.md 9); CoD1 and CoD4 fog per pixel
    @location(11) fog: f32,
    // CoD2's dynamic shadow lookup (`screen_coords`)
    @location(12) screen: vec4<f32>,
};

@vertex
fn vs_main(
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) bi: vec4<u32>,
    @location(4) bw: vec4<f32>,
    @location(5) m0: vec4<f32>,
    @location(6) m1: vec4<f32>,
    @location(7) m2: vec4<f32>,
    @location(8) m3: vec4<f32>,
    @location(9) bone_base: u32,
    @location(10) light_set: u32,
    @location(11) time_offset: f32,
    @location(12) vcol: vec4<f32>,
    @location(13) tangent: vec3<f32>,
    @location(14) binormal: vec3<f32>,
) -> VsOut {
    let model = mat4x4<f32>(m0, m1, m2, m3);
    var p = vec4<f32>(0.0);
    var n = vec3<f32>(0.0);
    var t = vec3<f32>(0.0);
    var b = vec3<f32>(0.0);
    for (var i = 0u; i < 4u; i++) {
        let m = bones[bone_base + bi[i]];
        p += bw[i] * (m * vec4<f32>(pos, 1.0));
        // rigid bone transforms, so the upper 3x3 is valid for normals
        let r = mat3x3<f32>(m[0].xyz, m[1].xyz, m[2].xyz);
        n += bw[i] * (r * normal);
        t += bw[i] * (r * tangent);
        b += bw[i] * (r * binormal);
    }
    // a CoD2 `lprobe_flag*` stage waves the posed model-space vertex
    // before the entity's transform, as `Flap` does before worldMatrix
    var col = vcol;
    let game_time = camera.time_pad.x + time_offset;
    if (stage.deform_kinds.w == COD2_FLAG) {
        let f = cod2_flap(p.xyz, n, t, b, uv, vcol, game_time);
        p = vec4<f32>(f.pos, 1.0);
        n = f.normal;
        col = f.vcol;
    }
    // a CoD4 `lp_flag_*` stage waves in model space too, and hands on the
    // vertex colour with alpha 1
    if (stage.deform_kinds.w == COD4_FLAG) {
        let f = cod4_flag_wave(p.xyz, n, t, b, uv, vcol.a, game_time);
        p = vec4<f32>(f.pos, 1.0);
        n = f.normal;
        b = f.binormal;
        col = vec4<f32>(vcol.rgb, 1.0);
    }
    // the deforms run on the posed model-space vertices, as on retail's
    // tess before the entity's transform
    let d = deform_vertex(p.xyz, n, uv);
    p = vec4<f32>(d.pos, 1.0);
    n = d.normal;
    let world = model * p;
    var out: VsOut;
    out.clip = camera.view_proj * world;
    out.screen = screen_coords(out.clip);
    // rotation + translation, so the upper 3x3 is valid for the normal
    out.normal = (model * vec4<f32>(n, 0.0)).xyz;
    var base = uv;
    if ((stage.flags & F_BUNDLE0_ENV) != 0u) {
        // world to the entity's frame (its axes are rigid)
        let to_local = transpose(mat3x3<f32>(normalize(m0.xyz), normalize(m1.xyz), normalize(m2.xyz)));
        base = env_uv(
            to_local * (camera.eye_fog_mode.xyz - world.xyz),
            normalize(to_local * out.normal),
        );
    }
    out.uv = stage.uv0 * vec3<f32>(base, 1.0);
    if (time_offset != 0.0) {
        // the stage's own clock runs `time_offset` further: a scroll
        // slides on, a rotate (RB_CalcRotateTexCoords, negated degrees)
        // turns on about the centre
        var uv = out.uv + vec2<f32>(stage.rate_s, stage.rate_t) * time_offset;
        if (stage.rate_rot != 0.0) {
            let a = radians(-stage.rate_rot * time_offset);
            let d = uv - vec2<f32>(0.5);
            uv = vec2<f32>(cos(a) * d.x - sin(a) * d.y, sin(a) * d.x + cos(a) * d.y) + vec2<f32>(0.5);
        }
        out.uv = uv;
    }
    out.world_pos = world.xyz;
    let ls = light_sets[light_set];
    // a probe stage lights per pixel and reads no vertex light
    if ((stage.flags & F_COD2_PROBE) == 0u) {
        out.light = gl_lighting(ls, world.xyz, normalize(out.normal));
    }
    out.vcol = col;
    let rot = mat3x3<f32>(model[0].xyz, model[1].xyz, model[2].xyz);
    let fr = cod2_frame(rot * n, rot * t, rot * b);
    out.n0 = fr.n0;
    out.t2 = fr.t2;
    out.b2 = fr.b2;
    out.half_angle = normalize(normalize(camera.eye_fog_mode.xyz - world.xyz) + ls.lights[1].diffuse.xyz);
    if (ls.ambient.w < -1.5) {
        // a CoD4 set (cod2_model.wgsl's `cod4_model`) reads the eye-to-surface vector
        out.half_angle = world.xyz - camera.eye_fog_mode.xyz;
    }
    out.light_set = light_set;
    out.game_time = game_time;
    if (camera.fog_range.z > 0.5) {
        out.fog = fog_amount(world.xyz);
    }
    out.uv = cod2_scroll(out.uv, stage.flags, out.game_time);
    return out;
}

// glFog GL_EXP / GL_LINEAR factors (see shader.wgsl); depth along the view
// forward, the fixed-function fog coordinate without GL_NV_fog_distance.
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

// A stage's colour before fog: the texel times the light set's colour
// (`lightingDiffuse`) or the tint, times the model's vertex colour where
// the stage reads it. A CoD2 probe's light multiplies the vertex colour
// too: `lprobe*.hlsl` light `colour map * vertex colour` (cod2-materials.md
// 9.2). `rgbGen vertex` (the half) stays the tint: no CoD1 model stores a
// colour.
fn stage_colour(tex: vec4<f32>, light: vec3<f32>, vcol: vec4<f32>, flags: u32, tint: vec4<f32>, identity: f32) -> vec4<f32> {
    var c: vec3<f32>;
    if ((flags & F_LIT) != 0u) {
        c = light * vcol.rgb;
    } else {
        c = tint.rgb;
        if ((flags & F_TINT_LIGHT) != 0u) {
            c = min(c * identity, vec3(1.0));
        }
        if ((flags & (F_VERTEX_RGB | F_VERTEX_HALF)) == F_VERTEX_RGB) {
            c *= vcol.rgb;
        }
    }
    var a = tex.a * tint.a;
    if ((flags & F_VERTEX_ALPHA) != 0u) {
        a *= vcol.a;
    }
    return vec4<f32>(tex.rgb * c, a);
}

// alphaFunc decode, as shader.wgsl's; GE128 carries both low bits.
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

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let tex = sample_image(t_diffuse, in.uv, stage.flags);
    var lit: vec4<f32>;
    if ((stage.flags & F_OBJECTIVE) != 0u) {
        let c = objective_colour(tex, in.normal, camera.view_fwd.xyz, in.game_time);
        lit = vec4<f32>(c.rgb * camera.time_pad.y, c.a);
    } else if ((stage.flags & F_COD2_PROBE) != 0u) {
        let fr = Cod2Frame(in.n0, in.t2, in.b2);
        let flag = stage.deform_kinds.w == COD2_FLAG;
        // a CoD4 set (ambient.w -2) reads no dynamic shadow
        var dyn = 1.0;
        if (light_sets[in.light_set].ambient.w >= -1.5) {
            dyn = dyn_shadow(in.screen);
        }
        lit = cod2_probe_sided(light_sets[in.light_set], tex, in.vcol, in.uv, fr, in.half_angle, stage.flags, flag, dyn, camera.eye_fog_mode.xyz);
    } else {
        lit = stage_colour(tex, in.light, in.vcol, stage.flags, stage.tint, camera.time_pad.y);
    }
    let a = lit.a;
    if (!alphafunc_pass(a)) { discard; }
    var rgb = lit.rgb;
    if ((stage.flags & F_SUN_SHADOW) != 0u) {
        rgb *= sun_shadow(in.world_pos, in.normal, camera.eye_fog_mode.xyz);
    }
    // CoD2 (fog_range.z 1) keeps a no-fog material clear and fades an
    // additive one to black (RB_SetIteratorFog); CoD1 fogs every model
    if (camera.fog_range.z > 0.5) {
        if ((stage.flags & F_NOFOG) != 0u) {
            return vec4<f32>(rgb, a);
        }
        if ((stage.flags & F_FOG_BLACK) != 0u) {
            return vec4<f32>(rgb * (1.0 - in.fog), a);
        }
        return vec4<f32>(mix(rgb, camera.fog_color_density.rgb, in.fog), a);
    }
    return vec4<f32>(mix(rgb, camera.fog_color_density.rgb, fog_amount(in.world_pos)), a);
}

// The depth prepass's `floatZ` write on a CoD4 frame a feather reads
// (cod2_world.wgsl's `fs_floatz`): an opaque first stage's view depth,
// after its alpha test where it has one.
override FLOATZ_NEAR: f32 = 4.0;
override FLOATZ_FAR: f32 = 60000.0;

@fragment
fn fs_floatz(in: VsOut) -> @location(0) vec4<f32> {
    if ((stage.flags & (F_AF_GT0 | F_AF_LT128)) != 0u) {
        let tex = sample_image(t_diffuse, in.uv, stage.flags);
        // fs_main's alpha: a probe stage's (cod2_probe_sided, cod4_model)
        // or a plain one's
        var a = stage_colour(tex, in.light, in.vcol, stage.flags, stage.tint, 1.0).a;
        if ((stage.flags & F_COD2_PROBE) != 0u) {
            a = select(1.0, tex.a * in.vcol.a, (stage.flags & F_COD2_ALPHA) != 0u);
        }
        if (!alphafunc_pass(a)) {
            discard;
        }
    }
    let n = FLOATZ_NEAR;
    let f = FLOATZ_FAR;
    return vec4<f32>(f * n / (f - in.clip.z * (f - n)), 0.0, 0.0, 0.0);
}
