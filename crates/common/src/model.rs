//! The game-neutral model: bones with their bind and hit boxes, LODs of
//! skinned surfaces, collision. Each on-disk format fills it through a
//! [`ModelFormat`]; [`load`] picks the format by the descriptor's version.
//! CoD1's v14 is [`crate::xmodel::V14`] (docs/research/xmodel-v14-format.md);
//! CoD2's v20 (docs/research/cod2-xmodel-v20-format.md) registers through
//! [`register_format`].

use crate::asset_source::{Asset, AssetKind, AssetSource};
use anyhow::{Result, anyhow, bail};
use glam::{Quat, Vec3};
use std::sync::RwLock;

#[derive(Clone)]
pub struct Model {
    /// LOD 0's name, used only in diagnostics and cache keys.
    pub name: String,
    /// LOD 0's skeleton, parents before children. Every LOD's vertex bone
    /// ids index it: both formats read one parts file, LOD 0's.
    pub bones: Vec<Bone>,
    /// The present LODs in order, never empty.
    pub lods: Vec<Lod>,
    /// The descriptor's collision mesh, baked to model space. Empty for the
    /// models that have none.
    pub collision: Vec<CollSurf>,
}

/// One LOD's mesh, skinned against its model's bones.
#[derive(Clone)]
pub struct Lod {
    pub name: String,
    /// The LOD distance it draws below; 0 draws at any distance.
    pub dist: f32,
    pub surfaces: Vec<Surface>,
    /// Material (v14: skin file) names, index-aligned with `surfaces`.
    pub materials: Vec<String>,
}

/// One collision surface. `contents` meets the trace mask the way a brush's
/// does (solid 0x1, lamp glass 0x10; tree canopies and signs carry 0), and
/// `flags` carries the sound material in bits 20-24.
/// docs/research/xmodel-v14-format.md, "Collision block".
#[derive(Clone)]
pub struct CollSurf {
    pub contents: u32,
    pub flags: u32,
    pub tris: Vec<[Vec3; 3]>,
    /// The stored `mins`/`maxs`, in the bone's space and unbaked: what the
    /// engine's static-model bounds are built from (combat doc 14.7).
    pub bounds: (Vec3, Vec3),
    /// The bone the surface rides, into `Model::bones`: `tris` are baked
    /// at its bind, and a posed model's trace moves them with it
    /// (docs/research/coduo-vehicles.md 13).
    pub bone: usize,
}

#[derive(Clone)]
pub struct Surface {
    pub verts: Vec<Vertex>,
    pub indices: Vec<u16>,
    /// Into the LOD's `materials`.
    pub material: usize,
}

/// A vertex baked to model space through its primary bone's bind; the
/// renderer uploads it as is.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    /// Model-local bone ids, padded with 0.
    pub bone_indices: [u8; 4],
    /// Sums to 1.0, padded with 0.0.
    pub bone_weights: [f32; 4],
    /// RGBA as the format stores it (CoD2's xmodelsurfs); white on v14,
    /// which stores none.
    pub color: [u8; 4],
    /// The normal map's tangent frame, baked as the normal is (CoD2's
    /// xmodelsurfs); zero on v14, which stores none.
    pub tangent: [f32; 3],
    pub binormal: [f32; 3],
}

/// `pos`/`rot` are the world bind the mesh is skinned against; `local_*` the
/// local the engine composes it from (on v14 viewhands, after the zeroing;
/// there `pos` is the bind the mesh implies, not the composition).
#[derive(Clone)]
pub struct Bone {
    pub name: String,
    pub parent: i32,
    pub pos: Vec3,
    pub rot: Quat,
    pub local_pos: Vec3,
    pub local_rot: Quat,
    /// The hit box in this bone's own frame; all-zero on a bone the artist
    /// turned off. v14 stores it in the parts file, v20 in the descriptor.
    pub hit_mins: Vec3,
    pub hit_maxs: Vec3,
    /// Index into the 19 hit-location names, 0 (`none`) when the bone
    /// inherits its parent's. docs/research/cod11-combat.md section 3.
    pub hit_location: u8,
}

impl Model {
    /// A model of `bones` alone: one LOD at distance 0 with no surfaces and
    /// no collision. For rigs built by hand.
    pub fn rig(name: &str, bones: Vec<Bone>) -> Model {
        Model {
            name: name.to_string(),
            bones,
            lods: vec![Lod {
                name: name.to_string(),
                dist: 0.0,
                surfaces: Vec::new(),
                materials: Vec::new(),
            }],
            collision: Vec::new(),
        }
    }

    /// LOD 0's surfaces.
    pub fn surfaces(&self) -> &[Surface] {
        &self.lods[0].surfaces
    }

    /// LOD 0's material names.
    pub fn materials(&self) -> &[String] {
        &self.lods[0].materials
    }

    /// Per LOD, the distance it draws below ([`select_lod`]'s table).
    pub fn lod_dists(&self) -> Vec<f32> {
        self.lods.iter().map(|l| l.dist).collect()
    }

    /// LOD `i`'s surfaces and materials; 0 is the model's own.
    pub fn lod_mesh(&self, i: usize) -> Option<(&[Surface], &[String])> {
        self.lods
            .get(i)
            .map(|l| (l.surfaces.as_slice(), l.materials.as_slice()))
    }
}

/// The view's LOD inputs, retail's `RE_RenderScene` (CoDMP.exe 0x4e9f..):
/// `scale` is `tan(min(fov_x, fov_y) / 2) / tan(40)` times `r_lodscale`,
/// `bias` is `r_lodbias`. docs/research/xmodel-v14-format.md, "LOD selection".
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LodView {
    pub scale: f32,
    pub bias: f32,
}

impl LodView {
    /// `fov_x`/`fov_y` in degrees, the refdef's.
    pub fn new(fov_x: f32, fov_y: f32, r_lodscale: f32, r_lodbias: f32) -> LodView {
        let fov = fov_x.min(fov_y);
        let scale = (fov.to_radians() * 0.5).tan() / 40f32.to_radians().tan() * r_lodscale;
        LodView {
            scale,
            bias: r_lodbias,
        }
    }

    /// The distance a model's LOD table is read at (0x4ea0c0). `model_scale`
    /// is the refEntity's scale, 0 for an unscaled model.
    pub fn distance(&self, eye: Vec3, origin: Vec3, model_scale: f32) -> f32 {
        let d = self.scale * eye.distance(origin) + self.bias;
        if model_scale != 0.0 {
            d / model_scale
        } else {
            d
        }
    }
}

/// Retail's pick (CoD1 0x48bd70, CoD2 `XModelGetLodForDist`): the first LOD
/// whose distance is 0 or beyond `lod_dist`; `None` past the last, and the
/// model is not drawn.
pub fn select_lod(lod_dists: &[f32], lod_dist: f32) -> Option<usize> {
    lod_dists.iter().position(|&d| d == 0.0 || lod_dist < d)
}

/// One on-disk model format. Every file of a format opens with its `u16`
/// version; each method gets the `xmodel/<name>` descriptor's bytes.
pub trait ModelFormat: Sync {
    fn version(&self) -> u16;
    /// The whole model, every LOD's surfaces included.
    fn load(&self, fs: &dyn AssetSource, descriptor: &[u8]) -> Result<Model>;
    /// The bones alone, hit boxes included, without reading any surface: what
    /// a dedicated server loads.
    fn load_bones(&self, fs: &dyn AssetSource, descriptor: &[u8]) -> Result<Vec<Bone>>;
    /// The descriptor's own `mins`/`maxs`. `None` for a short file.
    fn bounds(&self, descriptor: &[u8]) -> Option<(Vec3, Vec3)>;
}

static FORMATS: RwLock<Vec<&'static dyn ModelFormat>> = RwLock::new(Vec::new());

/// Adds a format [`load`] can dispatch to. v14 is built in; registering a
/// version twice keeps the first.
pub fn register_format(format: &'static dyn ModelFormat) {
    let mut formats = FORMATS.write().unwrap_or_else(|e| e.into_inner());
    if !formats.iter().any(|f| f.version() == format.version()) {
        formats.push(format);
    }
}

fn format_of(descriptor: &[u8]) -> Result<&'static dyn ModelFormat> {
    let [a, b, ..] = *descriptor else {
        bail!("xmodel descriptor too short for a version");
    };
    let version = u16::from_le_bytes([a, b]);
    if version == crate::xmodel::V14.version() {
        return Ok(&crate::xmodel::V14);
    }
    FORMATS
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .copied()
        .find(|f| f.version() == version)
        .ok_or_else(|| anyhow!("unsupported xmodel version {version}"))
}

fn read_descriptor(fs: &dyn AssetSource, name: &str) -> Result<Vec<u8>> {
    let path = format!("xmodel/{name}");
    fs.read(&path)
        .ok_or_else(|| anyhow!("{path} not found in pk3s"))
}

/// A model by its `xmodel/` entry's name (`viewmodel_kar98k`), in whichever
/// registered format its descriptor's version names. Every read goes
/// through [`AssetSource::read`], so a [`crate::pk3::Pk3Fs`] marks the paks
/// it came from. [`load`] builds through [`Asset::find`], not the cached
/// [`crate::asset_source::load`]: the client's and the server's model
/// caches own a model's lifetime, and the source's cache would keep every
/// prop and part of a level resident twice.
impl Asset for Model {
    const KIND: AssetKind = AssetKind::Model;

    fn find(src: &dyn AssetSource, name: &str) -> Result<Self> {
        if let Some(m) = src.decoded_model(name) {
            return m;
        }
        let desc = read_descriptor(src, name)?;
        format_of(&desc)?.load(src, &desc)
    }
}

/// [`Model::find`]: `name` is the `xmodel/` entry, e.g. `viewmodel_kar98k`.
pub fn load(fs: &dyn AssetSource, name: &str) -> Result<Model> {
    Model::find(fs, name)
}

/// The model's bones alone, without the surface geometry. The dedicated
/// server wants tag positions and hit boxes and has no use for vertices.
pub fn load_bones(fs: &dyn AssetSource, name: &str) -> Result<Vec<Bone>> {
    if let Some(m) = fs.decoded_model(name) {
        return Ok(m?.bones);
    }
    let desc = read_descriptor(fs, name)?;
    format_of(&desc)?.load_bones(fs, &desc)
}

/// The descriptor's own `mins`/`maxs`, which a `script_model` is filed in
/// the area tree under (docs/research/cod11-combat.md 14.7). `None` for a
/// missing, short or unknown file.
pub fn bounds(fs: &dyn AssetSource, name: &str) -> Option<(Vec3, Vec3)> {
    let desc = fs.read(&format!("xmodel/{name}"))?;
    format_of(&desc).ok()?.bounds(&desc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_lod_takes_the_first_lod_reaching_past_the_distance() {
        let head = [400.0, 800.0, 3000.0];
        assert_eq!(select_lod(&head, 0.0), Some(0));
        assert_eq!(select_lod(&head, 399.9), Some(0));
        assert_eq!(
            select_lod(&head, 400.0),
            Some(1),
            "strictly below the distance"
        );
        assert_eq!(select_lod(&head, 2999.0), Some(2));
        assert_eq!(
            select_lod(&head, 3000.0),
            None,
            "past the last LOD: not drawn"
        );
        let body = [800.0, 1200.0, 0.0];
        assert_eq!(
            select_lod(&body, 1e9),
            Some(2),
            "a 0 distance never runs out"
        );
        assert_eq!(select_lod(&[0.0], 1e9), Some(0));
        assert_eq!(
            select_lod(&[600.0], -50.0),
            Some(0),
            "a bias can go negative"
        );
    }

    #[test]
    fn lod_view_scales_by_the_narrower_fov_against_80_degrees() {
        // 80 across a square view reads the table at the true distance
        let v = LodView::new(80.0, 80.0, 1.0, 0.0);
        assert!((v.scale - 1.0).abs() < 1e-6);
        // cg_fov 80 across 4:3 is 64.38 tall, which reads the table nearer
        let fov_y = 2.0 * ((40f32.to_radians().tan() * 0.75).atan()).to_degrees();
        let v = LodView::new(80.0, fov_y, 1.0, 0.0);
        assert!((v.scale - 0.75).abs() < 1e-5, "{}", v.scale);
        // a zoomed scope reads it nearer still; Model Detail multiplies
        let zoomed = LodView::new(20.0, 15.0, 1.0, 0.0);
        assert!(zoomed.scale < 0.2);
        let aggressive = LodView::new(80.0, 80.0, 4.0, -200.0);
        assert_eq!(aggressive.bias, -200.0);
        let eye = Vec3::ZERO;
        let at = Vec3::new(300.0, 400.0, 0.0);
        assert!((aggressive.distance(eye, at, 0.0) - 1800.0).abs() < 1e-3);
        assert!((aggressive.distance(eye, at, 2.0) - 900.0).abs() < 1e-3);
        // `off` keeps every model on LOD 0
        let off = LodView::new(80.0, 64.0, 0.0, 0.0);
        assert_eq!(
            select_lod(&[150.0, 500.0, 1000.0], off.distance(eye, at * 100.0, 0.0)),
            Some(0)
        );
    }

    /// A version no registered format reads is an error naming it, not a
    /// misparse.
    #[test]
    fn dispatch_refuses_an_unknown_version() {
        let err = format_of(&99u16.to_le_bytes()).err().unwrap().to_string();
        assert!(err.contains("99"), "{err}");
        assert!(format_of(&[14]).is_err());
        assert_eq!(format_of(&14u16.to_le_bytes()).unwrap().version(), 14);
    }
}
