// r_gamma: retail's hardware gamma ramp as a final pass (crate::gamma).
// The float scene holds values up to 2.0 (encoded); the 512-entry table is
// indexed by value * 255 and `display_table` folds the overbright bit's e/2
// indexing into it.
@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var ramp: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

// The swapchain is an sRGB format: decode on sample, encode on write.
override SRGB: bool = true;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

fn to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

fn lookup(v: f32) -> f32 {
    let u = (v * 255.0 + 0.5) / 512.0;
    return textureSampleLevel(ramp, samp, vec2<f32>(u, 0.5), 0.0).r;
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    var c = max(textureLoad(scene, vec2<i32>(pos.xy), 0).rgb, vec3<f32>(0.0));
    if (SRGB) {
        c = to_srgb(c);
    }
    var o = vec3<f32>(lookup(c.r), lookup(c.g), lookup(c.b));
    if (SRGB) {
        o = to_linear(o);
    }
    return vec4<f32>(o, 1.0);
}
