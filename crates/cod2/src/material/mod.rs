//! CoD2's material chain, raw format layer (docs/research/cod2-materials.md):
//! the binary material file ([`MaterialFile`]), its technique set
//! ([`TechniqueSet`], `.techset`), the techniques ([`Technique`], `.tech`),
//! their statemaps ([`StateMap`], `.sm`), which technique type a pass runs
//! ([`TechniqueType::lit`], [`TechniqueType::with_fog`]) and the packed image
//! name rules ([`image_name`]). The dx7 fallback is not read.

mod file;
pub mod image_name;
mod lex;
mod statemap;
mod tech;
mod techset;

pub use file::{
    ConstantDef, DEFAULT_SAMPLER_STATES, Filter, HEADER_LEN, MaterialFile, SamplerState, StateBits,
    TextureDef, TextureImage, WaterDef, semantic,
};
pub use statemap::{GROUP_NAMES, StateMap, StateRule};
pub use tech::{
    ArgValue, DEFAULT_CODE_SAMPLERS, Pass, ShaderArg, ShaderRef, StreamDest, StreamSource,
    Technique, VertexRoute,
};
pub use techset::TechniqueSet;

/// `MaterialTechniqueType`, in `Material_TechniqueTypeForName`'s table
/// order; the label is the `.techset` spelling (4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum TechniqueType {
    DepthPrepass,
    Sky,
    BuildFloatZ,
    UnlitNoFog,
    UnlitLinearFog,
    UnlitExpFog,
    LightmapNoFog,
    LightmapLinearFog,
    LightmapExpFog,
    LightprobeNoFog,
    LightprobeLinearFog,
    LightprobeExpFog,
    LightprobeSmodelNoFog,
    LightprobeSmodelLinearFog,
    LightprobeSmodelExpFog,
    LightprobeSmodelCachedNoFog,
    LightprobeSmodelCachedLinearFog,
    LightprobeSmodelCachedExpFog,
    PointlightNoFog,
    PointlightLinearFog,
    PointlightExpFog,
    EmissiveNoFog,
    EmissiveLinearFog,
    EmissiveExpFog,
    EmissiveFeatheredNoFog,
    EmissiveFeatheredLinearFog,
    EmissiveFeatheredExpFog,
    FakelightNormal,
    FakelightView,
    SolidWireframe,
    ShadedWireframe,
    ShadowcookieCaster,
    ShadowcookieReceiver,
    DebugBumpmap,
}

/// How many technique types there are: a set's slots.
pub const TECHNIQUE_TYPE_COUNT: usize = 34;

/// The `.techset` labels, indexed by [`TechniqueType`].
pub const TECHNIQUE_TYPE_LABELS: [&str; TECHNIQUE_TYPE_COUNT] = [
    "depth prepass",
    "sky",
    "build floatz",
    "unlit no fog",
    "unlit linear fog",
    "unlit exp fog",
    "lightmap no fog",
    "lightmap linear fog",
    "lightmap exp fog",
    "lightprobe no fog",
    "lightprobe linear fog",
    "lightprobe exp fog",
    "lightprobe smodel no fog",
    "lightprobe smodel linear fog",
    "lightprobe smodel exp fog",
    "lightprobe smodel cached no fog",
    "lightprobe smodel cached linear fog",
    "lightprobe smodel cached exp fog",
    "pointlight no fog",
    "pointlight linear fog",
    "pointlight exp fog",
    "emissive no fog",
    "emissive linear fog",
    "emissive exp fog",
    "emissive feathered no fog",
    "emissive feathered linear fog",
    "emissive feathered exp fog",
    "fakelight normal",
    "fakelight view",
    "solid wireframe",
    "shaded wireframe",
    "shadowcookie caster",
    "shadowcookie receiver",
    "debug bumpmap",
];

/// `RB_FogOffset`: 0 with `r_fog` off, else the scene's fog mode (4.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fog {
    Off = 0,
    Linear = 1,
    Exp = 2,
}

/// What lights a surface in the lit pass, as `RB_RenderDrawSurfList` reads
/// it off the draw surface (4.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lighting {
    /// A world soup with a lightmap (index other than 0x1f).
    Lightmap,
    /// An entity, or a world soup without a lightmap.
    Lightprobe,
    /// An entity whose type is 2.
    Smodel,
    /// The cached static model list (entity slot 0x7ff).
    SmodelCached,
}

impl TechniqueType {
    pub const ALL: [TechniqueType; TECHNIQUE_TYPE_COUNT] = [
        Self::DepthPrepass,
        Self::Sky,
        Self::BuildFloatZ,
        Self::UnlitNoFog,
        Self::UnlitLinearFog,
        Self::UnlitExpFog,
        Self::LightmapNoFog,
        Self::LightmapLinearFog,
        Self::LightmapExpFog,
        Self::LightprobeNoFog,
        Self::LightprobeLinearFog,
        Self::LightprobeExpFog,
        Self::LightprobeSmodelNoFog,
        Self::LightprobeSmodelLinearFog,
        Self::LightprobeSmodelExpFog,
        Self::LightprobeSmodelCachedNoFog,
        Self::LightprobeSmodelCachedLinearFog,
        Self::LightprobeSmodelCachedExpFog,
        Self::PointlightNoFog,
        Self::PointlightLinearFog,
        Self::PointlightExpFog,
        Self::EmissiveNoFog,
        Self::EmissiveLinearFog,
        Self::EmissiveExpFog,
        Self::EmissiveFeatheredNoFog,
        Self::EmissiveFeatheredLinearFog,
        Self::EmissiveFeatheredExpFog,
        Self::FakelightNormal,
        Self::FakelightView,
        Self::SolidWireframe,
        Self::ShadedWireframe,
        Self::ShadowcookieCaster,
        Self::ShadowcookieReceiver,
        Self::DebugBumpmap,
    ];

    pub fn from_index(i: usize) -> Option<Self> {
        Self::ALL.get(i).copied()
    }

    pub fn from_label(label: &str) -> Option<Self> {
        let i = TECHNIQUE_TYPE_LABELS.iter().position(|&l| l == label)?;
        Self::from_index(i)
    }

    pub fn label(self) -> &'static str {
        TECHNIQUE_TYPE_LABELS[self as usize]
    }

    /// The type a fogged pass runs: types 3 to 26 (unlit through emissive
    /// feathered) take `RB_FogOffset()`, the rest are unchanged. `self` is
    /// the pass's no-fog base.
    pub fn with_fog(self, fog: Fog) -> Self {
        let i = self as usize;
        if (3..=26).contains(&i) {
            Self::from_index(i + fog as usize).unwrap_or(self)
        } else {
            self
        }
    }

    /// The lit pass's type for a draw surface (`R_RenderScene` queues 6,
    /// `RB_RenderDrawSurfList` picks the family, then adds the fog offset).
    pub fn lit(lighting: Lighting, fog: Fog) -> Self {
        let base = match lighting {
            Lighting::Lightmap => Self::LightmapNoFog,
            Lighting::Lightprobe => Self::LightprobeNoFog,
            Lighting::Smodel => Self::LightprobeSmodelNoFog,
            Lighting::SmodelCached => Self::LightprobeSmodelCachedNoFog,
        };
        base.with_fog(fog)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_round_trip_in_table_order() {
        for (i, t) in TechniqueType::ALL.iter().enumerate() {
            assert_eq!(*t as usize, i);
            assert_eq!(TechniqueType::from_label(t.label()), Some(*t));
        }
        assert_eq!(
            TechniqueType::from_label("lightprobe smodel cached exp fog"),
            Some(TechniqueType::LightprobeSmodelCachedExpFog)
        );
        assert_eq!(TechniqueType::from_label("Sky"), None);
    }

    #[test]
    fn pass_type_selection() {
        use TechniqueType as T;
        assert_eq!(T::lit(Lighting::Lightmap, Fog::Exp), T::LightmapExpFog);
        assert_eq!(T::lit(Lighting::Lightprobe, Fog::Off), T::LightprobeNoFog);
        assert_eq!(
            T::lit(Lighting::Smodel, Fog::Linear),
            T::LightprobeSmodelLinearFog
        );
        assert_eq!(
            T::lit(Lighting::SmodelCached, Fog::Exp),
            T::LightprobeSmodelCachedExpFog
        );
        assert_eq!(T::UnlitNoFog.with_fog(Fog::Linear), T::UnlitLinearFog);
        assert_eq!(T::PointlightNoFog.with_fog(Fog::Exp), T::PointlightExpFog);
        assert_eq!(
            T::EmissiveFeatheredNoFog.with_fog(Fog::Exp),
            T::EmissiveFeatheredExpFog
        );
        assert_eq!(T::Sky.with_fog(Fog::Exp), T::Sky);
        assert_eq!(T::DepthPrepass.with_fog(Fog::Exp), T::DepthPrepass);
        assert_eq!(T::DebugBumpmap.with_fog(Fog::Exp), T::DebugBumpmap);
    }
}
