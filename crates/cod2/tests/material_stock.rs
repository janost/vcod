//! The material chain against a CoD2 1.3 install (`COD2_DIR`): every stock
//! material, techset, technique and statemap parses, they reference each
//! other, and the map census of docs/research/cod2-materials.md 8 comes out
//! as `tools/re/cod2_mapcensus.py` printed it. Without the install it passes
//! vacuously.

use std::collections::{BTreeMap, HashMap, HashSet};
use vcod_cod2::material::{
    ArgValue, MaterialFile, StateMap, Technique, TechniqueSet, TextureImage, image_name,
};
use vcod_common::pk3::Pk3Fs;

struct Stock {
    materials: Vec<MaterialFile>,
    techsets: HashMap<String, TechniqueSet>,
    techniques: HashMap<String, Technique>,
    statemaps: HashMap<String, StateMap>,
}

/// The files directly in `dir/` with `ext`, parsed, keyed by base name.
fn parse_dir<T>(
    fs: &Pk3Fs,
    dir: &str,
    ext: &str,
    parse: impl Fn(&str) -> anyhow::Result<T>,
) -> HashMap<String, T> {
    fs.list_prefix(dir)
        .into_iter()
        .filter_map(|p| Some((p[dir.len()..].strip_suffix(ext)?.to_string(), p)))
        .map(|(name, path)| {
            let bytes = fs.read(&path).unwrap();
            let src = String::from_utf8_lossy(&bytes);
            let v = parse(&src).unwrap_or_else(|e| panic!("{path}: {e:#}"));
            (name, v)
        })
        .collect()
}

fn load(fs: &Pk3Fs) -> Stock {
    const SKIP: [&str; 4] = ["techniques/", "techniquesets/", "statemaps/", "shaders/"];
    let mut materials = Vec::new();
    for path in fs.list_prefix("materials/") {
        let rel = &path["materials/".len()..];
        if rel.is_empty() || rel.ends_with('/') || SKIP.iter().any(|s| rel.starts_with(s)) {
            continue;
        }
        let bytes = fs.read(&path).unwrap();
        let m = MaterialFile::parse(&bytes).unwrap_or_else(|e| panic!("{path}: {e:#}"));
        assert_eq!(m.name.to_lowercase(), rel, "{path}: name field");
        materials.push(m);
    }
    Stock {
        materials,
        techsets: parse_dir(
            fs,
            "materials/techniquesets/",
            ".techset",
            TechniqueSet::parse,
        ),
        techniques: parse_dir(fs, "materials/techniques/", ".tech", Technique::parse),
        statemaps: parse_dir(fs, "materials/statemaps/", ".sm", StateMap::parse),
    }
}

#[test]
fn stock_material_chain_parses_and_links() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let s = load(&fs);
    assert_eq!(s.materials.len(), 3765);
    assert_eq!(s.techsets.len(), 77);
    assert_eq!(s.techniques.len(), 258);
    assert_eq!(s.statemaps.len(), 27);

    let used: HashSet<&str> = s
        .materials
        .iter()
        .map(|m| m.technique_set.as_str())
        .collect();
    assert_eq!(used.len(), 64);
    // `shadowoverlay` names a set only `materials_dx7/` holds.
    let missing: Vec<&str> = used
        .iter()
        .copied()
        .filter(|ts| !s.techsets.contains_key(*ts))
        .collect();
    assert_eq!(missing, ["shadowoverlay"]);
    // Three `_scroll` sets no material uses name techniques that were
    // never shipped; every set in use resolves.
    let mut broken: Vec<&str> = s
        .techsets
        .iter()
        .filter(|(_, set)| set.iter().any(|(_, t)| !s.techniques.contains_key(t)))
        .map(|(name, _)| name.as_str())
        .collect();
    broken.sort();
    assert_eq!(
        broken,
        [
            "phong_alphatest_detail_scroll",
            "phong_alphatest_spec_detail_scroll",
            "phong_replace_detail_scroll"
        ]
    );
    assert!(broken.iter().all(|b| !used.contains(b)));
    let mut passes = 0;
    for (name, t) in &s.techniques {
        for p in &t.passes {
            passes += 1;
            assert!(
                s.statemaps.contains_key(&p.state_map),
                "{name}: {}",
                p.state_map
            );
        }
    }
    assert_eq!(passes, 260);

    // Every `material.<x>` a material's techniques read is one of its
    // textures or constants (`Material_ValidatePassArguments`), and every
    // pass's statemap has a rule for the material's stateBits.
    let mut checked = 0;
    let mut unmatched = Vec::new();
    for m in &s.materials {
        let Some(set) = s.techsets.get(&m.technique_set) else {
            continue;
        };
        for (_, t) in set.iter() {
            for p in &s.techniques[t].passes {
                if let Err(g) = s.statemaps[&p.state_map].apply(m.state_bits) {
                    unmatched.push((m.name.as_str(), t, p.state_map.as_str(), g));
                }
                for a in p.vertex_shader.args.iter().chain(&p.pixel_shader.args) {
                    if let ArgValue::Material(x) = &a.value {
                        checked += 1;
                        assert!(
                            m.texture(x).is_some() || m.constant(x).is_some(),
                            "{}: {t} reads material.{x}",
                            m.name
                        );
                    }
                }
            }
        }
    }
    assert!(checked > 0);
    assert!(unmatched.is_empty(), "no statemap rule: {unmatched:?}");

    // Image names: 8403 references, every `~` packed one well formed.
    let mut images = 0;
    let mut normals = 0;
    for t in s.materials.iter().flat_map(|m| &m.textures) {
        let TextureImage::Image(n) = &t.image else {
            continue;
        };
        images += 1;
        if let image_name::ImageName::Packed(p) = image_name::parse(n) {
            if p.normal_map_source().is_some() {
                normals += 1;
                assert_eq!(t.name, "normalMap");
            }
            let complete = if p.hash.is_some() {
                &p.parts[..p.parts.len() - 1]
            } else {
                &p.parts[..]
            };
            for part in complete {
                assert!(part.channels().is_some(), "{n}");
            }
        }
    }
    assert_eq!(images, 8403);
    assert_eq!(normals, 1339);

    // The opaque word under `fogged` turns alpha writes on.
    let fogged = &s.statemaps["fogged"];
    assert_eq!(fogged.apply([0x2812_8812, 5]), Ok([0x3812_8812, 5]));
}

fn lump(d: &[u8], i: usize) -> &[u8] {
    let at = 8 + 8 * i;
    let len = u32::from_le_bytes(d[at..at + 4].try_into().unwrap()) as usize;
    let ofs = u32::from_le_bytes(d[at + 4..at + 8].try_into().unwrap()) as usize;
    &d[ofs..ofs + len]
}

/// Printable runs of 3 or more, as the census tool reads an xmodel's
/// material names.
fn strings(d: &[u8]) -> Vec<String> {
    d.split(|b| !(0x20..=0x7e).contains(b))
        .filter(|r| r.len() >= 3)
        .map(|r| String::from_utf8_lossy(r).into_owned())
        .collect()
}

/// `misc_model` paths from a d3dbsp entity lump.
fn misc_models(ents: &str) -> Vec<String> {
    let mut out = Vec::new();
    for block in ents.split('{').skip(1) {
        let Some((block, _)) = block.split_once('}') else {
            continue;
        };
        let quoted: Vec<&str> = block.split('"').skip(1).step_by(2).collect();
        let kv: HashMap<&str, &str> = quoted
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&[k, v]| (k, v))
            .collect();
        if kv.get("classname") == Some(&"misc_model") {
            out.push(kv["model"].to_string());
        }
    }
    out
}

#[test]
fn stock_map_census_matches_the_doc() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let s = load(&fs);
    let by_name: HashMap<&str, &MaterialFile> =
        s.materials.iter().map(|m| (m.name.as_str(), m)).collect();
    let techset_of = |n: &str| by_name.get(n).map_or("?", |m| m.technique_set.as_str());

    let maps: Vec<String> = fs
        .list_prefix("maps/mp/")
        .into_iter()
        .filter(|p| p.ends_with(".d3dbsp"))
        .collect();
    assert_eq!(maps.len(), 15);
    let mut world: BTreeMap<&str, u64> = BTreeMap::new();
    let mut lightmapped = 0u64;
    let mut models: BTreeMap<String, u64> = BTreeMap::new();
    let mut xmodels: HashMap<String, Option<Vec<String>>> = HashMap::new();
    let mut no_xmodel = 0;
    for path in &maps {
        let d = fs.read(path).unwrap();
        assert_eq!(&d[..4], b"IBSP");
        assert_eq!(u32::from_le_bytes(d[4..8].try_into().unwrap()), 4);
        let names: Vec<String> = lump(&d, 0)
            .as_chunks::<72>()
            .0
            .iter()
            .map(|r| {
                let n = r[..64].split(|&b| b == 0).next().unwrap();
                String::from_utf8_lossy(n).into_owned()
            })
            .collect();
        for soup in lump(&d, 7).as_chunks::<16>().0 {
            let mat = i16::from_le_bytes([soup[0], soup[1]]) as usize;
            let lm = i16::from_le_bytes([soup[2], soup[3]]);
            let tris = u16::from_le_bytes([soup[10], soup[11]]) as u64 / 3;
            *world.entry(techset_of(&names[mat])).or_default() += tris;
            if lm != 0x1f && lm >= 0 {
                lightmapped += tris;
            }
        }
        let ents = vcod_common::text::latin1(lump(&d, 37));
        for model in misc_models(&ents) {
            let file = format!("xmodel/{}", model.trim_start_matches("xmodel/"));
            let mats = xmodels.entry(file.clone()).or_insert_with(|| {
                fs.read(&file).map(|x| {
                    strings(&x)
                        .into_iter()
                        .filter(|n| by_name.contains_key(n.as_str()))
                        .collect()
                })
            });
            if mats.is_none() {
                no_xmodel += 1;
            }
            for n in mats.iter().flatten() {
                *models.entry(techset_of(n).to_string()).or_default() += 1;
            }
        }
    }
    let ranked = |m: &BTreeMap<&str, u64>| {
        let mut v: Vec<(String, u64)> = m.iter().map(|(k, &c)| (k.to_string(), c)).collect();
        v.sort_by_key(|r| std::cmp::Reverse(r.1));
        v
    };
    let total: u64 = world.values().sum();
    assert_eq!(total, 1_646_795);
    assert_eq!(lightmapped * 1000 / total, 833);
    let w = ranked(&world);
    let expect = [
        ("phong_replace_spec_detail", 971_879),
        ("effect_multiply", 263_305),
        ("phong_replace_detail", 96_452),
        ("phong_blend_spec_detail", 93_255),
        ("phong_replace_spec", 76_573),
    ];
    for (got, (name, count)) in w.iter().zip(expect) {
        assert_eq!((got.0.as_str(), got.1), (name, count));
    }
    assert!(!world.contains_key("?"));

    let m = ranked(&models.iter().map(|(k, &c)| (k.as_str(), c)).collect());
    assert_eq!(m.iter().map(|r| r.1).sum::<u64>(), 38_267);
    assert_eq!((m[0].0.as_str(), m[0].1), ("ambient_alphatest", 19_991));
    assert_eq!((m[1].0.as_str(), m[1].1), ("phong_replace_spec", 8_049));
    assert_eq!(no_xmodel, 0);
}
