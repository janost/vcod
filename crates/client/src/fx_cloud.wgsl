
// CoD2's particle clouds (renderer::fx_cloud), appended to fx.wgsl:
// lib/particle_cloud.hlsl with the outdoor variant's test. Each flake of
// the shared buffer sits at its point of the cloud's cube under the
// cloud's world matrix, and its corner moves off it in the view's right
// and up plane by `(uv - 0.5)` through particleCloudMatrix
// (docs/research/cod2-materials.md 9.14).

struct CloudParams {
    // the outdoor map's box: worldOutdoorLookupMatrix is
    // (p - mins) * inv_extent
    mins: vec4<f32>,
    inv_extent: vec4<f32>,
    // the view's right and up, view space x and y
    right: vec4<f32>,
    up: vec4<f32>,
}
@group(3) @binding(0) var outdoor_map: texture_2d<f32>;
@group(3) @binding(1) var outdoor_sampler: sampler;
@group(3) @binding(2) var<uniform> cloud: CloudParams;

struct CloudIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    // world matrix rows, the origin in w
    @location(2) m0: vec4<f32>,
    @location(3) m1: vec4<f32>,
    @location(4) m2: vec4<f32>,
    @location(5) matrix: vec4<f32>,
    @location(6) rgba: vec4<f32>,
    @location(7) flags: u32,
    @location(8) outdoor: u32,
}
struct CloudOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) rgba: vec4<f32>,
    @location(2) world_pos: vec3<f32>,
    @location(3) @interpolate(flat) flags: u32,
    @location(4) outdoor_pos: vec3<f32>,
    @location(5) @interpolate(flat) outdoor: u32,
}

@vertex fn vs_cloud(in: CloudIn) -> CloudOut {
    var out: CloudOut;
    let flake = vec3(in.m0.w, in.m1.w, in.m2.w)
        + in.m0.xyz * in.pos.x + in.m1.xyz * in.pos.y + in.m2.xyz * in.pos.z;
    let st = in.uv - vec2(0.5);
    let off = st.x * in.matrix.xy + st.y * in.matrix.zw;
    let p = flake + cloud.right.xyz * off.x + cloud.up.xyz * off.y;
    out.clip = camera.view_proj * vec4(p, 1.0);
    out.uv = in.uv;
    out.rgba = in.rgba;
    out.world_pos = p;
    out.flags = in.flags;
    // the flake's point, not the corner's
    out.outdoor_pos = (flake - cloud.mins.xyz) * cloud.inv_extent.xyz;
    out.outdoor = in.outdoor;
    return out;
}

fn cloud_as_fx(in: CloudOut) -> VsOut {
    var v: VsOut;
    v.clip = in.clip;
    v.uv = in.uv;
    v.rgba = in.rgba;
    v.world_pos = in.world_pos;
    v.size = 0.0;
    v.flags = in.flags;
    v.feather = 0.0;
    return v;
}

// particle_cloud_outdoor's ps_main: alpha 0 under the outdoor map's height.
fn cloud_keep(in: CloudOut) -> f32 {
    if (in.outdoor == 0u) {
        return 1.0;
    }
    let h = textureSampleLevel(outdoor_map, outdoor_sampler, in.outdoor_pos.xy, 0.0).r;
    return select(1.0, 0.0, in.outdoor_pos.z < h);
}

@fragment fn fs_cloud(in: CloudOut) -> @location(0) vec4<f32> {
    let c = shade(cloud_as_fx(in));
    return vec4(c.rgb, c.a * cloud_keep(in));
}

// The zeroed alpha reaches the target only where the blend reads source
// alpha: `GL_SRC_ALPHA GL_ONE` does, `GL_ONE GL_ONE` and the screen not.
@fragment fn fs_cloud_additive(in: CloudOut) -> @location(0) vec4<f32> {
    let c = shade_additive(cloud_as_fx(in));
    if ((in.flags & FX_VERT_TEX_ALPHA) != 0u) {
        return c * cloud_keep(in);
    }
    return c;
}

@fragment fn fs_cloud_screen(in: CloudOut) -> @location(0) vec4<f32> {
    return shade_screen(cloud_as_fx(in));
}
