//! The light grid models are lit from: the lookup at a point
//! (`RB_GetLightingAtPoint`, Mac 0xeb5e4) and the eight-corner probe basis
//! the `lightprobe` techniques weight its colours with
//! (docs/research/cod2-d3dbsp-format.md, lumps 2 and 3;
//! docs/research/cod2-materials.md 9.2).

use std::collections::HashMap;

use glam::Vec3;
use vcod_common::mapworld::{LightGrid, LightGridEntry};

/// The grid's corner (0, 0, 0) sits at -131072 on every axis (Mac 0x326240).
const GRID_ORIGIN: f32 = 131072.0;
/// A total corner weight under this is renormalized (Mac 0x3262c8).
const FULL_WEIGHT: f32 = 0.98;
/// An occluded corner's sight trace ends this far off the corner toward the
/// query point (Mac 0x3262f0).
const TRACE_PULL: f32 = 0.01;
/// The sight trace's mask: solid and clipshot.
pub const SIGHT_MASK: u32 = 0x2001;

/// The probe basis' frame (`lightgridLookupMatrix`, Mac 0x32b7e0 and
/// `lib/lightprobe_sm.hlsl`): a world vector `v` turns to
/// `v.x * LOOKUP[0] + v.y * LOOKUP[1] + v.z * LOOKUP[2]`, which puts world
/// up on the cube's (1, 1, 1) corner.
pub const LOOKUP: [[f32; 3]; 3] = [
    [0.816_496_6, -0.408_248_28, -0.408_248_28],
    [0.0, 0.707_106_77, -0.707_106_77],
    [0.577_350_26, 0.577_350_26, 0.577_350_26],
];

/// One lookup's result: per channel the eight corner colours, corner `j`
/// at the cube's (x, y, z) = bits 0, 1, 2 of `j` (set is +1), and the sun's
/// visibility, the weight of the grid corners whose sun bit is set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridSample {
    pub rgb: [[f32; 8]; 3],
    pub sun: f32,
}

impl GridSample {
    /// The table a query takes when the grid is empty: corner `j` coloured
    /// by its own bits, no sun.
    pub const MISSING: GridSample = GridSample {
        rgb: [
            [0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0],
            [0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0],
            [0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
        ],
        sun: 0.0,
    };

    /// Corner `j`'s colour.
    pub fn corner(&self, j: usize) -> Vec3 {
        Vec3::new(self.rgb[0][j], self.rgb[1][j], self.rgb[2][j])
    }

    /// The probe's light for basis weights `w` (`ProbeLighting` in
    /// `lib/lightprobe.hlsl`).
    pub fn light(&self, w: &[f32; 8]) -> Vec3 {
        (0..8).map(|j| self.corner(j) * w[j]).sum()
    }

    /// `R_ScaleStaticModelLighting` (Mac 0x106970): `directLightScale` on
    /// the sun, `indirectLightScale` on the colours.
    pub fn scaled(mut self, direct: f32, indirect: f32) -> Self {
        self.sun *= direct;
        for c in self.rgb.iter_mut().flatten() {
            *c *= indirect;
        }
        self
    }

    /// `R_GetStaticModelLightingFromGround` (Mac 0x107070): every corner the
    /// ground colour's rgb, its alpha the sun.
    pub fn ground(rgba: [f32; 4]) -> Self {
        GridSample {
            rgb: [[rgba[0]; 8], [rgba[1]; 8], [rgba[2]; 8]],
            sun: rgba[3],
        }
    }

    /// As the static model cache stores it: every value a byte, so clamped
    /// to [0, 1] (`R_CacheStaticModelLighting`, Mac 0x107512).
    pub fn clamped(mut self) -> Self {
        for c in self.rgb.iter_mut().flatten() {
            *c = c.clamp(0.0, 1.0);
        }
        self.sun = self.sun.clamp(0.0, 1.0);
        self
    }
}

/// The entry for grid corner (x, y, z): the search key and the low byte
/// with the sun bit clear.
fn corner_key(x: i32, y: i32, z: i32) -> (u32, u8) {
    let key = ((x as u32 & !3) << 19) | ((y as u32 & !3) << 8) | (z >> 2) as u32;
    let low = ((x << 6) | ((y & 3) << 4) | ((z & 3) << 2)) as u8;
    (key, low)
}

/// The binary search on `(key, low & 0xfe)`: the match, or the index the
/// search's upper bound ended on, which the nearest-corner fallback reads.
fn find(entries: &[LightGridEntry], x: i32, y: i32, z: i32) -> Result<usize, isize> {
    let want = corner_key(x, y, z);
    let (mut lo, mut hi) = (0isize, entries.len() as isize - 1);
    while lo <= hi {
        let mid = (lo + hi) >> 1;
        let e = &entries[mid as usize];
        match (e.key, e.low & 0xfe).cmp(&want) {
            std::cmp::Ordering::Equal => return Ok(mid as usize),
            std::cmp::Ordering::Greater => hi = mid - 1,
            std::cmp::Ordering::Less => lo = mid + 1,
        }
    }
    Err(hi)
}

/// Grid corner (x, y, z) in the world: 32 units apart in x and y, 64 in z.
pub fn corner_point(x: i32, y: i32, z: i32) -> Vec3 {
    Vec3::new(
        (x * 32 - 0x20000) as f32,
        (y * 32 - 0x20000) as f32,
        (z * 64 - 0x20000) as f32,
    )
}

fn add_colour(out: &mut GridSample, grid: &LightGrid, e: &LightGridEntry, w: f32) {
    let c = &grid.colours[e.colour as usize];
    let k = w * (1.0 / 255.0);
    for (o, v) in out.rgb.iter_mut().zip(&c.rgb) {
        for (o, &b) in o.iter_mut().zip(v) {
            *o += f32::from(b) * k;
        }
    }
}

/// `RB_GetLightingAtPoint`: the eight corners of `p`'s cell weighted
/// trilinearly, a corner skipped when it is missing or when its occlusion
/// bit for this cell corner is set and `blocked(p, toward the corner)`
/// says the sight line is cut. A total weight under 0.98 is renormalized;
/// with none, the nearest corner alone answers. `blocked` `None` treats
/// every sight line as clear.
pub fn sample(
    grid: &LightGrid,
    p: Vec3,
    blocked: Option<&dyn Fn(Vec3, Vec3) -> bool>,
) -> GridSample {
    let gx = p.x + GRID_ORIGIN;
    let gy = p.y + GRID_ORIGIN;
    let gz = GRID_ORIGIN + p.z;
    let (ix, iy, iz) = ((gx as i32) >> 5, (gy as i32) >> 5, (gz as i32) >> 6);
    let fx = gx * (1.0 / 32.0) - ix as f32;
    let fy = (1.0 / 32.0) * gy - iy as f32;
    let fz = gz * (1.0 / 64.0) - iz as f32;
    let wx = [1.0 - fx, fx];
    let wy = [1.0 - fy, fy];
    let wz = [1.0 - fz, fz];

    let mut out = GridSample {
        rgb: [[0.0; 8]; 3],
        sun: 0.0,
    };
    let mut total = 0.0f32;
    let mut sun = 0.0f32;
    for c in 0..8 {
        let (bx, by, bz) = (c & 1, (c >> 1) & 1, (c >> 2) & 1);
        let (x, y, z) = (ix + bx as i32, iy + by as i32, iz + bz as i32);
        let Ok(i) = find(&grid.entries, x, y, z) else {
            continue;
        };
        let e = &grid.entries[i];
        if (e.occlusion_mask >> c) & 1 != 0
            && let Some(blocked) = blocked
        {
            let corner = corner_point(x, y, z);
            let to = (p - corner).normalize_or_zero() * TRACE_PULL + corner;
            if blocked(p, to) {
                continue;
            }
        }
        let w = wx[bx] * wy[by] * wz[bz];
        total += w;
        add_colour(&mut out, grid, e, w);
        if e.low & 1 != 0 {
            sun += w;
        }
    }
    if total >= FULL_WEIGHT {
        out.sun = sun;
        return out;
    }
    if total != 0.0 {
        let inv = 1.0 / total;
        for v in out.rgb.iter_mut().flatten() {
            *v *= inv;
        }
        out.sun = sun * inv;
        return out;
    }
    if grid.entries.is_empty() {
        return GridSample::MISSING;
    }
    let near = |f: f32| i32::from(0.5 < f);
    let i = match find(&grid.entries, ix + near(fx), iy + near(fy), iz + near(fz)) {
        Ok(i) => i,
        // Retail reads the slot the search stopped on, which can be one
        // before the first; vcod keeps it in range.
        Err(hi) => hi.max(0) as usize,
    };
    let e = &grid.entries[i];
    add_colour(&mut out, grid, e, 1.0);
    out.sun = if e.low & 1 != 0 { 1.0 } else { 0.0 };
    out
}

/// `v` in the probe basis' frame.
pub fn to_basis(v: Vec3) -> Vec3 {
    let [r0, r1, r2] = LOOKUP.map(Vec3::from_array);
    v.x * r0 + v.y * r1 + v.z * r2
}

/// Corner `j`'s direction in the world (unit length).
pub fn corner_dir(j: usize) -> Vec3 {
    let s = |b: usize| if (j >> b) & 1 != 0 { 1.0 } else { -1.0 };
    let c = Vec3::new(s(0), s(1), s(2)) / 3f32.sqrt();
    // LOOKUP is orthonormal: its transpose takes the basis frame back
    let [r0, r1, r2] = LOOKUP.map(Vec3::from_array);
    Vec3::new(r0.dot(c), r1.dot(c), r2.dot(c))
}

/// The eight corner weights for a normal: the normal turned into the basis
/// frame, pushed onto the cube's face along its longest axis, each axis
/// mapped to [0, 1] and, for `smooth`, eased by `3t^2 - 2t^3`.
fn weights(n: Vec3, smooth: bool) -> [f32; 8] {
    let r = to_basis(n);
    let longest = r.abs().max_element();
    if longest == 0.0 {
        return [0.125; 8];
    }
    let ease = |v: f32| {
        let t = v / longest * 0.5 + 0.5;
        if smooth { t * t * (t * -2.0 + 3.0) } else { t }
    };
    let f = [ease(r.x), ease(r.y), ease(r.z)];
    std::array::from_fn(|j| {
        (0..3)
            .map(|a| if (j >> a) & 1 != 0 { f[a] } else { 1.0 - f[a] })
            .product()
    })
}

/// The weights an entity's probe takes: `$lightgridweights0` (corners 0-3,
/// cube z -1) and `$lightgridweights1` (4-7), the cube maps
/// `Image_GetLightGridWeightsForVector` (Mac 0xfc8d4) fills, sampled at
/// the world normal.
pub fn entity_weights(n: Vec3) -> [f32; 8] {
    weights(n, true)
}

/// `$lightgridweights0` and `1`'s face size (`Image_Load`, Mac 0xfd52a).
pub const WEIGHT_CUBE_SIZE: usize = 32;

/// One texel of `$lightgridweights<image>` for the direction `dir`, in
/// A8R8G8B8's memory order (B, G, R, A): `Image_GetLightGridWeightsForVector`
/// (Mac 0xfc8d4). B, G, R, A read as the shader's x, y, z, w hold corners
/// 0-3 (image 0) or 4-7 (image 1), cod2-materials.md 9.2.
pub fn weight_texel(dir: Vec3, image: usize) -> [u8; 4] {
    let r = to_basis(dir);
    let a = r.abs();
    // `Vec3MajorAxis` compares squares; the first of a tie wins
    let mut i = 0;
    for k in 1..3 {
        if a[k] * a[k] > a[i] * a[i] {
            i = k;
        }
    }
    let r = r * (1.0 / a[i]);
    let ease = |t: f32| t * t * (t * -2.0 + 3.0);
    let fx = ease(r.x * 0.5 + 0.5);
    let fy = ease(r.y * 0.5 + 0.5);
    let fz = ease(r.z * if image == 0 { -0.5 } else { 0.5 } + 0.5);
    let gyz = (1.0 - fy) * fz;
    let gx = 1.0 - fx;
    let yz = fz * fy;
    let q = |w: f32| (w * 255.0 + 0.5).floor().clamp(0.0, 255.0) as u8;
    [q(gx * yz), q(fx * gyz), q(gyz * gx), q(fx * yz)]
}

/// Both weight cubes as `Image_GenerateCubemapFunction` (Mac 0xfcda6)
/// fills them: faces +x, -x, +y, -y, +z, -z, each `WEIGHT_CUBE_SIZE`
/// rows of texels in [`weight_texel`]'s byte order, a texel's direction its
/// centre on the face (D3D's face axes, `faceAxis` Mac 0x32b820).
pub fn weight_cubes() -> [Vec<u8>; 2] {
    // per face: the major axis, then the u and v axes, each `axis << 1 |
    // negative`
    const FACES: [[u32; 3]; 6] = [
        [0, 5, 3],
        [1, 4, 3],
        [2, 0, 4],
        [3, 0, 5],
        [4, 0, 3],
        [5, 1, 3],
    ];
    let n = WEIGHT_CUBE_SIZE;
    let step = 1.0 / n as f32;
    std::array::from_fn(|image| {
        let mut out = Vec::with_capacity(6 * n * n * 4);
        for face in FACES {
            let mut base = Vec3::ZERO;
            base[(face[0] >> 1) as usize] = if face[0] & 1 == 0 { 1.0 } else { -1.0 };
            let mut axis = |code: u32| {
                let k = (code >> 1) as usize;
                let (start, dir) = if code & 1 == 0 {
                    (-1.0, 2.0)
                } else {
                    (1.0, -2.0)
                };
                base[k] = start;
                let mut d = Vec3::ZERO;
                d[k] = dir * step;
                d
            };
            let (du, dv) = (axis(face[1]), axis(face[2]));
            for y in 0..n {
                let mut p = (y as f32 + 0.5) * dv + du * 0.5 + base;
                for _ in 0..n {
                    out.extend_from_slice(&weight_texel(p, image));
                    p += du;
                }
            }
        }
        out
    })
}

/// [`entity_weights`] as retail's pixel reads them: both cubes of
/// [`weight_cubes`] sampled bilinearly at `n` on the face its longest axis
/// picks, edges clamped (D3D9 filters within a face).
pub fn sampled_entity_weights(cubes: &[Vec<u8>; 2], n: Vec3) -> [f32; 8] {
    let a = n.abs();
    let size = WEIGHT_CUBE_SIZE;
    // D3D's face selection and (s, t) per face
    let (face, s, t, m) = if a.x >= a.y && a.x >= a.z {
        if n.x >= 0.0 {
            (0, -n.z, -n.y, a.x)
        } else {
            (1, n.z, -n.y, a.x)
        }
    } else if a.y >= a.z {
        if n.y >= 0.0 {
            (2, n.x, n.z, a.y)
        } else {
            (3, n.x, -n.z, a.y)
        }
    } else if n.z >= 0.0 {
        (4, n.x, -n.y, a.z)
    } else {
        (5, -n.x, -n.y, a.z)
    };
    let x = (s / m * 0.5 + 0.5) * size as f32 - 0.5;
    let y = (t / m * 0.5 + 0.5) * size as f32 - 0.5;
    let (x0, y0) = (x.floor(), y.floor());
    let (wx, wy) = (x - x0, y - y0);
    let texel = |img: usize, x: f32, y: f32, c: usize| {
        let x = (x as i32).clamp(0, size as i32 - 1) as usize;
        let y = (y as i32).clamp(0, size as i32 - 1) as usize;
        f32::from(cubes[img][((face * size + y) * size + x) * 4 + c]) / 255.0
    };
    std::array::from_fn(|j| {
        // corner j's channel: B, G, R, A hold x, y, z, w
        let c = [2, 1, 0, 3][j & 3];
        let img = j >> 2;
        let at = |dx: f32, dy: f32| texel(img, x0 + dx, y0 + dy, c);
        (at(0.0, 0.0) * (1.0 - wx) + at(1.0, 0.0) * wx) * (1.0 - wy)
            + (at(0.0, 1.0) * (1.0 - wx) + at(1.0, 1.0) * wx) * wy
    })
}

/// The weights a static model's probe takes: its 2 x 2 x 2 block of the
/// `*smodel_lighting` 3D texture read trilinearly at the normal pushed onto
/// the cube (`lib/lightprobe_sm.hlsl`; the lookup scale is half a texel,
/// `R_FinishStaticModelLightingCache` Mac 0x106f82), so linear, no easing.
pub fn static_weights(n: Vec3) -> [f32; 8] {
    weights(n, false)
}

/// A `misc_model`'s lighting keys, read by `R_LoadEntities` (Mac 0xe2b6c)
/// without case.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StaticModelKeys {
    /// `gndLt`, `RRGGBBSS` hex over 255.
    pub ground: Option<[f32; 4]>,
    /// `directLightScale`, 1 without.
    pub direct: f32,
    /// `indirectLightScale`, 1 without.
    pub indirect: f32,
}

impl StaticModelKeys {
    pub fn of(block: &HashMap<String, String>) -> Self {
        let get = |key: &str| {
            block
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v.trim())
        };
        let ground = get("gndLt").and_then(|v| {
            let b = |i: usize| u8::from_str_radix(v.get(i..i + 2)?, 16).ok();
            Some([b(0)?, b(2)?, b(4)?, b(6)?].map(|x| f32::from(x) / 255.0))
        });
        let scale = |key| get(key).and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.0);
        StaticModelKeys {
            ground,
            direct: scale("directLightScale"),
            indirect: scale("indirectLightScale"),
        }
    }
}

/// What a static model is lit with, as `R_LoadEntities` caches it: the
/// ground colour when its xmodel's flag bit 0 is set and it has `gndLt`,
/// else the grid at its bounds' centre (`R_GetStaticModelLightingFromGrid`,
/// Mac 0x1069cc); scaled by its two keys and stored as bytes.
pub fn static_model(
    grid: &LightGrid,
    centre: Vec3,
    ground_lit: bool,
    keys: &StaticModelKeys,
    blocked: Option<&dyn Fn(Vec3, Vec3) -> bool>,
) -> GridSample {
    let s = match keys.ground {
        Some(g) if ground_lit => GridSample::ground(g),
        _ => sample(grid, centre, blocked),
    };
    s.scaled(keys.direct, keys.indirect).clamped()
}

/// Where an entity samples the grid (`RB_SetupEntityLighting`, Mac
/// 0xec126): its lighting origin when the cgame set one (flag 0x80, a
/// player's or a turret's), else 4 units over its origin.
pub fn entity_point(origin: Vec3) -> Vec3 {
    origin + Vec3::Z * 4.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::mapworld::LightGridColour;

    fn entry(x: i32, y: i32, z: i32, sun: bool, occlusion_mask: u8, colour: u16) -> LightGridEntry {
        let (key, low) = corner_key(x, y, z);
        LightGridEntry {
            key,
            low: low | u8::from(sun),
            occlusion_mask,
            colour,
        }
    }

    fn grid(mut entries: Vec<LightGridEntry>, colours: Vec<[[u8; 8]; 3]>) -> LightGrid {
        entries.sort_by_key(|e| (e.key, e.low & 0xfe));
        LightGrid {
            entries,
            colours: colours
                .into_iter()
                .map(|rgb| LightGridColour { rgb })
                .collect(),
        }
    }

    /// The grid cell holding world point (0, 0, 0): corner (4096, 4096,
    /// 2048).
    const C: (i32, i32, i32) = (4096, 4096, 2048);

    /// A hand-built record: the key and low byte pack the corner as lump 2
    /// stores it, and a point a quarter of the way into the cell weights
    /// its eight corners trilinearly.
    #[test]
    fn keys_pack_and_corners_weight_trilinearly() {
        // corner (4096 + 1, 4096 + 2, 2048 + 3): key from the high bits,
        // low from the low two of each
        let (k, l) = corner_key(4097, 4098, 2051);
        assert_eq!(k, (1024 << 21) | (1024 << 10) | 512);
        assert_eq!(l, (1 << 6) | (2 << 4) | (3 << 2));
        assert_eq!(corner_point(C.0, C.1, C.2), Vec3::ZERO);

        // corner 0 black, the other seven a full red; the sun on corner 7
        let mut es = Vec::new();
        for c in 0..8 {
            let (x, y, z) = (C.0 + (c & 1), C.1 + ((c >> 1) & 1), C.2 + ((c >> 2) & 1));
            es.push(entry(x, y, z, c == 7, 0, u16::from(c != 0)));
        }
        let mut red = [[0u8; 8]; 3];
        red[0] = [255; 8];
        let g = grid(es, vec![[[0; 8]; 3], red]);
        let p = Vec3::new(8.0, 8.0, 16.0);
        let s = sample(&g, p, None);
        // corner 0 weighs 0.75^3; the rest share the remainder
        let w0 = 0.75f32.powi(3);
        assert!((s.rgb[0][0] - (1.0 - w0)).abs() < 1e-5, "{:?}", s.rgb[0]);
        assert_eq!(s.rgb[1], [0.0; 8]);
        assert!((s.sun - 0.25f32.powi(3)).abs() < 1e-6);
    }

    /// A missing corner leaves its weight out and the rest renormalize; an
    /// occluded one is skipped only when the sight line is cut; with no
    /// corner at all the nearest one answers alone.
    #[test]
    fn missing_and_occluded_corners() {
        let mut white = [[0u8; 8]; 3];
        white[1] = [102; 8];
        // only corners 0 and 1 present, corner 1 occluded for query corner 1
        let g = grid(
            vec![
                entry(C.0, C.1, C.2, true, 0, 0),
                entry(C.0 + 1, C.1, C.2, false, 0b10, 0),
            ],
            vec![white],
        );
        let p = Vec3::new(16.0, 0.0, 0.0);
        let s = sample(&g, p, None);
        assert!((s.rgb[1][3] - 0.4).abs() < 1e-6);
        // half the weight is on corner 0, the sunlit one
        assert!((s.sun - 0.5).abs() < 1e-6);
        let cut = |_: Vec3, _: Vec3| true;
        let s = sample(&g, p, Some(&cut));
        assert_eq!(s.sun, 1.0);
        // the trace runs from the point to just inside the corner
        let seen = std::cell::Cell::new(None);
        let probe = |a: Vec3, b: Vec3| {
            seen.set(Some((a, b)));
            false
        };
        sample(&g, p, Some(&probe));
        let (a, b) = seen.get().unwrap();
        assert_eq!(a, p);
        assert!((b - Vec3::new(31.99, 0.0, 0.0)).length() < 1e-4);

        // far from both: the nearest corner's search misses and reads the
        // slot it stopped on
        let far = sample(&g, Vec3::new(1000.0, 1000.0, 1000.0), None);
        assert!((far.rgb[1][0] - 0.4).abs() < 1e-6);
        assert_eq!(sample(&LightGrid::default(), p, None), GridSample::MISSING);
    }

    /// The basis: world up is corner 7, down corner 0; a normal along a
    /// corner's direction takes that corner alone, and the weights always
    /// sum to 1.
    #[test]
    fn basis_corners_and_weights() {
        assert!((corner_dir(7) - Vec3::Z).length() < 1e-5);
        assert!((corner_dir(0) + Vec3::Z).length() < 1e-5);
        for j in [1, 2, 4] {
            assert!((corner_dir(j).z + 1.0 / 3.0).abs() < 1e-5, "{j}");
        }
        for j in 0..8 {
            let w = entity_weights(corner_dir(j));
            assert!((w[j] - 1.0).abs() < 1e-4, "{j} {w:?}");
            let w = static_weights(corner_dir(j));
            assert!((w[j] - 1.0).abs() < 1e-4, "{j} {w:?}");
        }
        for n in [Vec3::X, Vec3::new(0.3, -0.5, 0.8).normalize(), -Vec3::Y] {
            for w in [entity_weights(n), static_weights(n)] {
                assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-5);
                assert!(w.iter().all(|&x| x >= 0.0));
            }
        }
        // a level normal leans on the lower corner nearest it: +x is
        // corner 1's side, and the down corner takes nothing
        let w = entity_weights(Vec3::X);
        let top = (0..8).max_by(|&a, &b| w[a].total_cmp(&w[b])).unwrap();
        assert_eq!(top, 1, "{w:?}");
        assert!(w[0] < 1e-6, "{w:?}");
    }

    /// The weight cubes hold the eased weights in bytes: a texel is
    /// `entity_weights` at its direction rounded to 1/255, and the cubes
    /// read bilinearly stay within four levels of the eased weights, under
    /// half a level on average: the filter's error on the ease, most where
    /// the basis cube's faces meet (a crease the bilinear read rounds).
    #[test]
    fn weight_cubes_hold_the_eased_weights() {
        let cubes = weight_cubes();
        assert_eq!(cubes[0].len(), 6 * WEIGHT_CUBE_SIZE * WEIGHT_CUBE_SIZE * 4);
        for n in [Vec3::X, Vec3::new(0.3, -0.5, 0.8), -Vec3::Y, corner_dir(7)] {
            let w = entity_weights(n.normalize());
            for img in 0..2 {
                let t = weight_texel(n, img);
                for (c, j) in [2, 1, 0, 3].into_iter().zip(0..4) {
                    let want = (w[img * 4 + j] * 255.0).round();
                    assert!(
                        (f32::from(t[c]) - want).abs() <= 1.0,
                        "{n} {img} {t:?} {w:?}"
                    );
                }
            }
        }
        let (mut worst, mut sum) = (0.0f32, 0.0f32);
        for i in 0..4000 {
            let a = i as f32 * 2.399_963;
            let z = 1.0 - (i as f32 + 0.5) / 2000.0;
            let r = (1.0 - z * z).max(0.0).sqrt();
            let n = Vec3::new(r * a.cos(), r * a.sin(), z);
            let (got, want) = (sampled_entity_weights(&cubes, n), entity_weights(n));
            for j in 0..8 {
                worst = worst.max((got[j] - want[j]).abs());
                sum += (got[j] - want[j]).abs();
            }
        }
        let mean = sum / 32000.0 * 255.0;
        eprintln!("worst {} levels, mean {mean}", worst * 255.0);
        assert!(
            worst * 255.0 < 4.0 && mean < 0.5,
            "{} {mean}",
            worst * 255.0
        );
    }

    #[test]
    fn static_model_keys_and_ground() {
        let block: HashMap<String, String> = [
            ("gndlt", "ff800040"),
            ("directLightScale", "0.5"),
            ("INDIRECTLIGHTSCALE", "2"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let keys = StaticModelKeys::of(&block);
        assert_eq!(keys.ground, Some([1.0, 128.0 / 255.0, 0.0, 64.0 / 255.0]));
        assert_eq!((keys.direct, keys.indirect), (0.5, 2.0));
        let g = LightGrid::default();
        let s = static_model(&g, Vec3::ZERO, true, &keys, None);
        // the indirect scale doubles the colours, the cache clamps them
        assert_eq!(s.rgb[0], [1.0; 8]);
        assert!((s.rgb[1][0] - 1.0).abs() < 1e-6);
        assert!((s.sun - 32.0 / 255.0).abs() < 1e-6);
        // without the flag the grid answers (here the missing table)
        let s = static_model(&g, Vec3::ZERO, false, &keys, None);
        assert_eq!(s.rgb[2], [0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0]);
    }
}
