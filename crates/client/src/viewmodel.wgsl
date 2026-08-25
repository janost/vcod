// First-person viewmodel: own projection, bind pose baked into the vertices,
// placed in view space by the motion transform. Unlit; a key light stands in
// for the engine's light grid.

struct VmUniform {
    proj: mat4x4<f32>,      // viewmodel projection (own FOV)
    model: mat4x4<f32>,     // motion transform (view space)
    light_dir: vec4<f32>,   // view-space, normalized, w unused
};
@group(0) @binding(0) var<uniform> u: VmUniform;
@group(1) @binding(0) var t_diffuse: texture_2d<f32>;
@group(1) @binding(1) var s_diffuse: sampler;
// View-space skin matrices; identity reproduces the baked bind pose.
@group(2) @binding(0) var<uniform> bones: array<mat4x4<f32>, 64>;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) normal: vec3<f32>,
};

@vertex
fn vs_main(
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) bi: vec4<u32>,
    @location(4) bw: vec4<f32>,
) -> VsOut {
    var p = vec4<f32>(0.0);
    var n = vec3<f32>(0.0);
    for (var i = 0u; i < 4u; i++) {
        let m = bones[bi[i]];
        p += bw[i] * (m * vec4<f32>(pos, 1.0));
        // rigid bone transforms, so the upper 3x3 is valid for normals
        n += bw[i] * (mat3x3<f32>(m[0].xyz, m[1].xyz, m[2].xyz) * normal);
    }
    var out: VsOut;
    out.pos = u.proj * u.model * p;
    out.uv = uv;
    out.normal = (u.model * vec4<f32>(n, 0.0)).xyz;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let tex = textureSample(t_diffuse, s_diffuse, in.uv);
    // same alpha-test threshold as the map's masked materials
    if (tex.a < 0.5) { discard; }
    let n = normalize(in.normal);
    let half_lambert = max(dot(n, normalize(u.light_dir.xyz)), 0.0) * 0.5 + 0.5;
    return vec4<f32>(tex.rgb * half_lambert, 1.0);
}
