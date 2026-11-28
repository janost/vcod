use glam::Vec3;

use super::{capsule_in_tri, point_dist2, segment_dist2, segment_meets_tri};

/// iw3mp.exe's own kernels run on each case by tools/re/x87emu.py.
const VECTORS: &str = include_str!("../../../../cod4/tests/fixtures/clip/postest-1.7-client.txt");
/// cod4_lnxded's inlined triangle test (0x81158fd..) run likewise.
const SERVER_VECTORS: &str =
    include_str!("../../../../cod4/tests/fixtures/clip/postest-1.7-server.txt");

fn floats(hex: &[&str]) -> Vec<f32> {
    hex.iter()
        .map(|h| f32::from_bits(u32::from_str_radix(h, 16).unwrap()))
        .collect()
}

fn v(f: &[f32]) -> Vec3 {
    Vec3::new(f[0], f[1], f[2])
}

/// Every case answers as retail's instructions do, distances to the bit.
#[test]
fn the_position_test_kernels_answer_as_iw3mps() {
    let mut runs = [0usize; 4];
    for line in VECTORS.lines().filter(|l| !l.starts_with('#')) {
        let (ins, out) = line.split_once(" -> ").unwrap();
        let ins: Vec<&str> = ins.split(' ').collect();
        let f = floats(&ins[1..]);
        let tri = || [v(&f[0..]), v(&f[3..]), v(&f[6..])];
        let (k, got, want) = match ins[0] {
            "pt" => (
                0,
                point_dist2(v(&f[9..]), tri()).to_bits(),
                floats(&[out])[0].to_bits(),
            ),
            "ss" => {
                let d = segment_dist2(v(&f[0..]), v(&f[3..]), v(&f[6..]), v(&f[9..]));
                (1, d.to_bits(), floats(&[out])[0].to_bits())
            }
            "cap" => {
                let hit = segment_meets_tri(v(&f[9..]), v(&f[12..]), f[15], tri());
                (2, u32::from(hit), out.parse().unwrap())
            }
            "pos" => {
                let hit = capsule_in_tri(v(&f[9..]), f[12], f[13], tri());
                (3, u32::from(hit), out.parse().unwrap())
            }
            other => panic!("kernel {other}"),
        };
        // A degenerate triangle answers NaN. x87's default NaN is negative
        // (0xffc00000) and AArch64's positive, and no caller reads its sign.
        let nan = |b: u32| k < 2 && f32::from_bits(b).is_nan();
        if !(nan(got) && nan(want)) {
            assert_eq!(got, want, "{line}");
        }
        runs[k] += 1;
    }
    assert!(runs.iter().all(|&n| n > 50), "{runs:?}");
}

/// The server's test answers every case as the client's does, the sphere
/// meeting the point nearest `2 v0 - p` (cod4-collision.md 3.1).
#[test]
fn the_servers_position_test_answers_as_the_clients() {
    let (mut cases, mut solid) = (0, 0);
    for line in SERVER_VECTORS.lines().filter(|l| !l.starts_with('#')) {
        let (ins, out) = line.split_once(" -> ").unwrap();
        let f = floats(&ins.split(' ').collect::<Vec<_>>()[1..]);
        let tri = [v(&f[0..]), v(&f[3..]), v(&f[6..])];
        let want = out == "1";
        assert_eq!(
            capsule_in_tri(v(&f[9..]), f[12], f[13], tri),
            want,
            "{line}"
        );
        cases += 1;
        solid += usize::from(want);
    }
    assert!(cases > 150 && solid > 30, "{cases} cases, {solid} solid");
}
