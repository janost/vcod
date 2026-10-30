// CoD2's shadow cookies (renderer/cod2_cookie.rs, cod2-materials.md 9.8):
// the casters' depth along the sun into an atlas tile
// (shadowcookie_caster.hlsl), the blur (shadowcookie_blur.hlsl) and the
// receivers' darkening of the screen's dynamic shadow
// (shadowcookie_receiver.hlsl), which the lit world and models read.

// ---- caster: an entity surface skinned as dynamic_model.wgsl does ----

// the cookie's caster matrix, world to its clip space
@group(0) @binding(0) var<uniform> caster_vp: mat4x4<f32>;
@group(1) @binding(0) var<storage, read> bones: array<mat4x4<f32>>;

struct CasterOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) depth: f32,
};

@vertex
fn vs_caster(
    @location(0) pos: vec3<f32>,
    @location(3) bi: vec4<u32>,
    @location(4) bw: vec4<f32>,
    @location(5) m0: vec4<f32>,
    @location(6) m1: vec4<f32>,
    @location(7) m2: vec4<f32>,
    @location(8) m3: vec4<f32>,
    @location(9) bone_base: u32,
) -> CasterOut {
    var p = vec4<f32>(0.0);
    for (var i = 0u; i < 4u; i++) {
        p += bw[i] * (bones[bone_base + bi[i]] * vec4<f32>(pos, 1.0));
    }
    var out: CasterOut;
    out.clip = caster_vp * (mat4x4<f32>(m0, m1, m2, m3) * p);
    // shadowLookupMatrix's z, the same row as the clip's
    out.depth = out.clip.z;
    return out;
}

@fragment
fn fs_caster(in: CasterOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.depth, in.depth, in.depth, 1.0);
}

// ---- blur: four taps a texel off, over one tile's inner 126 texels ----

const COOKIE: f32 = 128.0;
const ATLAS: vec2<f32> = vec2<f32>(768.0, 512.0);
const ATLAS_COLS: u32 = 6u;

@group(0) @binding(0) var t_src: texture_2d<f32>;
@group(0) @binding(1) var s_src: sampler;
// x: the half texel the pass's stretch shifts its reads by
@group(0) @binding(2) var<uniform> blur_shift: vec4<f32>;

fn tile_at(tile: u32) -> vec2<f32> {
    return vec2<f32>(f32(tile % ATLAS_COLS), f32(tile / ATLAS_COLS)) * COOKIE;
}

// A quad over tile `tile`'s texels 1..127; the ring keeps the clear.
@vertex
fn vs_blur(@builtin(vertex_index) v: u32, @builtin(instance_index) tile: u32) -> @builtin(position) vec4<f32> {
    let corner = vec2<f32>(f32((0x32u >> v) & 1u), f32((0x2cu >> v) & 1u));
    let px = tile_at(tile) + 1.0 + corner * (COOKIE - 2.0);
    return vec4<f32>(px.x / ATLAS.x * 2.0 - 1.0, 1.0 - px.y / ATLAS.y * 2.0, 0.0, 1.0);
}

@fragment
fn fs_blur(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    let c = at.xy + blur_shift.x;
    var sum = textureSampleLevel(t_src, s_src, (c + vec2<f32>(-1.0, 0.0)) / ATLAS, 0.0);
    sum += textureSampleLevel(t_src, s_src, (c + vec2<f32>(1.0, 0.0)) / ATLAS, 0.0);
    sum += textureSampleLevel(t_src, s_src, (c + vec2<f32>(0.0, -1.0)) / ATLAS, 0.0);
    sum += textureSampleLevel(t_src, s_src, (c + vec2<f32>(0.0, 1.0)) / ATLAS, 0.0);
    return sum * 0.25;
}

// ---- receiver: every cookie's keep at a world surface ----

// the world set's view: the frame's view-projection and the set's pose
struct RecvView {
    view_proj: mat4x4<f32>,
    // a moved brush model's pose; identity for the world's own draw
    model: mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> recv_view: RecvView;

struct Cookie {
    lookup: mat4x4<f32>,
    // xy the tile's top-left texel, z the fade (shadowParms.a)
    at: vec4<f32>,
};
struct Cookies {
    count: vec4<u32>,
    cookies: array<Cookie, 24>,
};
@group(1) @binding(0) var<uniform> recv: Cookies;
@group(1) @binding(1) var t_cookie: texture_2d<f32>;
@group(1) @binding(2) var s_cookie: sampler;

struct RecvOut {
    // invariant: over the world's depth prepass (cod2_world.wgsl's
    // vs_main, the same product) the receivers test LessEqual
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    // the cookies whose square the draw's box reaches, bit i cookie i
    // (renderer::cod2_cookie::Cookies::receiver_masks)
    @location(1) @interpolate(flat) mask: u32,
};

// The draw's first instance is its cookie mask.
@vertex
fn vs_receive(@location(0) pos: vec3<f32>, @builtin(instance_index) mask: u32) -> RecvOut {
    let w = recv_view.model * vec4<f32>(pos, 1.0);
    var out: RecvOut;
    out.clip = recv_view.view_proj * w;
    out.world = w.xyz;
    out.mask = mask;
    return out;
}

// The dynamic shadow at the pixel, as retail's: the white it
// clears to after its blend (Zero, InvSrcColor) of each cookie's
// `alpha * fade * clip` in turn. Off a cookie its clamped sample is the
// cleared ring, alpha 0, so that cookie is skipped. A cookie off the draw's
// mask is skipped without the test: its square misses the draw's box, so
// every pixel there would skip it. The rest multiply in cookie order.
@fragment
fn fs_receive(in: RecvOut) -> @location(0) vec4<f32> {
    var keep = 1.0;
    var m = in.mask & ((1u << recv.count.x) - 1u);
    while (m != 0u) {
        let i = firstTrailingBit(m);
        m &= m - 1u;
        let c = recv.cookies[i];
        let lp = c.lookup * vec4<f32>(in.world, 1.0);
        let t = lp.xy * COOKIE;
        if (any(t < vec2<f32>(0.0)) || any(t > vec2<f32>(COOKIE))) {
            continue;
        }
        let uv = (clamp(t, vec2<f32>(0.5), vec2<f32>(COOKIE - 0.5)) + c.at.xy) / ATLAS;
        let s = textureSampleLevel(t_cookie, s_cookie, uv, 0.0);
        let clip = saturate((lp.z - s.b) * 64.0 + 0.5);
        keep *= 1.0 - s.a * c.at.z * clip;
    }
    return vec4<f32>(keep, keep, keep, 1.0);
}
