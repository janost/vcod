//! `vcod_cod4::skel` against 1.7's own skeleton: the cobra's DObj as
//! `cod4_lnxded` built it on each of 159 helicopter thinks, read under gdb
//! (`fixtures/skel/cobra-idle-gdb.txt`, its header the recipe), rebuilt
//! here from the zone's model and the think's two gun angles: the base
//! locals, the controller's three tags and `DObjCalcSkel`'s composition,
//! word for word (docs/research/cod4-gsc.md 3.2, "The skeleton"). Needs
//! `COD4_DIR`.

use vcod_cod4::assets::model::XModel;
use vcod_cod4::skel::{AnimMat, SkelModel, angles_to_quat};
use vcod_cod4::testing::cod4_install;
use vcod_cod4::walk::{WalkOptions, walk};
use vcod_cod4::zone::Zone;

const DUMP: &str = include_str!("fixtures/skel/cobra-idle-gdb.txt");

/// The cobra's bones the controller sets: `tag_body`, `tag_turret`,
/// `tag_barrel` (vehicle +0x318, +0x31c, +0x324 in the dump's run).
const BODY: usize = 2;
const TURRET: usize = 41;
const BARREL: usize = 48;

fn cobra() -> Option<XModel> {
    let install = cod4_install()?;
    let z = Zone::open_path(&install.zone("english", "mp_crash")).unwrap();
    walk(z, WalkOptions::default())
        .unwrap()
        .assets
        .xmodels
        .into_iter()
        .find(|m| m.name == "vehicle_cobra_helicopter_fly")
}

fn bits(s: &str) -> f32 {
    f32::from_bits(u32::from_str_radix(s, 16).unwrap())
}

#[test]
fn the_cobras_skeleton_matches_retails_word_for_word() {
    let Some(x) = cobra() else {
        return;
    };
    let skel = SkelModel::of(&x).expect("the cobra's arrays");
    assert_eq!(skel.bones(), 54);
    let set = |angles: [f32; 3]| AnimMat {
        quat: angles_to_quat(angles),
        trans: [0.0; 3],
        trans_weight: 0.0,
    };
    let (mut hits, mut words) = (0, 0);
    let mut mats = Vec::new();
    let mut off = Vec::new();
    for line in DUMP.lines().filter(|l| !l.starts_with('#')) {
        let t: Vec<&str> = line.split_whitespace().collect();
        if t[0] == "hit" {
            let (pitch, yaw) = (bits(t[1]), bits(t[2]));
            mats = skel.base_locals();
            mats[BODY] = set([0.0; 3]);
            mats[TURRET] = set([0.0, yaw, 0.0]);
            mats[BARREL] = set([pitch, 0.0, 0.0]);
            skel.calc_skel(&mut mats);
            hits += 1;
            continue;
        }
        let b: usize = t[0].parse().unwrap();
        let m = mats[b];
        let ours = [
            m.quat[0],
            m.quat[1],
            m.quat[2],
            m.quat[3],
            m.trans[0],
            m.trans[1],
            m.trans[2],
            m.trans_weight,
        ];
        for (k, w) in t[1..].iter().enumerate() {
            words += 1;
            if ours[k].to_bits() != bits(w).to_bits() {
                off.push(format!(
                    "hit {hits} bone {b} word {k}: retail {w} ours {:08x}",
                    ours[k].to_bits()
                ));
            }
        }
    }
    assert_eq!(hits, 159);
    assert_eq!(words, 159 * 9 * 8);
    assert!(
        off.is_empty(),
        "{} words off: {:?}",
        off.len(),
        &off[..off.len().min(10)]
    );
}
