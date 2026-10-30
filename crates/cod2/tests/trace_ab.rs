//! The clip map against retail: `tests/fixtures/traces/<map>.txt` holds
//! what `cod2_lnxded` 1.3 returned for `bulletTrace`, `physicsTrace` and
//! `placeSpawnPoint` over a fixed set of segments and spots
//! (`examples/trace_probe.rs`), every float as its bits: mp_toujane and
//! mp_carentan the full set, the other stock maps their spawn points. Each
//! is replayed through [`ClipMap::trace`] with the builtin's mask and
//! diffed bit for bit (`docs/research/cod2-d3dbsp-format.md`, "The trace
//! A/B"). Needs `COD2_DIR`. `TRACE_AB_REPORT=1` prints every divergence.

use glam::Vec3;
use vcod_cod2::collision::{ClipMap, MASK_PLAYERSOLID};
use vcod_cod2::d3dbsp::D3dBsp;
use vcod_cod2::xmodel;
use vcod_common::testing;

/// `Scr_BulletTrace` with `hitchars` false: the shot mask without BODY.
const MASK_BULLETTRACE: u32 = 0x80_2831;
/// `Scr_PhysicsTrace`'s `G_TraceCapsule` mask (0x08113c0e).
const MASK_PHYSICSTRACE: u32 = 0x82_0011;

const PLAYER_MINS: Vec3 = Vec3::new(-15.0, -15.0, 0.0);
const PLAYER_MAXS: Vec3 = Vec3::new(15.0, 15.0, 70.0);

/// `Com_SurfaceTypeToName`'s table (0x08182dec, 20-byte rows): index
/// `surface_flags >> 20 & 0x1f`, 0 and past 22 `default`.
const SURFACE_NAMES: [&str; 23] = [
    "default", "bark", "brick", "carpet", "cloth", "concrete", "dirt", "flesh", "foliage", "glass",
    "grass", "gravel", "ice", "metal", "mud", "paper", "plaster", "rock", "sand", "snow", "water",
    "wood", "asphalt",
];

/// Records the gate tolerates, each with the doc section that explains it:
/// `(map, kind, tag, id, why)`.
const GAPS: &[(&str, char, &str, u32, &str)] = &[(
    "mp_toujane",
    's',
    "near",
    105,
    "placed on the script_model panzer at (1047, 2751, 44): an entity's clip \
     (cod2-d3dbsp-format.md, \"The trace A/B\")",
)];

fn bits(t: &str) -> f32 {
    let (neg, digits) = t.strip_prefix('-').map_or((false, t), |d| (true, d));
    let b: u32 = digits.parse().unwrap_or_else(|_| panic!("bad bits {t}"));
    f32::from_bits(b | if neg { 0x8000_0000 } else { 0 })
}

fn vec(t: &[&str]) -> Vec3 {
    Vec3::new(bits(t[0]), bits(t[1]), bits(t[2]))
}

/// Bitwise, with the two zeros equal: the probe prints both as `0`.
fn same(a: f32, b: f32) -> bool {
    a.to_bits() == b.to_bits() || (a == 0.0 && b == 0.0)
}

fn same_v(a: Vec3, b: Vec3) -> bool {
    (0..3).all(|k| same(a[k], b[k]))
}

/// `Vec3Lerp` (0x080e129e): `a + (b - a) * f`, extended, stored once.
fn lerp(a: Vec3, b: Vec3, f: f32) -> Vec3 {
    Vec3::from(std::array::from_fn::<f32, 3, _>(|k| {
        (f64::from(a[k]) + (f64::from(b[k]) - f64::from(a[k])) * f64::from(f)) as f32
    }))
}

/// `GScr_PlaceSpawnPoint` (0x081167ae): up 128 from the origin, then down
/// 262144 onto the floor, then a position test. The placed origin and
/// whether that test started solid.
fn place(clip: &ClipMap, o: Vec3) -> (Vec3, bool) {
    let top = Vec3::new(o.x, o.y, o.z + 128.0);
    let up = clip.trace(o, top, PLAYER_MINS, PLAYER_MAXS, MASK_PLAYERSOLID);
    let from = lerp(o, top, up.fraction);
    let to = Vec3::new(from.x, from.y, from.z - 262_144.0);
    let down = clip.trace(from, to, PLAYER_MINS, PLAYER_MAXS, MASK_PLAYERSOLID);
    let p = lerp(from, to, down.fraction);
    let t = clip.trace(p, p, PLAYER_MINS, PLAYER_MAXS, MASK_PLAYERSOLID);
    (p, t.startsolid)
}

/// Each `misc_model` whose xmodel has collision, as a sphere about its
/// origin that holds the model at its scale: `bulletTrace` clips static
/// models (`SV_Trace` with `locational` set calls 0x0805ec2c), the clip map
/// does not.
fn static_models(fs: &vcod_common::pk3::Pk3Fs, entities: &str) -> Vec<(Vec3, f32)> {
    let mut cache: std::collections::HashMap<String, Option<f32>> = Default::default();
    let mut out = Vec::new();
    for e in vcod_common::bsp::entity_blocks(entities) {
        if e.get("classname").map(String::as_str) != Some("misc_model") {
            continue;
        }
        let (Some(model), Some(origin)) = (
            e.get("model"),
            e.get("origin")
                .and_then(|o| vcod_common::bsp::parse_vec3(o)),
        ) else {
            continue;
        };
        let reach = *cache.entry(model.to_lowercase()).or_insert_with(|| {
            let d = xmodel::parse_descriptor(&fs.read(model)?, |lod0| {
                let parts = fs
                    .read(&format!("xmodelparts/{lod0}"))
                    .ok_or_else(|| anyhow::anyhow!("no xmodelparts/{lod0}"))?;
                Ok(xmodel::parse_parts(&parts)?.bones.len())
            })
            .ok()?;
            (!d.coll_surfs.is_empty()).then(|| {
                Vec3::from(d.mins)
                    .abs()
                    .max(Vec3::from(d.maxs).abs())
                    .length()
            })
        });
        let scale = e
            .get("modelscale_vec")
            .and_then(|v| vcod_common::bsp::parse_vec3(v))
            .map(|v| v.into_iter().fold(0.0f32, |a, b| a.max(b.abs())))
            .or_else(|| e.get("modelscale").and_then(|v| v.parse().ok()))
            .unwrap_or(1.0);
        if let Some(r) = reach {
            out.push((Vec3::from_array(origin), r * scale + 1.0));
        }
    }
    out
}

#[derive(Default)]
struct Tally {
    seen: usize,
    matched: usize,
    entity: usize,
    static_models: usize,
    gaps: usize,
    off: Vec<String>,
}

fn replay(map: &str, clip: &ClipMap, statics: &[(Vec3, f32)], fixture: &str) -> Tally {
    let mut t = Tally::default();
    // The last bulletTrace's segment and whether it met an entity: the
    // physicsTrace record after it traces the same segment.
    let mut last_b = None;
    for line in fixture.lines().filter(|l| !l.starts_with('#')) {
        let w: Vec<&str> = line.split_whitespace().collect();
        let (kind, tag, id) = (w[0].chars().next().unwrap(), w[1], w[2]);
        let id: u32 = id.parse().unwrap();
        t.seen += 1;
        let diff = match kind {
            'b' => {
                let (s, e) = (vec(&w[3..6]), vec(&w[6..9]));
                let frac = bits(w[9]);
                let normal = vec(&w[10..13]);
                let (surf, ent) = (w[13], w[14].parse::<i32>().unwrap());
                let pos = vec(&w[15..18]);
                last_b = Some((tag, id, s, e, ent != -1));
                if ent != -1 {
                    // An entity's clip, which the clip map has no part in.
                    t.entity += 1;
                    continue;
                }
                let ours = clip.trace(s, e, Vec3::ZERO, Vec3::ZERO, MASK_BULLETTRACE);
                let our_surf = if ours.fraction < 1.0 {
                    SURFACE_NAMES
                        .get((ours.surface_flags >> 20 & 0x1f) as usize)
                        .copied()
                        .unwrap_or("default")
                } else {
                    "none"
                };
                if frac < ours.fraction && statics.iter().any(|&(o, r)| o.distance(pos) <= r) {
                    // Met a static model's collision first.
                    t.static_models += 1;
                    continue;
                }
                let ok = same(ours.fraction, frac)
                    && (frac >= 1.0 || same_v(ours.normal, normal))
                    && our_surf == surf
                    && same_v(lerp(s, e, ours.fraction), pos);
                (!ok).then(|| {
                    format!(
                        "fraction {frac:e} vs ours {:e} normal {normal} vs {} {surf} vs {our_surf} \
                         ({s} -> {e})",
                        ours.fraction, ours.normal
                    )
                })
            }
            'p' => {
                let Some((btag, bid, s, e, met_entity)) = last_b else {
                    panic!("{map}: {line} follows no bulletTrace");
                };
                assert_eq!((btag, bid), (tag, id), "{map}: {line}");
                if met_entity {
                    // The bulletTrace met an entity on this segment.
                    t.entity += 1;
                    continue;
                }
                let pos = vec(&w[3..6]);
                let ours = clip.trace(s, e, Vec3::ZERO, Vec3::ZERO, MASK_PHYSICSTRACE);
                let at = lerp(s, e, ours.fraction);
                (!same_v(at, pos)).then(|| {
                    format!(
                        "end {pos} vs ours {at} (fraction {:e}, {s} -> {e})",
                        ours.fraction
                    )
                })
            }
            's' => {
                let (o, p) = (vec(&w[4..7]), vec(&w[7..10]));
                let solid = w[10] == "1";
                let (ours, our_solid) = place(clip, o);
                (!(same_v(ours, p) && our_solid == solid)).then(|| {
                    format!("placed {p} solid {solid} vs ours {ours} solid {our_solid} (from {o})")
                })
            }
            _ => panic!("{map}: bad record {line}"),
        };
        match diff {
            None => t.matched += 1,
            Some(_)
                if GAPS
                    .iter()
                    .any(|g| (g.0, g.1, g.2, g.3) == (map, kind, tag, id)) =>
            {
                t.gaps += 1;
            }
            Some(d) => t.off.push(format!("{map} {kind} {tag} {id}: {d}")),
        }
    }
    t
}

#[test]
fn clip_map_traces_match_retail() {
    let Some(fs) = testing::cod2_fs() else {
        return;
    };
    let report = std::env::var_os("TRACE_AB_REPORT").is_some();
    let mut off = Vec::new();
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/traces");
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    assert_eq!(files.len(), 15, "one fixture per stock map");
    for path in files {
        let map = path.file_stem().unwrap().to_str().unwrap();
        let fixture = std::fs::read_to_string(&path).unwrap();
        let data = fs.read(&format!("maps/mp/{map}.d3dbsp")).unwrap();
        let bsp = D3dBsp::parse(&data).unwrap();
        let clip = vcod_cod2::map::clip_map(&bsp).unwrap();
        let statics = static_models(&fs, &bsp.entities);
        let t = replay(map, &clip, &statics, &fixture);
        eprintln!(
            "{map}: {} records, {} match, {} entity and {} static model hits skipped, \
             {} known gaps, {} off",
            t.seen,
            t.matched,
            t.entity,
            t.static_models,
            t.gaps,
            t.off.len()
        );
        if report {
            for d in &t.off {
                eprintln!("  {d}");
            }
        }
        off.extend(t.off);
    }
    assert!(
        off.is_empty(),
        "{} records off retail, first: {:#?}",
        off.len(),
        &off[..off.len().min(8)]
    );
}
