// World-space fx quads: depth-tested, never writing depth. Per-vertex rgba
// multiplies the texture. `fs_main` is straight alpha, `fs_additive` is
// `GL_ONE GL_ONE` for glow materials (`assets::Shaders::additive`).

struct Camera { view_proj: mat4x4<f32> }
@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) rgba: vec4<f32>,
}
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) rgba: vec4<f32>,
}
@vertex fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = camera.view_proj * vec4(in.pos, 1.0);
    out.uv = in.uv;
    out.rgba = in.rgba;
    return out;
}
@fragment fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let t = textureSample(tex, samp, in.uv);
    return vec4(t.rgb * in.rgba.rgb, t.a * in.rgba.a);
}

// GL_ONE GL_ONE ignores source alpha, so the fade scales the colour. The
// engine likewise bakes the alpha curve into vertex RGB for emitters without
// `useAlpha` (docs/research/efx-grammar.md, section 3). Texture alpha is never read.
@fragment fn fs_additive(in: VsOut) -> @location(0) vec4<f32> {
    let t = textureSample(tex, samp, in.uv);
    return vec4(t.rgb * in.rgba.rgb * in.rgba.a, 0.0);
}
