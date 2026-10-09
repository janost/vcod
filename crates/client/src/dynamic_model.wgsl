// Snapshot entities: GPU-skinned xmodel instances in world space, lit per
// vertex by their light set as retail's GL lighting does
// (cod11-light-grid-and-leaf-lights.md, section 13).

struct Camera {
    view_proj: mat4x4<f32>,
    time_pad: vec4<f32>, // .x seconds since start; .y identityLight
    // xyz view origin; w fog mode: 0 off, 1 GL_EXP, 2 GL_LINEAR (configstring 12)
    eye_fog_mode: vec4<f32>,
    // rgb fog colour, a density (GL_EXP)
    fog_color_density: vec4<f32>,
    // x near, y far (GL_LINEAR)
    fog_range: vec4<f32>,
    // xyz unit view forward; fog depth rides along it
    view_fwd: vec4<f32>,
};
@group(0) @binding(0) var<uniform> camera: Camera;

@group(1) @binding(0) var t_diffuse: texture_2d<f32>;
@group(1) @binding(1) var s_diffuse: sampler;
// All instances' bone matrices, world space. Slot 0 is the shared identity
// block (bind pose).
@group(2) @binding(0) var<storage, read> bones: array<mat4x4<f32>>;

// One GL light; `entity_light::GpuLight`.
struct EntLight {
    pos: vec4<f32>,      // w 0: xyz is the unit vector towards the light
    diffuse: vec4<f32>,  // w spot exponent
    ambient: vec4<f32>,  // w cosine of the spot cutoff, below -1 for none
    atten: vec4<f32>,    // constant, linear, quadratic
    spot_dir: vec4<f32>,
};
struct LightSet {
    ambient: vec4<f32>,  // light model ambient; w the light count
    lights: array<EntLight, 8>,
};
// The frame's light sets; an instance names its own.
@group(2) @binding(1) var<storage, read> light_sets: array<LightSet>;

// GL_LIGHTING's vertex colour: the light model ambient plus each light's
// attenuated ambient and diffuse, clamped to 1. White material, no
// specular, one-sided.
fn gl_lighting(ls: LightSet, p: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    var c = ls.ambient.rgb;
    let count = min(u32(ls.ambient.w), 8u);
    for (var i = 0u; i < count; i++) {
        let l = ls.lights[i];
        var to_l = l.pos.xyz;
        var att = 1.0;
        if (l.pos.w != 0.0) {
            let v = l.pos.xyz - p;
            let d = length(v);
            to_l = v / max(d, 1e-6);
            att = 1.0 / (l.atten.x + (l.atten.y + l.atten.z * d) * d);
            if (l.ambient.w >= -1.0) {
                let s = dot(-to_l, normalize(l.spot_dir.xyz));
                if (s < l.ambient.w) {
                    att = 0.0;
                } else if (l.diffuse.w > 0.0) {
                    att *= pow(max(s, 0.0), l.diffuse.w);
                }
            }
        }
        c += att * (l.ambient.rgb + max(dot(n, to_l), 0.0) * l.diffuse.rgb);
    }
    return clamp(c, vec3(0.0), vec3(1.0));
}

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) world_pos: vec3<f32>,
    // GL's vertex colour, framebuffer units
    @location(3) light: vec3<f32>,
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
    @location(10) light_set: u32,
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
    out.light = gl_lighting(light_sets[light_set], world.xyz, normalize(out.normal));
    return out;
}

// glFog GL_EXP / GL_LINEAR factors (see shader.wgsl); depth along the view
// forward, the fixed-function fog coordinate without GL_NV_fog_distance.
fn fog_amount(world_pos: vec3<f32>) -> f32 {
    let mode = camera.eye_fog_mode.w;
    if (mode == 0.0) {
        return 0.0;
    }
    let d = max(dot(world_pos - camera.eye_fog_mode.xyz, camera.view_fwd.xyz), 0.0);
    if (mode == 1.0) {
        return 1.0 - exp(-camera.fog_color_density.a * d);
    }
    let span = max(camera.fog_range.y - camera.fog_range.x, 0.0001);
    return clamp((d - camera.fog_range.x) / span, 0.0, 1.0);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let tex = textureSample(t_diffuse, s_diffuse, in.uv);
    // same alpha-test threshold as the map's masked materials
    if (tex.a < 0.5) { discard; }
    let rgb = tex.rgb * in.light;
    return vec4<f32>(mix(rgb, camera.fog_color_density.rgb, fog_amount(in.world_pos)), 1.0);
}
