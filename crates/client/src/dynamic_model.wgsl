// Snapshot entities: GPU-skinned xmodel instances in world space. Unlit; a
// fixed key light stands in for the engine's light grid.

struct Camera {
    view_proj: mat4x4<f32>,
    time_pad: vec4<f32>, // .x = seconds since start; yzw reserved
};
@group(0) @binding(0) var<uniform> camera: Camera;

struct FxLights {
    pos_radius: array<vec4<f32>, 8>,
    color: array<vec4<f32>, 8>,
}
@group(0) @binding(1) var<uniform> fx_lights: FxLights;

@group(1) @binding(0) var t_diffuse: texture_2d<f32>;
@group(1) @binding(1) var s_diffuse: sampler;
// All instances' bone matrices, world space. Slot 0 is the shared identity
// block (bind pose).
@group(2) @binding(0) var<storage, read> bones: array<mat4x4<f32>>;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) world_pos: vec3<f32>,
};

@vertex
fn vs_main(
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) bi: vec4<u32>,
    @location(4) bw: vec4<f32>,
    @location(5) m0: vec4<f32>,
    @location(6) m1: vec4<f32>,
    @location(7) m2: vec4<f32>,
    @location(8) m3: vec4<f32>,
    @location(9) bone_base: u32,
) -> VsOut {
    let model = mat4x4<f32>(m0, m1, m2, m3);
    var p = vec4<f32>(0.0);
    var n = vec3<f32>(0.0);
    for (var i = 0u; i < 4u; i++) {
        let m = bones[bone_base + bi[i]];
        p += bw[i] * (m * vec4<f32>(pos, 1.0));
        // rigid bone transforms, so the upper 3x3 is valid for normals
        n += bw[i] * (mat3x3<f32>(m[0].xyz, m[1].xyz, m[2].xyz) * normal);
    }
    let world = model * p;
    var out: VsOut;
    out.clip = camera.view_proj * world;
    // rotation + translation, so the upper 3x3 is valid for the normal
    out.normal = (model * vec4<f32>(n, 0.0)).xyz;
    out.uv = uv;
    out.world_pos = world.xyz;
    return out;
}

// Quadratic falloff to zero at the radius; zero-radius slots are unused.
fn fx_light_term(world_pos: vec3<f32>) -> vec3<f32> {
    var sum = vec3(0.0);
    for (var i = 0u; i < 8u; i++) {
        let l = fx_lights.pos_radius[i];
        if (l.w <= 0.0) { continue; }
        let d = distance(world_pos, l.xyz);
        let a = clamp(1.0 - d / l.w, 0.0, 1.0);
        sum += fx_lights.color[i].rgb * a * a;
    }
    return sum;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let tex = textureSample(t_diffuse, s_diffuse, in.uv);
    // same alpha-test threshold as the map's masked materials
    if (tex.a < 0.5) { discard; }
    let n = normalize(in.normal);
    let light = normalize(vec3<f32>(-0.4, -0.3, 0.9));
    let half_lambert = max(dot(n, light), 0.0) * 0.5 + 0.5;
    return vec4<f32>(tex.rgb * (half_lambert + fx_light_term(in.world_pos)), 1.0);
}
