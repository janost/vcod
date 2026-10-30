// CoD2's model techniques (docs/research/cod2-materials.md 9.2, 9.4), shared
// by dynamic_model.wgsl and viewmodel.wgsl, which declare `LightSet`, the
// stage params and `sample_image`'s samplers. A probe light set is
// cod2_light::probe_set's; every direction here is world space.

// The stage's extra maps (entity_material::EntityMaterials' params
// groups): the normal, specular and detail maps, flat, black and grey
// where the material has none, and the code-built `$specularity`.
@group(3) @binding(1) var t_normal: texture_2d<f32>;
@group(3) @binding(2) var t_spec: texture_2d<f32>;
@group(3) @binding(3) var t_detail: texture_2d<f32>;
@group(3) @binding(4) var t_specularity: texture_2d<f32>;
// A CoD4 map's reflection probes, one cube per layer
// (entity_material::EntityMaterials::set_probes); an entity's set names
// its layer.
@group(3) @binding(5) var t_probes: texture_cube_array<f32>;

// renderer::STAGE_FLAG_COD2_*: an `lprobe*` stage lit per pixel and its
// `_a`, `_s`, `_dtl` and `_scroll` switches; an `objective_base` stage
const F_COD2_PROBE: u32 = 268435456u;
const F_COD2_ALPHA: u32 = 262144u;
const F_COD2_SPEC: u32 = 536870912u;
const F_COD2_DETAIL: u32 = 1073741824u;
const F_COD2_SCROLL: u32 = 524288u;
const F_OBJECTIVE: u32 = 2147483648u;
// renderer::DEFORM_COD4_FLAG in `deform_kinds.w`: a CoD4 `lp_flag_*`
// stage (cod4_flag.wgsl), whose sun takes |N.L|
const COD4_FLAG: u32 = 3u;

// CoD2's light grid probe (cod2-materials.md 9.2), as cod2_light::probe_set
// packs it: the normal turned into the basis frame and pushed onto the
// cube's face along its longest axis, each axis eased by 3t^2 - 2t^3,
// weights the eight corner colours (lightGridColorsR0..B1). No clamp, as
// the shader has none. What `$lightgridweights0`/`1` hold; the per-pixel
// probe reads those (`probe_grid_cubes`), the per-vertex fallbacks this.
fn probe_grid(ls: LightSet, n: vec3<f32>) -> vec3<f32> {
    let r = n.x * vec3<f32>(0.81649658, -0.40824829, -0.40824829)
        + n.y * vec3<f32>(0.0, 0.70710678, -0.70710678)
        + n.z * vec3<f32>(0.57735027);
    let a = abs(r);
    let longest = max(max(a.x, a.y), max(a.z, 1e-6));
    let t = clamp(r / longest * 0.5 + 0.5, vec3<f32>(0.0), vec3<f32>(1.0));
    let f = t * t * (3.0 - 2.0 * t);
    let g = 1.0 - f;
    // corner j: bit 0 the cube's x, bit 1 y, bit 2 z; 0-3 at z -1
    let xy = vec4<f32>(g.x * g.y, f.x * g.y, g.x * f.y, f.x * f.y);
    return probe_colours(ls, xy * g.z, xy * f.z);
}

// The eight corner colours (lightGridColorsR0..B1) weighted by `w0`
// (corners 0-3) and `w1` (4-7), `ProbeLighting`'s dots.
fn probe_colours(ls: LightSet, w0: vec4<f32>, w1: vec4<f32>) -> vec3<f32> {
    let l0 = ls.lights[0];
    let l1 = ls.lights[1];
    return vec3<f32>(
        dot(w0, l0.pos) + dot(w1, l0.diffuse),
        dot(w0, l0.ambient) + dot(w1, l0.atten),
        dot(w0, l0.spot_dir) + dot(w1, l1.pos),
    );
}

// The probe plus the sun's diffuse by its visibility: `ambient` is the
// sun's colour times it (lightColor0 * .a), light 1's `diffuse` the unit
// vector toward the sun (lightPosition0).
fn probe_lighting(ls: LightSet, n: vec3<f32>) -> vec3<f32> {
    return probe_lighting_sided(ls, n, false, 1.0);
}

// `two_sided`: `Phong_DoubleSidedDiffuseLighting` (lib/lightprobe_flag.hlsl),
// the sun's `N.L` taken by its size. `dyn` is the screen's dynamic shadow
// at the pixel, which the sun's visibility takes.
fn probe_lighting_sided(ls: LightSet, n: vec3<f32>, two_sided: bool, dyn: f32) -> vec3<f32> {
    return probe_grid(ls, n) + probe_sun(ls, n, two_sided, dyn);
}

// The sun's diffuse by its visibility and the dynamic shadow `dyn`.
fn probe_sun(ls: LightSet, n: vec3<f32>, two_sided: bool, dyn: f32) -> vec3<f32> {
    var n_dot_l = dot(n, ls.lights[1].diffuse.xyz);
    if (two_sided) {
        n_dot_l = abs(n_dot_l);
    }
    return ls.ambient.rgb * dyn * max(n_dot_l, 0.0);
}

// `Flap` (lib/flag.hlsl): three sine waves travelling over the texcoords
// at `game_time` seconds (gameTime.w) push the vertex along its normal by
// up to 5 * 2.44 units times the vertex alpha, and tilt the normal by the
// waves' slope along the tangent and binormal. The colour it hands on is
// the vertex's red, blue, green with alpha 1. Model space in, model space
// out.
struct Flapped {
    pos: vec3<f32>,
    normal: vec3<f32>,
    vcol: vec4<f32>,
};

fn cod2_flap(p: vec3<f32>, n: vec3<f32>, t: vec3<f32>, b: vec3<f32>, uv: vec2<f32>, vcol: vec4<f32>, game_time: f32) -> Flapped {
    let pi = 3.14159265;
    let u = 1.0 - uv.x;
    let v = 1.0 - uv.y;
    // per wave: scale, angle (degrees), speed, amplitude
    let sc = vec3<f32>(1.73, 0.75, 0.31);
    let ang = vec3<f32>(4.0, -10.0, 15.0) * (pi / 180.0);
    let speed = vec3<f32>(0.9, 1.5, 1.2);
    let amp = vec3<f32>(1.5, 0.6, 0.34);
    let su = 2.0 * pi / sc * cos(ang);
    let sv = 2.0 * pi / sc * sin(ang);
    let w = su * u + sv * v + 2.0 * pi * game_time * speed;
    let offset = 5.0 * vcol.a * dot(amp, sin(w));
    let nu = atan(dot(amp * su, cos(w)) / (2.0 * pi)) * vcol.a;
    let nv = atan(dot(amp * sv, cos(w)) / (2.0 * pi)) * vcol.a;
    // the compiled sqrt is RSQ then RCP, of the size (the Mac's
    // lprobe_flag_a_s.vsa), so a slope past 45 degrees stays a number
    let nout = sqrt(abs(1.0 - (nu * nu + nv * nv)));
    return Flapped(p + offset * n, nu * t + nv * b + nout * n, vec4<f32>(vcol.r, vcol.b, vcol.g, 1.0));
}

// The texcoords `TexScroll` (lib/texscroll.hlsl) gives a `_scroll`
// technique at the material clock `game_time` seconds: t falls by its
// fraction (gameTime.z).
fn cod2_scroll(uv: vec2<f32>, flags: u32, game_time: f32) -> vec2<f32> {
    if ((flags & F_COD2_SCROLL) != 0u) {
        return vec2<f32>(uv.x, uv.y - fract(game_time));
    }
    return uv;
}

// `Normal_VertexSetup`'s three vectors from the unit normal, tangent and
// binormal: N - T - B, 2T and 2B, so the normal map's (x, y) gives
// n0 + x t2 + y b2 = N + (2x - 1)T + (2y - 1)B.
struct Cod2Frame {
    n0: vec3<f32>,
    t2: vec3<f32>,
    b2: vec3<f32>,
};

fn cod2_frame(n: vec3<f32>, t: vec3<f32>, b: vec3<f32>) -> Cod2Frame {
    return Cod2Frame(n - t - b, 2.0 * t, 2.0 * b);
}

// `$lightgridweights0` and `1` (vcod_cod2::lightgrid::weight_cubes, built
// as retail's `Image_GetLightGridWeightsForVector` Mac 0xfc8d4 fills them),
// layers 0 and 1, one level, read through the linear clamp sampler.
@group(3) @binding(7) var t_grid_weights: texture_cube_array<f32>;

// `ProbeLighting` (lib/lightprobe.hlsl): the two weight cubes at the world
// normal weight the corner colours.
fn probe_grid_cubes(ls: LightSet, n: vec3<f32>) -> vec3<f32> {
    let w0 = textureSampleLevel(t_grid_weights, s_lin_clamp, n, 0, 0.0);
    let w1 = textureSampleLevel(t_grid_weights, s_lin_clamp, n, 1, 0.0);
    return probe_colours(ls, w0, w1);
}

// `lprobe*.hlsl`'s ps_main (lib/lightprobe.hlsl): the colour map (plus the
// detail map) times the vertex colour, lit by the probe at the normal
// mapped normal and the sun's diffuse, plus the sun's specular off the
// specular map under `_s`; alpha the diffuse alpha under `_a`, else 1.
// `half_angle` is the vertex's eye plus sun half vector; the light set's
// colours carry identityLight. The view weapon reads no dynamic shadow
// (its sample reads lit).
fn cod2_probe(ls: LightSet, tex: vec4<f32>, vcol: vec4<f32>, uv: vec2<f32>, fr: Cod2Frame, half_angle: vec3<f32>, flags: u32) -> vec4<f32> {
    return cod2_probe_sided(ls, tex, vcol, uv, fr, half_angle, flags, false, 1.0, vec3<f32>(0.0));
}

// [`cod2_probe`] with the flag techniques' two-sided sun (`two_sided`) and
// the screen's dynamic shadow `dyn` on the sun's visibility
// (`lightColor0.a * tex2Dproj( dynamicShadowSampler, screenCoords )`). A
// CoD4 set takes [`cod4_model`], `eye` the eye in the world.
fn cod2_probe_sided(ls: LightSet, tex: vec4<f32>, vcol: vec4<f32>, uv: vec2<f32>, fr: Cod2Frame, half_angle: vec3<f32>, flags: u32, two_sided: bool, dyn: f32, eye: vec3<f32>) -> vec4<f32> {
    if (ls.ambient.w < -1.5) {
        return cod4_model(ls, tex, vcol, uv, fr, half_angle, flags, eye);
    }
    let cl = flags & F_CLAMP;
    var diffuse = tex;
    // a probe stage's detailScale rides the sky's unused eye offset; the
    // detail, specular and specularity maps are read only where the
    // technique reads them (`lprobe.pse` fetches neither)
    if ((flags & F_COD2_DETAIL) != 0u) {
        let detail = sample_image(t_detail, uv * stage.skip[4].xy, 0u);
        diffuse = vec4<f32>(diffuse.rgb + detail.rgb - 0.5, diffuse.a);
    }
    diffuse = diffuse * vcol;
    // the PC channel pick: x in alpha, y in green
    let ns = sample_image(t_normal, uv, cl).wy;
    let n = normalize(fr.n0 + ns.x * fr.t2 + ns.y * fr.b2);
    let l1 = ls.lights[1];
    let light = probe_grid_cubes(ls, n) + probe_sun(ls, n, two_sided, dyn);
    var sun_spec = vec3<f32>(0.0);
    if ((flags & F_COD2_SPEC) != 0u) {
        let spec = sample_image(t_spec, uv, cl);
        let n_dot_h = dot(normalize(half_angle), n);
        let strength = textureSample(t_specularity, s_lin_clamp, vec2<f32>(spec.a, n_dot_h)).r;
        // light 1's ambient: lightSpecular0, w the sun's visibility
        sun_spec = mix(0.3, 1.0, l1.ambient.w * dyn) * l1.ambient.rgb * spec.rgb * strength;
    }
    var a = 1.0;
    if ((flags & F_COD2_ALPHA) != 0u) {
        a = diffuse.a;
    }
    return vec4<f32>(diffuse.rgb * light + sun_spec, a);
}

// `objective_base.hlsl`: the colour map's luminance plus a half under a
// colour swinging between its two constants with the facing (the view
// space normal's z toward the eye) and the clock's fraction; the colour
// map's alpha. Unlit, unfogged.
fn objective_colour(tex: vec4<f32>, n: vec3<f32>, fwd: vec3<f32>, game_time: f32) -> vec4<f32> {
    let lum = dot(vec3<f32>(0.333), tex.rgb);
    let facing = -dot(normalize(n), fwd);
    let k = 0.5 - sin((facing * 0.5 + fract(game_time)) * 3.14159 * 2.0) * 0.5;
    let c = mix(vec3<f32>(0.25, 0.15, 0.0), vec3<f32>(1.0, 1.0, 0.5), k) * (lum + 0.5);
    return vec4<f32>(c, tex.a);
}

// A CoD4 entity's set (entity_light::pack_cod4, cod4-world.md 3.3): the
// sun's colour in the ambient (w -2), light 0's pos the unit vector toward
// the sun with identityLight in w, light 0's diffuse.y its reflection
// probe's layer and .zw its block's centre in `t_model_lighting`
// (`baseLightingCoords`).

// The frame's entity model lighting volume (entity_material::Cod4Volume,
// 1.7's `modelLightingSampler`): a 4 x 4 x 4 block per lit model, R, G, B
// and the primary light's weight.
@group(3) @binding(6) var t_model_lighting: texture_3d<f32>;

// `lightingLookupScale` for the entity volume: 1.5 texels across, down and
// deep (entity_material::COD4_VOLUME_ROWS, cod4-world.md 3.2).
const COD4_LOOKUP_SCALE: vec3<f32> = vec3<f32>(1.5 / 256.0, 1.5 / 64.0, 0.375);

// The block read as `lp_*` reads it: one filtered fetch at the block's
// centre plus `q` (in [-1, 1]) times the lookup scale.
fn cod4_block(ls: LightSet, q: vec3<f32>) -> vec4<f32> {
    let at = vec3<f32>(ls.lights[0].diffuse.zw, 0.5) + q * COD4_LOOKUP_SCALE;
    return textureSampleLevel(t_model_lighting, s_lin_clamp, at, 0.0);
}

// Float `i` (4 or past) of a light set: light (i - 4) / 20's fields in
// order.
fn ls_float(ls: LightSet, i: u32) -> f32 {
    let l = ls.lights[(i - 4u) / 20u];
    let j = (i - 4u) % 20u;
    var v: vec4<f32>;
    switch (j / 4u) {
        case 0u: { v = l.pos; }
        case 1u: { v = l.diffuse; }
        case 2u: { v = l.ambient; }
        case 3u: { v = l.atten; }
        default: { v = l.spot_dir; }
    }
    return v[j % 4u];
}

// A spot or omni light's attenuation (entity_light::pack_cod4: its row's
// texels from float 104, the count in light 4's atten.w) at `u`, as the
// 1-high image's linear filter reads it, clamped; white without a row.
fn cod4_attenuation(ls: LightSet, u: f32) -> vec3<f32> {
    let n = u32(ls.lights[4].atten.w);
    if (n == 0u) {
        return vec3<f32>(1.0);
    }
    let x = clamp(u * f32(n) - 0.5, 0.0, f32(n - 1u));
    let i0 = u32(floor(x));
    let i1 = min(i0 + 1u, n - 1u);
    return mix(cod4_rgb(ls_float(ls, 104u + i0)), cod4_rgb(ls_float(ls, 104u + i1)), x - f32(i0));
}

// A packed `r + 256 g + 65536 b` float as a colour.
fn cod4_rgb(f: f32) -> vec3<f32> {
    let rgb = u32(f);
    return vec3<f32>(vec3<u32>(rgb & 255u, (rgb >> 8u) & 255u, rgb >> 16u)) / 255.0;
}

// CoD4's `lp_*` model techniques on an entity (cod4-world.md 3.2, 3.3,
// 3.5), as cod2_world.wgsl's `fs_cod4_model` on a static model: the
// colour map (plus the detail map) times the vertex colour, lit by the
// block read along the normal mapped normal pushed onto its faces by its
// longest axis at twice its colour, plus the primary light by the
// block's alpha (its visibility) on the diffuse `N.L` and the specular;
// `lp_amb_*` reads the centre and the sun without `N.L`. The primary
// light is the sun (`lp_sun_*`), or a spot or omni light the set carries
// at light 4 (`lp_spot_*`, `lp_omni_*`): toward its origin, its colour by
// its attenuation row at the distance over its radius, a spot's by
// `pow(saturate(L . spotDir * scale + bias), exponent)` (0 at or below
// 0). `to_surface` runs from the eye to the surface, `eye` the eye in
// the world; the stage's tcGen vector slots carry envMapParms
// (`skip[0]`) and the normal map and ambient switches (`skip[1].xy`).
// The specular adds the entity's reflection probe at the reflected view,
// mip 6 - 8 spec.a, times its alpha, as the static models' (9.7).
fn cod4_model(ls: LightSet, tex: vec4<f32>, vcol: vec4<f32>, uv: vec2<f32>, fr: Cod2Frame, to_surface: vec3<f32>, flags: u32, eye: vec3<f32>) -> vec4<f32> {
    let cl = flags & F_CLAMP;
    let v = normalize(to_surface);
    var diffuse = tex;
    // the detail and normal maps read only where their switch uses them
    if ((flags & F_COD2_DETAIL) != 0u) {
        let detail = sample_image(t_detail, uv * stage.skip[4].xy, 0u);
        diffuse = vec4<f32>(diffuse.rgb + detail.rgb - 0.5, diffuse.a);
    }
    diffuse = diffuse * vcol;
    var nt = vec2<f32>(0.0);
    if (stage.skip[1].x > 0.5) {
        let ns = sample_image(t_normal, uv, cl).wy;
        nt = vec2<f32>(ns.x * 4.08 - 2.08, ns.y * 4.0645161 - 2.0645161);
    }
    let t = 0.5 * fr.t2;
    let b = 0.5 * fr.b2;
    let n = normalize(fr.n0 + t + b + nt.x * t + nt.y * b);
    let ambient = stage.skip[1].y > 0.5;
    let an = abs(n);
    let q = select(n / max(max(an.x, an.y), max(an.z, 1e-6)), vec3<f32>(0.0), ambient);
    let ml = cod4_block(ls, q);
    let src = ls.lights[4];
    // `lp_sm_sun_*` (an `sm` set, the sun its light): the block's
    // visibility gives way to the sun shadow map's (cod4-world.md 3.6)
    var vis = ml.a;
    if (stage.skip[1].z > 0.5 && src.ambient.w <= 0.5) {
        vis = cod4_sun_visibility(eye + to_surface, vis);
    }
    // the primary light: its direction and its colour by the visibility
    var l_dir = ls.lights[0].pos.xyz;
    var l_col = vis * ls.ambient.rgb;
    if (src.ambient.w > 0.5) {
        let lv = src.diffuse.xyz - (eye + to_surface);
        let d = length(lv);
        l_dir = lv / max(d, 1e-6);
        var att = cod4_attenuation(ls, clamp(d * src.diffuse.w, 0.0, 1.0));
        if (src.ambient.w > 1.5) {
            let s = clamp(dot(l_dir, src.atten.xyz) * src.spot_dir.x + src.spot_dir.y, 0.0, 1.0);
            att *= select(pow(s, src.spot_dir.z), 0.0, s <= 0.0);
        }
        // `lp_sm_spot_*` (technique 13): the spot's shadow map
        var spot_vis = ml.a;
        if (stage.skip[1].z > 0.5 && src.ambient.w > 1.5) {
            spot_vis = cod4_spot_visibility(eye + to_surface, src.diffuse.xyz, spot_vis);
        }
        l_col = spot_vis * att * src.ambient.rgb;
    }
    var n_dot_l = select(clamp(dot(l_dir, n), 0.0, 1.0), 1.0, ambient);
    if (stage.deform_kinds.w == COD4_FLAG) {
        n_dot_l = abs(dot(l_dir, n));
    }
    let light = 2.0 * ml.rgb + l_col * n_dot_l;
    var spec = vec3<f32>(0.0);
    if ((flags & F_COD2_SPEC) != 0u) {
        let env = stage.skip[0];
        let sm = sample_image(t_spec, uv, cl);
        let vn = dot(v, n);
        let r = v - 2.0 * vn * n;
        let power = exp2(sm.a * 9.3775177) + 7.0;
        let glint = clamp(exp((dot(r, l_dir) - 0.99925) * power), 0.0, 1.0);
        let fresnel = mix(env.x, env.y, pow(1.0 - abs(vn), env.z));
        let layer = i32(ls.lights[0].diffuse.y);
        let probe = textureSampleLevel(t_probes, s_tex_clamp, r, layer, max(6.0 - 8.0 * sm.a, 0.0));
        spec = (l_col * glint * env.w + probe.rgb * probe.a) * sm.rgb * fresnel;
    }
    var a = 1.0;
    if ((flags & F_COD2_ALPHA) != 0u) {
        a = diffuse.a;
    }
    return vec4<f32>((diffuse.rgb * light + spec) * ls.lights[0].pos.w, a);
}
