// First-person viewmodel: the world's fov with its own near plane, bind pose
// baked into the vertices, placed in view space by the motion transform.
// Lit per vertex by its light set, in view space, as dynamic_model.wgsl
// lights entities.

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

struct VmUniform {
    proj: mat4x4<f32>,      // viewmodel projection (world fov, own near plane)
    model: mat4x4<f32>,     // motion transform (view space)
    lights: LightSet,       // view space
};
@group(0) @binding(0) var<uniform> u: VmUniform;
@group(1) @binding(0) var t_diffuse: texture_2d<f32>;
@group(1) @binding(1) var s_diffuse: sampler;
// View-space skin matrices; identity reproduces the baked bind pose.
@group(2) @binding(0) var<uniform> bones: array<mat4x4<f32>, 64>;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // GL's vertex colour, framebuffer units
    @location(1) light: vec3<f32>,
};

// Same as `gl_lighting` in dynamic_model.wgsl.
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
    let view = u.model * p;
    var out: VsOut;
    out.pos = u.proj * view;
    out.uv = uv;
    let vn = normalize((u.model * vec4<f32>(n, 0.0)).xyz);
    out.light = gl_lighting(u.lights, view.xyz, vn);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let tex = textureSample(t_diffuse, s_diffuse, in.uv);
    // same alpha-test threshold as the map's masked materials
    if (tex.a < 0.5) { discard; }
    // the display doubles the framebuffer
    return vec4<f32>(tex.rgb * in.light * 2.0, 1.0);
}
