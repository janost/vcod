use glam::Vec3;

use super::super::trace::Sweep;
use super::super::{CollBorder, IndexedMesh, MASK_PLAYERSOLID, TraceFloats};
use super::EdgeSweep;
use super::tests::map;

/// iw3mp.exe's edge, corner, point, border and border sphere kernels run on
/// each case by tools/re/x87emu.py (doc 3).
const VECTORS: &str = include_str!("../../../../cod4/tests/fixtures/clip/kernels-1.7-client.txt");

const KERNELS: [&str; 5] = ["edge", "vert", "point", "border", "bsph"];

/// The edge kernel's code and the trace's fraction, normal (bits),
/// `startsolid` and `walkable`.
type Out = (Option<u32>, u32, [u32; 3], bool, bool);

struct Case<'a> {
    line: &'a str,
    kernel: usize,
    f: Vec<f32>,
    ints: Vec<bool>,
    want: Out,
}

fn cases() -> Vec<Case<'static>> {
    let float = |h: &str| f32::from_bits(u32::from_str_radix(h, 16).unwrap());
    let flag = |s: &str| match s {
        "0" => false,
        "1" => true,
        _ => panic!("flag {s}"),
    };
    let mut out = Vec::new();
    for line in VECTORS.lines().filter(|l| !l.starts_with('#')) {
        let (ins, outs) = line.split_once(" -> ").unwrap();
        let ins: Vec<&str> = ins.split(' ').collect();
        let kernel = KERNELS.iter().position(|&k| k == ins[0]).unwrap();
        let n_ints = if ins[0] == "vert" { 2 } else { 1 };
        let (fs, is) = ins[1..].split_at(ins.len() - 1 - n_ints);
        let o: Vec<&str> = outs.split(' ').collect();
        let code = (o[0] != "-").then(|| o[0].parse().unwrap());
        let b = |i: usize| u32::from_str_radix(o[i], 16).unwrap();
        out.push(Case {
            line,
            kernel,
            f: fs.iter().map(|h| float(h)).collect(),
            ints: is.iter().map(|s| flag(s)).collect(),
            want: (code, b(1), [b(2), b(3), b(4)], flag(o[5]), flag(o[6])),
        });
    }
    out
}

/// Runs one case through the client's kernel, or the server's in its
/// place, on the trace work and trace the case names.
fn run(c: &Case, client: bool) -> Out {
    let f = &c.f;
    let v = |i: usize| Vec3::new(f[i], f[i + 1], f[i + 2]);
    let border = || CollBorder {
        dist_eq: [f[0], f[1], f[2]],
        z_base: f[3],
        z_slope: f[4],
        start: f[5],
        length: f[6],
    };
    // start, sweep, |sweep|, |sweep|^2, radius, sphere_z, fraction
    let (start, sweep, len, len2, radius, sphere_z, fraction) = match KERNELS[c.kernel] {
        "edge" => (Vec3::ZERO, v(9), 0.0, 0.0, f[12], 0.0, f[13]),
        "vert" => (Vec3::ZERO, v(6), 0.0, f[9], f[10], 0.0, f[11]),
        "point" => (v(9), v(12), 0.0, 0.0, 0.0, 0.0, f[15]),
        "border" => (v(7), v(10), f[13], f[14], f[15], f[16], f[17]),
        "bsph" => (v(7), v(10), 0.0, f[13], f[14], f[15].abs(), f[16]),
        k => panic!("kernel {k}"),
    };
    let floats = if client {
        TraceFloats::Cod4Client
    } else {
        TraceFloats::Cod4Server
    };
    let mut sw = Sweep::new(
        start,
        start + sweep,
        Vec3::ZERO,
        Vec3::ZERO,
        MASK_PLAYERSOLID,
        floats,
    );
    sw.start = start;
    sw.sweep = sweep;
    sw.sweep_len = len;
    sw.sweep_len2 = len2;
    sw.radius = radius;
    sw.sphere_z = sphere_z;
    let mut code = None;
    let tr = map().with_cx(sw, |cx, tr| {
        tr.fraction = fraction;
        tr.walkable = *c.ints.last().unwrap();
        match KERNELS[c.kernel] {
            "edge" => {
                let r = if client {
                    cx.sweep_edge_client(v(0), v(3), v(6), tr)
                } else {
                    cx.sweep_edge(v(0), v(3), v(6), tr)
                };
                code = Some(match r {
                    EdgeSweep::Struck => 0,
                    EdgeSweep::Clear => 1,
                    EdgeSweep::Beyond(end) => 2 + end as u32,
                });
            }
            "vert" if client => cx.vertex_client(v(0), v(3), c.ints[0], tr),
            "vert" => cx.vertex4(v(0), v(3), c.ints[0], tr),
            "point" => {
                let mesh = IndexedMesh {
                    verts: vec![v(0), v(3), v(6)],
                    tris: vec![[0, 1, 2]],
                    walkable: vec![0; 4],
                };
                if client {
                    cx.point_tri_client(&mesh, 0, tr);
                } else {
                    cx.point_tri4(&mesh, 0, tr);
                }
            }
            "border" if client => cx.capsule_border_client(&border(), tr),
            "border" => cx.capsule_border(&border(), true, tr),
            "bsph" if client => cx.border_sphere_client(&border(), f[15], tr),
            "bsph" => cx.border_sphere(&border(), f[15], tr),
            _ => unreachable!(),
        }
    });
    let n = tr.normal.to_array().map(f32::to_bits);
    (code, tr.fraction.to_bits(), n, tr.startsolid, tr.walkable)
}

/// Every case leaves the trace retail's instructions leave, to the bit.
#[test]
fn the_client_kernels_answer_as_iw3mps() {
    let mut runs = [0usize; 5];
    for c in cases() {
        assert_eq!(run(&c, true), c.want, "{}", c.line);
        runs[c.kernel] += 1;
    }
    assert!(runs.iter().all(|&n| n >= 40), "{runs:?}");
}

/// The server's kernel in a client kernel's place parts from some of its
/// cases: the vectors tell the two float models apart.
#[test]
fn the_servers_kernels_part_from_iw3mps() {
    let mut parted = [0usize; 5];
    for c in cases() {
        if run(&c, false) != c.want {
            parted[c.kernel] += 1;
        }
    }
    assert!(parted.iter().all(|&n| n > 0), "{parted:?}");
}
