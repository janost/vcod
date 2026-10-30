// cod2_model.wgsl's sun shadow hook where its pass binds no sun shadow map
// (the view model's): the baked visibility as is.
fn cod4_sun_visibility(world_pos: vec3<f32>, baked: f32) -> f32 {
    return baked;
}

// and its spot light's (cod4_sunshadow_recv.wgsl's `cod4_spot_shadow`)
fn cod4_spot_visibility(world_pos: vec3<f32>, light_pos: vec3<f32>, baked: f32) -> f32 {
    return baked;
}
