// crate::scene_copy: the single-sample scene target copied texel for
// texel into the copy the distortion effects bend.

@group(0) @binding(0) var t_scene: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let q = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(q * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_blit(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(t_scene, vec2<i32>(pos.xy), 0);
}
