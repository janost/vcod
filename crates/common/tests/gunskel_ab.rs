//! The turret gun's skeleton and muzzle tags against 1.5b's and UO's own,
//! read under gdb at the round's muzzle (`fixtures/skel11/`, each header
//! the recipe): every built bone's matrix, `tag_player`'s and
//! `tag_flash`'s world matrices word for word (cod11-turrets.md 13.8).
//! Needs a 1.5 `COD_DIR` for the 1.5b dump and a UO one for UO's.

use glam::Vec3;
use vcod_common::net::protocol::{PROTOCOL_V6, install_protocol};
use vcod_common::pk3::Pk3Fs;
use vcod_common::turretpose::{GunSkel, WorldTag};

const DUMP_15: &str = include_str!("fixtures/skel11/mp_carentan-turret-gun-15-gdb.txt");
const DUMP_UO: &str = include_str!("fixtures/skel11/mp_carentan-turret-gun-uo-gdb.txt");

fn words(t: &[&str]) -> Vec<u32> {
    t.iter()
        .map(|w| u32::from_str_radix(w, 16).unwrap())
        .collect()
}

fn vec(w: &[u32]) -> Vec3 {
    Vec3::new(
        f32::from_bits(w[0]),
        f32::from_bits(w[1]),
        f32::from_bits(w[2]),
    )
}

/// A matrix's three axis rows and origin as words.
fn tag_words(axes: &[Vec3; 3], origin: Vec3) -> Vec<u32> {
    axes.iter()
        .chain([&origin])
        .flat_map(|v| v.to_array().map(f32::to_bits))
        .collect()
}

/// One dumped round: the gun's `angles2`, origin and angles, then each
/// built bone's matrix and the two world tags.
struct Shot {
    angles2: [f32; 3],
    origin: Vec3,
    angles: Vec3,
    bones: Vec<(usize, Vec<u32>)>,
    player: Vec<u32>,
    flash: Vec<u32>,
}

fn shots(dump: &str) -> Vec<Shot> {
    let mut out: Vec<Shot> = Vec::new();
    for line in dump.lines().filter(|l| !l.starts_with('#')) {
        let t: Vec<&str> = line.split_whitespace().collect();
        match t[0] {
            "shot" => out.push(Shot {
                angles2: vec(&words(&t[3..6])).to_array(),
                origin: vec(&words(&t[7..10])),
                angles: vec(&words(&t[11..14])),
                bones: Vec::new(),
                player: Vec::new(),
                flash: Vec::new(),
            }),
            "bone" => {
                let s = out.last_mut().unwrap();
                s.bones.push((t[1].parse().unwrap(), words(&t[2..14])));
            }
            "player" => out.last_mut().unwrap().player = words(&t[1..13]),
            "flash" => out.last_mut().unwrap().flash = words(&t[1..13]),
            _ => {}
        }
    }
    out
}

fn gate(fs: &Pk3Fs, dump: &str) {
    let bones = vcod_common::model::load_bones(fs, "mg42_bipod").unwrap();
    let gun = GunSkel::new("mg42_bipod", &bones).unwrap();
    let shots = shots(dump);
    assert!(!shots.is_empty());
    for s in &shots {
        let ours = gun.build(s.angles2);
        for (i, theirs) in &s.bones {
            let b = &ours[*i];
            assert_eq!(
                &tag_words(&b.axes, b.origin),
                theirs,
                "bone {i} ({}) at {:?}",
                gun.skeleton().bones()[*i].name,
                s.angles2
            );
        }
        let (flash, player) = gun.flash_and_player(s.origin, s.angles, s.angles2);
        let w = |t: WorldTag| tag_words(&t.axes, t.origin);
        assert_eq!(w(player), s.player, "tag_player at {:?}", s.angles2);
        assert_eq!(w(flash), s.flash, "tag_flash at {:?}", s.angles2);
    }
}

#[test]
fn the_gun_skeleton_matches_15b_under_gdb() {
    let Some(fs) = vcod_common::testing::game_fs()
        .filter(|fs| install_protocol(fs).transport.version == PROTOCOL_V6.transport.version)
    else {
        return;
    };
    gate(&fs, DUMP_15);
}

#[test]
fn the_gun_skeleton_matches_uo_under_gdb() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    gate(&fs, DUMP_UO);
}
