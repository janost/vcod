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
/// `ET_MISSILE`, a grenade in the air (`docs/research/cod11-combat.md` 11.1).
const ET_MISSILE: i32 = 4;
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
        bots: 0,
        bots_shoot: false,
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
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
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
    // A usercmd carries absolute view angles, so B has to keep asking to face
    // A; the placement's yaw only survives until B's first cmd. Which way it
    // faces decides where the bullet enters, and the bullet is traced against
    // its posed bones.
    let facing_a = UserCmd {
        angles: [0, 32768, 0], // ANGLE2SHORT(180)
        ..NULL_USERCMD
    };
    let mut step = |sv: &mut vcod_server::Server, ca: &mut _, cb: &mut _| {
        now += Duration::from_millis(50);
        common::step_pair(sv, (&qa, ca), (&qb, cb), now)
    };
    for _ in 0..40 {
        ca.send_frame(&NULL_USERCMD);
        cb.send_frame(&facing_a);
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
    cb.send_frame(&facing_a);
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
    // No pain animation: retail's hit frame keeps the standing idle, or the
    // run the knockback selects for a frame (combat doc, 3.4).
    let legs = sa.entities[&(nb as u32)].field_i32(p, "legsAnim") & 511;
    let legs_name = anims.name(legs).expect("B's legs play an anim");
    assert!(
        !legs_name.contains("pain"),
        "B's legs play {legs_name}; retail plays no pain anim on the server"
    );
    assert_eq!(sa.ps.health(), 100, "A is untouched");

    // Release, wait out fireTime and the knockback, then tap again.
    for _ in 0..30 {
        ca.send_frame(&ads);
        cb.send_frame(&facing_a);
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
    cb.send_frame(&facing_a);
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
        cb.send_frame(&facing_a);
        step(&mut sv, &mut ca, &mut cb);
    }
    let sb = cb.snapshots().newest().unwrap();
    assert_eq!(sb.ps.field_f32(p, "viewHeightCurrent"), 8.0);
    assert_eq!(sb.ps.field_i32(p, "pm_type"), 6);
    ca.send_frame(&fire);
    cb.send_frame(&facing_a);
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
    // (`docs/research/clientstate-wire-format.md`). The dead player's own
    // entity is gone from A's list, the way `SVF_NOCLIENT` takes it off
    // retail's wire on the death frame (combat doc, 5.4).
    let sa = ca.snapshots().newest().unwrap();
    let sb = cb.snapshots().newest().unwrap();
    assert!(
        !sa.entities.contains_key(&(nb as u32)),
        "A is still sent an entity for dead player {nb}"
    );
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
        cb.send_frame(&facing_a);
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
    // `SortRanks`: the row order is score descending, so the killer's row
    // leads (the retail round-restart target capture reads the 0-score row
    // ahead of the -1-score one the same way).
    assert_eq!(cells[0], 0, "A, who scored, is the first row");
    let of = |slot: usize| {
        cells
            .chunks(5)
            .find(|c| c[0] == slot as i64)
            .unwrap_or_else(|| panic!("no row for client {slot}"))
            .to_vec()
    };
    assert_eq!(of(na)[1], 1, "A scored the kill");
    assert_eq!(of(nb)[3], 1, "B's death is in the deaths column");
    assert_eq!(of(na)[3], 0, "A has none");
    assert_eq!(of(nb)[4], 1, "B carries the dead status icon");
    assert_eq!(of(na)[4], 0, "a live player carries none");

    // B respawns on the use button, after dm's two-second wait.
    for _ in 0..50 {
        ca.send_frame(&NULL_USERCMD);
        cb.send_frame(&facing_a);
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
    assert!(
        ca.snapshots()
            .newest()
            .unwrap()
            .entities
            .contains_key(&(nb as u32)),
        "A is not sent the respawned player {nb}"
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
///
/// The corpse it leaves is the same one a bullet leaves: the death animation
/// and a settled trajectory. A client command runs during the packet pass, a
/// frame before `tick` advances the clock, so the body used to be stamped in
/// the past and skipped by its one refresh forever, which left it wearing the
/// pose the player had a frame before it died.
#[test]
fn the_kill_command_suicides_a_player() {
    use vcod_common::net::msg::NULL_USERCMD;

    let Some(fs) = vcod_common::testing::game_fs() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let anims = vcod_common::animtree::PlayerAnims::load(&fs).expect("the player anims");
    let bsp_path = fs.resolve_map(MAP).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(cfg(), now);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
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
    // Where B stood when it asked to die, and the `eFlags` its body carried
    // on the frame it was born: the 0x800 marker rides for 250 ms.
    let death_spot = cb.snapshots().newest().unwrap().ps.origin(p);
    let mut newborn_flags = None;
    cb.send_reliable("kill");
    for _ in 0..10 {
        ca.send_frame(&NULL_USERCMD);
        cb.send_frame(&NULL_USERCMD);
        step(&mut sv, &mut ca, &mut cb);
        seen_a = seen_a.or_else(|| obituary(ca.snapshots().newest().unwrap()));
        seen_b = seen_b.or_else(|| obituary(cb.snapshots().newest().unwrap()));
        if newborn_flags.is_none() {
            newborn_flags = ca
                .snapshots()
                .newest()
                .unwrap()
                .entities
                .get(&FIRST_BODY)
                .map(|c| c.field_i32(p, "eFlags"));
        }
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
    let corpse = sb
        .entities
        .get(&FIRST_BODY)
        .expect("the suicide spawned no corpse");
    // The body wears the death the suicide drew, not the pose B held a frame
    // earlier: the queue's one refresh only fires when the body was born on
    // the frame the snapshot is being built for.
    let legs = corpse.field_i32(p, "legsAnim") & 511;
    let name = anims.name(legs).expect("the corpse plays an anim");
    assert!(
        name.starts_with("pb_stand_death_"),
        "the suicide corpse plays {name}, not a standing death"
    );
    // Settled where B fell: `G_BounceItem`'s first contact writes `trType` 0,
    // no delta, `trTime` 0 and the world as the ground entity, and a retail
    // client clips a body against nothing (`cod11-combat.md` 5.3).
    assert_eq!(corpse.field_i32(p, "pos.trType"), 0);
    assert_eq!(corpse.field_i32(p, "pos.trTime"), 0);
    assert_eq!(corpse.field_i32(p, "groundEntityNum"), 1022);
    for axis in 0..3 {
        assert_eq!(corpse.field_f32(p, &format!("pos.trDelta[{axis}]")), 0.0);
    }
    let at = corpse.origin(p);
    assert!(
        (at[2] - death_spot[2]).abs() < 2.0,
        "the corpse settled at {at:?}, B died at {death_spot:?}"
    );
    // The fresh-corpse marker rides the first frames and the 250 ms think
    // takes it off; ten frames is 500 ms.
    let newborn_flags = newborn_flags.expect("no corpse reached A");
    assert_eq!(newborn_flags & 0x800, 0x800, "the corpse was born unmarked");
    assert_eq!(
        corpse.field_i32(p, "eFlags") & 0x800,
        0,
        "the 250 ms think never cleared the marker"
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

/// `Weapon_Melee` (combat doc, 2.5): a swing at a player inside 64 units
/// traces the same way a bullet does, spawns an `EV_MELEE_HIT` temp entity
/// naming the victim, and hurts it for `meleeDamage + rand()%5` through the
/// hit-location table. The retail melee capture is the numbers: two swings
/// killed a 100-health player at `head` for 81 and 79, and the kill's
/// obituary carried parm 135, `0x80 | MOD_MELEE`
/// (`mp_carentan-tdm-melee-shooter.txt`).
#[test]
fn a_melee_swing_hits_and_the_kill_shows_the_melee_icon() {
    use vcod_common::net::msg::{UserCmd, BUTTON_MELEE, NULL_USERCMD};

    let Some(fs) = vcod_common::testing::game_fs() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let bsp_path = fs.resolve_map(MAP).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(cfg(), now);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
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
    assert!(
        sv.test_clear_line(spot, 0.0, 30.0),
        "no clear 30 units along +x from the spawn"
    );
    sv.place_client(na, spot, 0.0);
    sv.place_client(nb, [spot[0] + 30.0, spot[1], spot[2]], 180.0);
    let facing_a = UserCmd {
        angles: [0, 32768, 0], // ANGLE2SHORT(180)
        ..NULL_USERCMD
    };
    let mut step = |sv: &mut vcod_server::Server, ca: &mut _, cb: &mut _| {
        now += Duration::from_millis(50);
        common::step_pair(sv, (&qa, ca), (&qb, cb), now)
    };
    for _ in 0..40 {
        ca.send_frame(&NULL_USERCMD);
        cb.send_frame(&facing_a);
        step(&mut sv, &mut ca, &mut cb);
    }
    assert_eq!(cb.snapshots().newest().unwrap().ps.health(), 100);

    // One frame of the melee bit every second: the bit is edge-latched, so it
    // has to go back up between swings, and `meleeTime` is 0.65 s.
    let swing = UserCmd {
        buttons: BUTTON_MELEE,
        ..NULL_USERCMD
    };
    let mut hits = 0;
    let mut after_first = None;
    for i in 0..80 {
        ca.send_frame(if i % 20 == 0 { &swing } else { &NULL_USERCMD });
        cb.send_frame(&facing_a);
        step(&mut sv, &mut ca, &mut cb);
        let sa = ca.snapshots().newest().unwrap();
        let landed = sa.entities.values().any(|e| {
            e.field_i32(p, "eType") == 12 + 166 && e.field_i32(p, "otherEntityNum") == nb as i32
        });
        if !landed {
            continue;
        }
        hits += 1;
        if hits == 1 {
            after_first = Some(cb.snapshots().newest().unwrap().ps.health());
        } else {
            break;
        }
    }
    assert_eq!(hits, 2, "two swings should have landed");
    // 50..54 through the head multiplier, truncated: 75..81 off 100.
    let h = after_first.expect("the first swing's health");
    assert!((19..=25).contains(&h), "health after one swing: {h}");

    let sb = cb.snapshots().newest().unwrap();
    assert_eq!(sb.ps.health(), 0);
    assert_eq!(sb.ps.field_i32(p, "pm_type"), 6, "B is dead");
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
    assert_eq!(
        obituary(ca.snapshots().newest().unwrap()),
        Some((nb as i32, na as i32, 135)),
        "the melee obituary the capture measured"
    );
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
}

/// `ANGLE2SHORT`: the wire's 16-bit angle, positive pitch looking down.
fn angle_short(deg: f32) -> i32 {
    (deg * 65536.0 / 360.0) as i32
}

/// The falloff a frag charges a player standing at `feet` from a blast at
/// `at`, with a clear line of sight (combat doc, 14.1). The tests assert
/// against this rather than a constant: where a grenade comes to rest is the
/// bounce's business, so the distance is only known once it has.
/// The stock frag's falloff at a distance, times `CanDamage`'s share, in
/// the server's own f64 arithmetic (`radius_damage`).
fn frag_damage(at: [f32; 3], feet: [f32; 3], fraction: f32) -> i32 {
    let d = dist(at, feet) as f64;
    if d >= 350.0 {
        return 0;
    }
    (fraction as f64 * (5.0 + (1.0 - d / 350.0) * 115.0)) as i32
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

type Client = vcod_common::net::NetClient<common::ClientEnd>;

/// The frag in a client's hands and up: `cmd.weapon` held at its index until
/// the playerstate carries it (combat doc, 1.8), then the raise ridden out,
/// since a trigger held through it is swallowed and then latched (1.4).
fn raise_frag(
    sv: &mut vcod_server::Server,
    step: &mut impl FnMut(&mut vcod_server::Server, &mut Client, &mut Client),
    a: &mut Client,
    b: &mut Client,
    b_cmd: &vcod_common::net::msg::UserCmd,
    aimed: &vcod_common::net::msg::UserCmd,
    frag: u8,
) {
    let p = &PROTOCOL_V1;
    let mut switched = false;
    for _ in 0..80 {
        a.send_frame(aimed);
        b.send_frame(b_cmd);
        step(sv, a, b);
        if a.snapshots()
            .newest()
            .is_some_and(|s| s.ps.field_i32(p, "weapon") == frag as i32)
        {
            switched = true;
            break;
        }
    }
    assert!(switched, "the frag never reached the thrower's hands");
    let mut ready = false;
    for _ in 0..60 {
        a.send_frame(aimed);
        b.send_frame(b_cmd);
        step(sv, a, b);
        if a.snapshots()
            .newest()
            .is_some_and(|s| s.ps.field_i32(p, "weaponstate") == 0)
        {
            ready = true;
            break;
        }
    }
    assert!(ready, "the frag never came up");
}

/// A thrown frag: `cmd.weapon` held at the frag's index until the playerstate
/// carries it (combat doc, 1.8), then the trigger held for a second of cook
/// and released along `aim`, which is a yaw and a downward pitch in degrees.
/// Steps until the fuse goes off and returns where it did.
fn cook_and_throw_down(
    sv: &mut vcod_server::Server,
    step: &mut impl FnMut(&mut vcod_server::Server, &mut Client, &mut Client),
    a: &mut Client,
    b: &mut Client,
    b_cmd: &vcod_common::net::msg::UserCmd,
    frag: u8,
    aim: (f32, f32),
) -> [f32; 3] {
    use vcod_common::net::msg::{UserCmd, BUTTON_ATTACK, NULL_USERCMD};
    let aimed = UserCmd {
        angles: [angle_short(aim.1), angle_short(aim.0), 0],
        weapon: frag,
        ..NULL_USERCMD
    };
    raise_frag(sv, step, a, b, b_cmd, &aimed, frag);
    let cook = UserCmd {
        buttons: BUTTON_ATTACK,
        ..aimed
    };
    for _ in 0..20 {
        a.send_frame(&cook);
        b.send_frame(b_cmd);
        step(sv, a, b);
    }
    for _ in 0..200 {
        a.send_frame(&aimed);
        b.send_frame(b_cmd);
        step(sv, a, b);
        if let Some(x) = sv.pending_explosions().first() {
            let at = x.at.into();
            // One more frame so both clients have been sent the damage the
            // blast did on the frame it went off.
            a.send_frame(&aimed);
            b.send_frame(b_cmd);
            step(sv, a, b);
            return at;
        }
    }
    panic!("the grenade never went off");
}

/// Two clients joined on the map: A left where it spawned, B placed wherever
/// the caller's `b_at` puts it. `None` without the paks.
struct Pair {
    sv: vcod_server::Server,
    ca: Client,
    cb: Client,
    qa: Rc<RefCell<Queues>>,
    qb: Rc<RefCell<Queues>>,
    now: Instant,
}

fn two_placed(b_at: impl Fn(&vcod_server::Server, [f32; 3]) -> [f32; 3]) -> Option<Pair> {
    let fs = vcod_common::testing::game_fs()?;
    let bsp_path = fs.resolve_map(MAP).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(cfg(), now);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let qa = Rc::new(RefCell::new(Queues::default()));
    let qb = Rc::new(RefCell::new(Queues::default()));
    let (ca, cb) = common::join_pair(
        &mut sv,
        &qa,
        &qb,
        &mut now,
        ("allies", "m1carbine_mp"),
        ("allies", "m1carbine_mp"),
    );
    let p = &PROTOCOL_V1;
    let num = |c: &Client| c.snapshots().newest().unwrap().ps.field_i32(p, "clientNum") as usize;
    let (na, nb) = (num(&ca), num(&cb));
    let a_spot = ca.snapshots().newest().unwrap().ps.origin(p);
    sv.place_client(na, a_spot, 0.0);
    sv.place_client(nb, b_at(&sv, a_spot), 180.0);
    Some(Pair {
        sv,
        ca,
        cb,
        qa,
        qb,
        now,
    })
}

fn frag_index() -> u8 {
    vcod_server::configstrings::weapon_index("fraggrenade_mp").expect("the frag in CS 7") as u8
}

/// `G_RadiusDamage` end to end (combat doc, 14.1): a frag thrown at the
/// ground behind the thrower hurts him and the player 150 units the other
/// way, each by the falloff at his own distance from where it came to rest
/// scaled by `CanDamage`'s share of clear probes, which the chair the frag
/// rolls behind on this spawn reads off the same static-model clip a
/// bullet does.
/// Retail's own pair capture is the same arithmetic: a blast at
/// (1329, 3297, -22) left the target at (1192, 3296, -23.9) on health 26, 74
/// off a distance of 137, and the thrower 151 units out on 30.
#[test]
fn a_thrown_grenade_damages_a_player_in_its_blast() {
    use vcod_common::net::msg::{UserCmd, NULL_USERCMD};

    let Some(pair) = two_placed(|sv, spot| {
        assert!(
            sv.test_clear_line(spot, 0.0, 150.0),
            "no clear 150 units along +x from the spawn"
        );
        [spot[0] + 150.0, spot[1], spot[2]]
    }) else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let Pair {
        mut sv,
        mut ca,
        mut cb,
        qa,
        qb,
        mut now,
    } = pair;
    let p = &PROTOCOL_V1;
    let facing_a = UserCmd {
        angles: [0, angle_short(180.0), 0],
        ..NULL_USERCMD
    };
    let mut step = |sv: &mut vcod_server::Server, a: &mut Client, b: &mut Client| {
        now += Duration::from_millis(50);
        common::step_pair(sv, (&qa, a), (&qb, b), now);
    };
    for _ in 0..40 {
        ca.send_frame(&NULL_USERCMD);
        cb.send_frame(&facing_a);
        step(&mut sv, &mut ca, &mut cb);
    }
    assert_eq!(ca.snapshots().newest().unwrap().ps.health(), 100);
    assert_eq!(cb.snapshots().newest().unwrap().ps.health(), 100);
    let a_feet = ca.snapshots().newest().unwrap().ps.origin(p);
    let b_feet = cb.snapshots().newest().unwrap().ps.origin(p);

    let at = cook_and_throw_down(
        &mut sv,
        &mut step,
        &mut ca,
        &mut cb,
        &facing_a,
        frag_index(),
        (180.0, 80.0),
    );
    for (who, feet, cl) in [("thrower", a_feet, &ca), ("target", b_feet, &cb)] {
        let fraction = sv.test_can_damage(at, feet);
        assert!(
            fraction > 0.0,
            "{who} is walled off from the blast, {:.0} units away",
            dist(at, feet)
        );
        let expected = frag_damage(at, feet, fraction);
        assert!(expected > 0, "{who} is out of the blast entirely");
        assert_eq!(
            cl.snapshots().newest().unwrap().ps.health(),
            100 - expected,
            "{who} at {:.0} units took the falloff",
            dist(at, feet)
        );
    }
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
}

/// 5.1 step 5 and 11.3: a player killed with a grenade cooking drops it live
/// where he stood, `r.currentOrigin` with z raised by 40, and the velocity is
/// three `rand()` draws whose signs put x and y in (-480, -160] and z in
/// (-160, 0] -- retail's own skew, not a random direction. The fuse is what
/// was left of `grenadeTimeLeft`, the full 4000 for a cook retail never counts
/// down (1.11).
///
/// The retail capture is
/// `fixtures/playerstate/mp_carentan-tdm-grenade-death-shooter.txt`: the dying
/// player's trace reads `origin=1216.9,1296.0,-7.9 grenadeTimeLeft=4000`, and
/// the missile on the death frame reads
/// `pos=5,1405500,1216.9,1296.0,32.1,-423.0,-214.0,-99.0` -- the +40 on z, a
/// delta inside those ranges, and an explode 3950 ms later.
#[test]
fn a_player_killed_mid_cook_drops_a_live_grenade() {
    use vcod_common::net::msg::{UserCmd, BUTTON_ATTACK, NULL_USERCMD};

    let Some(pair) = two_placed(|sv, spot| {
        assert!(
            sv.test_clear_line(spot, 0.0, 150.0),
            "no clear 150 units along +x from the spawn"
        );
        [spot[0] + 150.0, spot[1], spot[2]]
    }) else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let Pair {
        mut sv,
        mut ca,
        mut cb,
        qa,
        qb,
        mut now,
    } = pair;
    let p = &PROTOCOL_V1;
    let watching = UserCmd {
        angles: [0, angle_short(180.0), 0],
        ..NULL_USERCMD
    };
    let mut step = |sv: &mut vcod_server::Server, a: &mut Client, b: &mut Client| {
        now += Duration::from_millis(50);
        common::step_pair(sv, (&qa, a), (&qb, b), now);
    };
    for _ in 0..40 {
        ca.send_frame(&NULL_USERCMD);
        cb.send_frame(&watching);
        step(&mut sv, &mut ca, &mut cb);
    }
    let frag = frag_index();
    let armed = UserCmd {
        weapon: frag,
        ..NULL_USERCMD
    };
    raise_frag(
        &mut sv, &mut step, &mut ca, &mut cb, &watching, &armed, frag,
    );
    // The pin, and then the trigger stays down for the rest of the run: a
    // release would throw the grenade instead of leaving it in A's hand.
    let cook = UserCmd {
        buttons: BUTTON_ATTACK,
        ..armed
    };
    for _ in 0..10 {
        ca.send_frame(&cook);
        cb.send_frame(&watching);
        step(&mut sv, &mut ca, &mut cb);
    }
    let cooking = ca.snapshots().newest().unwrap();
    assert_eq!(
        cooking.ps.field_i32(p, "grenadeTimeLeft"),
        4000,
        "A pulled the pin"
    );
    let death_spot = cooking.ps.origin(p);

    ca.send_reliable("kill");
    let (mut death_frame, mut drop, mut explode_frame) = (None, None, None);
    for frame in 0..120i32 {
        ca.send_frame(&cook);
        cb.send_frame(&watching);
        step(&mut sv, &mut ca, &mut cb);
        let sa = ca.snapshots().newest().unwrap();
        if death_frame.is_none() && sa.ps.field_i32(p, "pm_type") == 6 {
            death_frame = Some(frame);
        }
        if drop.is_none() {
            // The watching client's own snapshot: a missile is broadcast, so
            // the drop reaches everyone and not just the man who died.
            drop = cb
                .snapshots()
                .newest()
                .unwrap()
                .entities
                .values()
                .find(|e| e.field_i32(p, "eType") == ET_MISSILE)
                .map(|e| {
                    let d = |axis| e.field_f32(p, &format!("pos.trDelta[{axis}]"));
                    (frame, e.origin(p), [d(0), d(1), d(2)])
                });
        }
        if explode_frame.is_none() && !sv.pending_explosions().is_empty() {
            explode_frame = Some(frame);
        }
    }
    let death_frame = death_frame.expect("A never died");
    let (drop_frame, base, delta) = drop.expect("the death dropped no grenade");
    assert!(
        (drop_frame - death_frame).abs() <= 2,
        "the drop is the death's own frame, not {drop_frame} against {death_frame}"
    );
    let want = [death_spot[0], death_spot[1], death_spot[2] + 40.0];
    assert!(
        dist(base, want) < 48.0,
        "the grenade left A's hand at {base:?}, not near {want:?}"
    );
    for axis in [0, 1] {
        assert!(
            (-480.0..=-160.0).contains(&delta[axis]),
            "trDelta[{axis}] is {}, outside 11.3's (-480, -160]",
            delta[axis]
        );
    }
    assert!(
        (-160.0..=0.0).contains(&delta[2]),
        "trDelta[2] is {}, outside 11.3's (-160, 0]",
        delta[2]
    );
    let explode_frame = explode_frame.expect("the dropped grenade never went off");
    // 4000 ms off the clock the drop was stamped with, which for a `kill` is
    // the frame before the one the missile first appears on. Retail's capture
    // has the same gap: the missile arrives with the death and goes off
    // 3944 ms later.
    assert!(
        (78..=82).contains(&(explode_frame - drop_frame)),
        "the full 4000 ms fuse is ~80 frames, not {}",
        explode_frame - drop_frame
    );
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
}

/// A spot the map holds a standing player at, within `100..330` units of
/// `from` and with no line of sight at all from a blast at `from`'s feet:
/// what the wall test needs, found by walking the geometry rather than
/// hardcoded, since a map's furniture is not a fixture.
fn shielded_spot(sv: &vcod_server::Server, from: [f32; 3]) -> Option<[f32; 3]> {
    let blast = [from[0], from[1], from[2] + 8.0];
    for step in 0..72 {
        let (s, c) = (step as f32 * 5.0).to_radians().sin_cos();
        for d in (110..=310).step_by(20) {
            let d = d as f32;
            let Some(feet) =
                sv.test_ground_under([from[0] + c * d, from[1] + s * d, from[2] + 64.0])
            else {
                continue;
            };
            // On the same floor: a spot that fell to a lower level is
            // shielded by the ceiling between them, which says nothing about
            // a wall.
            if (feet[2] - from[2]).abs() > 32.0 || !(100.0..330.0).contains(&dist(feet, from)) {
                continue;
            }
            if sv.test_can_damage(blast, feet) == 0.0 {
                return Some(feet);
            }
        }
    }
    None
}

/// `CanDamage`'s zero arm (combat doc, 14.3): the blast at the thrower's own
/// feet kills him and does nothing at all to the player standing behind a
/// wall, who is inside the radius and outside the second chance's reach.
#[test]
fn a_wall_shields_a_player_from_a_blast() {
    use vcod_common::net::msg::{UserCmd, NULL_USERCMD};

    let Some(pair) = two_placed(|sv, spot| {
        shielded_spot(sv, spot).expect("no shielded spot within 330 units of the spawn")
    }) else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let Pair {
        mut sv,
        mut ca,
        mut cb,
        qa,
        qb,
        mut now,
    } = pair;
    let p = &PROTOCOL_V1;
    let still = UserCmd {
        angles: [0, angle_short(180.0), 0],
        ..NULL_USERCMD
    };
    let mut step = |sv: &mut vcod_server::Server, a: &mut Client, b: &mut Client| {
        now += Duration::from_millis(50);
        common::step_pair(sv, (&qa, a), (&qb, b), now);
    };
    for _ in 0..40 {
        ca.send_frame(&NULL_USERCMD);
        cb.send_frame(&still);
        step(&mut sv, &mut ca, &mut cb);
    }
    let a_feet = ca.snapshots().newest().unwrap().ps.origin(p);
    let b_feet = cb.snapshots().newest().unwrap().ps.origin(p);
    assert_eq!(cb.snapshots().newest().unwrap().ps.health(), 100);

    // Straight down, so the grenade stays where the thrower stands.
    let at = cook_and_throw_down(
        &mut sv,
        &mut step,
        &mut ca,
        &mut cb,
        &still,
        frag_index(),
        (0.0, 90.0),
    );
    let range = dist(at, b_feet);
    assert!(
        range < 350.0,
        "the shielded player has to be inside the radius, not {range:.0} units out"
    );
    assert!(
        range > 350.0 * 0.2,
        "and outside the second chance's reach, not {range:.0} units in"
    );
    assert_eq!(
        sv.test_can_damage(at, b_feet),
        0.0,
        "the wall has to block every probe"
    );
    assert_eq!(
        cb.snapshots().newest().unwrap().ps.health(),
        100,
        "the shielded player took nothing"
    );
    assert!(
        ca.snapshots().newest().unwrap().ps.health() < 100,
        "the thrower stood on it, {:.0} units away",
        dist(at, a_feet)
    );
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
}
