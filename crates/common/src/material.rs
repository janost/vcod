//! The game-neutral material the renderer draws: sort key, surface bits,
//! cull, depth bias, per-stage blend, depth and alpha-test state, texture
//! bindings by semantic, the colour and texture-coordinate generators and
//! deforms, and a technique family that picks the draw path. CoD1 shader
//! scripts compile into it (`Material::from_shader` in `shader.rs`); CoD1's
//! stage-based multitexture is kept whole in `Material::stages`, so a
//! compiled script draws exactly as the script did.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum BlendFactor {
    Zero,
    One,
    DstColor,
    OneMinusDstColor,
    SrcColor,
    OneMinusSrcColor,
    SrcAlpha,
    OneMinusSrcAlpha,
    DstAlpha,
    OneMinusDstAlpha,
    SrcAlphaSaturate,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WaveForm {
    Sin,
    Square,
    Triangle,
    Sawtooth,
    InverseSawtooth,
    /// Retail's genfunc 6 (CoDMP.exe 0x4f1a1d): R_NoiseGet4f over time,
    /// read only by `rgbGen wave` (shader doc 4).
    Noise,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Wave {
    pub form: WaveForm,
    pub base: f32,
    pub amp: f32,
    pub phase: f32,
    pub freq: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TcMod {
    Scroll(f32, f32),
    Scale(f32, f32),
    Rotate(f32),
    Stretch(Wave),
    Turb { amp: f32, phase: f32, freq: f32 },
    Transform([f32; 6]),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ImageRef {
    Path(String),
    Lightmap,
    White,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AnimSpec {
    pub fps: f32,
    pub paths: Vec<String>,
}

/// A bundle image's wrap per axis: retail's image flags 0x10 (`s`) and 0x20
/// (`t`) pick `GL_CLAMP_TO_EDGE` over `GL_REPEAT` for `GL_TEXTURE_WRAP_S`
/// and `_T` (CoDMP.exe 0x4ec6e9, 0x4ec70f; shader doc 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Clamp {
    pub s: bool,
    pub t: bool,
}

impl Clamp {
    pub const NONE: Clamp = Clamp { s: false, t: false };
    pub const BOTH: Clamp = Clamp { s: true, t: true };

    pub fn any(self) -> bool {
        self.s || self.t
    }
}

/// One `map`/`animMap` group inside a stage; a stage has one or more.
#[derive(Debug, Clone, PartialEq)]
pub struct Bundle {
    pub image: ImageRef,
    pub anim: Option<AnimSpec>,
    pub clamp: Clamp,
    pub tcmods: Vec<TcMod>,
    pub tc_gen: TcGen,
    /// How the bundle combines with the one before it; bundle 0's is unused.
    pub env: TexEnv,
}

/// A `nextbundle`'s `blendFunc`, which picks the texture environment and
/// leaves the stage's blend alone (CoDMP.exe 0x4f8ba3, 0x4f8bce, 0x4f8bf3): `filter`
/// and `GL_ZERO GL_SRC_COLOR` modulate, `add` adds, `blend` decals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TexEnv {
    #[default]
    Modulate,
    Add,
    Decal,
}

/// A bundle's texture coordinates before its tcMods.
#[derive(Debug, Clone, PartialEq)]
pub enum TcGen {
    /// The vertex's own (`tcGen base`/`texture`, or none).
    Base,
    /// `tcGen vector` basis, sx sy sz tx ty tz; renderer dots world position.
    Vector([f32; 6]),
    /// `tcGen environment` (texgen 4, CoDMP.exe 0x4f9838): the view vector
    /// reflected about the normal in the model's frame, `s = 0.5 + 0.5 r.y`,
    /// `t = 0.5 - 0.5 r.z` (0x514d40; docs/research/cod11-shader-scripts.md, §3).
    Environment,
}

/// `depthFunc`: the stage's depth test (CoDMP.exe 0x4f8990, GL state bits
/// 0x20000 and 0x40000).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DepthFunc {
    #[default]
    LessEqual,
    Equal,
    Always,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AlphaFunc {
    Gt0,
    Lt128,
    Ge128,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RgbGen {
    IdentityLighting,
    Identity,
    ExactVertex,
    Vertex,
    Const([f32; 3]),
    ConstLighting([f32; 3]),
    Wave(Wave),
    /// `lightingDiffuse` (gen 10) and `lightingAmbient` (gen 9): GL lighting
    /// from the light grid, model skins only (cod11-light-grid-and-leaf-lights.md,
    /// section 13).
    LightingDiffuse,
    /// `lightingPrecalc` (gen 0xb): the entity's `shaderRGBA` on a model.
    LightingPrecalc,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AlphaGen {
    Identity,
    Vertex,
    Const(f32),
    Wave(Wave),
    /// The entity's `shaderRGBA` alpha.
    Entity,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stage {
    pub bundles: Vec<Bundle>,
    pub blend: Option<(BlendFactor, BlendFactor)>,
    /// `depthWrite` seen explicitly; absent means stage-default.
    pub depth_write: Option<bool>,
    pub alpha_func: Option<AlphaFunc>,
    pub depth_func: DepthFunc,
    pub rgb_gen: RgbGen,
    pub alpha_gen: AlphaGen,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SkyParms {
    pub env: String,
    pub cloud_height: f32,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SurfaceBits {
    pub sky: bool,
    pub nodraw: bool,
    pub trans: bool,
    pub water: bool,
    pub nonsolid: bool,
    pub nolightmap: bool,
    /// `surfaceParm nodlight`: surface flag 0x20000, which keeps the
    /// dlight pass off (0x500d3a).
    pub nodlight: bool,
}

/// One `deformVertexes` record (CoDMP.exe 0x4f9d20's 0x34-byte records).
#[derive(Debug, Clone, PartialEq)]
pub enum Deform {
    /// `wave <div> <func> <base> <amp> <phase> <rate>`, the ocean swell:
    /// along the normal by `base + amp * sin(2pi * (phase + t * rate +
    /// (x+y+z) * spread))` (0x513570); `spread` is 1/div.
    Wave { wave: Wave, spread: f32 },
    /// `flap <s|x|t|y> <div> <func> <base> <amp> <phase> <rate>`: the
    /// wave above scaled by the vertex's base `s` (or `t`), so a flag's
    /// hoist edge (s = 0) holds still (0x5136e0).
    Flap(Flap),
    /// `bulge <width> <height> <speed>` (0x513ad0).
    Bulge(Bulge),
    /// `move <x> <y> <z> <func> <base> <amp> <phase> <rate>`: every vertex
    /// moves along the vector by the wave (0x513b70).
    Move { by: [f32; 3], wave: Wave },
    /// `normal <amplitude> <frequency>` (0x513990).
    Normal(DeformNormal),
    /// A record vcod draws some other way or not at all: `autosprite`,
    /// `autosprite2` (see [`Material::sprite`]), `projectionShadow`,
    /// `text0`-`text7`, `syncNormal`. It still takes one of the three slots.
    Other,
}

impl Deform {
    /// Whether the record moves vertex positions.
    pub fn moves(&self) -> bool {
        matches!(
            self,
            Deform::Wave { .. } | Deform::Flap(_) | Deform::Bulge(_) | Deform::Move { .. }
        )
    }
}

/// A `deformVertexes flap`: `along_t` for the `t`/`y` axis (deform 3),
/// otherwise `s`/`x` (deform 2); `spread` is 1/div as for `wave`.
#[derive(Debug, Clone, PartialEq)]
pub struct Flap {
    pub along_t: bool,
    pub wave: Wave,
    pub spread: f32,
}

/// `deformVertexes bulge`: a sine along the base `s` coordinate, travelling
/// at `speed`, pushes each vertex out along its normal by up to `height`
/// (CoDMP.exe 0x513ad0; shader doc 8).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bulge {
    pub width: f32,
    pub height: f32,
    pub speed: f32,
}

/// `deformVertexes normal`: each normal is nudged by `amp` times the noise
/// at the vertex and renormalized (CoDMP.exe 0x513990; shader doc 8).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeformNormal {
    pub amp: f32,
    pub freq: f32,
}

/// The two sprite deforms (deform types 9 and 10 in the 1.1 parser).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sprite {
    /// Every four vertices become a quad facing the view plane.
    Auto,
    /// Every quad turns about its long axis to face the eye.
    Auto2,
}

/// Sort values (cod11-shader-scripts.md section 5).
pub const SORT_OCEAN: f32 = 3.0;
pub const SORT_BOATHULL: f32 = 4.0;
pub const SORT_OPAQUE: f32 = 5.0;
pub const SORT_DECAL: f32 = 6.0;
pub const SORT_SEETHROUGH: f32 = 7.0;
pub const SORT_BANNER: f32 = 8.0;
pub const SORT_UNDERWATER: f32 = 10.0;
pub const SORT_WATER: f32 = 11.0;
pub const SORT_BLEND: f32 = 15.0;
pub const SORT_ADDITIVE: f32 = 19.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawClass {
    Opaque,
    Blend,
    Additive,
}

/// The three polygon offset keywords: shader flags 0x10, 0x20 and 0x40,
/// drawn at `(r_offsetfactor, r_offsetunits)`, twice that pair, and `(0,
/// r_offsetunits)` (CoDMP.exe 0x4fbbdd, 0x4d5991; events doc 9.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PolygonOffset {
    Single,
    Double,
    Const,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassedStage {
    pub class: DrawClass,
    pub depth_write: bool,
    pub bias: bool,
    pub two_sided: bool,
}
/// The engine's default image (`*default`, `R_CreateDefaultImage` 0x4f0380);
/// renderers bind `assets::default_image` for it.
pub const DEFAULT_IMAGE: &str = "*default";

/// The engine's dlight image (`$dlight`, 0x4f0150); a bundle that names it
/// takes the lightmap's texture coordinates, which the dlight pass fills
/// with the projected light (0x4f8556, 0x500a8c).
pub const DLIGHT_IMAGE: &str = "$dlight";

/// The dlight pass's draw for one block (`Material::dlight`).
#[derive(Debug, Clone, PartialEq)]
pub enum DlightDraw {
    /// These stages, once per light: the block's `perLight` stages or the
    /// automatic copy.
    Stages(Vec<Stage>),
    /// `dlightshader` (`scripts/gfx.shader`): the light's colour through
    /// the dlight image, `GL_DST_COLOR GL_ONE`, `depthFunc equal`.
    Blob,
}

impl Bundle {
    /// Retail's bundle flag at `+0xc4` (0x4f8556): the lightmap and the
    /// dlight image read the lightmap texture coordinates.
    pub fn takes_lightmap_coords(&self) -> bool {
        match &self.image {
            ImageRef::Lightmap => true,
            ImageRef::Path(p) => p == DLIGHT_IMAGE,
            ImageRef::White => false,
        }
    }
}

impl Stage {
    /// Whether the stage writes depth: an explicit `depthWrite` wins; else
    /// on, unless a blend other than `(One, Zero)` turns it off.
    pub fn writes_depth(&self) -> bool {
        if let Some(dw) = self.depth_write {
            return dw;
        }
        match &self.blend {
            None => true,
            Some(pair) => *pair == (BlendFactor::One, BlendFactor::Zero),
        }
    }
}

/// The draw path a material takes, which picks its pipelines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Technique {
    /// Nothing to draw: no stages (`nodraw`, clip and trigger blocks).
    None,
    /// The sky: `skyParms` boxes, with any cloud stages over the dome.
    Sky,
    /// Script stages drawn one after another (`Material::stages`).
    Stages,
}

/// Which faces a material culls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Cull {
    #[default]
    Back,
    /// `cull none` / `twoSided`.
    None,
}

/// What a texture binding feeds in a technique.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TexSemantic {
    Color,
    Lightmap,
    Normal,
    Specular,
    Detail,
}

/// One texture a material binds, by its role.
#[derive(Debug, Clone, PartialEq)]
pub struct TextureBinding {
    pub semantic: TexSemantic,
    pub image: ImageRef,
}

/// A CoD2 lightmapped phong technique's switches, the `lmap*` family
/// (docs/research/cod2-materials.md 9): which of the colour, normal,
/// specular and detail maps it reads and whether it outputs the colour's
/// alpha.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhongLit {
    /// `_s`: the sun's specular term off the specular map.
    pub specular: bool,
    /// `_dtl`: the detail map at `detail_scale` times the texcoords.
    pub detail: bool,
    /// `_a`: the diffuse alpha, else 1.
    pub alpha: bool,
    /// The material's `detailScale` constant, x and y.
    pub detail_scale: [f32; 2],
    /// A CoD4 lit set: a `wc_l_*` one's lightmap reads as CoD4's (two
    /// secondary halves and a direction, `lm_*_sm3.hlsl`), a model set's
    /// light comes off the model lighting volume; with its normal map and
    /// `envMapParms`.
    pub cod4: Option<Cod4Lit>,
}

/// A CoD4 lit set's inputs past the colour map (cod4-world.md 9.2, 3.2).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cod4Lit {
    /// `n0`: the normal map turns the lightmap's direction and the sun.
    pub normal_map: bool,
    /// `envMapParms`: the fresnel's min and max, its exponent, and the
    /// sun specular's scale.
    pub env: [f32; 4],
    /// Where the light comes from.
    pub light: Cod4Light,
    /// A water set's `waterColor` (`Cod4Light::Water`).
    pub water_color: [f32; 4],
    /// `mc_l_*_flag_*`: the vertex shader waves the vertices and their
    /// normals, and the sun's diffuse takes `|N.L|` (cod4-world.md 3.4).
    pub flag: bool,
    /// `sm`: technique 9 reads the sun's shadow map in place of the baked
    /// visibility while one is drawn (cod4-world.md 3.6).
    pub sun_shadow: bool,
}

/// A CoD4 lit set's light source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Cod4Light {
    /// `wc_l_*`: the surface's lightmap (`lm_*`).
    #[default]
    Lightmap,
    /// `mc_l_*`: the model lighting volume at the normal, plus the sun
    /// by its `N.L` (`lp_*`).
    Grid,
    /// `mc_ambient_*`: the volume's block centre, plus the sun without
    /// `N.L` (`lp_amb_*`).
    GridAmbient,
    /// `wc_water`: `water_l_sun`, the normal map's red as three octaves of
    /// height, the reflection probe over `waterColor` by the fresnel and
    /// the sun's glint; no colour map, no lightmap.
    Water,
}

/// A CoD2 model's `lprobe*` technique switches, the name's `_a`, `_s`,
/// `_dtl`, `_scroll` and `_flag` (cod2-materials.md 9.4): the entity path
/// lights a probe stage per pixel with them, a static model its
/// `lprobe_smc*` technique's (9.6).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProbeLit {
    /// `_s`: the sun's specular term off the specular map.
    pub specular: bool,
    /// `_a`: the diffuse alpha, else 1.
    pub alpha: bool,
    /// `_dtl`: the detail map at `detail_scale` times the texcoords.
    pub detail: bool,
    /// `_scroll`: the texcoords' t falls by `gameTime.z`.
    pub scroll: bool,
    /// `lprobe_flag*`: `lib/flag.hlsl`'s `Flap` waves the vertices and the
    /// sun's diffuse lights both faces (cod2-materials.md 9.7).
    pub flag: bool,
    /// The material's `detailScale` constant, x and y.
    pub detail_scale: [f32; 2],
}

/// A material as the renderer draws it.
#[derive(Debug, Clone, PartialEq)]
pub struct Material {
    pub name: String,
    pub technique: Technique,
    /// Draw order key: the engine's sort value, opaque 5, additive 19.
    pub sort: f32,
    pub surface: SurfaceBits,
    pub cull: Cull,
    /// The polygon offset the material draws at.
    pub depth_bias: Option<PolygonOffset>,
    pub nofog: bool,
    pub nopicmip: bool,
    pub nomipmaps: bool,
    pub sky: Option<SkyParms>,
    /// The sky's `.sun` file name (`scripts/<name>.sun`).
    pub sunfile: Option<String>,
    /// Vertex deforms in order, run one after another on the same vertices.
    pub deforms: Vec<Deform>,
    /// Camera-facing quads.
    pub sprite: Option<Sprite>,
    /// The material's textures by role: what a technique binds. For a CoD1
    /// script, its sorting stage's colour image and lightmap; `stages`
    /// holds the full multitexture.
    pub textures: Vec<TextureBinding>,
    /// The base pass, stage by stage.
    pub stages: Vec<Stage>,
    /// What the dynamic light pass draws per light; `None` skips it.
    pub dlight: Option<DlightDraw>,
    /// The first two images the sorted material list breaks a sort tie on,
    /// `None` without stages (cod11-shader-scripts.md section 5).
    pub sort_images: Option<Vec<ImageRef>>,
    /// Stages the source dropped (unsupported forms); diagnostic only.
    pub dropped_stages: usize,
    /// Set by the renderer on a static prop's batch: the model's left and
    /// up axes in the world, so `tcGen environment` reflects in the model's
    /// frame on world-baked vertices (shader doc 8, "Static props").
    pub model_axes: Option<[[f32; 3]; 2]>,
    /// A CoD2 `lmap*` technique: the renderer's CoD2 world pipeline draws
    /// a lightmapped surface with it in place of `stages`. `None` on CoD1.
    pub phong: Option<PhongLit>,
    /// A CoD2 `vertcol_mul*` decal (`effect_multiply`): the stage multiplies
    /// the framebuffer by `lerp(1, colour * vertex rgb, vertex alpha)`
    /// (cod2-materials.md 9, "Multiply"). Always false on CoD1.
    pub vertex_multiply: bool,
    /// How a CoD2 model surface takes the map's light. Always
    /// [`ModelLight::None`] on CoD1.
    pub model_light: ModelLight,
    /// A [`ModelLight::Probe`] set's entity technique. `None` on CoD1.
    pub probe: Option<ProbeLit>,
    /// A [`ModelLight::Probe`] set's static model technique (`lightprobe
    /// smodel cached`, `lprobe_smc*`), which a static prop draws per pixel
    /// (cod2-materials.md 9.6). `None` on CoD1.
    pub smodel: Option<ProbeLit>,
    /// A `zfeather*` effect: `featherParms.x`, the scale on the depth gap
    /// to the scene that fades the sprite's alpha (cod2-materials.md 9.5,
    /// cod4-effects.md 14). `None` on CoD1 and every other technique.
    pub feather: Option<f32>,
    /// The feather's depths are the view depth itself, CoD4's
    /// `zfeather.hlsl`; false, CoD2's clip z `0.9995 * (depth - 4)`.
    pub feather_view_depth: bool,
    /// A CoD4 material 1.7's depth prepass draws into `floatZ`
    /// (`TECHNIQUE_BUILD_FLOAT_Z`, cod4-client-perf.md 35), and whether it
    /// alpha tests there. `None` on CoD1, CoD2 and a CoD4 material without
    /// the technique or with prepass key 3.
    pub floatz: Option<bool>,
    /// A CoD4 material whose lit state culls back faces (`GFXS0_CULL_BACK`,
    /// cod4-world.md 8): its entity surfaces are culled as 1.7 draws
    /// them (cod4-client-perf.md 37). False on CoD1 and CoD2, whose
    /// entity draws stay two-sided.
    pub entity_cull: bool,
    /// A CoD4 `distortion_*` effect: `distortionScale.xy`, how far its
    /// offset map bends the copy of the scene it draws
    /// (`distortion_scale_zfeather.hlsl`, cod4-effects.md 17). `None` on
    /// every other set.
    pub distortion: Option<[f32; 2]>,
    /// Whether a CoD2 material's set names the `shadowcookie caster` and
    /// `shadowcookie receiver` techniques: an entity surface with the
    /// first draws into a shadow cookie, a world surface with the second
    /// takes them (cod2-materials.md 9.8). Both false on CoD1.
    pub cookie: CookieRole,
    /// A CoD2 `water` set material (`water_l_sun`): the renderer's CoD2
    /// water pipeline draws it in place of `stages` (cod2-materials.md
    /// 9.12). `None` on CoD1.
    pub water: Option<WaterLit>,
    /// A CoD4 falloff material (`unlit_distfalloff`'s `hdrportal`
    /// doorway lighteners, the `falloff_add` light shafts): the stage
    /// colour's vertex colour times the falloff. `None` elsewhere.
    pub falloff: Option<Falloff>,
    /// A CoD4 set whose pixel shader writes its colour times its alpha
    /// (cod4-world.md 8.1). The renderer reads it only where no blend
    /// factor carries the alpha: (OneMinusDstColor, One). False elsewhere.
    pub premultiplied: bool,
    /// A CoD4 `eyeoffset` set's `eyeOffsetParms.x`: the vertices drawn that
    /// many units nearer the eye along their view ray. `None` elsewhere.
    pub eye_offset: Option<f32>,
    /// A CoD4 `reflexsight` material (the red dot): its `detailScale`,
    /// which spreads the dot the colour map draws over the view's angle
    /// off the lens (cod4-client-view.md 4.5). `None` elsewhere.
    pub reflex: Option<[f32; 2]>,
}

/// A CoD4 falloff vertex shader's weight `w = saturate(scale * x +
/// bias)` and colour `lerp(end, begin, w)` (cod4-world.md 9.4, 9.5).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Falloff {
    pub begin: [f32; 3],
    pub end: [f32; 3],
    pub scale: f32,
    pub bias: f32,
    pub kind: FalloffKind,
}

/// What a [`Falloff`]'s `x` is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FalloffKind {
    /// `vertcol_simple_fog_df.hlsl`: the xy distance from the eye to the
    /// vertex's texture coordinates, which hold the xy centre of the
    /// opening its face covers; the colour's alpha is scaled by `w` too.
    Distance,
    /// `vertcol_simple_fog_foa.hlsl` and kin: the squared cosine between
    /// the view ray and the vertex normal; the alpha is the vertex's.
    Angle,
}

/// What `water_l_sun` reads off a CoD2 water material
/// (cod2-materials.md 2.1, 9.12): the `waterColor` constant, the
/// `cubicEnvMap` image and the semantic 5 texture's water def, from which
/// the renderer builds the height map each frame.
#[derive(Debug, Clone, PartialEq)]
pub struct WaterLit {
    pub color: [f32; 4],
    /// The cube's image path; empty when the material names none.
    pub cube: String,
    /// `textureWidth`: the height map is this square.
    pub width: i32,
    /// `horizontalWorldLength`, `verticalWorldLength`.
    pub lengths: [f32; 2],
    pub amplitude: f32,
    pub wind_speed: f32,
    pub wind_direction: [f32; 2],
}

/// A material's part in CoD2's shadow cookies.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CookieRole {
    pub caster: bool,
    pub receiver: bool,
    /// CoD4: the material casts into the sun's shadow map (its
    /// `build_shadowmap_color` technique, cod4-world.md 3.6).
    pub sun_caster: bool,
}

/// Which light probe techniques a CoD2 material's set carries, so how a
/// model drawn with it is lit (cod2-materials.md 9.2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ModelLight {
    /// No probe technique (the multiply decals, the glows, the unlit sets):
    /// the compiled stages, which read the model's vertex colour.
    #[default]
    None,
    /// `lprobe*` on an entity, `lprobe_smc*` on a static model: colour map
    /// times vertex colour times the grid at the normal plus the sun's
    /// `N.L`.
    Probe,
    /// The `ambient_*` sets: an entity draws `vertcol_shaded`, unlit; a
    /// static model `lprobe_smc_amb`, its light at the model's centre plus
    /// the sun by its visibility, without `N.L`.
    Ambient,
    /// The `objective` set (`objective_base.hlsl`): unlit, the colour map's
    /// luminance under a colour pulsing with the facing and the clock,
    /// added.
    Objective,
}

impl Material {
    /// Whether some bundle takes `tcGen environment`.
    pub fn reflects(&self) -> bool {
        self.stages
            .iter()
            .flat_map(|st| &st.bundles)
            .any(|b| b.tc_gen == TcGen::Environment)
    }

    /// The first texture bound for `semantic`.
    pub fn texture(&self, semantic: TexSemantic) -> Option<&ImageRef> {
        self.textures
            .iter()
            .find(|t| t.semantic == semantic)
            .map(|t| &t.image)
    }

    /// Per-stage draw facts; `None` when `idx` is out of range.
    pub fn classify_stage(&self, idx: usize) -> Option<ClassedStage> {
        let st = self.stages.get(idx)?;
        let class = match st.blend {
            None => DrawClass::Opaque,
            Some((_, BlendFactor::One)) | Some((_, BlendFactor::SrcAlphaSaturate)) => {
                DrawClass::Additive
            }
            Some(_) => DrawClass::Blend,
        };
        Some(ClassedStage {
            class,
            depth_write: st.writes_depth(),
            bias: self.depth_bias.is_some(),
            two_sided: self.cull == Cull::None,
        })
    }

    /// The same material drawing `stages` alone and skipping the dlight
    /// pass: how the dlight pass draws one of its stages.
    pub fn with_stages(&self, stages: Vec<Stage>) -> Material {
        Material {
            stages,
            dlight: None,
            ..self.clone()
        }
    }
}

/// Every material of a game, keyed by lowercased name.
#[derive(Clone, Default)]
pub struct MaterialLib {
    pub(crate) by_name: HashMap<String, Material>,
    /// First texture path per material, what the implicit-material loaders
    /// read (lowercased, known image extension stripped).
    pub(crate) images: HashMap<String, String>,
    /// Unique warnings fired while building the library.
    pub(crate) warns: usize,
}

impl MaterialLib {
    /// A library of materials another game's loader compiled, each with
    /// its first image path (`None` when it names no real texture), and
    /// the unique warnings that loader fired.
    pub fn from_materials(
        materials: impl IntoIterator<Item = (Material, Option<String>)>,
        warns: usize,
    ) -> Self {
        let mut lib = MaterialLib {
            warns,
            ..Default::default()
        };
        for (m, image) in materials {
            let key = norm_key(&m.name);
            if let Some(img) = image {
                lib.images.insert(key.clone(), img);
            }
            lib.by_name.insert(key, m);
        }
        lib
    }

    pub fn get(&self, name: &str) -> Option<&Material> {
        self.by_name.get(&norm_key(name))
    }

    /// The material's first image path; `None` when it names no real
    /// texture.
    pub fn image(&self, name: &str) -> Option<&str> {
        self.images.get(&norm_key(name)).map(String::as_str)
    }

    /// Unique warnings fired while building the library.
    pub fn warn_count(&self) -> usize {
        self.warns
    }

    pub fn sky_blocks(&self) -> impl Iterator<Item = &Material> {
        self.by_name
            .values()
            .filter(|m| m.technique == Technique::Sky)
    }

    /// Every material's (normalized) name, in no particular order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.by_name.keys().map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}

/// A material name as the libraries key it.
pub(crate) fn norm_key(name: &str) -> String {
    name.to_lowercase().replace('\\', "/")
}

// ---- tcMod runtime evaluation ----
// Math pinned to RTCW-MP src/renderer/tr_shade_calc.c; table build loop is
// tr_init.c R_Init.

const FUNCTABLE_SIZE: usize = 1024;
const FUNCTABLE_MASK: i64 = (FUNCTABLE_SIZE - 1) as i64;

/// Compile-time stand-in for libm sin(): quadrant reduction then Maclaurin.
const fn sin_deg(deg: f64) -> f64 {
    let mut x = deg % 360.0;
    if x > 180.0 {
        x -= 360.0;
    } else if x < -180.0 {
        x += 360.0;
    }
    if x > 90.0 {
        x = 180.0 - x;
    } else if x < -90.0 {
        x = -180.0 - x;
    }
    let r = x * std::f64::consts::PI / 180.0;
    let sq = -r * r;
    let mut term = r;
    let mut sum = r;
    let mut k = 2.0f64;
    while k < 30.0 {
        term *= sq / (k * (k + 1.0));
        sum += term;
        k += 2.0;
    }
    sum
}

const fn build_sin_table() -> [f32; FUNCTABLE_SIZE] {
    let mut t = [0.0f32; FUNCTABLE_SIZE];
    let mut i = 0usize;
    while i < FUNCTABLE_SIZE {
        // RTCW spans the table over SIZE-1 degrees of arc, so entry 256 is
        // sin(90.088deg), not exactly 1
        t[i] = sin_deg(i as f64 * 360.0 / (FUNCTABLE_SIZE - 1) as f64) as f32;
        i += 1;
    }
    t
}

const fn build_square_table() -> [f32; FUNCTABLE_SIZE] {
    let mut t = [0.0f32; FUNCTABLE_SIZE];
    let mut i = 0usize;
    while i < FUNCTABLE_SIZE {
        t[i] = if i < FUNCTABLE_SIZE / 2 { 1.0 } else { -1.0 };
        i += 1;
    }
    t
}

const fn build_sawtooth_table() -> [f32; FUNCTABLE_SIZE] {
    let mut t = [0.0f32; FUNCTABLE_SIZE];
    let mut i = 0usize;
    while i < FUNCTABLE_SIZE {
        t[i] = i as f32 / FUNCTABLE_SIZE as f32;
        i += 1;
    }
    t
}

const fn build_inverse_sawtooth_table() -> [f32; FUNCTABLE_SIZE] {
    let mut t = [0.0f32; FUNCTABLE_SIZE];
    let mut i = 0usize;
    while i < FUNCTABLE_SIZE {
        t[i] = 1.0 - i as f32 / FUNCTABLE_SIZE as f32;
        i += 1;
    }
    t
}

const fn build_triangle_table() -> [f32; FUNCTABLE_SIZE] {
    let mut t = [0.0f32; FUNCTABLE_SIZE];
    let mut i = 0usize;
    while i < FUNCTABLE_SIZE {
        t[i] = if i < FUNCTABLE_SIZE / 2 {
            if i < FUNCTABLE_SIZE / 4 {
                i as f32 / (FUNCTABLE_SIZE / 4) as f32
            } else {
                1.0 - t[i - FUNCTABLE_SIZE / 4]
            }
        } else {
            -t[i - FUNCTABLE_SIZE / 2]
        };
        i += 1;
    }
    t
}

const SIN_TABLE: [f32; FUNCTABLE_SIZE] = build_sin_table();
const SQUARE_TABLE: [f32; FUNCTABLE_SIZE] = build_square_table();
const TRIANGLE_TABLE: [f32; FUNCTABLE_SIZE] = build_triangle_table();
const SAWTOOTH_TABLE: [f32; FUNCTABLE_SIZE] = build_sawtooth_table();
const INVERSE_SAWTOOTH_TABLE: [f32; FUNCTABLE_SIZE] = build_inverse_sawtooth_table();

/// R_NoiseInit (CoDMP.exe 0x4dd490, CoDUOMP.exe 0x50e7d0): `srand(1001)`,
/// then per entry one MSVC `rand()` for the value, `r * (1 / 32767) * 2 - 1`
/// (0x568ed8), and one for the permutation, `(r * 256) >> 15` (shader doc 4).
const fn build_noise_tables() -> ([f32; 256], [u8; 256]) {
    let mut table = [0.0f32; 256];
    let mut perm = [0u8; 256];
    let mut hold: u32 = 1001;
    let mut i = 0;
    while i < 256 {
        hold = hold.wrapping_mul(214013).wrapping_add(2531011);
        let r = (hold >> 16) & 0x7fff;
        let c = (1.0f32 / 32767.0) as f64;
        table[i] = (r as f64 * c * 2.0 - 1.0) as f32;
        hold = hold.wrapping_mul(214013).wrapping_add(2531011);
        let r = (hold >> 16) & 0x7fff;
        perm[i] = (r >> 7) as u8;
        i += 1;
    }
    (table, perm)
}

const NOISE: ([f32; 256], [u8; 256]) = build_noise_tables();
/// The noise's value and permutation tables.
pub const NOISE_TABLE: [f32; 256] = NOISE.0;
pub const NOISE_PERM: [u8; 256] = NOISE.1;

/// R_NoiseGet4f (CoDMP.exe 0x4dd4f0): the value at the lattice point
/// `INDEX(x, y, z, t) = perm[x + perm[y + perm[z + perm[t]]]]`, trilinear in
/// x, y, z, then linear in t. Each floor is `fistp(v - 0.49999999)`.
pub fn noise_get4f(x: f32, y: f32, z: f32, t: f32) -> f32 {
    let split = |v: f32| {
        let i = (v as f64).floor();
        (i as i32, v as f64 - i)
    };
    let (ix, fx) = split(x);
    let (iy, fy) = split(y);
    let (iz, fz) = split(z);
    let (it, ft) = split(t);
    let val = |a: i32| NOISE_PERM[(a & 0xff) as usize] as i32;
    let get = |x: i32, y: i32, z: i32, t: i32| {
        NOISE_TABLE[val(x + val(y + val(z + val(t)))) as usize] as f64
    };
    let lerp = |a: f64, b: f64, f: f64| a * (1.0 - f) + b * f;
    let at = |t: i32| {
        let front = lerp(
            lerp(get(ix, iy, iz, t), get(ix + 1, iy, iz, t), fx),
            lerp(get(ix, iy + 1, iz, t), get(ix + 1, iy + 1, iz, t), fx),
            fy,
        );
        let back = lerp(
            lerp(get(ix, iy, iz + 1, t), get(ix + 1, iy, iz + 1, t), fx),
            lerp(
                get(ix, iy + 1, iz + 1, t),
                get(ix + 1, iy + 1, iz + 1, t),
                fx,
            ),
            fy,
        );
        lerp(front, back, fz)
    };
    lerp(at(it), at(it + 1), ft) as f32
}

/// WAVEVALUE (tr_shade_calc.c:34): base + table[idx]*amp with
/// idx = ftol(phase + t*freq, scaled by 1024) & 1023. myftol is x87 fistp,
/// which rounds to nearest even, so 511.9 indexes 512 where an `as` cast
/// would truncate to 511.
pub fn wave_value(w: &Wave, t: f32) -> f32 {
    let table = match w.form {
        WaveForm::Sin => &SIN_TABLE,
        WaveForm::Square => &SQUARE_TABLE,
        WaveForm::Triangle => &TRIANGLE_TABLE,
        WaveForm::Sawtooth => &SAWTOOTH_TABLE,
        WaveForm::InverseSawtooth => &INVERSE_SAWTOOTH_TABLE,
        // only `rgbGen` reads noise ([`rgb_wave_value`]); retail's other
        // wave readers find no table for it and error (0x5133d0)
        WaveForm::Noise => return w.base,
    };
    let x = (w.phase + t * w.freq) * FUNCTABLE_SIZE as f32;
    let idx = x.round_ties_even() as i64 & FUNCTABLE_MASK;
    w.base + table[idx as usize] * w.amp
}

/// `rgbGen wave` before its clamp (CoDMP.exe 0x514c00): a noise wave is
/// `base + amp * R_NoiseGet4f(0, 0, 0, (t + phase) * freq)`, any other the
/// table wave, which the renderer then scales by identityLight.
pub fn rgb_wave_value(w: &Wave, t: f32) -> f32 {
    if w.form == WaveForm::Noise {
        return w.base + noise_get4f(0.0, 0.0, 0.0, (t + w.phase) * w.freq) * w.amp;
    }
    wave_value(w, t)
}

/// Column-major like wgpu mat3x2: [a,b,c,d,tx,ty] means
/// s' = a*s + c*t + tx ; t' = b*s + d*t + ty.
type Affine = [f32; 6];

const AFFINE_IDENTITY: Affine = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// outer applied after inner.
fn compose_affine(o: &Affine, i: &Affine) -> Affine {
    [
        o[0] * i[0] + o[2] * i[1],
        o[1] * i[0] + o[3] * i[1],
        o[0] * i[2] + o[2] * i[3],
        o[1] * i[2] + o[3] * i[3],
        o[0] * i[4] + o[2] * i[5] + o[4],
        o[1] * i[4] + o[3] * i[5] + o[5],
    ]
}

/// Folds the tcMods of one bundle left from identity in listed order.
/// Turb is not affine and rides separately in [`bundle_turb`].
pub fn bundle_affine(tcmods: &[TcMod], t: f32) -> [f32; 6] {
    let mut acc = AFFINE_IDENTITY;
    for m in tcmods {
        let next = match m {
            TcMod::Scroll(sx, sy) => {
                // RB_CalcScrollTexCoords keeps the fractional part only
                let fx = sx * t;
                let fy = sy * t;
                [1.0, 0.0, 0.0, 1.0, fx - fx.floor(), fy - fy.floor()]
            }
            TcMod::Scale(sx, sy) => [*sx, 0.0, 0.0, *sy, 0.0, 0.0],
            TcMod::Rotate(dps) => {
                // RB_CalcRotateTexCoords: negated degrees, index truncated
                // toward zero before masking, cos read a quarter ahead
                let degs = -dps * t;
                let idx = (degs * (FUNCTABLE_SIZE as f32 / 360.0)) as i64 & FUNCTABLE_MASK;
                let sin_v = SIN_TABLE[idx as usize];
                let cos_v =
                    SIN_TABLE[(idx + FUNCTABLE_SIZE as i64 / 4) as usize & FUNCTABLE_MASK as usize];
                [
                    cos_v,
                    sin_v,
                    -sin_v,
                    cos_v,
                    0.5 - 0.5 * cos_v + 0.5 * sin_v,
                    0.5 - 0.5 * sin_v - 0.5 * cos_v,
                ]
            }
            TcMod::Stretch(w) => {
                // RB_CalcStretchTexCoords: uniform 1/waveform about centre
                let p = 1.0 / wave_value(w, t);
                [p, 0.0, 0.0, p, 0.5 - 0.5 * p, 0.5 - 0.5 * p]
            }
            // args stored m00 m01 m10 m11 tS tT map verbatim onto this layout
            TcMod::Transform(a) => *a,
            TcMod::Turb { .. } => continue,
        };
        acc = compose_affine(&next, &acc);
    }
    acc
}

/// [amp0, now0, amp1, now1] with now = phase + t*freq for the last Turb of
/// the list; zeros when there is none. The VS adds
/// amp*sin(worldpos_axis/1024 + now) per axis.
pub fn bundle_turb(tcmods: &[TcMod], t: f32) -> [f32; 4] {
    let mut out = [0.0f32; 4];
    for m in tcmods {
        if let TcMod::Turb { amp, phase, freq } = m {
            let now = phase + t * freq;
            out = [*amp, now, *amp, now];
        }
    }
    out
}

/// Scale/Transform are static; everything else animates with time.
pub fn has_animated_tcmods(tcmods: &[TcMod]) -> bool {
    tcmods
        .iter()
        .any(|m| !matches!(m, TcMod::Scale(_, _) | TcMod::Transform(_)))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- tcMod runtime evaluation ----

    fn apply(m: &[f32; 6], s: f32, t: f32) -> (f32, f32) {
        (m[0] * s + m[2] * t + m[4], m[1] * s + m[3] * t + m[5])
    }

    fn close(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    fn assert_mat6(got: &[f32; 6], want: [f32; 6], eps: f32) {
        for i in 0..6 {
            assert!(
                close(got[i], want[i], eps),
                "[{i}] got {} want {}",
                got[i],
                want[i]
            );
        }
    }

    #[test]
    fn scroll_translates_fractional_speed_times_time() {
        // 0.5 * 2.3 = 1.15, wrapped to 0.15
        assert_mat6(
            &bundle_affine(&[TcMod::Scroll(0.5, 0.0)], 2.3),
            [1.0, 0.0, 0.0, 1.0, 0.15, 0.0],
            1e-6,
        );
        // negative speeds wrap upward: -0.75 -> 0.25, -0.25 -> 0.75
        assert_mat6(
            &bundle_affine(&[TcMod::Scroll(-0.75, -0.25)], 1.0),
            [1.0, 0.0, 0.0, 1.0, 0.25, 0.75],
            1e-6,
        );
    }

    #[test]
    fn rotate_negates_degrees_per_second() {
        // 90 deg/s for 1s: degs = -90, quarter turn about (0.5, 0.5) sends
        // UV point (1, 0.5) to (0.5, 0)
        let m = bundle_affine(&[TcMod::Rotate(90.0)], 1.0);
        assert_mat6(&m, [0.0, -1.0, 1.0, 0.0, 0.0, 1.0], 1e-4);
        let (sp, tp) = apply(&m, 1.0, 0.5);
        assert!(close(sp, 0.5, 1e-4) && close(tp, 0.0, 1e-4));

        // small positive time, CCW-positive speed: (1, 0.5) drifts toward
        // negative t (RB_CalcRotateTexCoords negates first)
        let m = bundle_affine(&[TcMod::Rotate(90.0)], 0.01);
        let (sp, tp) = apply(&m, 1.0, 0.5);
        assert!(sp > 0.99 && sp < 1.01, "s' {sp}");
        assert!(tp < 0.499, "t' must drop below 0.5, got {tp}");
    }

    #[test]
    fn stretch_inverts_wave_about_centre() {
        // phase+freq*t lands exactly on table index 256: sin ~= 1,
        // waveform = 1 + 0.5 ~= 1.5, p = 1/1.5 = 2/3
        let w = Wave {
            form: WaveForm::Sin,
            base: 1.0,
            amp: 0.5,
            phase: 0.0,
            freq: 0.25,
        };
        let m = bundle_affine(&[TcMod::Stretch(w)], 1.0);
        let p = m[0];
        assert!(close(p, 2.0 / 3.0, 1e-4), "p {p}");
        assert_mat6(&m, [p, 0.0, 0.0, p, 0.5 - 0.5 * p, 0.5 - 0.5 * p], 1e-6);
        // centre is a fixed point
        let (sp, tp) = apply(&m, 0.5, 0.5);
        assert!(close(sp, 0.5, 1e-5) && close(tp, 0.5, 1e-5));
    }

    #[test]
    fn transform_matrix_applies_verbatim_args() {
        let ident = bundle_affine(&[TcMod::Transform([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])], 7.0);
        assert_eq!(ident, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

        // args arrive m00 m01 m10 m11 tS tT; lone transform comes back
        // verbatim in [a,b,c,d,tx,ty] layout
        let args = [2.0, 3.0, 4.0, 5.0, 6.0, 7.0];
        let m = bundle_affine(&[TcMod::Transform(args)], 7.0);
        assert_eq!(m, args);
        // s' = m00*s + m10*t + tS ; t' = m01*s + m11*t + tT
        let (sp, tp) = apply(&m, 0.3, 0.8);
        assert!(close(sp, 2.0 * 0.3 + 4.0 * 0.8 + 6.0, 1e-5));
        assert!(close(tp, 3.0 * 0.3 + 5.0 * 0.8 + 7.0, 1e-5));
    }

    #[test]
    fn tcmod_composition_order_matters() {
        // scroll-then-scale carries the translation through the scale
        let a = bundle_affine(&[TcMod::Scroll(0.5, 0.0), TcMod::Scale(2.0, 2.0)], 2.3);
        assert_mat6(&a, [2.0, 0.0, 0.0, 2.0, 0.3, 0.0], 1e-6);
        // scale-then-scroll does not
        let b = bundle_affine(&[TcMod::Scale(2.0, 2.0), TcMod::Scroll(0.5, 0.0)], 2.3);
        assert_mat6(&b, [2.0, 0.0, 0.0, 2.0, 0.15, 0.0], 1e-6);
    }

    #[test]
    fn square_wave_flips_at_half_cycle_with_nearest_index() {
        let w = Wave {
            form: WaveForm::Square,
            base: 0.0,
            amp: 1.0,
            phase: 0.0,
            freq: 1.0,
        };
        assert!(close(wave_value(&w, 0.499), 1.0, 1e-6));
        // 0.4999 * 1024 = 511.898: myftol rounds to nearest, indexing 512
        // (truncation would stay on 511 and read +1)
        assert!(close(wave_value(&w, 0.4999), -1.0, 1e-6));
        assert!(close(wave_value(&w, 0.5), -1.0, 1e-6));
        assert!(close(wave_value(&w, 1.0), 1.0, 1e-6));
    }

    #[test]
    fn wave_value_matches_hand_computed_table_entries() {
        // sawtooth table[i] = i/1024: index 256 reads 0.25 exactly
        let saw = Wave {
            form: WaveForm::Sawtooth,
            base: 0.0,
            amp: 1.0,
            phase: 0.25,
            freq: 0.0,
        };
        assert!(close(wave_value(&saw, 123.0), 0.25, 1e-6));
        // inverse sawtooth mirrors it
        let inv = Wave {
            form: WaveForm::InverseSawtooth,
            ..saw.clone()
        };
        assert!(close(wave_value(&inv, 123.0), 0.75, 1e-6));
        // triangle peaks at the quarter, bottoms past three quarters
        let tri = |phase: f32| Wave {
            form: WaveForm::Triangle,
            base: 0.0,
            amp: 1.0,
            phase,
            freq: 0.0,
        };
        assert!(close(wave_value(&tri(0.125), 0.0), 0.5, 1e-6));
        assert!(close(wave_value(&tri(0.25), 0.0), 1.0, 1e-6));
        assert!(close(wave_value(&tri(0.75), 0.0), -1.0, 1e-6));
        // sin near its peak: sinTable[256] = sin(256*360/1023 deg)
        let sin = Wave {
            form: WaveForm::Sin,
            base: 100.0,
            amp: 20.0,
            phase: 0.25,
            freq: 0.0,
        };
        assert!(close(wave_value(&sin, 0.0), 120.0, 1e-4));
        // negative phases index through the mask, not negative slots
        let neg = Wave {
            form: WaveForm::Sawtooth,
            base: 0.0,
            amp: 1.0,
            phase: -0.25,
            freq: 0.0,
        };
        assert!(close(wave_value(&neg, 0.0), 0.75, 1e-6));
    }

    #[test]
    fn noise_tables_follow_msvc_rand_seeded_1001() {
        // srand(1001): the first rand() is (1001 * 214013 + 2531011) >> 16
        assert_eq!(
            NOISE_TABLE[0],
            (3307.0f64 * (1.0f32 / 32767.0) as f64 * 2.0 - 1.0) as f32
        );
        assert!(NOISE_TABLE.iter().all(|v| (-1.0..=1.0).contains(v)));
        // a permutation byte is rand() >> 7, so all of 0..255 can appear
        assert!(NOISE_PERM.iter().any(|&p| p > 200));
        // on the lattice the noise is one table entry, between it a blend
        let index = |t: i32| {
            let val = |a: i32| NOISE_PERM[(a & 0xff) as usize] as i32;
            NOISE_TABLE[val(val(val(val(t)))) as usize]
        };
        assert_eq!(noise_get4f(0.0, 0.0, 0.0, 7.0), index(7));
        let mid = noise_get4f(0.0, 0.0, 0.0, 7.25);
        assert!(close(mid, index(7) * 0.75 + index(8) * 0.25, 1e-6));
        // negative times floor down, not toward zero
        assert!(close(
            noise_get4f(0.0, 0.0, 0.0, -0.5),
            (index(-1) + index(0)) * 0.5,
            1e-6
        ));
    }

    #[test]
    fn rgb_noise_wave_flickers_about_its_base() {
        // UO ba.shader textures/ardennes/windows/firewindow
        let w = Wave {
            form: WaveForm::Noise,
            base: 0.4,
            amp: 0.1,
            phase: 0.0,
            freq: 10.0,
        };
        let v: Vec<f32> = (0..50)
            .map(|i| rgb_wave_value(&w, i as f32 * 0.013))
            .collect();
        assert!(v.iter().all(|x| (0.3..=0.5).contains(x)));
        assert!(v.iter().any(|x| (x - 0.4).abs() > 0.02), "{v:?}");
        // noise elsewhere (alphaGen, tcMod stretch) has no table: base
        assert_eq!(wave_value(&w, 0.37), 0.4);
    }

    #[test]
    fn sin_table_matches_libm_over_full_range() {
        for (i, v) in SIN_TABLE.iter().enumerate() {
            let want = (i as f64 * 360.0 / (FUNCTABLE_SIZE - 1) as f64)
                .to_radians()
                .sin() as f32;
            assert!((v - want).abs() < 1e-5, "SIN_TABLE[{i}] {v} vs libm {want}");
        }
    }

    #[test]
    fn wave_value_is_periodic_over_one_cycle() {
        let forms = [
            WaveForm::Sin,
            WaveForm::Square,
            WaveForm::Triangle,
            WaveForm::Sawtooth,
            WaveForm::InverseSawtooth,
        ];
        for form in forms {
            let label = format!("{form:?}");
            let w = Wave {
                form: form.clone(),
                base: 0.3,
                amp: 0.8,
                phase: 0.11,
                freq: 1.0,
            };
            let mut t = 0.0f32;
            while t < 1.0 {
                assert!(
                    close(wave_value(&w, t), wave_value(&w, t + 1.0), 1e-4),
                    "{label} at t={t}"
                );
                t += 0.037;
            }
        }
    }

    #[test]
    fn turb_bundle_folds_time_into_now_last_wins() {
        assert_eq!(bundle_turb(&[], 3.0), [0.0; 4]);
        assert_eq!(bundle_turb(&[TcMod::Scroll(1.0, 1.0)], 3.0), [0.0; 4]);
        // now = phase + freq*t, unwrapped; same now for both axes
        let turb = TcMod::Turb {
            amp: 2.0,
            phase: 0.25,
            freq: 0.5,
        };
        assert_eq!(
            bundle_turb(std::slice::from_ref(&turb), 3.0),
            [2.0, 1.75, 2.0, 1.75]
        );
        // the affine pass leaves turbulence untouched
        assert_eq!(
            bundle_affine(
                &[TcMod::Turb {
                    amp: 2.0,
                    phase: 0.25,
                    freq: 0.5
                }],
                3.0
            ),
            [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]
        );
        // later Turb replaces earlier
        let late = TcMod::Turb {
            amp: 3.0,
            phase: 1.0,
            freq: 1.0,
        };
        assert_eq!(
            bundle_turb(&[turb, TcMod::Scale(2.0, 2.0), late], 2.0),
            [3.0, 3.0, 3.0, 3.0]
        );
    }

    #[test]
    fn has_animated_tcmods_truth_table() {
        assert!(!has_animated_tcmods(&[]));
        assert!(!has_animated_tcmods(&[TcMod::Scale(2.0, 2.0)]));
        assert!(!has_animated_tcmods(&[TcMod::Transform([1.0; 6])]));
        assert!(has_animated_tcmods(&[TcMod::Scroll(0.1, 0.0)]));
        assert!(has_animated_tcmods(&[TcMod::Rotate(30.0)]));
        assert!(has_animated_tcmods(&[TcMod::Stretch(Wave {
            form: WaveForm::Sin,
            base: 1.0,
            amp: 0.1,
            phase: 0.0,
            freq: 1.0
        })]));
        assert!(has_animated_tcmods(&[TcMod::Turb {
            amp: 1.0,
            phase: 0.0,
            freq: 1.0
        }]));
        // static mods alongside an animated one still animate
        assert!(has_animated_tcmods(&[
            TcMod::Scale(2.0, 2.0),
            TcMod::Scroll(0.1, 0.0)
        ]));
    }

    #[test]
    fn empty_tcmods_yield_identity_affine() {
        assert_eq!(bundle_affine(&[], 5.0), [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    }
}
