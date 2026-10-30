// CoD2's water, `water_l_sun` (`lib/water.hlsl`, docs/research/
// cod2-materials.md 9.12): the frame's height map (vcod_cod2::water) read
// as a three-octave turbulence after one parallax step, its slope the
// normal; the cube reflected off it over `waterColor` times the normal's
// z by the cube's alpha, plus the sun's `pow(R.L, 64)` highlight. No
// lightmap. Drawn by renderer/cod2_water.rs; tex_samplers.wgsl is
// appended. Colours leave scaled by identityLight, then fogged, as
// cod2_world.wgsl's.

struct Camera {
    view_proj: mat4x4<f32>,
    // .y identityLight
    time_pad: vec4<f32>,
    // xyz view origin; w fog mode: 0 off, 1 exp, 2 linear
    eye_fog_mode: vec4<f32>,
    // rgb fog colour, a exp density
    fog_color_density: vec4<f32>,
    // x near, y far (linear); w the exp fog's start (CoD4)
    fog_range: vec4<f32>,
    view_fwd: vec4<f32>,
    model: mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> camera: Camera;

// renderer::cod2_water::WaterUniform
struct Water {
    // `waterColor`
    color: vec4<f32>,
    // x: M_NOFOG
    flags: vec4<u32>,
};
@group(1) @binding(0) var<uniform> water: Water;
// `cubicEnvMap`
@group(1) @binding(1) var t_cube: texture_cube<f32>;

const M_NOFOG: u32 = 8u;

// renderer::cod2::SunUniform: lightPosition0, lightColor0
struct Sun {
    dir: vec4<f32>,
    color: vec4<f32>,
    specular: vec4<f32>,
};
@group(2) @binding(0) var<uniform> sun: Sun;
// The height map, `normalMapSampler` (sampler byte 6: linear, point mip, wrap).
@group(2) @binding(1) var t_height: texture_2d<f32>;
@group(2) @binding(2) var s_height: sampler;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(4) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
    // world position less the eye
    @location(2) eye_vec: vec3<f32>,
    // the fog's keep, per vertex as `water_l_sun_efog.vsn` writes it
    // (cod2-client-perf.md 9)
    @location(3) fog: f32,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let world = camera.model * vec4<f32>(in.pos, 1.0);
    out.clip = camera.view_proj * world;
    out.color = in.color;
    out.uv = in.uv;
    out.eye_vec = world.xyz - camera.eye_fog_mode.xyz;
    out.fog = fog_keep(world.xyz);
    return out;
}

fn fog_keep(world_pos: vec3<f32>) -> f32 {
    let mode = camera.eye_fog_mode.w;
    let d = length(world_pos - camera.eye_fog_mode.xyz);
    if (mode == 1.0) {
        return exp(-camera.fog_color_density.a * max(d - camera.fog_range.w, 0.0));
    }
    if (mode == 2.0) {
        let span = max(camera.fog_range.y - camera.fog_range.x, 0.0001);
        return clamp((camera.fog_range.y - d) / span, 0.0, 1.0);
    }
    return 1.0;
}

// water.hlsl's `turbulance`: lacunarity 3.7, scale rate 0.6.
fn turbulence(uv: vec2<f32>) -> f32 {
    var t = 0.0;
    var scale = 1.0;
    var at = uv;
    for (var i = 0; i < 3; i++) {
        t += textureSample(t_height, s_height, at).r * scale;
        at *= 3.7;
        scale *= 0.6;
    }
    return t;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let eye_dir = normalize(in.eye_vec);
    var h = 0.5;
    let err = h - textureSample(t_height, s_height, in.uv * 0.5).r;
    h -= err;
    let uv = in.uv + err * eye_dir.xy * (1.5 / 64.0);
    let h00 = turbulence(uv);
    let h10 = turbulence(uv + vec2<f32>(0.5 / 128.0, 0.0));
    let h01 = turbulence(uv + vec2<f32>(0.0, 0.5 / 128.0));
    let n = normalize(vec3<f32>(h10 - h00, h01 - h00, 1.0));
    var r = reflect(eye_dir, n);
    let spec_dot = max(0.0, dot(r, sun.dir.xyz));
    r.z = abs(r.z);
    let env = textureSample(t_cube, s_tex_clamp, r);
    let diffuse = water.color.rgb * n.z;
    let lit = mix(diffuse, env.rgb, env.a) + pow(spec_dot, 64.0) * sun.color.rgb;
    var rgb = lit * camera.time_pad.y;
    if ((water.flags.x & M_NOFOG) == 0u) {
        rgb = mix(camera.fog_color_density.rgb, rgb, in.fog);
    }
    return vec4<f32>(rgb, in.color.a);
}
