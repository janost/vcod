//! Exports a stock map's render world to Wavefront OBJ + MTL with each
//! material's colour map as a TGA, for a look in a model viewer:
//! `cargo run -p vcod-cod4 --example map_obj [map] [out dir]` (default
//! `mp_crash` into the workspace's `tmp/cod4-obj/<map>/`). Surfaces whose
//! techset draws nothing in the lit view (`wc_tools`, `wc_shadowcaster`)
//! are left out. Prints the bounds and the surface counts.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use vcod_cod4::map::Map;
use vcod_cod4::source::Cod4Source;
use vcod_cod4::techset::Family;
use vcod_cod4::testing::cod4_install;
use vcod_common::assets::{Image, ImageData, TextureFormat};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let name = args.next().unwrap_or_else(|| "mp_crash".into());
    let out = args.next().map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../tmp/cod4-obj/{name}"))
    });
    let install = cod4_install().context("COD4_DIR is not a CoD4 install")?;
    let (map, walked) = Map::load(&install, "english", &name)?;
    let mut src = Cod4Source::open(&install)?;
    src.add_zone(Arc::new(walked.assets));
    std::fs::create_dir_all(out.join("images"))?;

    let mut obj = format!("mtllib {name}.mtl\n");
    for v in &map.vertices {
        writeln!(obj, "v {} {} {}", v.pos[0], v.pos[2], -v.pos[1])?;
    }
    for v in &map.vertices {
        writeln!(obj, "vt {} {}", v.uv[0], 1.0 - v.uv[1])?;
    }
    for v in &map.vertices {
        writeln!(obj, "vn {} {} {}", v.normal[0], v.normal[2], -v.normal[1])?;
    }
    // Surfaces by material, so each material is one `usemtl` run.
    let mut by_material: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    let mut skipped = 0;
    for (i, s) in map.surfaces.iter().enumerate() {
        let m = &map.materials[s.material as usize];
        if m.class.is_some_and(|c| c.family == Family::Invisible) {
            skipped += 1;
            continue;
        }
        by_material.entry(s.material).or_default().push(i);
    }
    let (mut mins, mut maxs) = ([f32::MAX; 3], [f32::MIN; 3]);
    let (mut tris, mut surfaces) = (0, 0);
    let mut mtl = String::new();
    let mut textured = 0;
    for (&mi, list) in &by_material {
        let m = &map.materials[mi as usize];
        let id = format!("m{mi}");
        writeln!(mtl, "newmtl {id}\n# {} ({:?})\nKd 1 1 1", m.name, m.techset)?;
        let colour = m.textures.iter().find(|t| t.sampler == Some("colorMap"));
        if let Some(img) = colour.and_then(|t| t.image.as_deref()) {
            match src.image(img).map(|i| to_rgba(&i)) {
                Ok(Some((w, h, px))) => {
                    let file = format!("images/{}.tga", sanitize(img));
                    std::fs::write(out.join(&file), tga(w, h, &px))?;
                    writeln!(mtl, "map_Kd {file}")?;
                    textured += 1;
                }
                Ok(None) => {}
                Err(e) => eprintln!("{img}: {e:#}"),
            }
        }
        writeln!(obj, "usemtl {id}")?;
        for &si in list {
            let s = &map.surfaces[si];
            surfaces += 1;
            for t in map.triangles(s) {
                tris += 1;
                for &k in &t {
                    let p = map.vertices[k as usize].pos;
                    for a in 0..3 {
                        mins[a] = mins[a].min(p[a]);
                        maxs[a] = maxs[a].max(p[a]);
                    }
                }
                let [a, b, c] = t.map(|k| k + 1);
                writeln!(obj, "f {a}/{a}/{a} {b}/{b}/{b} {c}/{c}/{c}")?;
            }
        }
    }
    std::fs::write(out.join(format!("{name}.obj")), obj)?;
    std::fs::write(out.join(format!("{name}.mtl")), mtl)?;
    println!(
        "{}: {surfaces} surfaces ({skipped} invisible left out), {tris} triangles, {} materials ({textured} textured)",
        out.display(),
        by_material.len()
    );
    println!(
        "drawn bounds {mins:?} .. {maxs:?}; the world's {:?} .. {:?}",
        map.mins, map.maxs
    );
    Ok(())
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// An uncompressed 32-bit TGA, top row first.
fn tga(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = vec![0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    out.extend((w as u16).to_le_bytes());
    out.extend((h as u16).to_le_bytes());
    out.extend([32, 0x28]);
    out.extend(rgba.chunks(4).flat_map(|p| [p[2], p[1], p[0], p[3]]));
    out
}

/// The image's top level as RGBA, block formats decoded.
fn to_rgba(img: &Image) -> Option<(u32, u32, Vec<u8>)> {
    let (w, h) = (img.width, img.height);
    match &img.data {
        ImageData::Rgba8(px) => Some((w, h, px.clone())),
        ImageData::Bc { format, mips } => Some((w, h, decode_bc(*format, w, h, &mips[0]))),
    }
}

fn rgb565(c: u16) -> [u8; 3] {
    let (r, g, b) = ((c >> 11) & 31, (c >> 5) & 63, c & 31);
    [
        (r << 3 | r >> 2) as u8,
        (g << 2 | g >> 4) as u8,
        (b << 3 | b >> 2) as u8,
    ]
}

fn decode_bc(format: TextureFormat, w: u32, h: u32, data: &[u8]) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let mut out = vec![0u8; w * h * 4];
    let block = format.block_size() as usize;
    let bw = w.div_ceil(4);
    for (bi, b) in data.chunks_exact(block).enumerate() {
        let (bx, by) = (bi % bw, bi / bw);
        let (alpha, colour) = b.split_at(block - 8);
        let c0 = u16::from_le_bytes([colour[0], colour[1]]);
        let c1 = u16::from_le_bytes([colour[2], colour[3]]);
        let (p0, p1) = (rgb565(c0), rgb565(c1));
        // Weighted mean of the endpoints: (wa * p0 + wb * p1) / (wa + wb).
        let mix = |wa: u16, wb: u16| {
            [0, 1, 2].map(|k| ((wa * u16::from(p0[k]) + wb * u16::from(p1[k])) / (wa + wb)) as u8)
        };
        let four = format != TextureFormat::Bc1RgbaUnorm || c0 > c1;
        let pal: [[u8; 4]; 4] = if four {
            [
                rgba(p0, 255),
                rgba(p1, 255),
                rgba(mix(2, 1), 255),
                rgba(mix(1, 2), 255),
            ]
        } else {
            [
                rgba(p0, 255),
                rgba(p1, 255),
                rgba(mix(1, 1), 255),
                [0, 0, 0, 0],
            ]
        };
        let bits = u32::from_le_bytes(colour[4..8].try_into().unwrap());
        let alphas = alpha_block(format, alpha);
        for i in 0..16 {
            let (x, y) = (bx * 4 + i % 4, by * 4 + i / 4);
            if x >= w || y >= h {
                continue;
            }
            let mut p = pal[(bits >> (2 * i) & 3) as usize];
            if let Some(a) = alphas {
                p[3] = a[i];
            }
            out[(y * w + x) * 4..][..4].copy_from_slice(&p);
        }
    }
    out
}

fn rgba(c: [u8; 3], a: u8) -> [u8; 4] {
    [c[0], c[1], c[2], a]
}

fn alpha_block(format: TextureFormat, a: &[u8]) -> Option<[u8; 16]> {
    let mut out = [255u8; 16];
    match format {
        TextureFormat::Bc1RgbaUnorm => return None,
        TextureFormat::Bc2RgbaUnorm => {
            for (i, o) in out.iter_mut().enumerate() {
                let n = (a[i / 2] >> (4 * (i % 2))) & 15;
                *o = n << 4 | n;
            }
        }
        TextureFormat::Bc3RgbaUnorm => {
            let (a0, a1) = (u16::from(a[0]), u16::from(a[1]));
            let bits = u64::from_le_bytes([a[2], a[3], a[4], a[5], a[6], a[7], 0, 0]);
            for (i, o) in out.iter_mut().enumerate() {
                let c = (bits >> (3 * i) & 7) as u16;
                *o = match c {
                    0 => a0,
                    1 => a1,
                    c if a0 > a1 => ((8 - c) * a0 + (c - 1) * a1) / 7,
                    6 => 0,
                    7 => 255,
                    c => ((6 - c) * a0 + (c - 1) * a1) / 5,
                } as u8;
            }
        }
    }
    Some(out)
}
