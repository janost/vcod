// r_gamma and the overbright doubling: retail's hardware gamma ramp as a
// final pass (crate::gamma). The scene holds retail's framebuffer bytes;
// the 256-entry table is the ramp, shifted by the overbright bits.
@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var ramp: texture_2d<f32>;

// The swapchain is an sRGB format: encode on write, so linearise here.
override SRGB: bool = true;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

fn lookup(v: f32) -> f32 {
    let k = i32(round(clamp(v, 0.0, 1.0) * 255.0));
    return textureLoad(ramp, vec2<i32>(k, 0), 0).r;
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = textureLoad(scene, vec2<i32>(pos.xy), 0).rgb;
    var o = vec3<f32>(lookup(c.r), lookup(c.g), lookup(c.b));
    if (SRGB) {
        o = to_linear(o);
    }
    return vec4<f32>(o, 1.0);
}
