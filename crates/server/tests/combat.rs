//! Two clients on one server, one shooting the other. Stage 6's gate for the
//! path no single-client capture can reach: a hit registered against another
//! sim, the callbacks, the corpse, the dropped weapon, the obituary, the
//! scoreboard and the respawn.
//!
//! Needs `COD_DIR`; without the paks it returns early.

mod common;

use common::Queues;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::protocol::PROTOCOL_V1;
use vcod_common::net::NetEvent;

const MAP: &str = "mp_carentan";
/// `ET_ITEM`, a placed or dropped weapon (`crate::game::wire`).
const ET_ITEM: i32 = 3;
/// The first entity number the body queue uses
/// (`docs/research/cod11-combat.md` section 5.2).
const FIRST_BODY: u32 = 64;

fn cfg() -> vcod_server::ServerConfig {
    vcod_server::ServerConfig {
        map: MAP.into(),
        hostname: "vcod test".into(),
        max_clients: 8,
        gametype: "dm".into(),
        test_entities: 0,
        trace: false,
    }
}

/// The damage path end to end, on the retail hit capture's numbers (combat
/// doc, section 8.4): A puts a carbine round into B's head from 40 units
/// down the sight, and B's next snapshot reads health 33, `damageCount` 67,
/// `EV_PAIN` 33, the flesh impact reaches A and not B; a second round kills,
/// and B reads `pm_type` 6, `EV_DEATH`, the dead yaw toward A, and both are
/// sent the `MOD_HEAD_SHOT` obituary.
///
/// Then what dm's `Callback_PlayerKilled` leaves behind: the corpse, the
/// dropped carbine, A's scoreboard row, and B back on its feet on the use
/// button.
#[test]
fn a_shot_takes_health_and_a_second_one_kills() {
    use vcod_common::net::msg::{UserCmd, BUTTON_ADS, BUTTON_ATTACK, NULL_USERCMD};

    let Some(fs) = vcod_common::testing::game_fs() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let anims = vcod_common::animtree::PlayerAnims::load(&fs).expect("the player anims");
    let bsp_path = fs.resolve_map(MAP).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(cfg(), now);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let qa = Rc::new(RefCell::new(Queues::default()));
    let qb = Rc::new(RefCell::new(Queues::default()));
    let (mut ca, mut cb) = common::join_pair(
        &mut sv,
        &qa,
        &qb,
        &mut now,
        ("allies", "m1carbine_mp"),
        ("allies", "m1carbine_mp"),
    );
    let p = &PROTOCOL_V1;
    let na = ca
        .snapshots()
        .newest()
        .unwrap()
        .ps
        .field_i32(p, "clientNum") as usize;
    let nb = cb
        .snapshots()
        .newest()
        .unwrap()
        .ps
        .field_i32(p, "clientNum") as usize;
    let spot = ca.snapshots().newest().unwrap().ps.origin(p);
    // A faces +x at B, 40 units away and facing back. The sightline is the
    // test's own precondition: a spawn point that moved into a wall would
    // otherwise fail this as "B was not damaged".
    assert!(
        sv.test_clear_line(spot, 0.0, 40.0),
        "no clear 40 units along +x from the spawn"
    );
    sv.place_client(na, spot, 0.0);
    sv.place_client(nb, [spot[0] + 40.0, spot[1], spot[2]], 180.0);
    let mut step = |sv: &mut vcod_server::Server, ca: &mut _, cb: &mut _| {
        now += Duration::from_millis(50);
        common::step_pair(sv, (&qa, ca), (&qb, cb), now)
    };
    for _ in 0..40 {
        ca.send_frame(&NULL_USERCMD);
        cb.send_frame(&NULL_USERCMD);
        step(&mut sv, &mut ca, &mut cb);
    }
    let sb = cb.snapshots().newest().unwrap();
    assert_eq!(sb.ps.health(), 100, "B spawned with full health");
    assert_eq!(sb.ps.max_health(), 100);
    assert_eq!(sb.ps.field_i32(p, "pm_type"), 0);
    // What the stock loadout left B holding, to compare the respawn against.
    let spawn_clip = sb.ps.arrays.ammoclip;
    // The frag's file reads `clipOnly 1`, so it has three in the clip and no
    // reserve at all: retail's spawn line is `clip=3:7,6:3,10:15
    // ammo=3:56,10:400`, with no `ammo` entry for index 6.
    assert_eq!(sb.ps.clip(6), 3, "three frags in the clip");
    assert_eq!(sb.ps.ammo(6), 0, "a clipOnly weapon carries no reserve");
    assert_eq!(sb.ps.ammo(10), 400, "the carbine's reserve is written");
    // The per-life teleport bit, to compare the respawn against.
    let life_eflags = sb.ps.field_i32(p, "eFlags");

    // One tap down the sight: the carbine is semi-automatic.
    let ads = UserCmd {
        buttons: BUTTON_ADS,
        ..NULL_USERCMD
    };
    let fire = UserCmd {
        buttons: BUTTON_ADS | BUTTON_ATTACK,
        ..NULL_USERCMD
    };
    ca.send_frame(&fire);
    cb.send_frame(&NULL_USERCMD);
    step(&mut sv, &mut ca, &mut cb);
    let sa = ca.snapshots().newest().unwrap();
    let sb = cb.snapshots().newest().unwrap();
    assert_eq!(sb.ps.health(), 33, "a head hit takes 67 of 100");
    assert_eq!(sb.ps.field_i32(p, "damageEvent"), 1);
    assert_eq!(sb.ps.field_i32(p, "damageCount"), 67);
    assert_eq!(sb.ps.field_i32(p, "damageYaw"), 0, "A shoots along +x");
    assert_eq!(sb.ps.field_i32(p, "eventSequence"), 1);
    assert_eq!(sb.ps.field_i32(p, "events[0]"), 187);
    assert_eq!(sb.ps.field_i32(p, "eventParms[0]"), 33);
    let vx = sb.ps.field_f32(p, "velocity[0]");
    assert!(vx > 70.0, "the knockback pushes B along the shot, {vx}");
    let flesh = |snap: &vcod_common::net::snapshot::Snapshot| {
        snap.entities
            .values()
            .any(|e| e.field_i32(p, "eType") == 12 + 174 && e.field_i32(p, "surfType") == 7)
    };
    assert!(flesh(sa), "A is sent the flesh impact");
    assert!(!flesh(sb), "the victim is not");
    // The clause a standing player's pain reaches lists one anim, so the
    // random draw has one answer here; only the prone clause lists two.
    let legs = sa.entities[&(nb as u32)].field_i32(p, "legsAnim") & 511;
    assert_eq!(
        anims.name(legs),
        Some("pb_crouch_pain_holdStomach"),
        "B's legs play the stock pain clause"
    );
    assert_eq!(sa.ps.health(), 100, "A is untouched");

    // Release, wait out fireTime, and tap again.
    for _ in 0..10 {
        ca.send_frame(&ads);
        cb.send_frame(&NULL_USERCMD);
        step(&mut sv, &mut ca, &mut cb);
    }
    let sb = cb.snapshots().newest().unwrap();
    assert_eq!(sb.ps.health(), 33);
    assert_eq!(
        sb.ps.field_i32(p, "eventSequence"),
        1,
        "one pain event, not one per frame"
    );
    assert_eq!(
        sb.ps.field_f32(p, "velocity[0]"),
        0.0,
        "the knockback has decayed"
    );

    ca.send_frame(&fire);
    cb.send_frame(&NULL_USERCMD);
    step(&mut sv, &mut ca, &mut cb);
    let sa = ca.snapshots().newest().unwrap();
    let sb = cb.snapshots().newest().unwrap();
    assert_eq!(sb.ps.health(), 0);
    assert_eq!(sb.ps.field_i32(p, "pm_type"), 6);
    assert_eq!(sb.ps.arrays.stats[1], 180, "A stands at bearing 180 from B");
    assert_eq!(sb.ps.field_i32(p, "eventSequence"), 2);
    assert_eq!(sb.ps.field_i32(p, "events[1]"), 189);
    assert_eq!(sb.ps.field_i32(p, "eventParms[1]"), 0);
    assert_eq!(
        sb.ps.field_i32(p, "damageEvent"),
        1,
        "the killing hit leaves the feedback alone"
    );
    assert_eq!(sb.ps.field_i32(p, "torsoAnim"), 512);
    // The `DEATH` clause a standing player reaches lists eight anims and the
    // draw takes one of them; which one is the server's own rng, so the gate
    // pins the block rather than the anim.
    let death_legs = sb.ps.field_i32(p, "legsAnim") & 511;
    let death_name = anims.name(death_legs).expect("B's legs play a death anim");
    assert!(
        death_name.starts_with("pb_stand_death_"),
        "B's legs play {death_name}, not a standing death"
    );
    let obituary = |snap: &vcod_common::net::snapshot::Snapshot| {
        snap.entities
            .values()
            .find(|e| e.field_i32(p, "eType") == 12 + 201)
            .map(|e| {
                (
                    e.field_i32(p, "otherEntityNum"),
                    e.field_i32(p, "attackerEntityNum"),
                    e.field_i32(p, "eventParm"),
                )
            })
    };
    let expect = Some((nb as i32, na as i32, 0x88));
    assert_eq!(obituary(sa), expect, "A's obituary");
    assert_eq!(obituary(sb), expect, "B's obituary");
    assert_eq!(sv.client_field(nb, "sessionstate").as_deref(), Some("dead"));

    // Dead: the eye drops to 8, and a third round finds nobody.
    for _ in 0..10 {
        ca.send_frame(&ads);
        cb.send_frame(&NULL_USERCMD);
        step(&mut sv, &mut ca, &mut cb);
    }
    let sb = cb.snapshots().newest().unwrap();
    assert_eq!(sb.ps.field_f32(p, "viewHeightCurrent"), 8.0);
    assert_eq!(sb.ps.field_i32(p, "pm_type"), 6);
    ca.send_frame(&fire);
    cb.send_frame(&NULL_USERCMD);
    step(&mut sv, &mut ca, &mut cb);
    let sb = cb.snapshots().newest().unwrap();
    assert_eq!(
        sb.ps.field_i32(p, "eventSequence"),
        2,
        "a corpse takes no more hits"
    );

    // `cloneplayer` filled the body queue's first slot, and both clients are
    // sent it: the dead client's number rides on the corpse, which is how the
    // receiving client picks the body model
    // (`docs/research/clientstate-wire-format.md`).
    let sa = ca.snapshots().newest().unwrap();
    let sb = cb.snapshots().newest().unwrap();
    for (who, snap) in [("A", sa), ("B", sb)] {
        let corpse = snap
            .entities
            .get(&FIRST_BODY)
            .unwrap_or_else(|| panic!("{who} was sent no corpse"));
        assert_eq!(
            corpse.field_i32(p, "eType"),
            2,
            "{who}'s corpse is ET_CORPSE"
        );
        assert_eq!(corpse.field_i32(p, "clientNum"), nb as i32);
        // The body keeps the anim the death drew: the clone is re-read after
        // the script frame exactly so the corpse lies the way B fell.
        assert_eq!(
            corpse.field_i32(p, "legsAnim") & 511,
            death_legs,
            "{who}'s corpse plays {death_name}"
        );
    }

    // `dropItem` put B's carbine on the ground where it fell. The map's own
    // placed weapons are `ET_ITEM` too, so the drop is the one within a stride
    // of B's origin.
    let death_spot = cb.snapshots().newest().unwrap().ps.origin(p);
    let dropped = ca
        .snapshots()
        .newest()
        .unwrap()
        .entities
        .values()
        .find(|e| {
            e.field_i32(p, "eType") == ET_ITEM
                && (0..3).all(|i| (e.origin(p)[i] - death_spot[i]).abs() < 32.0)
        })
        .cloned()
        .expect("no dropped weapon at B's death spot");
    // `m1carbine_mp` is configstring 7's twelfth entry, and a placed weapon's
    // `index` is that 1-based number.
    assert_eq!(dropped.field_i32(p, "index"), 12, "B dropped its carbine");
    // The rounds went with it. Retail's death frame reads `clip=3:7,6:3
    // ammo=3:56`: the carbine's index 10 is gone from both arrays, while the
    // pistol's and the frag's entries stand (combat doc, 9.1).
    let dead = cb.snapshots().newest().unwrap();
    assert_eq!(dead.ps.clip(10), 0, "the dropped carbine's clip");
    assert_eq!(dead.ps.ammo(10), 0, "the dropped carbine's reserve");
    assert_eq!(dead.ps.clip(3), 7, "the pistol's clip stands");
    assert_eq!(dead.ps.ammo(3), 56, "the pistol's reserve stands");

    // The killed callback ran all the way to `respawn()`: no thread died on a
    // builtin we do not have.
    assert_eq!(sv.script_aborts(), Vec::<String>::new());

    // The scoreboard: A scored one, and B carries the dead status icon, the
    // first slot dm's `precacheStatusIcon` took
    // (`docs/research/cod11-hud-protocol.md` section 3).
    ca.send_reliable("score");
    let mut row = None;
    for _ in 0..10 {
        ca.send_frame(&NULL_USERCMD);
        cb.send_frame(&NULL_USERCMD);
        let (ea, _) = step(&mut sv, &mut ca, &mut cb);
        for e in ea {
            if let NetEvent::ServerCommand(t) = e {
                if t.first().map(String::as_str) == Some("b") {
                    row = Some(t);
                }
            }
        }
    }
    let row = row.expect("no scoreboard reply");
    // b <numRows> <axis> <allies>{ <client> <score> <ping> <time> <icon>}*
    assert_eq!(row[1], "2", "two clients online");
    // Retail's dm capture reads 0 in both slots: dm never calls
    // `setTeamScore` and `level.teamScores[]` starts zeroed.
    assert_eq!(row[2], "0", "dm sets no team scores");
    assert_eq!(row[3], "0");
    let cells: Vec<i64> = row[4..].iter().map(|s| s.parse().unwrap()).collect();
    let of = |slot: usize| {
        cells
            .chunks(5)
            .find(|c| c[0] == slot as i64)
            .unwrap_or_else(|| panic!("no row for client {slot}"))
            .to_vec()
    };
    assert_eq!(of(na)[1], 1, "A scored the kill");
    assert_eq!(of(nb)[4], 1, "B carries the dead status icon");
    assert_eq!(of(na)[4], 0, "a live player carries none");

    // B respawns on the use button, after dm's two-second wait.
    for _ in 0..50 {
        ca.send_frame(&NULL_USERCMD);
        cb.send_frame(&NULL_USERCMD);
        step(&mut sv, &mut ca, &mut cb);
    }
    let use_ = UserCmd {
        buttons: vcod_common::net::msg::BUTTON_USE,
        ..NULL_USERCMD
    };
    for _ in 0..20 {
        ca.send_frame(&NULL_USERCMD);
        cb.send_frame(&use_);
        step(&mut sv, &mut ca, &mut cb);
    }
    let sb = cb.snapshots().newest().unwrap();
    assert_eq!(sb.ps.field_i32(p, "pm_type"), 0, "B is playing again");
    assert_eq!(sb.ps.health(), 100);
    assert_eq!(
        sb.ps.arrays.ammoclip, spawn_clip,
        "the respawn re-gave the loadout"
    );
    assert_eq!(sb.ps.clip(10), 15, "the carbine's clip is full again");
    // Retail alternates `eFlags` 16 and 24 across lives; a client breaks
    // interpolation on the changed word (combat doc, 9.2).
    assert_ne!(
        sb.ps.field_i32(p, "eFlags") & 0x8,
        life_eflags & 0x8,
        "the respawn did not flip the teleport bit"
    );
    assert_eq!(
        sb.ps.field_i32(p, "eventSequence"),
        0,
        "the respawn cleared the event ring"
    );
    assert_ne!(
        sb.ps.origin(p),
        death_spot,
        "the respawn moved B off the corpse"
    );
    assert_eq!(
        sv.client_field(nb, "sessionstate").as_deref(),
        Some("playing")
    );

    // And the respawned player shoots: the weapon machine came back ready,
    // not stuck in whatever state the death left it.
    let seq = sb.ps.field_i32(p, "eventSequence");
    let attack = UserCmd {
        buttons: BUTTON_ATTACK,
        ..NULL_USERCMD
    };
    for _ in 0..2 {
        ca.send_frame(&NULL_USERCMD);
        cb.send_frame(&attack);
        step(&mut sv, &mut ca, &mut cb);
    }
    let sb = cb.snapshots().newest().unwrap();
    assert!(
        sb.ps.field_i32(p, "eventSequence") > seq,
        "the respawned player raised no event"
    );
    // `EV_FIRE_WEAPON` / `EV_FIRE_WEAPON_LASTSHOT`.
    assert!(
        (0..4).any(|i| {
            let e = sb.ps.field_i32(p, &format!("events[{i}]"));
            e == 159 || e == 161
        }),
        "the respawned player did not fire"
    );
    assert_eq!(sb.ps.clip(10), 14, "one round out of the fresh clip");
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
}

/// `Cmd_Kill_f`: the `kill` client command, the death half of the retail hit
/// capture. B asks to die; B's next snapshot reads `pm_type` 6 and both
/// clients are sent the obituary with victim == attacker and the
/// `MOD_SUICIDE` parm 0x96 the capture measured (`cod11-combat.md` 8.3).
#[test]
fn the_kill_command_suicides_a_player() {
    use vcod_common::net::msg::NULL_USERCMD;

    let Some(fs) = vcod_common::testing::game_fs() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let bsp_path = fs.resolve_map(MAP).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(cfg(), now);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let qa = Rc::new(RefCell::new(Queues::default()));
    let qb = Rc::new(RefCell::new(Queues::default()));
    let (mut ca, mut cb) = common::join_pair(
        &mut sv,
        &qa,
        &qb,
        &mut now,
        ("allies", "m1carbine_mp"),
        ("allies", "m1carbine_mp"),
    );
    let p = &PROTOCOL_V1;
    let na = ca
        .snapshots()
        .newest()
        .unwrap()
        .ps
        .field_i32(p, "clientNum") as usize;
    let nb = cb
        .snapshots()
        .newest()
        .unwrap()
        .ps
        .field_i32(p, "clientNum") as usize;
    let mut step = |sv: &mut vcod_server::Server, ca: &mut _, cb: &mut _| {
        now += Duration::from_millis(50);
        common::step_pair(sv, (&qa, ca), (&qb, cb), now)
    };
    // The two spawn wherever dm put them; nothing here needs a sightline.
    for _ in 0..40 {
        ca.send_frame(&NULL_USERCMD);
        cb.send_frame(&NULL_USERCMD);
        step(&mut sv, &mut ca, &mut cb);
    }
    assert_eq!(cb.snapshots().newest().unwrap().ps.health(), 100);

    let obituary = |snap: &vcod_common::net::snapshot::Snapshot| {
        snap.entities
            .values()
            .find(|e| e.field_i32(p, "eType") == 12 + 201)
            .map(|e| {
                (
                    e.field_i32(p, "otherEntityNum"),
                    e.field_i32(p, "attackerEntityNum"),
                    e.field_i32(p, "eventParm"),
                )
            })
    };
    // The obituary is a temp entity: it rides one frame and is gone, so it is
    // caught as it passes rather than read off the last snapshot.
    let (mut seen_a, mut seen_b) = (None, None);
    cb.send_reliable("kill");
    for _ in 0..10 {
        ca.send_frame(&NULL_USERCMD);
        cb.send_frame(&NULL_USERCMD);
        step(&mut sv, &mut ca, &mut cb);
        seen_a = seen_a.or_else(|| obituary(ca.snapshots().newest().unwrap()));
        seen_b = seen_b.or_else(|| obituary(cb.snapshots().newest().unwrap()));
    }
    let sb = cb.snapshots().newest().unwrap();
    assert_eq!(sb.ps.health(), 0, "the kill command took B's health");
    assert_eq!(sb.ps.field_i32(p, "pm_type"), 6, "B is dead");
    assert_eq!(sv.client_field(nb, "sessionstate").as_deref(), Some("dead"));
    // `EV_DEATH` with the literal 0 parm (combat doc, 5.1 item 7).
    assert!(
        (0..4).any(|i| sb.ps.field_i32(p, &format!("events[{i}]")) == 189),
        "B raised no EV_DEATH"
    );
    // `MOD_SUICIDE` is index 22 and one of the seven the `0x80` flag covers.
    let expect = Some((nb as i32, nb as i32, 0x96));
    assert_eq!(seen_a, expect, "A's obituary");
    assert_eq!(seen_b, expect, "B's obituary");
    assert_eq!(sv.script_aborts(), Vec::<String>::new());

    // A corpse landed in the queue's first slot, the same as a shot death.
    assert!(
        sb.entities.contains_key(&FIRST_BODY),
        "the suicide spawned no corpse"
    );

    // A scored nothing: a suicide is not a frag.
    ca.send_reliable("score");
    let mut row = None;
    for _ in 0..10 {
        ca.send_frame(&NULL_USERCMD);
        cb.send_frame(&NULL_USERCMD);
        let (ea, _) = step(&mut sv, &mut ca, &mut cb);
        for e in ea {
            if let NetEvent::ServerCommand(t) = e {
                if t.first().map(String::as_str) == Some("b") {
                    row = Some(t);
                }
            }
        }
    }
    let row = row.expect("no scoreboard reply");
    let cells: Vec<i64> = row[4..].iter().map(|s| s.parse().unwrap()).collect();
    let score = |slot: usize| {
        cells
            .chunks(5)
            .find(|c| c[0] == slot as i64)
            .unwrap_or_else(|| panic!("no row for client {slot}"))[1]
    };
    assert_eq!(score(na), 0, "A scored nothing off B's suicide");

    // A second `kill` while dead does nothing.
    cb.send_reliable("kill");
    for _ in 0..5 {
        ca.send_frame(&NULL_USERCMD);
        cb.send_frame(&NULL_USERCMD);
        step(&mut sv, &mut ca, &mut cb);
    }
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
    assert_eq!(
        cb.snapshots().newest().unwrap().ps.field_i32(p, "pm_type"),
        6
    );
}
