struct Camera { view_proj: mat4x4<f32> };
@group(0) @binding(0) var<uniform> camera: Camera;

struct FxLights {
    pos_radius: array<vec4<f32>, 8>,
    color: array<vec4<f32>, 8>,
}
@group(0) @binding(1) var<uniform> fx_lights: FxLights;

@group(1) @binding(0) var t_diffuse: texture_2d<f32>;
@group(1) @binding(1) var t_lightmap: texture_2d<f32>;
@group(1) @binding(2) var s_diffuse: sampler;
@group(1) @binding(3) var s_lightmap: sampler;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) lm_uv: vec2<f32>,
    @location(3) normal: vec3<f32>,
    @location(4) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) lm_uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    // BSP vertices are already world space.
    @location(3) world_pos: vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = camera.view_proj * vec4<f32>(in.pos, 1.0);
    out.uv = in.uv;
    out.lm_uv = in.lm_uv;
    out.color = in.color;
    out.world_pos = in.pos;
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

fn shade(in: VsOut, albedo: vec4<f32>) -> vec3<f32> {
    // Lightmapped surfaces have white vertex color; vertex-lit surfaces get a
    // white 1x1 lightmap bound instead. Multiplying all three covers both.
    let light = textureSample(t_lightmap, s_lightmap, in.lm_uv).rgb;
    return albedo.rgb * (light * in.color.rgb * 2.0 + fx_light_term(in.world_pos));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let albedo = textureSample(t_diffuse, s_diffuse, in.uv);
    if (albedo.a < 0.5) { discard; }
    return vec4<f32>(shade(in, albedo), 1.0);
}

// Alpha-to-coverage: sampled alpha drives MSAA coverage for antialiased cutout edges.
@fragment
fn fs_overlay(in: VsOut) -> @location(0) vec4<f32> {
    let albedo = textureSample(t_diffuse, s_diffuse, in.uv);
    return vec4<f32>(shade(in, albedo), albedo.a);
}

// polygonOffset terrain blends, weighted by vertex alpha (`alphagen vertex`).
@fragment
fn fs_layer(in: VsOut) -> @location(0) vec4<f32> {
    let albedo = textureSample(t_diffuse, s_diffuse, in.uv);
    return vec4<f32>(shade(in, albedo), albedo.a * in.color.a);
}
