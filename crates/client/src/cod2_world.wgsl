// CoD2's lightmapped phong family, the `lmap*` techniques
// (docs/research/cod2-materials.md 9, "Lightmap phong"): the colour map
// (plus the detail map) times the vertex colour, lit by the lightmap's four
// basis lights weighted by the normal map and by the sun's diffuse and
// specular terms, a static model's `lprobe_smc*` (`fs_smodel`), and
// CoD4's lit world sets (`cod4_lit`) and model sets (`fs_cod4_model`). Drawn
// by renderer/cod2.rs; tex_samplers.wgsl is appended.
// Colours are computed in display units, scaled by identityLight as the
// scene target holds them and fogged; both ride the vertex's `fog`
// (cod2-client-perf.md 9).

struct Camera {
    view_proj: mat4x4<f32>,
    // .y identityLight (0.5 full screen, 1 windowed); .w 1 drops a CoD4
    // set's specular (`r_specular` 0)
    time_pad: vec4<f32>,
    // xyz view origin; w fog mode: 0 off, 1 exp, 2 linear
    eye_fog_mode: vec4<f32>,
    // rgb fog colour, a exp density
    fog_color_density: vec4<f32>,
    // x near, y far (linear); w the exp fog's start (CoD4)
    fog_range: vec4<f32>,
    view_fwd: vec4<f32>,
    // a moved brush model's pose; identity for the world's own draw
    model: mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> camera: Camera;

// renderer::cod2::MaterialUniform
struct Cod2Material {
    // xy the material's detailScale
    detail_scale: vec4<f32>,
    // x: M_* switches; y: alpha test (AT_*); z: sample_image clamp flags of
    // the colour, normal and specular maps; w: the maps with mipmaps (MIP_*)
    flags: vec4<u32>,
    // a CoD4 set's envMapParms: fresnel min, max, exponent; sun specular
    // scale
    env: vec4<f32>,
    // a CoD4 water set's waterColor
    water: vec4<f32>,
};
@group(1) @binding(0) var<uniform> mat: Cod2Material;
@group(1) @binding(1) var t_color: texture_2d<f32>;
@group(1) @binding(2) var t_normal: texture_2d<f32>;
@group(1) @binding(3) var t_spec: texture_2d<f32>;
@group(1) @binding(4) var t_detail: texture_2d<f32>;

const M_SPECULAR: u32 = 1u;
const M_DETAIL: u32 = 2u;
const M_ALPHA: u32 = 4u;
const M_NOFOG: u32 = 8u;
const M_COD4: u32 = 16u;
const M_NORMAL: u32 = 32u;
const M_AMBIENT: u32 = 64u;
const M_FLAG: u32 = 128u;
// a CoD4 `sm` set: technique 9 reads the sun shadow map
// (cod4_sunshadow_recv.wgsl)
const M_SUN_SHADOW: u32 = 256u;
const MIP_COLOR: u32 = 1u;
const MIP_NORMAL: u32 = 2u;
const MIP_SPEC: u32 = 4u;
const MIP_DETAIL: u32 = 8u;
const AT_GT0: u32 = 1u;
const AT_LT128: u32 = 2u;
const AT_GE128: u32 = 3u;

// renderer::cod2::SunUniform: lightPosition0, lightColor0, lightSpecular0,
// then a CoD4 static model's spot or omni light in the sun's place
// (`fs_cod4_model`)
struct Sun {
    // xyz unit vector toward the sun
    dir: vec4<f32>,
    color: vec4<f32>,
    specular: vec4<f32>,
    // lightPosition: origin, one over the radius
    src_pos: vec4<f32>,
    // colour; w 0 the sun lights, 1 an omni light, 2 a spot
    src_col: vec4<f32>,
    // lightSpotDir, w the attenuation row's length
    src_dir: vec4<f32>,
    // lightSpotFactors: scale, bias, exponent
    src_factors: vec4<f32>,
    atten: array<vec4<f32>, 32>,
};
@group(2) @binding(0) var<uniform> sun: Sun;

// A lit pipeline's path (renderer::cod2::LitPath, cod4-client-perf.md 29,
// cod2-client-perf.md 8): with LIT_SPLIT the fragment stage was built for
// one material's switches (LIT_FLAGS), alpha test (LIT_TEST) and light kind
// (LIT_LIGHT), as 1.7 and CoD2 compile one technique per combination, so
// each draw runs its own arm alone. Without it (water, vs_main) they are
// read off the uniforms. A split pipeline's uniforms hold the same values.
override LIT_SPLIT: bool = false;
override LIT_FLAGS: u32 = 0u;
override LIT_TEST: u32 = 0u;
override LIT_LIGHT: f32 = 0.0;

// `mat.flags.x`
fn switches() -> u32 {
    if (LIT_SPLIT) {
        return LIT_FLAGS;
    }
    return mat.flags.x;
}

// `mat.flags.y`
fn alpha_test() -> u32 {
    if (LIT_SPLIT) {
        return LIT_TEST;
    }
    return mat.flags.y;
}

// `sun.src_col.w`
fn light_kind() -> f32 {
    if (LIT_SPLIT) {
        return LIT_LIGHT;
    }
    return sun.src_col.w;
}
// The page's three coefficient images and the sun visibility.
@group(2) @binding(1) var t_lm_r: texture_2d<f32>;
@group(2) @binding(2) var t_lm_g: texture_2d<f32>;
@group(2) @binding(3) var t_lm_b: texture_2d<f32>;
@group(2) @binding(4) var t_lm_sun: texture_2d<f32>;
// The screen's dynamic shadow (renderer::cod2_cookie::DynamicShadows), the
// share of the sun the shadow cookies leave (1 where none reaches).
@group(0) @binding(17) var t_dyn_shadow: texture_2d<f32>;

// lib/lightmap.hlsl's `tex2Dproj( dynamicShadowSampler, screenCoords )`,
// `sc` the vertex's `VsOut::screen`.
fn dyn_shadow(sc: vec4<f32>) -> f32 {
    return textureSampleLevel(t_dyn_shadow, s_lin_clamp, sc.xy / sc.w, 0.0).r;
}

// `$lightmapweights` and `$specularity` (renderer::cod2).
@group(2) @binding(5) var t_weights: texture_2d<f32>;
@group(2) @binding(6) var t_specularity: texture_2d<f32>;
// A CoD4 surface's reflection probe (black elsewhere).
@group(2) @binding(7) var t_probe: texture_cube<f32>;
// A CoD4 static model's lighting volume, 1.7's `modelLightingSampler`
// (renderer::cod2's `model_volume_group`); one texel elsewhere.
@group(2) @binding(8) var t_model_volume: texture_3d<f32>;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) lm_uv: vec2<f32>,
    @location(3) normal: vec3<f32>,
    @location(4) color: vec4<f32>,
    @location(5) tangent: vec3<f32>,
    @location(6) binormal: vec3<f32>,
};

struct VsOut {
    // invariant: the depth prepass runs vs_main without a fragment stage
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) lm_uv: vec2<f32>,
    // N - T - B, 2T and 2B: N + (2x - 1)T + (2y - 1)B is then
    // n0 + x * t2 + y * b2 for a normal-map sample (x, y) in [0, 1]; on a
    // CoD4 set N, T and B themselves
    @location(3) n0: vec3<f32>,
    @location(4) t2: vec3<f32>,
    @location(5) b2: vec3<f32>,
    @location(6) world_pos: vec3<f32>,
    // normalized eye direction plus the sun direction, per vertex
    @location(7) half_angle: vec3<f32>,
    // the fog, per vertex as retail's vertex shaders take it (CoD2's
    // `Fog_VertexCalc`, cod2-client-perf.md 9; cod4-world.md 11.1): rgb
    // the fog colour times one less the keep, w identityLight times the
    // keep, so a pixel is `colour * w + rgb` (`vertex_fog`)
    @location(8) fog: vec4<f32>,
    // `Transform_ClipSpacePosToTexCoords` (lib/transform.hlsl): the screen's
    // texture coordinates times w, for the dynamic shadow's projective read
    @location(9) screen: vec4<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    var pos = in.pos;
    var normal = in.normal;
    var binormal = in.binormal;
    out.color = in.color;
    // a CoD4 flag static model waves its baked world-space vertex
    // (cod4_flag.wgsl) and hands on its colour with alpha 1
    if ((mat.flags.x & M_FLAG) != 0u) {
        let f = cod4_flag_wave(pos, normal, in.tangent, binormal, in.uv, in.color.a, camera.time_pad.x);
        pos = f.pos;
        normal = f.normal;
        binormal = f.binormal;
        out.color = vec4<f32>(in.color.rgb, 1.0);
    }
    let world = camera.model * vec4<f32>(pos, 1.0);
    let m3 = mat3x3<f32>(camera.model[0].xyz, camera.model[1].xyz, camera.model[2].xyz);
    let t = m3 * in.tangent;
    let b = m3 * binormal;
    out.clip = camera.view_proj * world;
    out.screen = vec4<f32>(0.5 * (out.clip.x + out.clip.w), 0.5 * (out.clip.w - out.clip.y), 0.0, out.clip.w);
    out.uv = in.uv;
    out.lm_uv = in.lm_uv;
    out.world_pos = world.xyz;
    if ((mat.flags.x & M_COD4) != 0u) {
        // CoD4's sets take N, T and B as they are (`cod4_normal`), and
        // no half angle
        out.n0 = m3 * normal;
        out.t2 = t;
        out.b2 = b;
        out.fog = vertex_fog(world.xyz);
        return out;
    }
    out.n0 = m3 * normal - t - b;
    out.t2 = 2.0 * t;
    out.b2 = 2.0 * b;
    let eye_dir = normalize(camera.eye_fog_mode.xyz - world.xyz);
    out.half_angle = normalize(eye_dir + sun.dir.xyz);
    out.fog = vertex_fog(world.xyz);
    return out;
}

fn alpha_passes(a: f32) -> bool {
    switch alpha_test() {
        case 1u: { return a > 0.0; }
        case 2u: { return a < 128.0 / 255.0; }
        case 3u: { return a >= 128.0 / 255.0; }
        default: { return true; }
    }
}

// A vertex's `VsOut::fog`: what the fog adds and what it keeps of the
// surface times identityLight, at the vertex. A `nofog` material keeps it
// all. The pixel's lerp then runs on the interpolated keep, as retail's
// fixed-function fog blends the interpolated FOG output.
fn vertex_fog(world_pos: vec3<f32>) -> vec4<f32> {
    let il = camera.time_pad.y;
    if ((mat.flags.x & M_NOFOG) != 0u) {
        return vec4<f32>(0.0, 0.0, 0.0, il);
    }
    let keep = fog_keep(world_pos);
    return vec4<f32>(camera.fog_color_density.rgb * (1.0 - keep), il * keep);
}

// The fraction of the surface colour fog keeps over the radial distance:
// exp(-density * (d - start)) capped at 1 (CoD4 fogConsts: -density,
// start * density), or (far - d) / (far - near) clamped (fogConsts, 9).
fn fog_keep(world_pos: vec3<f32>) -> f32 {
    let mode = camera.eye_fog_mode.w;
    let d = length(world_pos - camera.eye_fog_mode.xyz);
    if (mode == 1.0) {
        return exp(-camera.fog_color_density.a * max(d - camera.fog_range.w, 0.0));
    }
    if (mode == 2.0) {
        let span = max(camera.fog_range.y - camera.fog_range.x, 0.0001);
        return clamp((camera.fog_range.y - d) / span, 0.0, 1.0);
    }
    return 1.0;
}

// The colour map (plus the detail map) times the vertex colour, the
// normal mapped normal and the sun's specular (`Phong_SpecularLighting`
// times `Phong_SpecularVisibility`) at visibility `vis`.
struct Surface {
    diffuse: vec4<f32>,
    n: vec3<f32>,
    ns: vec2<f32>,
};

// The detail and normal maps are read only where a switch uses them: a
// CoD4 set reads its normal map only under M_NORMAL (`cod4_lit`,
// `cod4_normal`), and none of its paths reads `n` (cod4-client-perf.md 21).
fn surface(in: VsOut) -> Surface {
    let cl = mat.flags.z;
    var diffuse = sample_mipped(t_color, in.uv, cl, (mat.flags.w & MIP_COLOR) != 0u);
    if ((switches() & M_DETAIL) != 0u) {
        let detail = sample_mipped(t_detail, in.uv * mat.detail_scale.xy, 0u, (mat.flags.w & MIP_DETAIL) != 0u);
        diffuse = vec4<f32>(diffuse.rgb + detail.rgb - 0.5, diffuse.a);
    }
    diffuse = diffuse * in.color;
    // the PC channel pick: x in alpha, y in green
    var ns = vec2<f32>(0.0);
    if ((switches() & (M_COD4 | M_NORMAL)) != M_COD4) {
        ns = sample_mipped(t_normal, in.uv, cl, (mat.flags.w & MIP_NORMAL) != 0u).wy;
    }
    let n = normalize(in.n0 + ns.x * in.t2 + ns.y * in.b2);
    return Surface(diffuse, n, ns);
}

// The specular and specularity maps are read only under M_SPECULAR, as
// retail's `lmap` and `lmap_dtl` shaders read neither.
fn sun_specular(in: VsOut, n: vec3<f32>, vis: f32) -> vec3<f32> {
    var out = vec3<f32>(0.0);
    if ((switches() & M_SPECULAR) != 0u) {
        let spec = sample_mipped(t_spec, in.uv, mat.flags.z, (mat.flags.w & MIP_SPEC) != 0u);
        let n_dot_h = dot(normalize(in.half_angle), n);
        let strength = textureSample(t_specularity, s_lin_clamp, vec2<f32>(spec.a, n_dot_h)).r;
        out = mix(0.3, 1.0, vis) * sun.specular.rgb * spec.rgb * strength;
    }
    return out;
}

// `light` lights the diffuse, the specular adds; alpha under `_a`, the
// alpha test, then identityLight and the fog in one multiply-add off the
// vertex (`vertex_fog`).
fn finish(in: VsOut, diffuse: vec4<f32>, light: vec3<f32>, spec: vec3<f32>) -> vec4<f32> {
    var a = 1.0;
    if ((switches() & M_ALPHA) != 0u) {
        a = diffuse.a;
    }
    if (!alpha_passes(a)) {
        discard;
    }
    // the camera's fog colour already carries identityLight
    return vec4<f32>((diffuse.rgb * light + spec) * in.fog.w + in.fog.rgb, a);
}

// CoD4's lm_sun_r0c0n0s0_sm3.hlsl (cod4-world.md 9.2, 9.6), technique 9's
// `lm_sm_sun_*` on an `sm` set while the sun shadow map is drawn (3.6),
// the spot's and omni's `lm_spot_*` and `lm_omni_*` (10, 12) on a surface
// whose primary light is one, and `lm_*` (7) with none (3.5): the page's
// first two coefficient images hold the secondary's halves A and B as
// stored, the direction's x and y in their alphas; the normal map's alpha
// and green decode as that direction does. A lights by the normal's z, B
// by the cosine between the direction and the normal, the primary light
// by the normal mapped normal at the primary's visibility, its specular
// as `cod4_specular_from`.
fn cod4_lit(in: VsOut, sf: Surface) -> vec4<f32> {
    let a = textureSample(t_lm_r, s_lin_clamp, in.lm_uv);
    let b = textureSample(t_lm_g, s_lin_clamp, in.lm_uv);
    let dir = vec2<f32>(a.a * 4.08 - 2.08, b.a * 4.0645161 - 2.0645161);
    var nt = vec2<f32>(0.0);
    if ((switches() & M_NORMAL) != 0u) {
        nt = vec2<f32>(sf.ns.x * 4.08 - 2.08, sf.ns.y * 4.0645161 - 2.0645161);
    }
    let rl = inverseSqrt(dot(dir, dir) + 1.0);
    let rn = inverseSqrt(dot(nt, nt) + 1.0);
    let cosine = clamp((dot(dir, nt) + 1.0) * rl * rn, 0.0, 1.0);
    var light = a.rgb * rn + b.rgb * cosine;
    let n = cod4_normal(in, sf);
    var vis = textureSample(t_lm_sun, s_lin_clamp, in.lm_uv).r;
    let src = cod4_primary(in.world_pos);
    if (light_kind() < -0.5) {
        // `lm_*` (technique 7): no primary light
        vis = 0.0;
    } else if (light_kind() < 0.5) {
        // `lm_sm_sun_*`: no sun where the primary is 0, else the shadow map's
        if ((switches() & M_SUN_SHADOW) != 0u && vis != 0.0) {
            vis = cod4_sun_shadow(in.world_pos, vis);
        }
        // CoD4 has no shadow cookies: CoD2's dynamic shadow is never
        // drawn on a CoD4 map, so it is not read
    } else if (light_kind() > 1.5 && (switches() & M_SUN_SHADOW) != 0u) {
        // `lm_sm_spot_*` (technique 11): the spot's shadow map
        vis = cod4_spot_shadow(in.world_pos, sun.src_pos.xyz, vis);
    }
    light += vis * src.col * clamp(dot(src.dir, n), 0.0, 1.0);
    return finish(in, sf.diffuse, light, cod4_specular_from(in, n, src.dir, vis, src.spec));
}

// The light a CoD4 surface's group carries (`Sun`): the sun's direction,
// colour and specular colour, or a spot or omni light's toward its origin,
// its colour by its attenuation row at the distance over its radius (the
// world sets read the row out of the secondary lightmap at
// `lightFalloffPlacement`, the same texels), a spot's by its cone
// (`lm_spot_*`, `lp_spot_*`; cod4-world.md 3.5).
struct Cod4Primary {
    dir: vec3<f32>,
    col: vec3<f32>,
    spec: vec3<f32>,
};

fn cod4_primary(world_pos: vec3<f32>) -> Cod4Primary {
    var p = Cod4Primary(sun.dir.xyz, sun.color.rgb, sun.specular.rgb);
    if (light_kind() > 0.5) {
        let lv = sun.src_pos.xyz - world_pos;
        let d = length(lv);
        p.dir = lv / max(d, 1e-6);
        var att = cod4_attenuation(clamp(d * sun.src_pos.w, 0.0, 1.0));
        if (light_kind() > 1.5) {
            let s = clamp(dot(p.dir, sun.src_dir.xyz) * sun.src_factors.x + sun.src_factors.y, 0.0, 1.0);
            att *= select(pow(s, sun.src_factors.z), 0.0, s <= 0.0);
        }
        p.col = att * sun.src_col.rgb;
        p.spec = p.col;
    }
    return p;
}

// A CoD4 set's specular off a light toward `l_dir` of colour `l_col` at
// visibility `vis`: its reflection, sharpened by the specular map's alpha,
// plus the reflection probe at the reflected view (its level 6 - 8 spec.a)
// times its alpha, the sum times the specular map and the fresnel of
// envMapParms.
fn cod4_specular_from(in: VsOut, n: vec3<f32>, l_dir: vec3<f32>, vis: f32, l_col: vec3<f32>) -> vec3<f32> {
    var spec = vec3<f32>(0.0);
    // camera.time_pad.w 1: `r_specular` 0 (renderer::Cod4Options)
    if ((switches() & M_SPECULAR) != 0u && camera.time_pad.w == 0.0) {
        let sm = sample_mipped(t_spec, in.uv, mat.flags.z, (mat.flags.w & MIP_SPEC) != 0u);
        let v = normalize(in.world_pos - camera.eye_fog_mode.xyz);
        let vn = dot(v, n);
        let r = v - 2.0 * vn * n;
        let power = exp2(sm.a * 9.3775177) + 7.0;
        let glint = clamp(exp((dot(r, l_dir) - 0.99925) * power), 0.0, 1.0);
        let fresnel = mix(mat.env.x, mat.env.y, pow(1.0 - abs(vn), mat.env.z));
        let probe = textureSampleLevel(t_probe, s_tex_clamp, r, max(6.0 - 8.0 * sm.a, 0.0));
        spec = (vis * glint * mat.env.w * l_col + probe.rgb * probe.a) * sm.rgb * fresnel;
    }
    return spec;
}

// The CoD4 normal mapped normal: N + x T + y B, as lm_*_sm3.hlsl's
// `mad`s take it off the vertex's N, T and B (`vs_main`), the map's alpha
// and green decoded as the lightmap's direction is; N alone without a
// map.
fn cod4_normal(in: VsOut, sf: Surface) -> vec3<f32> {
    var nt = vec2<f32>(0.0);
    if ((switches() & M_NORMAL) != 0u) {
        nt = vec2<f32>(sf.ns.x * 4.08 - 2.08, sf.ns.y * 4.0645161 - 2.0645161);
    }
    return normalize(in.n0 + nt.x * in.t2 + nt.y * in.b2);
}

// CoD4's model lighting volume (cod4-world.md 3.2): 4 x 4 x 4 texel
// blocks, read in one filtered fetch as `lp_*`'s `texld` of s4 does. A
// model's lightmap coords are its block's centre (`baseLightingCoords`, z
// 0.5); the lookup moves `q` times `lightingLookupScale`, 1.5 texels on
// each axis.
fn model_volume(base: vec2<f32>, q: vec3<f32>) -> vec4<f32> {
    let h = f32(textureDimensions(t_model_volume).y);
    let at = vec3<f32>(base, 0.5) + q * vec3<f32>(1.5 / 256.0, 1.5 / h, 0.375);
    return textureSampleLevel(t_model_volume, s_lin_clamp, at, 0.0);
}

// CoD4's `lp_*` model techniques (lp_sun_r0c0d0n0s0_sm3.hlsl,
// cod4-world.md 3.2): the volume read along the normal mapped normal,
// pushed onto the block's faces by its longest axis, lights the diffuse
// at twice its colour, its alpha the sun's visibility on the diffuse
// `N.L` and the specular. `lp_amb_*` (M_AMBIENT) reads the block's centre
// and adds the sun without `N.L`. A batch on a spot or omni light
// (`lp_spot_*`, `lp_omni_*`, cod4-world.md 3.5) takes it in the sun's
// place (`cod4_primary`); one on no primary light (index 0, `lp_*`) the
// volume alone.
@fragment
fn fs_cod4_model(in: VsOut) -> @location(0) vec4<f32> {
    let sf = surface(in);
    let n = cod4_normal(in, sf);
    let ambient = (switches() & M_AMBIENT) != 0u;
    let an = abs(n);
    let q = select(n / max(max(an.x, an.y), max(an.z, 1e-6)), vec3<f32>(0.0), ambient);
    let ml = model_volume(in.lm_uv, q);
    var vis = ml.a;
    if (light_kind() < -0.5) {
        // `lp_*` (technique 7): no primary light
        vis = 0.0;
    } else if (light_kind() < 0.5 && (switches() & M_SUN_SHADOW) != 0u) {
        // `lp_sm_sun_*`: the block's visibility gives way to the shadow map's
        vis = cod4_sun_shadow(in.world_pos, vis);
    } else if (light_kind() > 1.5 && (switches() & M_SUN_SHADOW) != 0u) {
        // `lp_sm_spot_*` (technique 13): the spot's shadow map
        vis = cod4_spot_shadow(in.world_pos, sun.src_pos.xyz, vis);
    }
    let src = cod4_primary(in.world_pos);
    let l_dir = src.dir;
    var n_dot_l = select(clamp(dot(l_dir, n), 0.0, 1.0), 1.0, ambient);
    // `lp_flag_*`: both faces of a flag take the light
    if ((switches() & M_FLAG) != 0u) {
        n_dot_l = abs(dot(l_dir, n));
    }
    let light = 2.0 * ml.rgb + vis * src.col * n_dot_l;
    return finish(in, sf.diffuse, light, cod4_specular_from(in, n, l_dir, vis, src.spec));
}

// A spot or omni light's attenuation row at `u`, as the 1-high image's
// linear filter reads it, clamped; white without a row.
fn cod4_attenuation(u: f32) -> vec3<f32> {
    let n = u32(sun.src_dir.w);
    if (n == 0u) {
        return vec3<f32>(1.0);
    }
    let x = clamp(u * f32(n) - 0.5, 0.0, f32(n - 1u));
    let i0 = u32(floor(x));
    let i1 = min(i0 + 1u, n - 1u);
    return mix(sun.atten[i0].rgb, sun.atten[i1].rgb, x - f32(i0));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let sf = surface(in);
    if ((switches() & M_COD4) != 0u) {
        return cod4_lit(in, sf);
    }
    let w = textureSample(t_weights, s_lin_clamp, sf.ns);
    let lm = vec3<f32>(
        dot(textureSample(t_lm_r, s_lin_clamp, in.lm_uv), w),
        dot(textureSample(t_lm_g, s_lin_clamp, in.lm_uv), w),
        dot(textureSample(t_lm_b, s_lin_clamp, in.lm_uv), w),
    );
    let vis = textureSample(t_lm_sun, s_lin_clamp, in.lm_uv).r * dyn_shadow(in.screen);
    let sun_diffuse = vis * sun.color.rgb * max(0.0, dot(sun.dir.xyz, sf.n));
    return finish(in, sf.diffuse, lm + sun_diffuse, sun_specular(in, sf.n, vis));
}

// The depth prepass's `floatZ` write on a CoD4 frame a feather reads
// (cod4-client-perf.md 35; 1.7's `floatz_build*.hlsl` under
// `TECHNIQUE_BUILD_FLOAT_Z`, iw3mp.exe 0x658f40): the view depth as
// soft_particles.wgsl's copy linearises the depth buffer, and under
// FLOATZ_ATEST the lit stage's alpha test off the colour map.
override FLOATZ_ATEST: bool = false;
override FLOATZ_NEAR: f32 = 4.0;
override FLOATZ_FAR: f32 = 60000.0;

@fragment
fn fs_floatz(in: VsOut) -> @location(0) vec4<f32> {
    if (FLOATZ_ATEST) {
        var a = 1.0;
        if ((switches() & M_ALPHA) != 0u) {
            a = (sample_mipped(t_color, in.uv, mat.flags.z, (mat.flags.w & MIP_COLOR) != 0u) * in.color).a;
        }
        if (!alpha_passes(a)) {
            discard;
        }
    }
    let n = FLOATZ_NEAR;
    let f = FLOATZ_FAR;
    return vec4<f32>(f * n / (f - in.clip.z * (f - n)), 0.0, 0.0, 0.0);
}

// A static model's `lprobe_smc*` technique (lib/lightprobe_sm.hlsl,
// cod2-materials.md 9.6): its group 2 binds the static model cache's two
// layers (cod2_light::SmodelCache) as `t_lm_r` and `t_lm_g`, and its
// lightmap coords are its block's centre. The normal turned into the probe
// basis and pushed onto the cube's face along its longest axis moves the
// lookup half a texel per axis, so the filter weights the block's eight
// corners linearly (`ProbeLighting`); the z layers blend as the 3D
// texture's filter would. The light is the probe plus the sun's diffuse by
// the texel's visibility.
@fragment
fn fs_smodel(in: VsOut) -> @location(0) vec4<f32> {
    let sf = surface(in);
    let r = sf.n.x * vec3<f32>(0.81649658, -0.40824829, -0.40824829)
        + sf.n.y * vec3<f32>(0.0, 0.70710678, -0.70710678)
        + sf.n.z * vec3<f32>(0.57735027);
    let ar = abs(r);
    let q = r / max(max(ar.x, ar.y), max(ar.z, 1e-6));
    let uv = in.lm_uv + q.xy * (0.5 / vec2<f32>(textureDimensions(t_lm_r)));
    let probe = mix(
        textureSample(t_lm_r, s_lin_clamp, uv),
        textureSample(t_lm_g, s_lin_clamp, uv),
        0.5 + 0.5 * q.z,
    );
    let vis = probe.a;
    let sun_diffuse = vis * sun.color.rgb * max(0.0, dot(sun.dir.xyz, sf.n));
    return finish(in, sf.diffuse, probe.rgb + sun_diffuse, sun_specular(in, sf.n, vis));
}

// The water's height: the normal map's red at `uv`, 3.7 and 3.7^2 times
// finer, weighted 1, 0.6 and 0.36 (water_l_sun.hlsl's three `rep` passes).
fn water_height(uv: vec2<f32>) -> f32 {
    var h = 0.0;
    var at = uv;
    var w = 1.0;
    for (var i = 0; i < 3; i++) {
        h += textureSample(t_normal, s_tex, at).r * w;
        at *= 3.7;
        w *= 0.6;
    }
    return h;
}

// CoD4's `water_l_sun` (cod4-world.md 9.8): the normal map's red, half as
// fine, moves the texture coords along the view by `(0.5 - h) * 0.0234375`;
// the height's differences 1/256 along u and v turn (du, dv, 1) into the
// normal; the reflection probe along the view reflected off it, its z made
// positive, times its alpha and 4, lerps from `waterColor` times the
// normal's z by envMapParms' fresnel, and the sun's glint, the fresnel
// times the reflection's cosine to the sun plus 0.00075 to the 64th, adds
// `sunDiffuse` times envMapParms.w. The vertex alpha is the alpha; no
// colour map, no lightmap.
@fragment
fn fs_cod4_water(in: VsOut) -> @location(0) vec4<f32> {
    let v = normalize(in.world_pos - camera.eye_fog_mode.xyz);
    let lift = 0.5 - textureSample(t_normal, s_tex, 0.5 * in.uv).r;
    let uv = in.uv + lift * v.xy * 0.0234375;
    let h0 = water_height(uv);
    let hu = water_height(uv + vec2<f32>(0.00390625, 0.0));
    let hv = water_height(uv + vec2<f32>(0.0, 0.00390625));
    let n = normalize(vec3<f32>(hu - h0, hv - h0, 1.0));
    let vn = dot(v, n);
    let r = v - 2.0 * vn * n;
    let probe = textureSample(t_probe, s_tex_clamp, vec3<f32>(r.xy, abs(r.z)));
    let reflected = clamp(probe.rgb * probe.a * 4.0, vec3<f32>(0.0), vec3<f32>(1.0));
    let fresnel = clamp(mix(mat.env.x, mat.env.y, pow(1.0 - abs(vn), mat.env.z)), 0.0, 1.0);
    let base = mat.water.rgb * n.z;
    let glint = pow(fresnel * max(dot(r, sun.dir.xyz) + 0.00075, 0.0), 64.0);
    let rgb = mix(base, reflected, fresnel) + glint * sun.color.rgb * mat.env.w;
    return finish(in, vec4<f32>(rgb, in.color.a), vec3<f32>(1.0), vec3<f32>(0.0));
}
