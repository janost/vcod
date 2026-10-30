// CoD4's sun shadow map casters (crate::cod4_sunshadow): technique 3,
// `shadowmap_build.hlsl` and `shadowmap_build_sm3.hlsl`
// (docs/research/cod4-world.md 3.6). The pixel writes its depth plus
// `shadowmapPolygonOffset`'s constant and its slope part times the depth's
// summed screen derivatives; the atest variant first drops a texel under 128.
// World surfaces and static models through `vs_cast`, entities skinned
// through `vs_cast_entity` (alpha-tested ones into `fs_cast_entity_cut`).

struct Partition {
    view_proj: mat4x4<f32>,
    // shadowmapPolygonOffset in xy; z 0 stores clip z over w (the sun's),
    // else clip z times it (a spot's: crate::cod4_spotshadow); w half a
    // texel in clip units at w 1
    offset: vec4<f32>,
};
@group(0) @binding(0) var<uniform> part: Partition;
@group(0) @binding(1) var s_cut: sampler;
// an alpha-tested caster's colour map (binding 0 is the entities' bones)
@group(1) @binding(1) var t_cut: texture_2d<f32>;

struct CastOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) z: f32,
};

fn cast_at(clip_in: vec4<f32>, uv: vec2<f32>) -> CastOut {
    var out: CastOut;
    var clip = clip_in;
    out.z = select(clip.z / clip.w, clip.z * part.offset.z, part.offset.z > 0.0);
    // D3D9 puts texel i's centre at clip 2i / N - 1, half a texel left of
    // (and above) where wgpu's rasterizer does: shift, so the lookup meets
    // the texel centres retail drew.
    clip.x += clip.w * part.offset.w;
    clip.y -= clip.w * part.offset.w;
    out.clip = clip;
    out.uv = uv;
    return out;
}

@vertex
fn vs_cast(@location(0) pos: vec3<f32>, @location(1) uv: vec2<f32>) -> CastOut {
    return cast_at(part.view_proj * vec4<f32>(pos, 1.0), uv);
}

// An entity surface, skinned as dynamic_model.wgsl does.
@group(1) @binding(0) var<storage, read> bones: array<mat4x4<f32>>;

@vertex
fn vs_cast_entity(
    @location(0) pos: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) bi: vec4<u32>,
    @location(4) bw: vec4<f32>,
    @location(5) m0: vec4<f32>,
    @location(6) m1: vec4<f32>,
    @location(7) m2: vec4<f32>,
    @location(8) m3: vec4<f32>,
    @location(9) bone_base: u32,
) -> CastOut {
    var p = vec4<f32>(0.0);
    for (var i = 0u; i < 4u; i++) {
        p += bw[i] * (bones[bone_base + bi[i]] * vec4<f32>(pos, 1.0));
    }
    return cast_at(part.view_proj * (mat4x4<f32>(m0, m1, m2, m3) * p), uv);
}

fn stored(z: f32) -> f32 {
    return z + part.offset.x + part.offset.y * (abs(dpdx(z)) + abs(dpdy(z)));
}

@fragment
fn fs_cast(in: CastOut) -> @builtin(frag_depth) f32 {
    return stored(in.z);
}

@fragment
fn fs_cast_cut(in: CastOut) -> @builtin(frag_depth) f32 {
    let a = textureSample(t_cut, s_cut, in.uv).a;
    let z = stored(in.z);
    if (a < 128.0 / 255.0) {
        discard;
    }
    return z;
}

// An alpha-tested entity caster's colour map, in the entity skin layout.
@group(2) @binding(0) var t_ent_cut: texture_2d<f32>;

@fragment
fn fs_cast_entity_cut(in: CastOut) -> @builtin(frag_depth) f32 {
    let a = textureSample(t_ent_cut, s_cut, in.uv).a;
    let z = stored(in.z);
    if (a < 128.0 / 255.0) {
        discard;
    }
    return z;
}
