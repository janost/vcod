// CoD4's depth of field (crate::cod4_dof, cod4-client-view.md 12): 1.7's
// `dof_downsample`, the gaussian, `dof_near_coc`, `small_blur` and
// `postfx_dof` pixel shaders over the resolved world.

struct Params {
    // dofEquationScene: near scale, far scale, near bias, far bias
    scene: vec4<f32>,
    // dofEquationViewModelAndFarBlur: the view model's near scale and
    // bias in x and z, (farBlur / nearBlur) ^ r_dof_bias in w
    vm: vec4<f32>,
    lerp_scale: vec4<f32>,
    lerp_bias: vec4<f32>,
    // x the gaussian's sigma in quarter pixels; y, z the world's near
    // and far planes; w the view model's far plane
    a: vec4<f32>,
    // x the view model's near plane, y its depth range's front slice
    b: vec4<f32>,
};

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var src0: texture_2d<f32>;
@group(0) @binding(2) var src1: texture_2d<f32>;
@group(0) @binding(3) var src2: texture_2d<f32>;
@group(0) @binding(4) var t_depth: texture_depth_multisampled_2d;
@group(0) @binding(5) var smp: sampler;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let q = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(q * 2.0 - 1.0, 0.0, 1.0);
}

// retail's floatZ: the view depth, negative on the view model, which
// draws into the depth range's front slice off its own planes
fn float_z(px: vec2<i32>) -> f32 {
    let size = vec2<i32>(textureDimensions(t_depth));
    let d = textureLoad(t_depth, clamp(px, vec2<i32>(0), size - 1), 0);
    if (d < p.b.y) {
        let n = p.b.x;
        let f = p.a.w;
        return -(f * n / (f - (d / p.b.y) * (f - n)));
    }
    let n = p.a.y;
    let f = p.a.z;
    return f * n / (f - d * (f - n));
}

// the near blur of one depth: the scene's or, on the view model, its own
fn near_coc(z: f32) -> f32 {
    let scene = clamp(p.scene.x * z + p.scene.z, 0.0, 1.0);
    let vm = clamp(p.vm.x * -z + p.vm.z, 0.0, 1.0);
    return min(scene, vm);
}

fn load(t: texture_2d<f32>, px: vec2<i32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(t));
    return textureLoad(t, clamp(px, vec2<i32>(0), size - 1), 0);
}

// dof_downsample.hlsl: each quarter pixel the mean of its 4x4 scene
// pixels, alpha the most near blur among their depths
@fragment
fn fs_downsample(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let base = vec2<i32>(pos.xy) * 4;
    var rgb = vec3<f32>(0.0);
    var coc = 0.0;
    for (var y = 0; y < 4; y++) {
        for (var x = 0; x < 4; x++) {
            let px = base + vec2<i32>(x, y);
            rgb += load(src0, px).rgb;
            coc = max(coc, near_coc(float_z(px)));
        }
    }
    return vec4<f32>(rgb / 16.0, coc);
}

// RB_GaussianFilterImage's weights, exp(-x^2 / 2 sigma^2), on one axis
fn gaussian(pos: vec4<f32>, axis: vec2<i32>) -> vec4<f32> {
    let sigma = p.a.x;
    let px = vec2<i32>(pos.xy);
    if (sigma < 0.3) {
        return load(src0, px);
    }
    let r = min(i32(ceil(sigma * 3.0)), 32);
    let k = -0.5 / (sigma * sigma);
    var sum = vec4<f32>(0.0);
    var total = 0.0;
    for (var i = -r; i <= r; i++) {
        let w = exp(f32(i * i) * k);
        sum += load(src0, px + axis * i) * w;
        total += w;
    }
    return sum / total;
}

@fragment
fn fs_blur_h(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return gaussian(pos, vec2<i32>(1, 0));
}

@fragment
fn fs_blur_v(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return gaussian(pos, vec2<i32>(0, 1));
}

// dof_near_coc.hlsl: the unblurred quarter's colour (src1), its blur
// widened over the blurred one's (src0): 2 max(blurred, own) - own
@fragment
fn fs_near_coc(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let px = vec2<i32>(pos.xy);
    let blurred = load(src0, px);
    let own = load(src1, px);
    return vec4<f32>(own.rgb, 2.0 * max(blurred.a, own.a) - own.a);
}

// small_blur.hlsl: four bilinear taps half a pixel out on each axis
@fragment
fn fs_small_blur(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(src0));
    let uv = pos.xy / size;
    let h = 0.5 / size;
    let s = textureSampleLevel(src0, smp, uv + vec2<f32>(-h.x, -h.y), 0.0)
        + textureSampleLevel(src0, smp, uv + vec2<f32>(h.x, -h.y), 0.0)
        + textureSampleLevel(src0, smp, uv + vec2<f32>(-h.x, h.y), 0.0)
        + textureSampleLevel(src0, smp, uv + vec2<f32>(h.x, h.y), 0.0);
    return s * 0.25;
}

// postfx_dof.hlsl: the scene (src0) sharp, through its 17-texel small
// blur, the quarter's medium blur (src1) and the gaussian's large one
// (src2), weighted by the pixel's blur: the near one the quarter carries,
// widened by its own far one on anything nearer than the sky
@fragment
fn fs_composite(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(src0));
    let uv = pos.xy / size;
    let t = 1.0 / size;
    let tap = 4.0 / 17.0;
    var small = textureSampleLevel(src0, smp, uv + t * vec2<f32>(0.5, -1.5), 0.0).rgb * tap;
    let sharp = textureSampleLevel(src0, smp, uv, 0.0).rgb;
    small += sharp / 17.0;
    small += textureSampleLevel(src0, smp, uv + t * vec2<f32>(-1.5, -0.5), 0.0).rgb * tap;
    small += textureSampleLevel(src0, smp, uv + t * vec2<f32>(-0.5, 1.5), 0.0).rgb * tap;
    small += textureSampleLevel(src0, smp, uv + t * vec2<f32>(1.5, 0.5), 0.0).rgb * tap;
    let z = float_z(vec2<i32>(pos.xy));
    let far = clamp(p.scene.y * z + p.scene.w, 0.0, 1.0) * p.vm.w;
    let medium = textureSampleLevel(src1, smp, uv, 0.0);
    var coc = medium.a;
    if (z <= 1500000.0) {
        coc = max(far, coc);
    }
    let w = clamp(coc * p.lerp_scale + p.lerp_bias, vec4<f32>(0.0), vec4<f32>(1.0));
    let large = textureSampleLevel(src2, smp, uv, 0.0).rgb;
    let rgb = small * min(1.0 - w.x, w.y) + sharp * w.x + medium.rgb * min(1.0 - w.y, w.z)
        + large * w.w;
    return vec4<f32>(rgb, 1.0);
}
