//! Writes the gsc gametype behind `tests/fixtures/traces/<map>.txt`: a fixed
//! set of segments and spawn placements for the retail CoD2 server to trace
//! with `bulletTrace`, `physicsTrace` and `placeSpawnPoint`, every float
//! printed as its bits (`docs/research/cod2-d3dbsp-format.md`, "The trace
//! A/B"). Needs `COD2_DIR`.
//!
//!   cargo run -p vcod-cod2 --example trace_probe -- mp_toujane > probe_trace.gsc
//!
//! `--spawns-only` places the map's spawn points and traces nothing.

use std::fmt::Write as _;

use glam::Vec3;
use vcod_cod2::d3dbsp::D3dBsp;

/// A small xorshift, so the set repeats run to run.
struct Rng(u64);

impl Rng {
    fn unit(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }

    fn below(&mut self, n: usize) -> usize {
        ((self.unit() * n as f32) as usize).min(n - 1)
    }
}

/// A coordinate snapped to 1/64, so the literal is exact in any parser.
fn q(x: f32) -> f32 {
    (x * 64.0).round() / 64.0
}

fn qv(v: Vec3) -> Vec3 {
    Vec3::new(q(v.x), q(v.y), q(v.z))
}

fn lit(v: Vec3) -> String {
    format!("({}, {}, {})", v.x, v.y, v.z)
}

struct Probe {
    segs: Vec<(&'static str, Vec3, Vec3)>,
    places: Vec<(&'static str, Vec3)>,
}

fn main() -> anyhow::Result<()> {
    let map = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "mp_toujane".into());
    let fs = vcod_common::testing::cod2_fs().ok_or_else(|| anyhow::anyhow!("COD2_DIR unset"))?;
    let data = fs
        .read(&format!("maps/mp/{map}.d3dbsp"))
        .ok_or_else(|| anyhow::anyhow!("{map} not in the iwds"))?;
    let b = D3dBsp::parse(&data)?;
    let blocks = vcod_common::bsp::entity_blocks(&b.entities);
    let mut classes: Vec<String> = blocks
        .iter()
        .filter_map(|e| e.get("classname"))
        .filter(|c| c.starts_with("mp_") && c.contains("_spawn"))
        .cloned()
        .collect();
    classes.sort();
    classes.dedup();
    let spawns: Vec<Vec3> = blocks
        .iter()
        .filter(|e| classes.iter().any(|c| e.get("classname") == Some(c)))
        .filter_map(|e| vcod_common::bsp::parse_vec3(e.get("origin")?))
        .map(Vec3::from_array)
        .collect();

    let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ map.len() as u64);
    let mut p = Probe {
        segs: Vec::new(),
        places: Vec::new(),
    };

    if std::env::args().any(|a| a == "--spawns-only") {
        print!("{}", gsc(&map, &p, &classes));
        return Ok(());
    }

    if map == "mp_toujane" {
        // The doorway and window of `collision_maps.rs`.
        for x in [890.0, 927.0, 964.0] {
            let s = Vec3::new(x, 1646.0, 196.0);
            p.segs.push(("door", s, s + Vec3::Y * 200.0));
        }
        let s = Vec3::new(927.0, 1646.0, 250.0);
        p.segs.push(("door", s, s + Vec3::Y * 200.0));
        let w = Vec3::new(968.0, 3348.0, 49.125);
        let dir = Vec3::new(-0.195_090_38, -0.980_785_25, 0.0) * 800.0;
        for i in 0..=60 {
            let s = w + Vec3::Z * (i as f32 * 2.0);
            p.segs.push(("window", s, qv(s + dir)));
        }
        for i in 0..=8 {
            p.places
                .push(("door", Vec3::new(927.0, 1660.0 + i as f32 * 8.0, 160.0)));
            p.places
                .push(("window", qv(w + dir * (0.2 + i as f32 * 0.01))));
        }
    }

    // The area the spawns cover, grown by 512, inside the world's bounds.
    let m = &b.models[0];
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for &s in &spawns {
        lo = lo.min(s);
        hi = hi.max(s);
    }
    lo = (lo - Vec3::splat(512.0)).max(m.mins);
    hi = (hi + Vec3::splat(512.0)).min(m.maxs);
    let n = 24;
    for i in 0..n {
        for j in 0..n {
            let x = q(lo.x + (hi.x - lo.x) * (i as f32 + 0.5) / n as f32) + 0.375;
            let y = q(lo.y + (hi.y - lo.y) * (j as f32 + 0.5) / n as f32) + 0.625;
            let top = Vec3::new(x, y, (m.maxs.z - 8.0).floor());
            p.segs
                .push(("grid", top, Vec3::new(x, y, (m.mins.z + 8.0).ceil())));
        }
    }

    // Axial rays grazing a brush's box edge at a set of offsets.
    let first = m.first_brush as usize;
    let count = m.brush_count as usize;
    let offs = [-1.0 / 64.0, 0.0, 1.0 / 64.0, 0.125, 0.125 + 1.0 / 64.0];
    for k in 0..240 {
        let i = first + rng.below(count);
        let bx = b.brush_box(i);
        if bx.contents & 0x2011 == 0 {
            continue;
        }
        let (bmin, bmax) = (Vec3::from_array(bx.mins), Vec3::from_array(bx.maxs));
        let a = k % 3;
        let (u, v) = ((a + 1) % 3, (a + 2) % 3);
        let mut s = Vec3::ZERO;
        let side_u = if rng.unit() < 0.5 { bmin[u] } else { bmax[u] };
        let side_v = if rng.unit() < 0.5 { bmin[v] } else { bmax[v] };
        let du = offs[rng.below(offs.len())] * if side_u == bmax[u] { 1.0 } else { -1.0 };
        let dv = offs[rng.below(offs.len())] * if side_v == bmax[v] { 1.0 } else { -1.0 };
        s[u] = side_u + du;
        s[v] = side_v + dv;
        let mut e = s;
        s[a] = bmin[a] - 48.0;
        e[a] = bmax[a] + 48.0;
        if k % 2 == 1 {
            std::mem::swap(&mut s, &mut e);
        }
        p.segs.push(("edge", qv(s), qv(e)));
        // The same edge crossed on a slant.
        let mut s2 = s;
        let mut e2 = e;
        s2[u] -= 24.0 * du.signum();
        e2[u] += 24.0 * du.signum();
        p.segs.push(("edge", qv(s2), qv(e2)));
    }

    // Collision triangles: down through the middle, onto a vertex, along an
    // edge, just outside it, and a glancing ray.
    let tris: Vec<usize> = (0..b.coll_tris.len())
        .filter(|&t| b.coll_tris[t].verts.iter().all(|&v| v >= 0))
        .collect();
    for _ in 0..if tris.is_empty() { 0 } else { 90 } {
        let t = &b.coll_tris[tris[rng.below(tris.len())]];
        let v = t.verts.map(|i| b.coll_verts[i as usize].xyz);
        let nrm = Vec3::new(t.plane[0], t.plane[1], t.plane[2]);
        let mid = (v[0] + v[1] + v[2]) / 3.0;
        let k = rng.below(3);
        let edge_mid = (v[k] + v[(k + 1) % 3]) * 0.5;
        let out = (edge_mid - mid).normalize_or_zero() / 32.0;
        for (pt, tag) in [
            (mid, "tri"),
            (v[k], "trivert"),
            (edge_mid, "triedge"),
            (edge_mid + out, "triedge"),
        ] {
            let s = qv(pt + Vec3::Z * 48.0);
            p.segs.push((tag, s, Vec3::new(s.x, s.y, q(pt.z - 48.0))));
        }
        let along = (v[(k + 1) % 3] - v[k]).normalize_or_zero();
        let s = qv(mid + nrm * 8.0 - along * 256.0);
        p.segs
            .push(("triglance", s, qv(mid - nrm * 2.0 + along * 256.0)));
    }

    // From a spawn's eye out to a random point 2000 units around.
    for i in 0..240 {
        let s = qv(spawns[i % spawns.len()] + Vec3::Z * 60.0);
        let d = Vec3::new(rng.unit() - 0.5, rng.unit() - 0.5, rng.unit() - 0.6) * 4000.0;
        p.segs.push(("eye", s, qv(s + d)));
    }

    // Placements around the spawns.
    for _ in 0..240 {
        let s = spawns[rng.below(spawns.len())];
        let d = Vec3::new(
            (rng.unit() - 0.5) * 800.0,
            (rng.unit() - 0.5) * 800.0,
            rng.unit() * 128.0 - 32.0,
        );
        p.places.push(("near", qv(s + d)));
    }

    print!("{}", gsc(&map, &p, &classes));
    eprintln!(
        "{map}: {} segments, {} placements, spawn classes {classes:?}",
        p.segs.len(),
        p.places.len()
    );
    Ok(())
}

/// The gametype: each float goes out as its IEEE bits, built from exact
/// halvings and doublings, since gsc prints floats `%g`.
fn gsc(map: &str, p: &Probe, classes: &[String]) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "// Generated by `cargo run -p vcod-cod2 --example trace_probe -- {map}`.\n\
main()\n{{\n\
\tlevel.callbackStartGameType = ::start;\n\
\tlevel.callbackPlayerConnect = ::nop;\n\
\tlevel.callbackPlayerDisconnect = ::nop;\n\
\tlevel.callbackPlayerDamage = ::nop;\n\
\tlevel.callbackPlayerKilled = ::nop;\n\
\tmaps\\mp\\gametypes\\_callbacksetup::SetupCallbacks();\n}}\n\n\
nop(a, b, c, d, e, f, g, h, i, j)\n{{\n}}\n\n\
f(x)\n{{\n\
\t// gsc compares floats with a tolerance (1e-8 == 0 holds), so the sign\n\
\t// is read off the value scaled by 2^100 and a zero is the doubling\n\
\t// that never ends.\n\
\ty = x * 1.0 * 1048576 * 1048576 * 1048576 * 1048576 * 1048576;\n\
\tsign = \"\";\n\
\tif(y < 0)\n\t{{\n\t\tsign = \"-\";\n\t\ty = 0 - y;\n\t}}\n\
\te = -100;\n\
\twhile(y >= 16777216)\n\t{{\n\t\ty = y * 0.5;\n\t\te++;\n\t}}\n\
\tn = 0;\n\
\twhile(y < 8388608 && n < 400)\n\t{{\n\t\ty = y * 2;\n\t\te--;\n\t\tn++;\n\t}}\n\
\tif(n == 400)\n\t\treturn \"0\";\n\
\treturn sign + ((e + 150) * 8388608 + (int(y) - 8388608));\n}}\n\n\
v(p)\n{{\n\treturn f(p[0]) + \" \" + f(p[1]) + \" \" + f(p[2]);\n}}\n\n\
b(tag, id, s, e)\n{{\n\
\ttr = bulletTrace(s, e, false, undefined);\n\
\tent = -1;\n\
\tif(isDefined(tr[\"entity\"]))\n\t\tent = tr[\"entity\"] getEntityNumber();\n\
\tlogPrint(\"PROBE b \" + tag + \" \" + id + \" \" + v(s) + \" \" + v(e) + \" \" + f(tr[\"fraction\"]) + \" \" + v(tr[\"normal\"]) + \" \" + tr[\"surfacetype\"] + \" \" + ent + \" \" + v(tr[\"position\"]) + \"\\n\");\n\
\tpt = physicsTrace(s, e);\n\
\tlogPrint(\"PROBE p \" + tag + \" \" + id + \" \" + v(pt) + \"\\n\");\n}}\n\n\
place(tag, id, o)\n{{\n\
\tent = spawn(\"script_origin\", o);\n\
\tfrom = ent.origin;\n\
\tent placeSpawnPoint();\n\
\tlogPrint(\"PROBE s \" + tag + \" \" + id + \" \" + ent getEntityNumber() + \" \" + v(from) + \" \" + v(ent.origin) + \"\\n\");\n\
\tent delete();\n}}\n\n\
spawns(cls)\n{{\n\
\tarr = getEntArray(cls, \"classname\");\n\
\tfor(i = 0; i < arr.size; i++)\n\t{{\n\
\t\tfrom = arr[i].origin;\n\
\t\tarr[i] placeSpawnPoint();\n\
\t\tlogPrint(\"PROBE s \" + cls + \" \" + i + \" \" + arr[i] getEntityNumber() + \" \" + v(from) + \" \" + v(arr[i].origin) + \"\\n\");\n\
\t}}\n}}\n\n\
start()\n{{\n\tthread run();\n}}\n"
    );
    let per = 25;
    let mut groups = Vec::new();
    let mut body = String::new();
    let mut n = 0;
    let flush = |body: &mut String, groups: &mut Vec<String>| {
        if !body.is_empty() {
            groups.push(std::mem::take(body));
        }
    };
    for (id, (tag, s, e)) in p.segs.iter().enumerate() {
        let _ = writeln!(body, "\tb(\"{tag}\", {id}, {}, {});", lit(*s), lit(*e));
        n += 1;
        if n % per == 0 {
            flush(&mut body, &mut groups);
        }
    }
    flush(&mut body, &mut groups);
    for (id, (tag, o)) in p.places.iter().enumerate() {
        let _ = writeln!(body, "\tplace(\"{tag}\", {id}, {});", lit(*o));
        n += 1;
        if n % per == 0 {
            flush(&mut body, &mut groups);
        }
    }
    flush(&mut body, &mut groups);
    for c in classes {
        groups.push(format!("\tspawns(\"{c}\");\n"));
    }
    let _ = writeln!(out, "run()\n{{\n\twait 0.5;");
    for i in 0..groups.len() {
        let _ = writeln!(out, "\tg{i}();\n\twait 0.05;");
    }
    let _ = writeln!(out, "\tlogPrint(\"PROBE done\\n\");\n}}\n");
    for (i, g) in groups.iter().enumerate() {
        let _ = writeln!(out, "g{i}()\n{{\n{g}}}\n");
    }
    out
}
