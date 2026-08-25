// On-screen HUD quads, positions already clip space (`Renderer::set_hud_quads`).
// Straight alpha like the fx pass; font pages and icons are not premultiplied.
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

struct VsIn {
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) rgba: vec4<f32>,
}
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) rgba: vec4<f32>,
}

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var o: VsOut;
    o.clip = vec4(in.pos, 0.0, 1.0);
    o.uv = in.uv;
    o.rgba = in.rgba;
    return o;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let t = textureSample(tex, samp, in.uv);
    return vec4(t.rgb * in.rgba.rgb, t.a * in.rgba.a);
}
