//! Retail's use-key weapon swap (`vcod-cod2`'s `pickup/carentan-swap.txt`,
//! docs/research/cod2-combat.md 6.3) replayed through `Cod2Level` under
//! the same `probe_pickup.gsc`: a kar98k in hand and a thompson in the
//! other slot, an mp40 hanging ahead. The use press drops the kar98k where
//! the mp40 lay, takes the mp40 (`BG_TakePlayerWeapon` empties the hand),
//! sends `a <mp40>`, and the cmds that then name the mp40 raise it. Each
//! snapshot's cmds run before its frame, as its `commandTime` says.
//! Needs `COD2_DIR`. `PICKUP_REPORT=1` prints every snapshot's diff.

use std::rc::Rc;

use glam::Vec3;
use vcod_cod2::net::delta::field_index;
use vcod_cod2::net::fields::{ENTITY_STATE_FIELDS, PLAYER_STATE_FIELDS};
use vcod_cod2::net::{EntityState2, PlayerState2, UserCmd2};
use vcod_cod2::pmove::PlayerState;

use crate::game::cod2::Cod2Level;

const CAPTURE: &str = include_str!("../../../cod2/tests/fixtures/pickup/carentan-swap.txt");
const GSC: &str = include_str!("../../../cod2/tests/fixtures/pickup/probe_pickup.gsc");
/// Retail's mp40 item, and the kar98k the swap dropped.
const ITEM: u32 = 298;
const DROPPED: u32 = 299;

struct Snap {
    time: i32,
    ps: PlayerState2,
    /// The swap's dropped weapon, once sent.
    dropped: Option<EntityState2>,
}

struct Capture {
    cmds: Vec<UserCmd2>,
    snaps: Vec<Snap>,
    item: EntityState2,
    /// Server commands, with the last snapshot's time before them.
    commands: Vec<(i32, String)>,
}

fn parse() -> Capture {
    let mut c = Capture {
        cmds: Vec::new(),
        snaps: Vec::new(),
        item: EntityState2::null(ITEM),
        commands: Vec::new(),
    };
    let mut ps = PlayerState2::default();
    let mut dropped: Option<EntityState2> = None;
    let fields = |e: &mut EntityState2, kv: std::str::SplitWhitespace| {
        for kv in kv {
            let (k, v) = kv.split_once('=').unwrap();
            e.f[field_index(&ENTITY_STATE_FIELDS, k)] = u32::from_str_radix(v, 16).unwrap();
        }
    };
    for line in CAPTURE.lines() {
        if let Some(rest) = line.strip_prefix("# cmd ") {
            let (t, text) = rest.split_once(' ').unwrap();
            c.commands.push((t.parse().unwrap(), text.to_string()));
            continue;
        }
        let mut w = line.split_whitespace();
        match w.next() {
            Some("c") => {
                let n: Vec<i64> = w
                    .enumerate()
                    .map(|(k, t)| {
                        if k == 1 {
                            i64::from_str_radix(t, 16).unwrap()
                        } else {
                            t.parse().unwrap()
                        }
                    })
                    .collect();
                c.cmds.push(UserCmd2 {
                    server_time: n[0] as i32,
                    buttons: n[1] as i32,
                    weapon: n[2] as u8,
                    off_hand_index: n[3] as u8,
                    angles: [n[4] as i32, n[5] as i32, n[6] as i32],
                    forward_move: n[7] as i8,
                    right_move: n[8] as i8,
                });
            }
            Some("s") => {
                let time = w.nth(1).unwrap().parse().unwrap();
                for kv in w {
                    let (k, v) = kv.split_once('=').unwrap();
                    let v = u32::from_str_radix(v, 16).unwrap();
                    let at = |k: &str| k[2..k.len() - 1].parse::<usize>().unwrap();
                    match &k[..2] {
                        "S[" => ps.stats[at(k)] = v as i32,
                        "A[" => ps.ammo[at(k)] = v as i32,
                        "C[" => ps.ammoclip[at(k)] = v as i32,
                        _ => ps.f[field_index(&PLAYER_STATE_FIELDS, k)] = v,
                    }
                }
                c.snaps.push(Snap {
                    time,
                    ps: ps.clone(),
                    dropped,
                });
            }
            Some("e") => match w.next().and_then(|n| n.parse::<u32>().ok()) {
                Some(ITEM) => fields(&mut c.item, w),
                Some(DROPPED) => {
                    let e = dropped.get_or_insert(EntityState2::null(DROPPED));
                    fields(e, w);
                    c.snaps.last_mut().unwrap().dropped = dropped;
                }
                _ => {}
            },
            _ => {}
        }
    }
    c
}

fn i(ps: &PlayerState2, n: &str) -> i32 {
    ps.f[field_index(&PLAYER_STATE_FIELDS, n)] as i32
}

/// The words compared each snapshot.
const WORDS: &[&str] = &[
    "weapon",
    "weaponstate",
    "weapons[0]",
    "weaponslots[0]",
    "eventSequence",
    "events[0]",
    "eventParms[0]",
    "events[1]",
];

/// The replay's diffs, one line per mismatching word and snapshot, the
/// server commands ours sent with the frame they went out on, and where
/// the dropped kar98k lay.
type Replayed = (Vec<String>, Vec<(i32, String)>, Option<Vec3>);

fn replay() -> Option<Replayed> {
    let fs = vcod_common::testing::cod2_fs()?;
    let c = parse();
    let first = &c.snaps[0];
    let mut cvars = crate::cvars::Cvars::new();
    for (k, v) in [
        ("probe_x", "1718"),
        ("probe_y", "1824"),
        ("probe_z", "5"),
        ("developer", "1"),
    ] {
        cvars.set(k, v);
    }
    let mut level = Cod2Level::load_with_script(
        Rc::new(fs),
        "mp_carentan",
        "probe_pickup",
        (
            "maps/mp/gametypes/probe_pickup".to_string(),
            GSC.to_string(),
        ),
        cvars,
        first.time - 2000,
    )
    .expect("load probe_pickup");
    let names: Vec<String> = (1..=6)
        .map(|k| level.rt.host.weapon_name(k).unwrap_or_default())
        .collect();
    assert_eq!(
        names[3..],
        ["kar98k_mp", "thompson_mp", "mp40_mp"],
        "retail's weapon order"
    );
    level.connect(0, "vcodmoves");
    let mut now = first.time - 1950;
    level.run_frame(now);
    level.begin(0);
    while now < first.time {
        now += 50;
        level.run_frame(now);
    }
    // vcod's CoD2 `spawn` of a `weapon_*` classname makes no item: hang
    // one at retail's, still, as the suspended spawn flag leaves it.
    let at = Vec3::new(
        c.item.f32("pos.trBase[0]"),
        c.item.f32("pos.trBase[1]"),
        c.item.f32("pos.trBase[2]"),
    );
    let mp40 = level.rt.host.weapon_index("mp40_mp").unwrap();
    let item = level.rt.with_host(|host, cx| {
        crate::game::item::launch(host, cx, mp40, at, Vec3::ZERO, None).unwrap()
    });
    if let Some(s) = level
        .rt
        .host
        .ents
        .get_mut(item)
        .and_then(|e| e.item.as_mut())
    {
        s.pos = None;
        s.ground = 0;
    }
    let p = level.players[0].as_mut().unwrap();
    p.ps = PlayerState::from_wire(&first.ps);
    let ct = first.ps.f[0] as i32;
    let mut ci = c.cmds.iter().take_while(|k| k.server_time <= ct).count();
    p.oldcmd = UserCmd2 {
        server_time: ct,
        buttons: 0,
        ..c.cmds[ci]
    };
    level.run_frame(first.time);
    let report = std::env::var_os("PICKUP_REPORT").is_some();
    let mut diffs = Vec::new();
    let mut sent = Vec::new();
    let mut dropped_at = None;
    let seq0 = (
        i(&first.ps, "eventSequence"),
        level.wire_playerstate(0, 0).map(|w| i(&w, "eventSequence")),
    );
    for snap in &c.snaps[1..] {
        let ct = snap.ps.f[0] as i32;
        while now + 50 < snap.time {
            now += 50;
            level.run_frame(now);
        }
        while ci < c.cmds.len() && c.cmds[ci].server_time <= ct {
            level.client_think(0, &c.cmds[ci]);
            ci += 1;
        }
        now = snap.time;
        level.run_frame(now);
        for (_, text) in level.rt.take_client_commands() {
            sent.push((now, text));
        }
        let ours = level.wire_playerstate(0, 0).unwrap();
        let retail = &snap.ps;
        for &n in WORDS {
            let (mut a, mut r) = (i(&ours, n), i(retail, n));
            if n == "eventSequence" {
                // Our ring counts from its own start.
                a -= seq0.1.unwrap_or(0);
                r -= seq0.0;
            }
            if n.starts_with("events") && a == 0 && r == 0 {
                (a, r) = (0, 0);
            }
            if a != r {
                diffs.push(format!("{} {n}: ours {a:x} retail {r:x}", snap.time));
            }
        }
        // The weapon's anim, its restart toggle aside.
        let anim = |ps: &PlayerState2| i(ps, "weapAnim") & !0x200;
        if anim(&ours) != anim(retail) {
            diffs.push(format!(
                "{} weapAnim: ours {:x} retail {:x}",
                snap.time,
                i(&ours, "weapAnim"),
                i(retail, "weapAnim")
            ));
        }
        // The swapped weapons' rounds: the kar98k's go with it, the mp40's
        // clip fills. The mp40's reserve is a `rand()` draw between
        // `dropAmmoMin` and `dropAmmoMax`, which no replay repeats.
        for w in ["kar98k_mp", "mp40_mp"] {
            let k = level.rt.host.weapon_index(w).unwrap();
            let d = level.rt.host.weapons.get(k).unwrap();
            let (ca, aa) = (d.clip_index, d.ammo_index);
            let reserve = |ps: &PlayerState2| if w == "mp40_mp" { 0 } else { ps.ammo[aa] };
            let ours_r = (ours.ammoclip[ca], reserve(&ours));
            let retail_r = (retail.ammoclip[ca], reserve(retail));
            if ours_r != retail_r {
                diffs.push(format!(
                    "{} {w} rounds: ours {ours_r:?} retail {retail_r:?}",
                    snap.time
                ));
            }
        }
        if report {
            eprintln!(
                "{} weapon {} st {} anim {:x} | retail {} st {} anim {:x}",
                snap.time,
                i(&ours, "weapon"),
                i(&ours, "weaponstate"),
                i(&ours, "weapAnim"),
                i(retail, "weapon"),
                i(retail, "weaponstate"),
                i(retail, "weapAnim"),
            );
        }
        if snap.dropped.is_some() && dropped_at.is_none() {
            let kar = level.rt.host.weapon_index("kar98k_mp").unwrap() as u8;
            let id = level
                .rt
                .host
                .ents
                .iter_inuse()
                .filter_map(|(id, e)| Some((id, e.item?)))
                .find(|(_, s)| s.index == kar && !s.taken)
                .map(|(id, _)| id);
            dropped_at = id.map(|id| Vec3::from(level.rt.entity_origin_of(id).unwrap()));
        }
    }
    if report {
        for d in &diffs {
            eprintln!("{d}");
        }
    }
    Some((diffs, sent, dropped_at))
}

/// The swap on the wire as retail's: the hand empties on the press, the
/// `a` names the mp40, the mp40 rises (`weaponstate` 1, `weapAnim` 10)
/// on the cmds that ask for it and fires; the kar98k lies where the mp40
/// hung.
#[test]
fn retail_use_key_swap_replays_on_the_cod2_server() {
    let Some((diffs, sent, dropped_at)) = replay() else {
        return;
    };
    let c = parse();
    let retail_a: Vec<&str> = c
        .commands
        .iter()
        .map(|(_, t)| t.as_str())
        .filter(|t| t.starts_with("a "))
        .collect();
    assert_eq!(retail_a, ["a 6"]);
    let ours_a: Vec<&str> = sent
        .iter()
        .map(|(_, t)| t.as_str())
        .filter(|t| t.starts_with("a "))
        .collect();
    assert_eq!(ours_a, retail_a, "the swap selects the new weapon");
    let at = Vec3::new(
        c.item.f32("pos.trBase[0]"),
        c.item.f32("pos.trBase[1]"),
        c.item.f32("pos.trBase[2]"),
    );
    let dropped = dropped_at.expect("the kar98k was dropped");
    assert!(dropped.distance(at) < 1.0, "{dropped} vs {at}");
    assert!(
        diffs.is_empty(),
        "{} diffs:\n{}",
        diffs.len(),
        diffs.join("\n")
    );
}
