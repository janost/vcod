//! The blocks a BSP's lightmap pages pack, for `vc_bicubic`'s filter: per
//! texel, how far its block runs left, up, right and down, so a 4x4
//! footprint clamps to its own block instead of reading a neighbour.
//! Blocks sit flush on the page with no gutter, so plain bilinear already
//! bleeds half a texel at their seams; a cubic kernel would reach one more.

use std::collections::HashMap;

use vcod_common::mapworld;
use vcod_common::mapworld::MapWorld;

const SIZE: usize = mapworld::LIGHTMAP_SIZE;
const NONE: u32 = u32::MAX;

/// A lightmap page and its block runs ([`edges`]), material bindings 1 and 4.
#[derive(Clone)]
pub struct LightmapPage {
    pub view: wgpu::TextureView,
    pub edges: wgpu::TextureView,
}

/// A square `Rgba8Uint` page of [`edges`] texels.
pub fn upload_edges(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    size: u32,
    px: &[[u8; 4]],
) -> wgpu::TextureView {
    crate::renderer::upload_texel4(
        device,
        queue,
        label,
        size,
        size,
        wgpu::TextureFormat::Rgba8Uint,
        px.as_flattened(),
    )
}

/// Per page, `SIZE * SIZE` texels of `[left, up, right, down]`: how many
/// texels of the same block follow this one along its row and column on
/// each side, capped at 255. A texel no triangle maps reads all zero.
pub fn edges(bsp: &MapWorld) -> Vec<Vec<[u8; 4]>> {
    // pages are independent: one a core
    vcod_common::laps::par_map(&page_tris(bsp), |page| page_edges(&labels(page)))
}

/// The triangles mapped onto one page.
#[derive(Default)]
struct PageTris {
    /// Each triangle's texel rect ([`texel_rect`]).
    rects: Vec<[u16; 4]>,
    /// Triangle pairs that share a vertex: the same position at the same
    /// lightmap coordinate, so one continuous chart.
    links: Vec<(u32, u32)>,
}

/// A block is a chart of triangles joined through shared vertices, plus
/// any chart whose texels overlap it. The vertex links matter where a
/// chart's coordinates sit on texel borders: its triangles' rects then
/// meet without sharing a texel, exactly like two unrelated blocks packed
/// flush. Terrain charts are not rectangles, so the runs follow the
/// block's own outline rather than its bounds (a bounding box would
/// swallow the small blocks packed into its concave corners).
fn page_tris(bsp: &MapWorld) -> Vec<PageTris> {
    let mut pages: Vec<PageTris> = Vec::new();
    pages.resize_with(bsp.lightmaps.len(), Default::default);
    let mut seen: Vec<HashMap<[u32; 5], u32>> = vec![HashMap::new(); bsp.lightmaps.len()];
    for s in &bsp.soups {
        let Some(page) = pages.get_mut(s.lightmap as usize) else {
            continue;
        };
        let seen = &mut seen[s.lightmap as usize];
        let first = s.first_vertex as usize;
        let idx = bsp
            .indices
            .get(s.first_index as usize..s.first_index as usize + s.index_count as usize)
            .unwrap_or(&[]);
        for tri in idx.as_chunks::<3>().0 {
            let verts: Option<Vec<&mapworld::DrawVert>> = tri
                .iter()
                .map(|&i| bsp.verts.get(first + i as usize))
                .collect();
            let Some(verts) = verts else {
                continue;
            };
            let id = page.rects.len() as u32;
            let uv: Vec<[f32; 2]> = verts.iter().map(|v| v.lm_uv).collect();
            page.rects.push(texel_rect(&uv));
            for v in verts {
                let key = [
                    v.pos[0].to_bits(),
                    v.pos[1].to_bits(),
                    v.pos[2].to_bits(),
                    v.lm_uv[0].to_bits(),
                    v.lm_uv[1].to_bits(),
                ];
                let other = *seen.entry(key).or_insert(id);
                if other != id {
                    page.links.push((other, id));
                }
            }
        }
    }
    pages
}

/// The texels whose cells a triangle's lightmap coordinates span,
/// inclusive `[x0, y0, x1, y1]`. A coordinate on a texel centre (most of
/// them) keeps that texel; one on a cell border stops before the next.
fn texel_rect(uv: &[[f32; 2]]) -> [u16; 4] {
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for c in uv {
        for a in 0..2 {
            lo[a] = lo[a].min(c[a] * SIZE as f32);
            hi[a] = hi[a].max(c[a] * SIZE as f32);
        }
    }
    let texel = |c: f32| (c.max(0.0) as usize).min(SIZE - 1) as u16;
    let (x0, y0) = (texel(lo[0].floor()), texel(lo[1].floor()));
    [
        x0,
        y0,
        texel(hi[0].ceil() - 1.0).max(x0),
        texel(hi[1].ceil() - 1.0).max(y0),
    ]
}

fn find(parent: &mut [u32], mut i: u32) -> u32 {
    while parent[i as usize] != i {
        parent[i as usize] = parent[parent[i as usize] as usize];
        i = parent[i as usize];
    }
    i
}

/// Paints every triangle's rect into a page of block labels; linked
/// triangles and rects that share a texel get one label.
fn labels(page: &PageTris) -> Vec<u32> {
    let mut grid = vec![NONE; SIZE * SIZE];
    let mut parent: Vec<u32> = (0..page.rects.len() as u32).collect();
    for &(a, b) in &page.links {
        let (a, b) = (find(&mut parent, a), find(&mut parent, b));
        parent[a as usize] = b;
    }
    for (id, r) in page.rects.iter().enumerate() {
        let id = id as u32;
        for y in r[1]..=r[3] {
            for x in r[0]..=r[2] {
                let cell = &mut grid[y as usize * SIZE + x as usize];
                if *cell != NONE {
                    let (a, b) = (find(&mut parent, *cell), find(&mut parent, id));
                    parent[a as usize] = b;
                }
                *cell = id;
            }
        }
    }
    for cell in grid.iter_mut().filter(|c| **c != NONE) {
        *cell = find(&mut parent, *cell);
    }
    grid
}

fn page_edges(grid: &[u32]) -> Vec<[u8; 4]> {
    let mut px = vec![[0u8; 4]; SIZE * SIZE];
    // rows fill slots 0 and 2, columns 1 and 3; step k along line l
    for column in [false, true] {
        let index = |k: usize, l: usize| if column { k * SIZE + l } else { l * SIZE + k };
        let before = column as usize;
        for l in 0..SIZE {
            let mut k = 0;
            while k < SIZE {
                let label = grid[index(k, l)];
                let mut end = k;
                while end + 1 < SIZE && label != NONE && grid[index(end + 1, l)] == label {
                    end += 1;
                }
                for j in k..=end {
                    let p = &mut px[index(j, l)];
                    p[before] = (j - k).min(255) as u8;
                    p[before + 2] = (end - j).min(255) as u8;
                }
                k = end + 1;
            }
        }
    }
    px
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texel_rect_keeps_centres_and_drops_borders() {
        let s = SIZE as f32;
        assert_eq!(
            texel_rect(&[[2.5 / s, 4.5 / s], [5.5 / s, 4.5 / s], [5.5 / s, 6.5 / s]]),
            [2, 4, 5, 6]
        );
        assert_eq!(
            texel_rect(&[[2.0 / s, 4.0 / s], [6.0 / s, 4.0 / s], [6.0 / s, 4.0 / s]]),
            [2, 4, 5, 4]
        );
    }

    #[test]
    fn runs_follow_an_l_shaped_block_and_stop_at_a_neighbour() {
        // an L: a bar on row 0 and a leg down column 0, a separate block
        // in the L's corner at (2, 2)
        let page = PageTris {
            rects: vec![[0, 0, 4, 0], [0, 0, 0, 4], [2, 2, 3, 3]],
            links: vec![],
        };
        let px = page_edges(&labels(&page));
        assert_eq!(px[0], [0, 0, 4, 4]);
        assert_eq!(px[2 * SIZE + 2], [0, 0, 1, 1]);
        assert_eq!(px[3 * SIZE + 3], [1, 1, 0, 0]);
        // the leg's texel at (0, 2) runs no further right
        assert_eq!(px[2 * SIZE], [0, 2, 0, 2]);
        assert_eq!(px[SIZE * SIZE - 1], [0; 4]);
    }

    #[test]
    fn a_shared_vertex_joins_rects_that_only_meet() {
        // a chart on texel borders: x 19..25 and 25..28 meet at 25.0
        let rects = vec![[19, 0, 24, 3], [25, 0, 27, 3]];
        let apart = labels(&PageTris {
            rects: rects.clone(),
            links: vec![],
        });
        assert_ne!(apart[24], apart[25]);
        let joined = page_edges(&labels(&PageTris {
            rects,
            links: vec![(0, 1)],
        }));
        assert_eq!(joined[24], [5, 0, 3, 3]);
    }

    /// mp_carentan's papered wall (page 3, around texel (25, 384)) is one
    /// chart whose coordinates sit on texel borders; split at x 25 it drew
    /// a seam under vc_bicubic.
    #[test]
    fn carentan_wall_chart_is_one_block() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let bsp = vcod_common::bsp::parse(&fs.read("maps/mp/mp_carentan.bsp").unwrap()).unwrap();
        let grid = labels(&page_tris(&bsp)[3]);
        assert_ne!(grid[384 * SIZE + 24], NONE);
        assert_eq!(grid[384 * SIZE + 24], grid[384 * SIZE + 25]);
    }

    /// Stock maps: the triangle rects join into thousands of blocks. The
    /// biggest (mp_carentan 99119 texels, mp_ship 138869) stay well short
    /// of a whole page.
    #[test]
    fn stock_pages_split_into_blocks() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        for map in ["mp_carentan", "mp_ship"] {
            let data = fs.read(&format!("maps/mp/{map}.bsp")).unwrap();
            let bsp = vcod_common::bsp::parse(&data).unwrap();
            let (mut blocks, mut biggest) = (0, 0);
            for page in page_tris(&bsp) {
                let mut sizes = HashMap::<u32, usize>::new();
                for l in labels(&page).into_iter().filter(|&l| l != NONE) {
                    *sizes.entry(l).or_default() += 1;
                }
                blocks += sizes.len();
                biggest = biggest.max(sizes.into_values().max().unwrap_or(0));
            }
            eprintln!("{map}: {blocks} blocks, biggest {biggest} texels");
            assert!(blocks > 500, "{map}: {blocks}");
            assert!(biggest < SIZE * SIZE * 3 / 4, "{map}: {biggest}");
        }
    }
}
