// CoD2's glow (crate::cod2_glow; docs/research/cod2-materials.md 9.9):
// glow_setup.hlsl, filter_symmetric_*.hlsl, glow_apply_bloom.hlsl,
// glow_apply_sky_bleed.hlsl and the menu blur's feedbackblend (9.12), one
// full-screen triangle each. uv is the destination pixel's centre over
// params.count.yz.
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;

struct Params {
    taps: array<vec4<f32>, 8>,
    // x tap count, yz the destination's 1 / size
    count: vec4<f32>,
    // glowSetup (cutoff, cutoff rescale, 0, desaturation) or glowApply
    // (sky bleed, 0, 0, bloom)
    glow: vec4<f32>,
    inv_view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    // CoD4's setup: colorBias, colorTintBase (w identityLight),
    // colorTintDelta
    film_bias: vec4<f32>,
    film_base: vec4<f32>,
    film_delta: vec4<f32>,
};
@group(0) @binding(2) var<uniform> params: Params;
@group(0) @binding(3) var sky: texture_cube<f32>;

const LUMA: vec3<f32> = vec3<f32>(0.299, 0.587, 0.114);

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

fn uv_of(pos: vec4<f32>) -> vec2<f32> {
    return pos.xy * params.count.yz;
}

// SampleContribution: the bright part of a scene texel, rescaled past the
// cutoff, times its non-sky fraction; alpha the sky fraction. The scene's
// own alpha stands in for max(scene, post-sun) (9.9).
fn contribution(uv: vec2<f32>) -> vec4<f32> {
    let px = textureSampleLevel(src, smp, uv, 0.0);
    let intensity = dot(px.rgb, LUMA);
    var rgb = vec3<f32>(0.0);
    if intensity >= params.glow.x {
        rgb = 1.0 + (px.rgb - 1.0) * params.glow.y;
    }
    let non_sky = clamp(4.0 * px.a, 0.0, 1.0);
    return vec4<f32>(rgb * non_sky, 1.0 - non_sky);
}

@fragment
fn fs_setup(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = uv_of(pos);
    let h = 0.5 * params.count.yz;
    var c = contribution(uv + vec2<f32>(-h.x, -h.y));
    c += contribution(uv + vec2<f32>(-h.x, h.y));
    c += contribution(uv + vec2<f32>(h.x, -h.y));
    c += contribution(uv + vec2<f32>(h.x, h.y));
    c *= 0.25;
    let intensity = dot(c.rgb, LUMA);
    return vec4<f32>(mix(c.rgb, vec3<f32>(intensity), params.glow.w), c.a);
}

// CoD4's glow_consistent_setup.hlsl (cod4-world.md 11.4): each tap's
// luma over the cutoff, rescaled, times the tap through postfx_color's
// film tweak; the mean of four pulled to its luma by the desaturation.
// The scene is read as retail's 8-bit framebuffer, display values.
fn consistent(uv: vec2<f32>) -> vec3<f32> {
    let il = params.film_base.w;
    let c = clamp(textureSampleLevel(src, smp, uv, 0.0).rgb / il, vec3<f32>(0.0), vec3<f32>(1.0));
    let l = dot(c, LUMA);
    let tint = params.film_delta.rgb * l + params.film_base.rgb;
    let film = (c * params.film_bias.w + l) * tint + params.film_bias.rgb;
    return clamp(l - params.glow.x, 0.0, 1.0) * film * params.glow.y;
}

@fragment
fn fs_setup4(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = uv_of(pos);
    let h = 0.5 * params.count.yz;
    let sum = consistent(uv + vec2<f32>(-h.x, -h.y)) + consistent(uv + vec2<f32>(-h.x, h.y))
        + consistent(uv + vec2<f32>(h.x, -h.y)) + consistent(uv + h);
    let c = sum * 0.25;
    return vec4<f32>(mix(c, vec3<f32>(dot(c, LUMA)), params.glow.w), 1.0);
}

@fragment
fn fs_filter(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = uv_of(pos);
    var c = vec4<f32>(0.0);
    let n = u32(params.count.x);
    for (var i = 0u; i < n; i++) {
        let t = params.taps[i];
        c += t.w * (textureSampleLevel(src, smp, uv + t.xy, 0.0)
            + textureSampleLevel(src, smp, uv - t.xy, 0.0));
    }
    return c;
}

@fragment
fn fs_bloom(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = textureSampleLevel(src, smp, uv_of(pos), 0.0);
    return vec4<f32>(c.rgb * params.glow.w, c.a);
}

@fragment
fn fs_sky_bleed(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = uv_of(pos);
    let blurred = textureSampleLevel(src, smp, uv, 0.0);
    // the pixel's view ray, glow_apply_sky_bleed's nearPlane interpolation
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.5, 1.0);
    let p = params.inv_view_proj * ndc;
    let dir = p.xyz / p.w - params.eye.xyz;
    let sky_px = textureSampleLevel(sky, smp, dir, 0.0);
    let a = clamp(blurred.a * 2.0, 0.0, 1.0) * params.glow.x;
    return vec4<f32>(sky_px.rgb * a + blurred.rgb * params.glow.w, a);
}

// feedbackblend (passthru_vertalpha, vertcol_notexalpha.hlsl): the
// blurred view times a white vertex colour, its alpha the colour's,
// glow.x here (RB_ApplyLatePostEffectsCmd's ramp below 3 pixels)
@fragment
fn fs_blur_composite(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = textureSampleLevel(src, smp, uv_of(pos), 0.0);
    return vec4<f32>(c.rgb, params.glow.x);
}
