//! Which entity kinds retail sends with `pos.trBase` in whole units, over
//! every committed snapshot capture (`tests/fixtures/net/snapshots*.bin`):
//! players (`BG_PlayerStateToEntityState`) and temp entities
//! (`G_TempEntity`) always; corpses, items, missiles, script movers, fx,
//! and on UO vehicles, each off a whole unit somewhere (docs/protocol-1.1.md,
//! "A player's `pos.trBase` is whole units").

use std::collections::BTreeMap;
use vcod_common::net::cod1::flags::{
    ET_CORPSE, ET_FX, ET_ITEM, ET_MISSILE, ET_PLAYER, ET_SCRIPTMOVER,
};
use vcod_common::net::protocol::{PROTOCOL_V1, PROTOCOL_V6, PROTOCOL_V22};

/// UO's vehicle `eType`.
const ET_VEHICLE_UO: i32 = 12;

#[test]
fn only_players_and_temp_entities_go_out_in_whole_units() {
    let dir = vcod_common::manifest_path!("/tests/fixtures/net");
    // (version, eType) -> (whole, fractional) entity states.
    let mut by: BTreeMap<(&str, i32), (usize, usize)> = BTreeMap::new();
    let mut files = 0;
    for e in std::fs::read_dir(dir).unwrap() {
        let name = e.unwrap().file_name().into_string().unwrap();
        let Some(tag) = name
            .strip_prefix("snapshots")
            .and_then(|t| t.strip_suffix(".bin"))
        else {
            continue;
        };
        if !std::path::Path::new(&format!("{dir}/gamestate{tag}.bin")).exists() {
            continue;
        }
        let (ver, p, et_events) = if tag.contains("uo") {
            ("uo", &PROTOCOL_V22, 16)
        } else if tag.contains("15") || tag.contains("1.5") {
            ("1.5", &PROTOCOL_V6, 12)
        } else {
            ("1.1", &PROTOCOL_V1, 12)
        };
        files += 1;
        let frames = vcod_common::testing::fixture_frames(
            p,
            &format!("net/gamestate{tag}.bin"),
            &format!("net/snapshots{tag}.bin"),
        );
        for f in frames {
            for (num, ent) in &f.snapshot.entities {
                let et = ent.field_i32(p, "eType");
                let base = [0, 1, 2].map(|i| ent.field_f32(p, &format!("pos.trBase[{i}]")));
                let whole = base.iter().all(|v| v.fract() == 0.0);
                if et == ET_PLAYER || et >= et_events {
                    assert!(whole, "{name} entity {num} eType {et}: {base:?}");
                }
                let k = by.entry((ver, et.min(et_events))).or_default();
                if whole {
                    k.0 += 1;
                } else {
                    k.1 += 1;
                }
            }
        }
    }
    assert!(files > 40, "only {files} captures");
    for (ver, et) in [
        ("1.1", ET_CORPSE),
        ("1.1", ET_ITEM),
        ("1.1", ET_MISSILE),
        ("uo", ET_SCRIPTMOVER),
        ("uo", ET_FX),
        ("uo", ET_VEHICLE_UO),
    ] {
        let (w, f) = by.get(&(ver, et)).copied().unwrap_or_default();
        assert!(f > 0, "{ver} eType {et}: {w} whole, none fractional");
    }
}
