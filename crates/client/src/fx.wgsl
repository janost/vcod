// World-space fx quads: depth-tested, writing depth only unblended. Each
// quad is drawn once per stage of its material, and its per-vertex rgba is
// that stage's gen colour (`renderer::fx_stage_rgba`), which multiplies the
// texture. One entry point per `FxBlend` (`renderer::fx_stage_draw`):
// `fs_main` straight alpha and unblended, `fs_additive` onto `GL_ONE`,
// `fs_screen` `GL_ONE GL_ONE_MINUS_SRC_COLOR`.

// Same uniform as shader.wgsl; the fog fields are documented there.
struct Camera {
    view_proj: mat4x4<f32>,
    time_pad: vec4<f32>,
    eye_fog_mode: vec4<f32>,
    fog_color_density: vec4<f32>,
    fog_range: vec4<f32>,
    view_fwd: vec4<f32>,
}
@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var tex: texture_2d<f32>;

// vc_softParticles (crate::soft_particles); only the soft entry points
// read group 2, and only their pipelines declare it.
struct SoftParams {
    // x near, y far, z vc_softParticles
    p: vec4<f32>,
}
@group(2) @binding(0) var<uniform> soft: SoftParams;
@group(2) @binding(1) var scene_z: texture_2d<f32>;
// crate::scene_copy: the scene's colour at the split, which only
// fs_distort reads (its pipeline's group 2 is the copy's layout).
@group(2) @binding(2) var scene_copy: texture_2d<f32>;
@group(2) @binding(3) var scene_samp: sampler;

// renderer::FX_VERT_*
const FX_VERT_NOFOG: u32 = 1u;
const FX_VERT_TEX_ALPHA: u32 = 2u;
const FX_VERT_ALPHA_GT0: u32 = 4u;
const FX_VERT_ALPHA_LT128: u32 = 8u;
const FX_VERT_ALPHA_GE128: u32 = 16u;
const FX_VERT_FEATHER_W: u32 = 32u;
const FX_VERT_DISTORT: u32 = 64u;
// F_CLAMP_S and F_CLAMP_T (tex_samplers.wgsl) ride the flags too, so
// sample_image reads them straight off.

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) rgba: vec4<f32>,
    @location(3) size: f32,
    @location(4) flags: u32,
    @location(5) feather: f32,
}
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) rgba: vec4<f32>,
    @location(2) world_pos: vec3<f32>,
    @location(3) size: f32,
    @location(4) @interpolate(flat) flags: u32,
    @location(5) @interpolate(flat) feather: f32,
    // CoD2's fog amount, per vertex as `vertex_color_simple.hlsl`'s
    // `Fog_VertexCalc` writes it (cod2-client-perf.md 9); CoD1 and CoD4
    // fog per pixel
    @location(6) fog: f32,
}

// CoD2's z_feather.hlsl on retail's projection (cod2-materials.md 9.5):
// clip z is FEATHER_K * (view depth - FEATHER_NEAR) (the infinite
// perspective's 0.9995 and r_znear 4), times featherParms.x.
// CoD4's zfeather.hlsl reads the view depth (clip w) itself
// (FX_VERT_FEATHER_W; cod4-effects.md 14).
const FEATHER_K: f32 = 0.99950027;
const FEATHER_NEAR: f32 = 4.0;

fn feather_depth(depth: f32, flags: u32) -> f32 {
    if ((flags & FX_VERT_FEATHER_W) != 0u) {
        return depth;
    }
    return FEATHER_K * (depth - FEATHER_NEAR);
}

@vertex fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = camera.view_proj * vec4(in.pos, 1.0);
    out.uv = in.uv;
    out.rgba = in.rgba;
    // the near-camera attenuation, per vertex as z_feather.hlsl's vs_main
    // folds it into the vertex alpha; clip w is the view depth
    if (in.feather > 0.0 && (in.flags & FX_VERT_DISTORT) == 0u) {
        out.rgba.a *= clamp(feather_depth(out.clip.w, in.flags) * in.feather, 0.0, 1.0);
    }
    out.feather = in.feather;
    out.world_pos = in.pos;
    out.size = in.size;
    out.flags = in.flags;
    if (camera.fog_range.z > 0.5) {
        out.fog = fog_at(in.pos, in.flags);
    }
    return out;
}

// glFog GL_EXP / GL_LINEAR factors (see shader.wgsl); depth along the view
// forward, the fixed-function fog coordinate without GL_NV_fog_distance.
// A `nofog` material never fogs (CoDMP.exe 0x4fbc71).
fn fog_at(world_pos: vec3<f32>, flags: u32) -> f32 {
    let mode = camera.eye_fog_mode.w;
    if (mode == 0.0 || (flags & FX_VERT_NOFOG) != 0u) {
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

// The fragment's fog amount: the vertex's on CoD2, [`fog_at`] here
// elsewhere.
fn fog_amount(in: VsOut) -> f32 {
    if (camera.fog_range.z > 0.5) {
        return in.fog;
    }
    return fog_at(in.world_pos, in.flags);
}

// The stage's alphaFunc on the fragment's alpha, texture times gen
// (GL_ALPHA_TEST after GL_MODULATE).
fn alpha_test(in: VsOut, a: f32) {
    let f = in.flags;
    let k = 128.0 / 255.0;
    if (((f & FX_VERT_ALPHA_GT0) != 0u && a <= 0.0)
        || ((f & FX_VERT_ALPHA_LT128) != 0u && a >= k)
        || ((f & FX_VERT_ALPHA_GE128) != 0u && a < k)) {
        discard;
    }
}

// Fogged like the surface a decal sits on, so bullet holes and smoke
// take the same colour as the wall behind them.
fn shade(in: VsOut) -> vec4<f32> {
    let t = sample_image(tex, in.uv, in.flags);
    alpha_test(in, t.a * in.rgba.a);
    let rgb = mix(t.rgb * in.rgba.rgb, camera.fog_color_density.rgb, fog_amount(in));
    return vec4(rgb, t.a * in.rgba.a);
}

// A sprite without `useAlpha` carries its fade in its rgb and alpha 1
// (`fx::sim`, CoDMP.exe 0x48e7d0); a `GL_ONE GL_ONE` material reads no
// source alpha, so `Renderer::set_fx_quads` hands it alpha 1 too. What alpha
// is left scales the colour, times the texture's alpha under
// `GL_SRC_ALPHA GL_ONE` (`FX_VERT_TEX_ALPHA`).
// Fog onto `GL_ONE` fades toward black, not the fog colour (CoDMP.exe
// 0x4d2ad0): adding fog colour would brighten glows the further into the
// fog they sit.
fn shade_additive(in: VsOut) -> vec4<f32> {
    let t = sample_image(tex, in.uv, in.flags);
    alpha_test(in, t.a * in.rgba.a);
    var a = in.rgba.a;
    if ((in.flags & FX_VERT_TEX_ALPHA) != 0u) {
        a = a * t.a;
    }
    let rgb = t.rgb * in.rgba.rgb * a * (1.0 - fog_amount(in));
    return vec4(rgb, 0.0);
}

// `GL_ONE GL_ONE_MINUS_SRC_COLOR`: the colour adds and screens what is
// behind by itself, so black draws nothing; alpha is never read. Its
// destination is not `GL_ONE`, so it fogs toward the fog colour.
fn shade_screen(in: VsOut) -> vec4<f32> {
    let t = sample_image(tex, in.uv, in.flags);
    alpha_test(in, t.a * in.rgba.a);
    let rgb = mix(t.rgb * in.rgba.rgb, camera.fog_color_density.rgb, fog_amount(in));
    return vec4(rgb, 0.0);
}

@fragment fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return shade(in);
}

@fragment fn fs_additive(in: VsOut) -> @location(0) vec4<f32> {
    return shade_additive(in);
}

@fragment fn fs_screen(in: VsOut) -> @location(0) vec4<f32> {
    return shade_screen(in);
}

// The fade over the gap to the opaque scene behind: a sprite's fade
// distance is half its smaller half-extent times vc_softParticles, at least
// a unit and at most 32, so a spark at a wall stays visible and a big
// explosion card does not vanish into the ground.
//
// A feathered quad (`in.feather` > 0) takes z_feather.hlsl's (or CoD4's
// zfeather.hlsl's) fade in its place: alpha times saturate(|scene z - sprite z| * featherParms.x) in
// retail's clip z, whose near offset cancels in the difference. Neither
// fades a quad drawn in a soft pipeline only for the other.
fn soft_fade(in: VsOut) -> f32 {
    let frag = in.clip;
    let n = soft.p.x;
    let f = soft.p.y;
    let scene = textureLoad(scene_z, vec2<i32>(frag.xy), 0).r;
    let z = f * n / (f - frag.z * (f - n));
    if (in.feather > 0.0) {
        let gap = feather_depth(scene, in.flags) - feather_depth(z, in.flags);
        return clamp(abs(gap * in.feather), 0.0, 1.0);
    }
    if (soft.p.z <= 0.0) {
        return 1.0;
    }
    let dist = soft.p.z * clamp(in.size * 0.5, 1.0, 32.0);
    return clamp((scene - z) / dist, 0.0, 1.0);
}

@fragment fn fs_soft(in: VsOut) -> @location(0) vec4<f32> {
    let c = shade(in);
    return vec4(c.rgb, c.a * soft_fade(in));
}

@fragment fn fs_soft_additive(in: VsOut) -> @location(0) vec4<f32> {
    let c = shade_additive(in);
    return vec4(c.rgb * soft_fade(in), 0.0);
}

// A screen source scaled toward black draws less of itself and screens
// less of what is behind, so the fade scales the colour as the additive's.
@fragment fn fs_soft_screen(in: VsOut) -> @location(0) vec4<f32> {
    let c = shade_screen(in);
    return vec4(c.rgb * soft_fade(in), 0.0);
}

// CoD4's distortion_scale_zfeather_dtex.hlsl and distortion_zfeather.hlsl
// (cod4-effects.md 17) on a quad whose `feather` and `size` carry the
// material's distortionScale.xy (FX_VERT_DISTORT): the offset map (the
// material's colour map), as 2 n - 1, bends the scene copy's lookup
// along the sprite's tangent and binormal in clip space, scaled by the
// vertex colour's red and green; where the copy's depth at the bent
// point is nearer than the sprite's clip z, the unbent point is read
// instead. The colour is the copy times the vertex blue, the alpha the
// map's times the vertex alpha. The tangent is the quad's u axis, the
// binormal tangent x normal with the normal toward the eye. D3D9's half
// texel lookup offset is left out: wgpu samples at texel centres.
@fragment fn fs_distort(in: VsOut) -> @location(0) vec4<f32> {
    let n = sample_image(tex, in.uv, in.flags);
    let p = in.world_pos;
    let dpx = dpdx(p);
    let dpy = dpdy(p);
    let dux = dpdx(in.uv);
    let duy = dpdy(in.uv);
    let det = dux.x * duy.y - dux.y * duy.x;
    var t = (dpx * duy.y - dpy * dux.y) * select(1.0 / det, 0.0, det == 0.0);
    t = t * inverseSqrt(max(dot(t, t), 1e-12));
    var nrm = cross(dpx, dpy);
    nrm = nrm * inverseSqrt(max(dot(nrm, nrm), 1e-12));
    if (dot(nrm, camera.eye_fog_mode.xyz - p) < 0.0) {
        nrm = -nrm;
    }
    let b = cross(t, nrm);
    let c = camera.view_proj * vec4(p, 1.0);
    let tc = camera.view_proj * vec4(t, 0.0);
    let bc = camera.view_proj * vec4(b, 0.0);
    // clipSpaceLookupScale (0.5, -0.5) and its offset (0.5, 0.5) times w;
    // the bytecode's tangent terms keep +0.5 on y
    let s = vec4(c.x * 0.5 + c.w * 0.5, -c.y * 0.5 + c.w * 0.5, 0.0, c.w);
    let o3 = in.feather * in.rgba.r * vec4(tc.x * 0.5, tc.y * 0.5, 0.0, tc.w);
    let o4 = -in.size * in.rgba.g * vec4(bc.x * 0.5, bc.y * 0.5, 0.0, bc.w);
    let off = n.xy * 2.0 - 1.0;
    let q = s + off.x * o3 + off.y * o4;
    let bent = q.xy / q.w;
    let flat_uv = s.xy / s.w;
    let dims = vec2<f32>(textureDimensions(scene_z));
    let texel = clamp(vec2<i32>(bent * dims), vec2(0), vec2<i32>(dims) - 1);
    let z = textureLoad(scene_z, texel, 0).r;
    let bent_rgb = textureSampleLevel(scene_copy, scene_samp, bent, 0.0).rgb;
    let flat_rgb = textureSampleLevel(scene_copy, scene_samp, flat_uv, 0.0).rgb;
    let rgb = select(flat_rgb, bent_rgb, abs(z) - FEATHER_K * (c.w - FEATHER_NEAR) >= 0.0);
    let a = n.a * in.rgba.a;
    alpha_test(in, a);
    return vec4(rgb * in.rgba.b, a);
}
