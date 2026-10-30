// First-person viewmodel: the world's fov with its own near plane, bind pose
// baked into the vertices, placed in view space by the motion transform.
// Each stage of a surface's material draws as dynamic_model.wgsl draws an
// entity's: lit per vertex by its light set (here in view space), or its
// rgbGen colour, with alphaGen, alphaFunc, tcMods and tcGen environment.

// One GL light; `entity_light::GpuLight`.
struct EntLight {
    pos: vec4<f32>,      // w 0: xyz is the unit vector towards the light
    diffuse: vec4<f32>,  // w spot exponent
    ambient: vec4<f32>,  // w cosine of the spot cutoff, below -1 for none
    atten: vec4<f32>,    // constant, linear, quadratic
    spot_dir: vec4<f32>,
};
struct LightSet {
    ambient: vec4<f32>,  // light model ambient; w the light count
    lights: array<EntLight, 8>,
};

struct VmUniform {
    proj: mat4x4<f32>,      // viewmodel projection (world fov, own near plane)
    model: mat4x4<f32>,     // motion transform (view space)
    lights: LightSet,       // view space; a CoD2 probe set's world space
    // x identityLight, y 1 on CoD2 (cod2_model.wgsl), z the material
    // clock in seconds
    misc: vec4<f32>,
    // view space to world: a CoD2 probe set reads world directions
    view_to_world: mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> u: VmUniform;
@group(1) @binding(0) var t_diffuse: texture_2d<f32>;
// View-space skin matrices; identity reproduces the baked bind pose.
@group(2) @binding(0) var<uniform> bones: array<mat4x4<f32>, 128>;

// The stage's `renderer::StageParams`; only the fields read here are named.
struct StageParams {
    uv0: mat3x2<f32>,
    uv1: mat3x2<f32>,
    turb01: vec4<f32>,
    // rgbGen colour and alphaGen value
    tint: vec4<f32>,
    flags: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
    // tcGen vector axes and the sky's eye offset; [4].xy a CoD2 probe's
    // or a CoD4 reflex sight's `detailScale`
    skip: array<vec4<f32>, 5>,
    // the deform records and their kinds (deform.wgsl)
    deform: array<vec4<f32>, 3>,
    deform_kinds: vec4<u32>,
};
@group(3) @binding(0) var<uniform> stage: StageParams;

const F_AF_GT0: u32 = 16u;
const F_AF_LT128: u32 = 32u;
const F_AF_GE128: u32 = 48u;
const F_TINT_LIGHT: u32 = 4096u;
const F_LIT: u32 = 8192u;
// renderer::STAGE_FLAG_VERTEX_RGB, _ALPHA, _RGB_HALF, read on CoD2 only
const F_VERTEX_RGB: u32 = 1u;
const F_VERTEX_ALPHA: u32 = 2u;
const F_VERTEX_HALF: u32 = 1024u;
const F_BUNDLE0_ENV: u32 = 16384u;
const ATEST128: f32 = 0.5019607843137255;
// renderer::DEFORM_COD4_REFLEX in `deform_kinds.w`
const COD4_REFLEX: u32 = 2u;

struct VsOut {
    @builtin(position) @invariant pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // GL's vertex colour, framebuffer units
    @location(1) light: vec3<f32>,
    // CoD2: the colour the model stores, the world-space normal and the
    // probe stage's tangent frame and half vector (cod2_model.wgsl)
    @location(2) vcol: vec4<f32>,
    @location(3) normal: vec3<f32>,
    @location(4) n0: vec3<f32>,
    @location(5) t2: vec3<f32>,
    @location(6) b2: vec3<f32>,
    @location(7) half_angle: vec3<f32>,
    // a CoD4 reflex sight's dot coordinates (`reflexsight.hlsl`)
    @location(8) reflex_uv: vec2<f32>,
};

// tcGen environment, as dynamic_model.wgsl's `env_uv`.
fn env_uv(to_eye: vec3<f32>, n: vec3<f32>) -> vec2<f32> {
    let v = normalize(to_eye);
    let r = 2.0 * dot(n, v) * n - v;
    return vec2<f32>(0.5 + 0.5 * r.y, 0.5 - 0.5 * r.z);
}

// Same as `gl_lighting` in dynamic_model.wgsl.
fn gl_lighting(ls: LightSet, p: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
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

@vertex
fn vs_main(
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) bi: vec4<u32>,
    @location(4) bw: vec4<f32>,
    @location(12) vcol: vec4<f32>,
    @location(13) tangent: vec3<f32>,
    @location(14) binormal: vec3<f32>,
) -> VsOut {
    var p = vec4<f32>(0.0);
    var n = vec3<f32>(0.0);
    var t = vec3<f32>(0.0);
    var b = vec3<f32>(0.0);
    for (var i = 0u; i < 4u; i++) {
        let m = bones[bi[i]];
        p += bw[i] * (m * vec4<f32>(pos, 1.0));
        // rigid bone transforms, so the upper 3x3 is valid for normals
        let r = mat3x3<f32>(m[0].xyz, m[1].xyz, m[2].xyz);
        n += bw[i] * (r * normal);
        t += bw[i] * (r * tangent);
        b += bw[i] * (r * binormal);
    }
    // the bones are view space here, so a deform's world terms (the
    // wave's x+y+z, the normal's noise) read view-space coordinates
    let d = deform_vertex(p.xyz, n, uv);
    p = vec4<f32>(d.pos, 1.0);
    n = d.normal;
    let view = u.model * p;
    var out: VsOut;
    out.pos = u.proj * view;
    let vn = normalize((u.model * vec4<f32>(n, 0.0)).xyz);
    var base = uv;
    if ((stage.flags & F_BUNDLE0_ENV) != 0u) {
        // view space to the weapon's frame; the eye is the view origin
        let to_local = transpose(mat3x3<f32>(
            normalize(u.model[0].xyz),
            normalize(u.model[1].xyz),
            normalize(u.model[2].xyz),
        ));
        base = env_uv(to_local * -view.xyz, to_local * vn);
    }
    out.uv = stage.uv0 * vec3<f32>(base, 1.0);
    out.vcol = vcol;
    out.reflex_uv = vec2<f32>(0.0);
    if (stage.deform_kinds.w == COD4_REFLEX) {
        // `reflexsight.hlsl`'s vertex shader: the eye-to-vertex direction
        // against the lens's binormal and tangent, so the dot sits at
        // infinity (cod4-client-view.md 4.5)
        let r = mat3x3<f32>(u.model[0].xyz, u.model[1].xyz, u.model[2].xyz);
        let dir = normalize(view.xyz);
        out.reflex_uv = vec2<f32>(dot(r * b, dir), dot(r * t, dir)) * stage.skip[4].xy + vec2<f32>(0.5);
    }
    if (u.misc.y > 0.5) {
        // CoD2: the probe at the eye reads world directions
        let to_world = mat3x3<f32>(u.view_to_world[0].xyz, u.view_to_world[1].xyz, u.view_to_world[2].xyz)
            * mat3x3<f32>(u.model[0].xyz, u.model[1].xyz, u.model[2].xyz);
        let wn = normalize(to_world * n);
        out.normal = wn;
        out.light = probe_lighting(u.lights, wn);
        let fr = cod2_frame(to_world * n, to_world * t, to_world * b);
        out.n0 = fr.n0;
        out.t2 = fr.t2;
        out.b2 = fr.b2;
        let to_eye = (u.view_to_world * vec4<f32>(-view.xyz, 0.0)).xyz;
        out.half_angle = normalize(normalize(to_eye) + u.lights.lights[1].diffuse.xyz);
        if (u.lights.ambient.w < -1.5) {
            // a CoD4 set (cod2_model.wgsl's `cod4_model`) reads the eye-to-surface vector
            out.half_angle = -to_eye;
        }
        out.uv = cod2_scroll(out.uv, stage.flags, u.misc.z);
    } else {
        out.light = gl_lighting(u.lights, view.xyz, vn);
    }
    return out;
}

// alphaFunc decode, as dynamic_model.wgsl's.
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
    if (stage.deform_kinds.w == COD4_REFLEX) {
        return reflex_fragment(in);
    }
    if (u.misc.y > 0.5) {
        return cod2_fragment(in, tex);
    }
    var c: vec3<f32>;
    if ((stage.flags & F_LIT) != 0u) {
        c = in.light;
    } else {
        c = stage.tint.rgb;
        if ((stage.flags & F_TINT_LIGHT) != 0u) {
            c = min(c * u.misc.x, vec3(1.0));
        }
    }
    let a = tex.a * stage.tint.a;
    if (!alphafunc_pass(a)) { discard; }
    return vec4<f32>(tex.rgb * c, a);
}

// A CoD2 stage, as dynamic_model.wgsl draws an entity's: a probe stage per
// pixel, an objective pulse, else the tint or the probe at the vertex
// times the vertex colour where the stage reads it. The view looks down
// view space's -z, so the facing reads the world normal against that.
fn cod2_fragment(in: VsOut, tex: vec4<f32>) -> vec4<f32> {
    var lit: vec4<f32>;
    if ((stage.flags & F_OBJECTIVE) != 0u) {
        let fwd = (u.view_to_world * vec4<f32>(0.0, 0.0, -1.0, 0.0)).xyz;
        let c = objective_colour(tex, in.normal, fwd, u.misc.z);
        lit = vec4<f32>(c.rgb * u.misc.x, c.a);
    } else if ((stage.flags & F_COD2_PROBE) != 0u) {
        let fr = Cod2Frame(in.n0, in.t2, in.b2);
        // the eye: view space's origin in the world
        lit = cod2_probe_sided(u.lights, tex, in.vcol, in.uv, fr, in.half_angle, stage.flags, false, 1.0, u.view_to_world[3].xyz);
    } else {
        var c = stage.tint.rgb;
        if ((stage.flags & F_LIT) != 0u) {
            c = in.light * in.vcol.rgb;
        } else {
            if ((stage.flags & F_TINT_LIGHT) != 0u) {
                c = min(c * u.misc.x, vec3(1.0));
            }
            if ((stage.flags & (F_VERTEX_RGB | F_VERTEX_HALF)) == F_VERTEX_RGB) {
                c *= in.vcol.rgb;
            }
        }
        var a = tex.a * stage.tint.a;
        if ((stage.flags & F_VERTEX_ALPHA) != 0u) {
            a *= in.vcol.a;
        }
        lit = vec4<f32>(tex.rgb * c, a);
    }
    if (!alphafunc_pass(lit.a)) { discard; }
    return lit;
}

// A CoD4 reflex sight's dot, `reflexsight.hlsl`'s pixel shader: the clamped
// colour map at the dot coordinates gives m, the colour (2m, 1.2(m - 0.2),
// 1.2(m - 0.2)) and alpha 1.2m, saturated. Retail blends that premultiplied
// (One, InvSrcAlpha); the stage draws (SrcAlpha, InvSrcAlpha), so the colour
// goes out divided by its alpha. Left out: the detail map's mask, which
// needs a second texture (cod4-client-view.md 4.5).
fn reflex_fragment(in: VsOut) -> vec4<f32> {
    let m = sample_image(t_diffuse, clamp(in.reflex_uv, vec2<f32>(0.0), vec2<f32>(1.0)), stage.flags).r;
    let o = clamp(vec4<f32>(2.0 * m, 1.2 * (m - 0.2), 1.2 * (m - 0.2), 1.2 * m), vec4<f32>(0.0), vec4<f32>(1.0));
    if (o.a <= 0.0) { discard; }
    return vec4<f32>(min(o.rgb / o.a, vec3<f32>(1.0)), o.a);
}
