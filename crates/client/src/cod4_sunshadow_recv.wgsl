// CoD4's sun shadow map as the technique 9 pixel shaders read it
// (`lm_sm_sun_*_sm3.hlsl`, `lp_sm_sun_*_sm3.hlsl`; crate::cod4_sunshadow,
// docs/research/cod4-world.md 3.6), in the camera group. Appended to the
// lit world and model shaders.

struct Cod4SunShadow {
    // shadowLookupMatrix: the world to the near partition's atlas x, y
    // and depth, w the distance along the view in near partition reaches
    lookup: mat4x4<f32>,
    // shadowmapSwitchPartition: far coords = near * w + xy
    switch_partition: vec4<f32>,
    // shadowmapScale
    scale: vec4<f32>,
    // x 1 while the map is drawn this frame
    on: vec4<f32>,
    // the spot lights' maps (crate::cod4_spotshadow): spotShadowmapPixelAdjust
    spot_adjust: vec4<f32>,
    // a shadowed light a tile
    spots: array<Cod4SpotTile, 4>,
};

struct Cod4SpotTile {
    // shadowLookupMatrix: the world to the map's x, y and clip z
    lookup: mat4x4<f32>,
    // the light's origin, w one over its far plane
    origin: vec4<f32>,
    // x 1 while drawn, y lightSpotFactors.w (the fade), z the tile
    on: vec4<f32>,
};
@group(0) @binding(18) var t_cod4_sun: texture_depth_2d;
@group(0) @binding(19) var<uniform> cod4_sun: Cod4SunShadow;
// shadowmapSamplerSun's point filter and clamp, the compare in the sampler
@group(0) @binding(20) var s_cod4_sun: sampler_comparison;

// The four point taps around `at`, each lit when its stored depth is at or
// past `z`, averaged: retail's four `texldl`, `cmp` and `dp4 0.25`, the
// compare in the sampler. The partitions are the left 1024 of the 1536
// wide atlas, so x scales by 2/3; the taps are retail's in atlas uv.
fn cod4_sun_pcf(at: vec2<f32>, z: f32) -> f32 {
    let uv = at * vec2<f32>(2.0 / 3.0, 1.0);
    let d = vec4<f32>(
        textureSampleCompareLevel(t_cod4_sun, s_cod4_sun, uv + vec2<f32>(-1.0 / 3072.0, 1.0 / 8192.0), z),
        textureSampleCompareLevel(t_cod4_sun, s_cod4_sun, uv + vec2<f32>(1.0 / 6144.0, 1.0 / 4096.0), z),
        textureSampleCompareLevel(t_cod4_sun, s_cod4_sun, uv + vec2<f32>(-1.0 / 6144.0, -1.0 / 4096.0), z),
        textureSampleCompareLevel(t_cod4_sun, s_cod4_sun, uv + vec2<f32>(1.0 / 3072.0, -1.0 / 8192.0), z),
    );
    return dot(d, vec4<f32>(0.25));
}

// The sun's visibility at `world_pos` for a surface whose own is `baked`
// (a lightmap's primary, a model block's alpha): the near partition's test
// near the view, blended into the far one's over the last quarter of its
// reach, then the far one's faded into `baked` toward its edge. `baked`
// as is while no map is drawn.
fn cod4_sun_shadow(world_pos: vec3<f32>, baked: f32) -> f32 {
    if (cod4_sun.on.x < 0.5) {
        return baked;
    }
    let s = cod4_sun.lookup * vec4<f32>(world_pos, 1.0);
    let far_at = s.xy * cod4_sun.switch_partition.w + cod4_sun.switch_partition.xy;
    if (s.w <= 1.0) {
        let near = cod4_sun_pcf(s.xy, s.z);
        if (s.w <= 0.75) {
            return near;
        }
        let t = s.w * 4.0 - 3.0;
        return mix(near, cod4_sun_pcf(far_at, s.z), t);
    }
    let e = abs(far_at - vec2<f32>(0.5, 0.75)) * cod4_sun.scale.xy;
    let edge = max(e.x, e.y);
    if (edge - 8.0 >= 0.0) {
        return baked;
    }
    return mix(cod4_sun_pcf(far_at, s.z), baked, clamp(edge - 7.0, 0.0, 1.0));
}

// cod2_model.wgsl's hook: an entity's sun visibility through the map
// (cod4_sunshadow_none.wgsl where no map is bound).
fn cod4_sun_visibility(world_pos: vec3<f32>, baked: f32) -> f32 {
    return cod4_sun_shadow(world_pos, baked);
}

// lm_sm_spot_*_sm3.hlsl and lp_sm_spot_*_sm3.hlsl (techniques 11, 13;
// cod4-world.md 3.5) on a surface whose spot light at `light_pos` is one
// a tile was drawn from: four point taps at the lookup moved by
// spotShadowmapPixelAdjust's xy and zw both ways, each lit where the map
// holds the clip z or more, averaged, then lerped from `baked` (the
// primary lightmap's red, the model block's alpha) by the fade. `baked`
// as is for any other light or with no map drawn.
fn cod4_spot_shadow(world_pos: vec3<f32>, light_pos: vec3<f32>, baked: f32) -> f32 {
    for (var k = 0; k < 4; k++) {
        let sp = cod4_sun.spots[k];
        if (sp.on.x < 0.5 || distance(light_pos, sp.origin.xyz) > 0.5) {
            continue;
        }
        let s = sp.lookup * vec4<f32>(world_pos, 1.0);
        if (s.w <= 0.0) {
            return baked;
        }
        let at = s.xy / s.w;
        let z = s.z * sp.origin.w;
        // the lookup and the taps in retail's 512 x 2048 spot map, the
        // atlas's column from x 1024; each tap kept to the light's tile
        let size = vec2<f32>(512.0, 2048.0);
        let top = i32(sp.on.z) * 512;
        let a = cod4_sun.spot_adjust;
        var taps = array<vec2<f32>, 4>(a.xy, -a.xy, a.zw, -a.zw);
        var lit = 0.0;
        for (var i = 0; i < 4; i++) {
            let t = clamp(vec2<i32>(floor((at + taps[i]) * size)), vec2<i32>(0, top), vec2<i32>(511, top + 511));
            lit += select(0.0, 1.0, textureLoad(t_cod4_sun, t + vec2<i32>(1024, 0), 0) >= z);
        }
        return mix(baked, lit * 0.25, sp.on.y);
    }
    return baked;
}

// cod2_model.wgsl's hook for an entity's spot light (cod4_sunshadow_none.wgsl
// where no map is bound).
fn cod4_spot_visibility(world_pos: vec3<f32>, light_pos: vec3<f32>, baked: f32) -> f32 {
    return cod4_spot_shadow(world_pos, light_pos, baked);
}
