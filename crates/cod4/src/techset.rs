//! Technique sets classified by name, the way the stock names spell what a
//! set draws (`wc_l_sm_r0c0n0s0`: world, lit, sun shadow, opaque, colour,
//! normal and specular maps). The grammar and its census are in
//! `docs/research/cod4-world.md` section 4.

/// What a set draws, as far as its name says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Family {
    /// `l`: lightmapped (world) or light-grid lit (model).
    Lit,
    /// `unlit`: the colour map alone.
    Unlit,
    Sky,
    Water,
    /// `distortion_*`: refracts what is behind it.
    Distortion,
    /// `tools`, `shadowcaster`: not drawn in the lit view.
    Invisible,
    /// `mc_reflexsight`: a reflex sight's dot, projected off the view's
    /// angle to the lens (`reflexsight.hlsl`, cod4-client-view.md 4.5).
    ReflexSight,
    /// Anything else (`default`, `ambient_*`, `threed_cinematic`, the 2D
    /// and effect sets): no family yet.
    Other,
}

/// How the set blends, from a lit set's leading code (`r0`, `t0`, `b0`,
/// `a0`) or an unlit set's suffix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Blend {
    Replace,
    AlphaTest,
    Blend,
    Add,
    Multiply,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TechClass {
    pub family: Family,
    pub blend: Blend,
    /// `wc_`: a world set; `mc_` a model one.
    pub world: bool,
    /// `sm`: the sun's shadow map is applied.
    pub sun_shadow: bool,
    /// `n0`, `s0`, `d0`: a normal, specular or detail map is sampled.
    pub normal_map: bool,
    pub spec_map: bool,
    pub detail_map: bool,
    /// `unlit_distfalloff`: the vertex colour scaled by the material's
    /// falloff colours over the eye's distance (`vertcol_simple_fog_df`).
    pub dist_falloff: bool,
    /// `falloff_add`: the vertex colour scaled by the falloff colours over
    /// the angle between the view and the normal (`*_foa*` vertex shaders).
    pub angle_falloff: bool,
    /// `eyeoffset`: the vertices pulled toward the eye (`*_eo*`, `*eyeofs*`).
    pub eye_offset: bool,
    /// `zfeather`: faded where it meets the depth behind it.
    pub zfeather: bool,
    /// `nofog`: the pixel shader adds no fog.
    pub nofog: bool,
    /// `flag`: a lit model set whose vertex shader waves the vertices
    /// (`lp_flag_*`, cod4-world.md 3.4).
    pub flag: bool,
    /// The pixel shader writes its colour times its alpha: the `add`
    /// sets' (`vertcol_simple_add*`, `zfeather_add*`) and the lit `b0`
    /// ones' (cod4-world.md 8.1).
    pub premultiplied: bool,
}

/// Classifies the set `name`; a leading `,` (a name stub resolved from
/// another zone) is ignored.
pub fn classify(name: &str) -> TechClass {
    let name = name.trim_start_matches(',');
    let mut c = TechClass {
        family: Family::Other,
        blend: Blend::Replace,
        world: false,
        sun_shadow: false,
        normal_map: false,
        spec_map: false,
        detail_map: false,
        dist_falloff: false,
        angle_falloff: false,
        eye_offset: false,
        zfeather: false,
        nofog: false,
        flag: false,
        premultiplied: false,
    };
    let rest = if let Some(r) = name.strip_prefix("wc_") {
        c.world = true;
        r
    } else if let Some(r) = name.strip_prefix("mc_") {
        r
    } else if name == "effect" || name.starts_with("effect_") {
        // The effects' own sets carry no prefix and spell `mc_effect`'s
        // grammar (`effect_zfeather_add_nofog`).
        name
    } else {
        // The effect sets carry no prefix (`distortion_scale_zfeather`).
        if name.starts_with("distortion_") {
            c.family = Family::Distortion;
            c.blend = Blend::Blend;
        }
        // `particle_cloud*_add.hlsl` writes its colour times its alpha
        // (cod4-effects.md 13).
        if name.starts_with("particle_cloud") && name.ends_with("_add") {
            c.premultiplied = true;
        }
        return c;
    };
    let mut tokens = rest.split('_');
    match tokens.next() {
        Some("l") => {
            c.family = Family::Lit;
            for t in tokens {
                // `hsm`: another sun shadow map variant, read as `sm`
                if t == "sm" || t == "hsm" {
                    c.sun_shadow = true;
                    continue;
                }
                // `mc_l_sm_skin_*`: the view hands' skin, lit as the rest.
                if t == "skin" {
                    continue;
                }
                if t == "flag" {
                    c.flag = true;
                    continue;
                }
                // Letter-digit codes glued together: r0c0n0s0.
                let b = t.as_bytes();
                if b.len() % 2 != 0 || !b.chunks(2).all(|p| p[1].is_ascii_digit()) {
                    c.family = Family::Other;
                    return c;
                }
                for p in b.chunks(2) {
                    match p[0] {
                        b'r' => c.blend = Blend::Replace,
                        b't' => c.blend = Blend::AlphaTest,
                        b'b' => {
                            c.blend = Blend::Blend;
                            c.premultiplied = true;
                        }
                        b'a' => c.blend = Blend::Add,
                        b'n' => c.normal_map = true,
                        b's' => c.spec_map = true,
                        b'd' => c.detail_map = true,
                        b'c' => {}
                        _ => {
                            c.family = Family::Other;
                            return c;
                        }
                    }
                }
            }
        }
        Some("unlit") => {
            c.family = Family::Unlit;
            let rest: Vec<&str> = tokens.collect();
            c.dist_falloff = rest.last() == Some(&"distfalloff");
            c.angle_falloff = rest.contains(&"falloff");
            c.nofog = rest.last() == Some(&"nofog");
            c.premultiplied = rest.last() == Some(&"add");
            c.blend = match rest.last().copied() {
                None | Some("replace" | "nofog" | "distfalloff") => Blend::Replace,
                Some("alphatest") => Blend::AlphaTest,
                Some("blend") => Blend::Blend,
                Some("add") => Blend::Add,
                Some("multiply") => Blend::Multiply,
                Some(_) => {
                    c.family = Family::Other;
                    Blend::Replace
                }
            };
        }
        // `mc_effect_[zfeather_][falloff_][add][_nofog][_eyeoffset]`: an
        // unlit set with its vertex shader's extras; only the add sets'
        // pixel shaders premultiply (`vertcol_simple_add*`,
        // `zfeather_add*`), the rest (`vertcol_simple_fog`, `zfeather*`)
        // write their colour as is (cod4-effects.md 13).
        Some("effect") => {
            let rest: Vec<&str> = tokens.collect();
            let add = rest.contains(&"add");
            c.family = Family::Unlit;
            c.blend = if add { Blend::Add } else { Blend::Blend };
            c.premultiplied = add;
            c.angle_falloff = rest.contains(&"falloff");
            c.zfeather = rest.contains(&"zfeather");
            c.nofog = rest.contains(&"nofog");
            c.eye_offset = rest.contains(&"eyeoffset");
        }
        Some("sky") => c.family = Family::Sky,
        Some("water") => c.family = Family::Water,
        Some("distortion") => {
            c.family = Family::Distortion;
            c.blend = Blend::Blend;
        }
        Some("tools" | "shadowcaster") => c.family = Family::Invisible,
        // `reflexsight.hlsl` writes its colour times its alpha
        // (cod4-client-view.md 4.5).
        Some("reflexsight") => {
            c.family = Family::ReflexSight;
            c.blend = Blend::Blend;
            c.premultiplied = true;
        }
        _ => {}
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_stock_codes() {
        let c = classify(",wc_l_sm_b0c0n0s0");
        assert_eq!(
            (c.family, c.blend, c.world),
            (Family::Lit, Blend::Blend, true)
        );
        assert!(c.sun_shadow && c.normal_map && c.spec_map && !c.detail_map);
        let c = classify("wc_l_r0c0");
        assert_eq!(
            (c.family, c.blend, c.sun_shadow),
            (Family::Lit, Blend::Replace, false)
        );
        assert_eq!(classify("wc_l_sm_t0c0").blend, Blend::AlphaTest);
        assert!(classify("wc_l_sm_r0c0d0n0s0").detail_map);
        assert_eq!(classify("wc_unlit_multiply").blend, Blend::Multiply);
        assert_eq!(classify("wc_unlit_falloff_add").blend, Blend::Add);
        assert_eq!(classify("wc_unlit").family, Family::Unlit);
        assert!(classify("wc_unlit_distfalloff").dist_falloff);
        assert!(!classify("wc_unlit_falloff_add").dist_falloff);
        let f = classify("wc_unlit_falloff_add");
        assert!(f.angle_falloff && f.premultiplied && !f.eye_offset);
        let e = classify("mc_effect_zfeather_falloff_add_nofog_eyeoffset");
        assert_eq!((e.family, e.blend), (Family::Unlit, Blend::Add));
        assert!(e.angle_falloff && e.zfeather && e.nofog && e.eye_offset && e.premultiplied);
        let a = classify("mc_effect_add_eyeoffset");
        assert!(a.eye_offset && !a.angle_falloff && a.premultiplied);
        assert!(classify("wc_l_sm_b0c0n0s0").premultiplied);
        assert!(!classify("wc_l_sm_a0c0").premultiplied);
        assert!(!classify("wc_unlit_blend").premultiplied);
        assert!(classify("mc_unlit_nofog").nofog);
        assert_eq!(classify("wc_sky").family, Family::Sky);
        assert_eq!(classify("wc_shadowcaster").family, Family::Invisible);
        assert_eq!(
            classify("wc_distortion_scale_zfeather").family,
            Family::Distortion
        );
        assert_eq!(
            classify("distortion_scale_zfeather").family,
            Family::Distortion
        );
        // The effects' unprefixed sets read as `mc_effect`'s.
        let z = classify("effect_zfeather_add_nofog");
        assert_eq!((z.family, z.blend), (Family::Unlit, Blend::Add));
        assert!(z.zfeather && z.nofog && z.premultiplied);
        let e = classify("effect");
        assert_eq!((e.family, e.blend), (Family::Unlit, Blend::Blend));
        assert!(!e.premultiplied && !e.nofog);
        assert!(classify("effect_zfeather_nofog").nofog);
        assert!(!classify("mc_effect").premultiplied);
        assert!(classify("particle_cloud_outdoor_add").premultiplied);
        assert!(!classify("particle_cloud").premultiplied);
        assert!(!classify("effectx").premultiplied);
        assert_eq!(classify("wc_default").family, Family::Other);
        assert_eq!(classify("mc_reflexsight").family, Family::ReflexSight);
        assert_eq!(classify("mc_l_sm_skin_r0c0n0s0").family, Family::Lit);
        let f = classify(",mc_l_hsm_flag_t0c0n0s0");
        assert_eq!((f.family, f.blend), (Family::Lit, Blend::AlphaTest));
        assert!(f.flag && f.normal_map && f.spec_map && f.sun_shadow);
        assert!(!classify("mc_l_sm_r0c0").flag);
        assert_eq!(classify("2d").family, Family::Other);
    }
}
