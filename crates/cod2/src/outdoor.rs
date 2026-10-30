//! The outdoor map (`$outdoor`): the height of the first solid under the
//! sky over a 512 x 512 grid of the world's surface bounds, which
//! `particle_cloud_outdoor` tests a snowflake against, so snow stops at
//! roofs. `R_RegisterOutdoorImage` (Mac 0x00108248) and
//! `R_GenerateOutdoorImage` (Mac 0x00108460); docs/research/cod2-materials.md
//! 9.14.

use crate::collision::ClipMap;
use crate::d3dbsp::D3dBsp;
use glam::Vec3;

/// `outdoorMapSize` (Mac 0x32b960): texels across x and y, height levels.
pub const SIZE: [u32; 3] = [512, 512, 256];

/// The downward trace's mask (`CM_BoxTrace(..., 0, 0x2001)`).
pub const MASK: u32 = 0x2001;

/// One map's `$outdoor` image and the box it spans.
#[derive(Clone, Debug, PartialEq)]
pub struct OutdoorMap {
    pub mins: Vec3,
    pub maxs: Vec3,
    /// Row-major, `y * 512 + x`: the floored height level, 0..=255.
    pub heights: Vec<u8>,
}

/// The bounds `R_RegisterOutdoorImage` gathers: every draw surface but a
/// sky one (`gameFlags & 8`, `sky(material)`), an empty axis made 0, a
/// flat one grown half a unit each way.
pub fn bounds(b: &D3dBsp, sky: impl Fn(usize) -> bool) -> (Vec3, Vec3) {
    let points = b
        .surfaces
        .iter()
        .filter(|s| s.material >= 0 && !sky(s.material as usize))
        .flat_map(|s| {
            let first = s.first_vertex.max(0) as usize;
            let verts = b.verts.get(first..first + s.vertex_count as usize);
            verts.into_iter().flatten().map(|v| v.xyz)
        });
    box_of(points)
}

/// [`bounds`] with the sky read off the bsp: surface flag 0x4, which on
/// the stock maps that snow says what the material file's `gameFlags & 8`
/// says. The load takes it ([`crate::map::MapFile::outdoor_bounds`]) and
/// [`OutdoorMap::generate`] traces it through the level's one `ClipMap`.
pub fn sky_bounds(b: &D3dBsp) -> (Vec3, Vec3) {
    bounds(b, |m| {
        b.materials.get(m).is_some_and(|m| m.surface_flags & 4 != 0)
    })
}

/// `ClearBounds`, `ExpandBounds` over `points`, then the register step's
/// fix-ups.
fn box_of(points: impl Iterator<Item = Vec3>) -> (Vec3, Vec3) {
    let (mut lo, mut hi) = points.fold(
        (Vec3::splat(131072.0), Vec3::splat(-131072.0)),
        |(lo, hi), p| (lo.min(p), hi.max(p)),
    );
    for k in 0..3 {
        if lo[k] == 131072.0 {
            lo[k] = 0.0;
            hi[k] = 0.0;
        }
        if hi[k] - lo[k] < 1.0 {
            lo[k] -= 0.5;
            hi[k] += 0.5;
        }
    }
    (lo, hi)
}

impl OutdoorMap {
    /// `R_GenerateOutdoorImage`: a point trace per texel from a unit over
    /// the box to a unit under it, its end's height floored into 0..=255.
    pub fn generate(clip: &ClipMap, mins: Vec3, maxs: Vec3) -> OutdoorMap {
        // The globals the register step leaves: scale, its reciprocal and
        // the offset `-mins * scale`.
        let scale = Self::scale(mins, maxs);
        let inv = 1.0 / scale;
        let offset = -mins * scale;
        let (top, bottom) = (maxs.z + 1.0, mins.z - 1.0);
        let mut heights = vec![0u8; (SIZE[0] * SIZE[1]) as usize];
        for row in 0..SIZE[1] {
            let y = ((row as f32 + 0.5) - offset.y) * inv.y;
            for col in 0..SIZE[0] {
                let x = ((col as f32 + 0.5) - offset.x) * inv.x;
                let tr = clip.trace(
                    Vec3::new(x, y, top),
                    Vec3::new(x, y, bottom),
                    Vec3::ZERO,
                    Vec3::ZERO,
                    MASK,
                );
                let z = top + (bottom - top) * tr.fraction;
                let level = (z * scale.z + offset.z).floor() as i32;
                heights[(row * SIZE[0] + col) as usize] = level.clamp(0, 255) as u8;
            }
        }
        OutdoorMap {
            mins,
            maxs,
            heights,
        }
    }

    /// Texels (levels) per unit on each axis: `(size - 1) / extent`.
    fn scale(mins: Vec3, maxs: Vec3) -> Vec3 {
        let size = Vec3::new(SIZE[0] as f32, SIZE[1] as f32, SIZE[2] as f32);
        (size - 1.0) / (maxs - mins)
    }

    /// `worldOutdoorLookupMatrix` on a world point: the box mapped onto
    /// 0..1 on each axis.
    pub fn lookup(&self, p: Vec3) -> Vec3 {
        (p - self.mins) / (self.maxs - self.mins)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_world_spans_a_unit_round_the_origin() {
        let (lo, hi) = box_of(std::iter::empty());
        assert_eq!((lo, hi), (Vec3::splat(-0.5), Vec3::splat(0.5)));
    }

    /// mp_downtown's map: a height per texel, nothing under the corner.
    #[test]
    fn mp_downtown_is_open_over_its_street() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let data = fs.read("maps/mp/mp_downtown.d3dbsp").unwrap();
        let file = crate::map::MapFile::parse(&data).unwrap();
        let (mins, maxs) = file.outdoor_bounds();
        let map = OutdoorMap::generate(&file.clip().unwrap(), mins, maxs);
        assert_eq!(
            (map.mins, map.maxs),
            (
                Vec3::new(-1857.0, -4864.0, -128.0),
                Vec3::new(5184.0, 3328.0, 1204.0)
            )
        );
        // Streets low, roofs high, the void past the map at level 0.
        let levels: std::collections::HashSet<u8> = map.heights.iter().copied().collect();
        assert!(levels.len() > 200, "{} levels", levels.len());
        assert_eq!(map.heights[0], 0);
    }

    /// On the four maps that snow, a material's sky surface flag (0x4)
    /// says what its file's `gameFlags & 8` says, so [`OutdoorMap::of_bsp`]
    /// reads the bounds off the bsp alone.
    #[test]
    fn the_sky_flag_matches_the_material_files_on_the_snow_maps() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        for map in ["mp_downtown", "mp_harbor", "mp_leningrad", "mp_railyard"] {
            let b = D3dBsp::parse(&fs.read(&format!("maps/mp/{map}.d3dbsp")).unwrap()).unwrap();
            for m in &b.materials {
                let file = fs
                    .read(&format!("materials/{}", m.name))
                    .and_then(|d| crate::material::MaterialFile::parse(&d).ok());
                let game_sky = file.is_some_and(|f| f.game_flags & 8 != 0);
                assert_eq!(m.surface_flags & 4 != 0, game_sky, "{map} {}", m.name);
            }
        }
    }
}
