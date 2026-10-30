//! The map's light grid and the static models' lighting off it, as 1.7's
//! `R_CalcModelLighting` (0x628990) fills the model lighting volume the
//! `lp_*` model techniques sample (`docs/research/cod4-world.md` 3.1,
//! 3.2): the eight grid corners around a point, the run-length rows that
//! find them, the sight traces a `needsTrace` corner waits on, the
//! corners' 56-colour sets blended in fixed point, and the 4 x 4 x 4 block
//! of texels a set fills.

use crate::assets::world::GfxLightGrid;
use anyhow::{Result, ensure};

/// `GfxLightGridEntry`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridEntry {
    pub colors: u16,
    pub primary_light: u8,
    /// Bit `c`: corner `c` of a lookup is used only past a sight trace.
    pub needs_trace: u8,
}

/// `GfxLightGridColors`: 56 RGB samples, the faces of a 4 x 4 x 4 block.
pub type GridColors = [[u8; 3]; 56];

/// A map's light grid as stored.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LightGrid {
    pub has_light_regions: bool,
    pub sun_primary_light: u8,
    pub mins: [u16; 3],
    pub maxs: [u16; 3],
    pub row_axis: usize,
    pub col_axis: usize,
    pub row_data_start: Vec<u16>,
    pub raw_row_data: Vec<u8>,
    pub entries: Vec<GridEntry>,
    pub colors: Vec<GridColors>,
}

/// What a lookup puts in a model's block: the blended colours and the
/// share of the primary light's corners that see it, 0..255
/// (`GfxModelLightingPatch`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelLighting {
    pub colors: GridColors,
    pub primary_weight: u8,
    /// The primary light the lookup settled on (0 none).
    pub primary_light: u8,
}

/// `R_IsValidLightGridSample`'s `CM_BoxSightTrace` mask, 0x2001.
pub const SIGHT_MASK: u32 = 0x2001;

/// The grid's first corner sits at -131072 on every axis; a cell is 32
/// units across and 64 high.
const GRID_ORIGIN: f32 = -131072.0;

impl LightGrid {
    /// The walked grid; `None` when a part was not kept.
    pub fn from_world(g: &GfxLightGrid) -> Result<Option<LightGrid>> {
        let (Some(rows), Some(raw), Some(entries), Some(colors)) =
            (&g.row_data_start, &g.raw_row_data, &g.entries, &g.colors)
        else {
            return Ok(None);
        };
        ensure!(
            (g.row_axis, g.col_axis) == (0, 1) || (g.row_axis, g.col_axis) == (1, 0),
            "light grid axes {} {}",
            g.row_axis,
            g.col_axis
        );
        let entries: Vec<GridEntry> = entries
            .as_chunks::<4>()
            .0
            .iter()
            .map(|e| GridEntry {
                colors: u16::from_le_bytes([e[0], e[1]]),
                primary_light: e[2],
                needs_trace: e[3],
            })
            .collect();
        let colors: Vec<GridColors> = colors
            .as_chunks::<0xA8>()
            .0
            .iter()
            .map(|c| std::array::from_fn(|i| [c[3 * i], c[3 * i + 1], c[3 * i + 2]]))
            .collect();
        for e in &entries {
            ensure!(
                usize::from(e.colors) < colors.len(),
                "light grid colours {} of {}",
                e.colors,
                colors.len()
            );
        }
        Ok(Some(LightGrid {
            has_light_regions: g.has_light_regions,
            sun_primary_light: g.sun_primary_light_index as u8,
            mins: g.mins,
            maxs: g.maxs,
            row_axis: g.row_axis as usize,
            col_axis: g.col_axis as usize,
            row_data_start: rows
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u16::from_le_bytes(*b))
                .collect(),
            raw_row_data: raw.clone(),
            entries,
            colors,
        }))
    }

    /// Walks every row's runs: their columns add up to the row's
    /// `colCount`, a run's entries stay inside the entries, and each row's
    /// entries start where the last row's ended. Returns the entries the
    /// rows cover.
    pub fn check_rows(&self) -> Result<usize> {
        let mut next = None;
        let mut covered = 0usize;
        for (r, &start) in self.row_data_start.iter().enumerate() {
            if start == 0xFFFF {
                continue;
            }
            let at = 4 * usize::from(start);
            let raw = &self.raw_row_data;
            ensure!(at + 12 <= raw.len(), "row {r} at {at} past {}", raw.len());
            let h = &raw[at..at + 12];
            let col_count = usize::from(u16::from_le_bytes([h[2], h[3]]));
            let z_count = u16::from_le_bytes([h[6], h[7]]);
            let first = u32::from_le_bytes([h[8], h[9], h[10], h[11]]) as usize;
            if let Some(n) = next {
                ensure!(first == n, "row {r} starts at entry {first}, not {n}");
            }
            let wide = usize::from(z_count > 255);
            let (mut cols, mut o, mut entries) = (0usize, at + 12, 0usize);
            while cols < col_count {
                ensure!(o + 2 <= raw.len(), "row {r} runs past the data");
                let (c, z) = (usize::from(raw[o]), usize::from(raw[o + 1]));
                ensure!(c > 0, "row {r}: a run of no columns");
                cols += c;
                entries += c * z;
                o += if z != 0 { 3 + wide } else { 2 };
            }
            ensure!(
                cols == col_count,
                "row {r}: runs of {cols} columns for {col_count}"
            );
            ensure!(
                first + entries <= self.entries.len(),
                "row {r}: entries {first}+{entries} of {}",
                self.entries.len()
            );
            next = Some(first + entries);
            covered += entries;
        }
        Ok(covered)
    }

    /// The row the grid stores for `row` along the row axis, if any.
    fn row(&self, row: u32) -> Option<Row<'_>> {
        let ra = self.row_axis;
        let (lo, hi) = (u32::from(self.mins[ra]), u32::from(self.maxs[ra]));
        let index = row.wrapping_sub(lo);
        if index >= (hi + 1).wrapping_sub(lo) {
            return None;
        }
        let start = *self.row_data_start.get(index as usize)?;
        if start == 0xFFFF {
            return None;
        }
        let at = 4 * usize::from(start);
        let head = self.raw_row_data.get(at..at + 12)?;
        let half = |o: usize| u32::from(u16::from_le_bytes([head[o], head[o + 1]]));
        Some(Row {
            col_start: half(0),
            col_count: half(2),
            z_start: half(4),
            z_count: half(6),
            first_entry: u32::from_le_bytes([head[8], head[9], head[10], head[11]]),
            runs: &self.raw_row_data[at + 12..],
        })
    }

    /// 0x6279b0: the entries at the grid point `cell` and the three
    /// beside it on the column axis and up, as [`Quad`]. Every compare is
    /// on `u32`s as 1.7's: a height one under the row's lowest wraps to
    /// `u32::MAX` and reads as no floor.
    fn quad(&self, cell: [u32; 3]) -> Quad {
        let mut q = Quad::default();
        let Some(row) = self.row(cell[self.row_axis]) else {
            return q;
        };
        let col = cell[self.col_axis].wrapping_sub(row.col_start);
        let z = cell[2].wrapping_sub(row.z_start);
        if col.wrapping_add(1) > row.col_count {
            return q;
        }
        if z.wrapping_add(1) > row.z_count {
            q.under_floor = cell[2] < row.z_start;
            return q;
        }
        let first = row.run(0, row.first_entry);
        if col == u32::MAX {
            // the column before the row's first borders only the first run
            q.entries[2..].copy_from_slice(&first.column(0, z));
            q.under_floor = z < first.base;
            return q;
        }
        let mut run = first;
        let mut left = col;
        while left >= run.cols {
            if run.cols == 0 {
                return q;
            }
            left -= run.cols;
            run = row.after(&run);
        }
        if run.z_count != 0 {
            q.under_floor = z < run.base;
        } else {
            // an empty column is under the floor below the top of the
            // next run's
            let next = row.after(&run);
            q.under_floor = next.z_count != 0 && z < next.base + next.z_count;
        }
        q.entries[..2].copy_from_slice(&run.column(left, z));
        if left + 1 < run.cols {
            q.entries[2..].copy_from_slice(&run.column(left + 1, z));
        } else if col + 1 != row.col_count {
            q.entries[2..].copy_from_slice(&row.after(&run).column(0, z));
        }
        q
    }

    /// `R_LightGridLookup` (0x627e90): the grid cell around `p`, its eight
    /// corners' weights, the corners kept, and the primary light they vote
    /// for. A corner under 0.001 of weight is dropped. One whose
    /// `needsTrace` bit is set and that cannot see `p` is kept only until
    /// a corner that sees `p` turns up: that one drops every corner before
    /// it and restarts the vote, and from then on such a corner is
    /// dropped.
    fn lookup(&self, p: [f32; 3], blocked: &dyn Fn([f32; 3], [f32; 3]) -> bool) -> Lookup {
        let (ra, ca) = (self.row_axis, self.col_axis);
        // floored to an int, then shifted arithmetically
        let shift = [5, 5, 6];
        let cell: [u32; 3] =
            std::array::from_fn(|k| ((p[k].floor() as i32 + 0x20000) >> shift[k]) as u32);
        let unit = |k: usize| if k == 2 { 0.015625 } else { 0.03125 };
        let frac = [ra, ca, 2].map(|k| (p[k] - GRID_ORIGIN) * unit(k) - cell[k] as f32);
        let near = |c: usize, bit: usize, f: f32| if c & bit != 0 { f } else { 1.0 - f };
        let weights: [f32; 8] = std::array::from_fn(|c| {
            near(c, ROW_BIT, frac[0]) * (near(c, COL_BIT, frac[1]) * near(c, UP_BIT, frac[2]))
        });
        let mut over = cell;
        over[ra] += 1;
        let quads = [self.quad(cell), self.quad(over)];

        let mut out = Lookup {
            corners: [None; 8],
            primary: 0,
            under_floor: quads.iter().any(|q| q.under_floor),
        };
        let mut held = (0u8, 0.0f32);
        let mut anchored = false;
        for (c, &w) in weights.iter().enumerate() {
            let quad = &quads[usize::from(c & ROW_BIT != 0)];
            let Some(e) = quad.entries[c & (COL_BIT | UP_BIT)].and_then(|i| self.entries.get(i))
            else {
                continue;
            };
            if w < 0.001 {
                continue;
            }
            let sees = e.needs_trace & (1 << c) == 0 || self.sees_corner(cell, c, p, blocked);
            if sees && !anchored {
                anchored = true;
                out.corners = [None; 8];
                held = (e.primary_light, w);
            } else if !sees && anchored {
                continue;
            } else if outvotes((e.primary_light, w), held) {
                held = (e.primary_light, w);
            }
            out.corners[c] = Some((*e, w));
        }
        out.primary = held.0;
        out
    }

    /// `R_IsValidLightGridSample` (0x627d50): whether `p` sees corner `c`
    /// of `cell`, through a point sight trace (`SIGHT_MASK`) from `p` to
    /// the corner moved 0.01 toward `p`.
    fn sees_corner(
        &self,
        cell: [u32; 3],
        c: usize,
        p: [f32; 3],
        blocked: &dyn Fn([f32; 3], [f32; 3]) -> bool,
    ) -> bool {
        let mut step = [0.0f32; 3];
        step[self.row_axis] = if c & ROW_BIT != 0 { 32.0 } else { 0.0 };
        step[self.col_axis] = if c & COL_BIT != 0 { 32.0 } else { 0.0 };
        step[2] = if c & UP_BIT != 0 { 64.0 } else { 0.0 };
        let size = [32.0, 32.0, 64.0];
        let corner: [f32; 3] =
            std::array::from_fn(|k| cell[k] as f32 * size[k] + GRID_ORIGIN + step[k]);
        let to_p: [f32; 3] = std::array::from_fn(|k| p[k] - corner[k]);
        let dir = normalize(to_p);
        let end: [f32; 3] = std::array::from_fn(|k| corner[k] + 0.01 * dir[k]);
        !blocked(p, end)
    }

    /// `R_CalcModelLighting` (0x628990) for a model whose own primary
    /// light is `non_sun_light`, extrapolating where no corner is found (a
    /// static model's mode): the corners' colour sets blended by weight,
    /// and the share of the corners that see the primary light the lookup
    /// voted for. A primary of 255 is the sun; with light regions a vote
    /// for anything but the sun takes the model's own light. A corner on
    /// the light counts as seeing it, one on no light or (under a light)
    /// on 255 as not; 1.7 counts the latter only when 0x6288a0 finds the
    /// light reaches the point, which vcod does not port, so they all
    /// count.
    pub fn lighting_at(
        &self,
        p: [f32; 3],
        non_sun_light: u8,
        blocked: &dyn Fn([f32; 3], [f32; 3]) -> bool,
    ) -> ModelLighting {
        let found = self.lookup(p, blocked);
        let primary = match found.primary {
            255 => self.sun_primary_light,
            v if self.has_light_regions && v != self.sun_primary_light => non_sun_light,
            v => v,
        };
        let mut sets: Vec<(u16, f32)> = Vec::with_capacity(8);
        let (mut total, mut lit, mut unlit) = (0.0f32, 0.0f32, 0.0f32);
        for (e, w) in found.corners.iter().flatten() {
            match e.primary_light {
                l if l == primary => lit += w,
                0 => unlit += w,
                255 if primary != 0 => unlit += w,
                _ => {}
            }
            total += w;
            if let Some(s) = sets.iter_mut().find(|s| s.0 == e.colors) {
                s.1 += w;
            } else {
                sets.push((e.colors, *w));
            }
        }
        if sets.is_empty() {
            return self.extrapolated(found.under_floor);
        }
        let share = match (primary, unlit != 0.0) {
            (0, _) => 0.0,
            (_, true) => lit / (lit + unlit),
            (_, false) if lit != 0.0 => 1.0,
            _ => 0.0,
        };
        let colors = match sets.as_slice() {
            [(only, _)] => self.colors[usize::from(*only)],
            _ => self.blend(&fixed_weights(&sets, 1.0 / total)),
        };
        ModelLighting {
            colors,
            primary_weight: unit_byte(share),
            primary_light: primary,
        }
    }

    /// No corner found: the colour set of entry 1, or of entry 0 under the
    /// floor, with the sun in full; the last set, lit by nothing, when the
    /// grid has no such set (0x628b10-0x628b67, `r_showMissingLightGrid`
    /// off).
    fn extrapolated(&self, under_floor: bool) -> ModelLighting {
        let want = usize::from(!under_floor);
        let (set, light) = if want < self.colors.len() {
            (want, self.sun_primary_light)
        } else {
            (self.colors.len().saturating_sub(1), 0)
        };
        ModelLighting {
            colors: self.colors.get(set).copied().unwrap_or([[0; 3]; 56]),
            primary_weight: unit_byte(1.0),
            primary_light: light,
        }
    }

    /// 0x627810: the colour sets summed in 16-bit fixed point by their
    /// 256ths, each byte rounded back `(sum + 127) >> 8`.
    fn blend(&self, sets: &[(u16, u16)]) -> GridColors {
        let mut sum = [[0u16; 3]; 56];
        for &(set, f) in sets {
            for (acc, rgb) in sum.iter_mut().zip(&self.colors[usize::from(set)]) {
                for (a, &v) in acc.iter_mut().zip(rgb) {
                    *a = a.wrapping_add(f.wrapping_mul(u16::from(v)));
                }
            }
        }
        sum.map(|rgb| rgb.map(|v| ((u32::from(v) + 127) >> 8) as u8))
    }
}

/// Corner bits of a lookup: the row axis, the column axis, up.
const ROW_BIT: usize = 4;
const COL_BIT: usize = 2;
const UP_BIT: usize = 1;

/// A row of the grid: its head and the run bytes after it, (columns, z
/// count, base z), the base two bytes wide when the row's `zCount` passes
/// 255, an empty run (z count 0) two bytes.
struct Row<'a> {
    col_start: u32,
    col_count: u32,
    z_start: u32,
    z_count: u32,
    first_entry: u32,
    runs: &'a [u8],
}

/// A run of columns: `cols` columns of `z_count` entries each, starting at
/// height `base` and entry `first`; `next` is the byte its successor
/// starts at.
#[derive(Clone, Copy, Debug)]
struct Run {
    cols: u32,
    z_count: u32,
    base: u32,
    first: u32,
    next: usize,
}

impl Row<'_> {
    /// The run at byte `at`, its first entry `first`. Bytes past the data
    /// read 0. An empty run's base is read like any other's, off the bytes
    /// that follow it.
    fn run(&self, at: usize, first: u32) -> Run {
        let byte = |o: usize| self.runs.get(at + o).copied().map_or(0, u32::from);
        let wide = self.z_count > 255;
        let z_count = byte(1);
        let base = byte(2) | if wide { byte(3) << 8 } else { 0 };
        let len = match (z_count, wide) {
            (0, _) => 2,
            (_, false) => 3,
            (_, true) => 4,
        };
        Run {
            cols: byte(0),
            z_count,
            base,
            first,
            next: at + len,
        }
    }

    fn after(&self, run: &Run) -> Run {
        self.run(run.next, run.first.wrapping_add(run.cols * run.z_count))
    }
}

impl Run {
    /// Column `col` of the run at heights `z` and `z + 1`, as entry
    /// indices.
    fn column(&self, col: u32, z: u32) -> [Option<usize>; 2] {
        [0, 1].map(|up| {
            let local = z.wrapping_sub(self.base).wrapping_add(up);
            (local < self.z_count).then(|| {
                self.first
                    .wrapping_add(col * self.z_count)
                    .wrapping_add(local) as usize
            })
        })
    }
}

/// What 0x6279b0 finds at a grid point: the entries of its column at `z`
/// and `z + 1`, then of the next column, and whether the point lies under
/// the grid's floor there.
#[derive(Clone, Copy, Debug, Default)]
struct Quad {
    entries: [Option<usize>; 4],
    under_floor: bool,
}

/// A lookup's corners with their weights, by corner bits, and their vote.
struct Lookup {
    corners: [Option<(GridEntry, f32)>; 8],
    primary: u8,
    under_floor: bool,
}

/// Whether a corner on light `cand.0` at weight `cand.1` takes the vote
/// from the one held: anything beats no light, then no light never wins,
/// then anything beats 255, then 255 never wins, then the heavier corner.
fn outvotes(cand: (u8, f32), held: (u8, f32)) -> bool {
    match (held.0, cand.0) {
        (0, _) => true,
        (_, 0) => false,
        (255, _) => true,
        (_, 255) => false,
        _ => cand.1 > held.1,
    }
}

/// 0x4076d0: `v` times the inverse of its length, unchanged when that is
/// not positive.
fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    let inv = if len > 0.0 { 1.0 / len } else { 1.0 };
    v.map(|x| x * inv)
}

/// 0x627790: the sets' weights times `scale` in 256ths, each
/// `(w * scale * 256 + 0.5)` truncated, the first of the largest taking
/// what the rounding left off 256.
fn fixed_weights(sets: &[(u16, f32)], scale: f32) -> Vec<(u16, u16)> {
    let to_fixed = scale * 256.0;
    let mut out: Vec<(u16, u16)> = sets
        .iter()
        .map(|&(set, w)| (set, (w * to_fixed + 0.5) as i32 as u16))
        .collect();
    let total = out.iter().fold(0u16, |t, s| t.wrapping_add(s.1));
    let mut top = 0;
    for (i, s) in out.iter().enumerate() {
        if s.1 > out[top].1 {
            top = i;
        }
    }
    out[top].1 = out[top].1.wrapping_add(256u16.wrapping_sub(total));
    out
}

/// A 0..1 share as a byte, `(v * 255 + 0.5)` truncated (0x6275a0).
fn unit_byte(v: f32) -> u8 {
    (v * 255.0 + 0.5) as u8
}

/// Which of the 56 colours each texel of the 4 x 4 x 4 block holds, by
/// `x + 4 y + 16 z`, as 0x626e10 writes them; the inner texels repeat a
/// neighbouring face's (cod4-world.md 3.2).
pub const BLOCK_COLORS: [u8; 64] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, //
    16, 17, 18, 19, 20, 0, 3, 21, 22, 12, 15, 23, 24, 25, 26, 27, //
    28, 29, 30, 31, 32, 40, 43, 33, 34, 52, 55, 35, 36, 37, 38, 39, //
    40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55,
];

/// A block's 64 texels, R, G, B and the primary weight in alpha, by
/// `x + 4 y + 16 z`.
pub fn block_texels(l: &ModelLighting) -> [[u8; 4]; 64] {
    std::array::from_fn(|t| {
        let [r, g, b] = l.colors[usize::from(BLOCK_COLORS[t])];
        [r, g, b, l.primary_weight]
    })
}

/// A ground-lit static model's block: every texel the packed colour
/// (`R_SetModelGroundLighting`), stored B, G, R, A as the volume's
/// A8R8G8B8 memory holds it.
pub fn ground_texels(packed: [u8; 4]) -> [[u8; 4]; 64] {
    [[packed[2], packed[1], packed[0], packed[3]]; 64]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 64 texels name every one of the 56 colours, the 56 faces' own
    /// once each in order of first use.
    #[test]
    fn block_layout_covers_the_56_colours() {
        let mut seen = [false; 56];
        for &c in &BLOCK_COLORS {
            seen[usize::from(c)] = true;
        }
        assert!(seen.iter().all(|&s| s));
        // the eight inner texels repeat a face's
        for (x, y, z) in [(1, 1, 1), (2, 1, 1), (1, 2, 1), (2, 2, 1)] {
            let t = x + 4 * y + 16 * z;
            assert!(BLOCK_COLORS[t] < 16, "{t}");
        }
    }

    /// Three samples at weights 0.5, 0.3 and 0.2 blend in 256ths as the
    /// fixed point does: 128, 77 and 51 sum to 256.
    #[test]
    fn blend_rounds_in_fixed_point() {
        let g = LightGrid {
            colors: vec![[[255, 0, 10]; 56], [[0, 255, 10]; 56], [[0, 0, 200]; 56]],
            ..Default::default()
        };
        let c = g.blend(&fixed_weights(&[(0, 0.5), (1, 0.3), (2, 0.2)], 1.0));
        // (255 * 128 + 127) >> 8, (255 * 77 + 127) >> 8,
        // (10 * 128 + 10 * 77 + 200 * 51 + 127) >> 8
        assert_eq!(c[0], [127, 77, 48]);
    }
}
