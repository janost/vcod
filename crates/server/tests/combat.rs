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
use vcod_common::net::NetEvent;
use vcod_common::net::protocol::PROTOCOL_V1;

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
/// `EV_PAIN` 33, the flesh impact reaches A and the client impact B; a second round kills,
/// and B reads `pm_type` 6, `EV_DEATH`, the dead yaw toward A, and both are
/// sent the `MOD_HEAD_SHOT` obituary.
///
/// Then what dm's `Callback_PlayerKilled` leaves behind: the corpse, the
/// dropped carbine, A's scoreboard row, and B back on its feet on the use
/// button.
#[test]
fn a_shot_takes_health_and_a_second_one_kills() {
    use vcod_common::net::msg::{BUTTON_ADS, BUTTON_ATTACK, NULL_USERCMD, UserCmd};

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
    // A seed whose spread carries the round on past B into a wall A sees.
    sv.test_seed_rng(1);
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
    // 80 along the shot, less one frame of friction: B's packet came in
    // behind A's, so its 50 ms cmd ran after the hit and outlasted the 50 ms
    // slide the knockback started (combat doc, 4.5 and 16).
    let vx = sb.ps.field_f32(p, "velocity[0]");
    assert!(
        (40.0..80.0).contains(&vx),
        "the knockback pushes B along the shot, {vx}"
    );
    // The carbine is a `rifleBullet` weapon, so the large pair: 174 for
    // everyone but the victim, 176 for the victim alone. The round goes on
    // through B (combat doc 2.4, step 5), and the 174 it may leave on the
    // world behind is not the flesh copy.
    let event = |snap: &vcod_common::net::snapshot::Snapshot, ev: i32| {
        snap.entities
            .values()
            .find(|e| e.field_i32(p, "eType") == 12 + ev && e.field_i32(p, "surfType") == 7)
            .cloned()
    };
    let plain = event(sa, 174).expect("A is sent the flesh impact");
    // B's callback runs before the round's next leg is traced, so the flesh
    // pair it raises numbers below the wall impact behind B (combat doc 2.4).
    let number = |snap: &vcod_common::net::snapshot::Snapshot, flesh: bool| {
        snap.entities
            .iter()
            .find(|(_, e)| {
                e.field_i32(p, "eType") == 12 + 174 && (e.field_i32(p, "surfType") == 7) == flesh
            })
            .map(|(n, _)| *n)
    };
    let wall = number(sa, false).expect("the round goes on through B into the world");
    assert!(
        number(sa, true).unwrap() < wall,
        "the flesh impact numbers below the wall behind"
    );
    assert!(event(sb, 174).is_none(), "the victim is not");
    let client = event(sb, 176).expect("the victim is sent the client impact");
    assert!(event(sa, 176).is_none(), "A is not");
    let along_x = vcod_common::net::events::dir_to_byte([1.0, 0.0, 0.0]);
    for (name, want) in [
        ("surfType", 7),
        ("otherEntityNum", na as i32),
        ("eventParm", along_x),
        ("_union.scale", along_x),
        ("clientNum", 0),
    ] {
        assert_eq!(plain.field_i32(p, name), want, "174 {name}");
    }
    for (name, want) in [
        ("surfType", 7),
        ("otherEntityNum", na as i32),
        ("eventParm", 0),
        ("_union.scale", 0),
        ("clientNum", nb as i32),
    ] {
        assert_eq!(client.field_i32(p, name), want, "176 {name}");
    }
    assert_eq!(plain.origin(p), client.origin(p), "both at the hit point");
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
    // B's cmd behind the shot disarms it, as retail's bullet death reads
    // `events=187,189,155` (combat doc, 8.4).
    assert_eq!(sb.ps.field_i32(p, "eventSequence"), 3);
    assert_eq!(sb.ps.field_i32(p, "events[1]"), 189);
    assert_eq!(sb.ps.field_i32(p, "eventParms[1]"), 0);
    assert_eq!(sb.ps.field_i32(p, "events[2]"), 155);
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
        3,
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

    // `dropItem` threw B's carbine from where it fell; it flies about 90
    // units before it lands (docs/research/cod11-items.md 14). The map's own
    // placed weapons are `ET_ITEM` too, and so is the health pack the death
    // drops at B's feet, so the drop is the carbine near B's origin.
    // `m1carbine_mp` is configstring 7's twelfth entry, and a placed weapon's
    // `index` is that 1-based number.
    let death_spot = cb.snapshots().newest().unwrap().ps.origin(p);
    ca.snapshots()
        .newest()
        .unwrap()
        .entities
        .values()
        .find(|e| {
            e.field_i32(p, "eType") == ET_ITEM
                && e.field_i32(p, "index") == 12
                && (0..3).all(|i| (e.origin(p)[i] - death_spot[i]).abs() < 160.0)
        })
        .expect("B dropped its carbine near its death spot");
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
            if let NetEvent::ServerCommand(t) = e
                && t.first().map(String::as_str) == Some("b")
            {
                row = Some(t);
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
/// dm's `K;` line carries the weapon, damage and hit location retail's
/// `player_die` call passes (5.1).
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
    let kills: Vec<&String> = sv
        .script_log()
        .iter()
        .filter(|l| l.starts_with("K;"))
        .collect();
    assert_eq!(kills.len(), 1, "{kills:?}");
    assert!(
        kills[0]
            .trim_end()
            .ends_with(";none;100000;MOD_SUICIDE;none"),
        "{}",
        kills[0]
    );

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
            if let NetEvent::ServerCommand(t) = e
                && t.first().map(String::as_str) == Some("b")
            {
                row = Some(t);
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

/// The `kill` death frame, field for field against retail's. Retail runs a
/// packet's client commands ahead of its usercmds, and those cmds still read
/// the `pm_type` 0 the last end frame wrote: the carbine the death dropped
/// goes down to empty hands with `EV_RAISE_WEAPON` behind `EV_DEATH`, and the
/// eye target is still the standing one. The `dm` hit-target capture reads
/// `events=189,155` and `eventSequence` 2 off 0 on all three deaths; the
/// three-probe follow run read `weapon` 0 and `viewHeightTarget` 60 there and
/// 8 a frame later (`cod11-spectator-follow.md` 5 and 11).
#[test]
fn the_kill_commands_death_frame_is_retails() {
    use vcod_common::net::msg::NULL_USERCMD;

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
    let carbine = vcod_server::configstrings::weapon_index("m1carbine_mp").unwrap() as u8;
    // The byte a retail client sends: the weapon it holds.
    let holding = vcod_common::net::msg::UserCmd {
        weapon: carbine,
        ..NULL_USERCMD
    };
    let mut step = |sv: &mut vcod_server::Server, ca: &mut _, cb: &mut _| {
        now += Duration::from_millis(50);
        common::step_pair(sv, (&qa, ca), (&qb, cb), now)
    };
    for _ in 0..40 {
        ca.send_frame(&NULL_USERCMD);
        cb.send_frame(&holding);
        step(&mut sv, &mut ca, &mut cb);
    }
    let alive = cb.snapshots().newest().unwrap().clone();
    assert_eq!(alive.ps.field_i32(p, "weapon"), i32::from(carbine));
    let seq = alive.ps.field_i32(p, "eventSequence");

    cb.send_reliable("kill");
    ca.send_frame(&NULL_USERCMD);
    cb.send_frame(&holding);
    step(&mut sv, &mut ca, &mut cb);
    let death = cb.snapshots().newest().unwrap().clone();
    assert_eq!(death.ps.field_i32(p, "pm_type"), 6, "the kill's own frame");
    assert_eq!(death.ps.health(), 0);
    let event = |s: &vcod_common::net::snapshot::Snapshot, at: i32| {
        s.ps.field_i32(p, &format!("events[{}]", at & 3))
    };
    assert_eq!(death.ps.field_i32(p, "eventSequence"), seq + 2);
    assert_eq!(
        (event(&death, seq), event(&death, seq + 1)),
        (189, 155),
        "EV_DEATH, then the disarm's EV_RAISE_WEAPON"
    );
    assert_eq!(death.ps.field_i32(p, "weapon"), 0);
    assert_eq!(death.ps.field_i32(p, "viewHeightTarget"), 60);
    assert_eq!(death.ps.field_f32(p, "viewHeightCurrent"), 60.0);

    ca.send_frame(&NULL_USERCMD);
    cb.send_frame(&holding);
    step(&mut sv, &mut ca, &mut cb);
    let after = cb.snapshots().newest().unwrap();
    assert_eq!(after.ps.field_i32(p, "viewHeightTarget"), 8);
    assert!(after.ps.field_f32(p, "viewHeightCurrent") < 60.0);
    assert_eq!(after.ps.field_i32(p, "eventSequence"), seq + 2);

    // The respawn frame already carries the standing idle: every respawn in
    // the capture reads `legsAnim` 634 on its first frame (combat doc, 9.2).
    for _ in 0..50 {
        ca.send_frame(&NULL_USERCMD);
        cb.send_frame(&holding);
        step(&mut sv, &mut ca, &mut cb);
    }
    let use_ = vcod_common::net::msg::UserCmd {
        buttons: vcod_common::net::msg::BUTTON_USE,
        ..holding
    };
    let mut respawn = None;
    for _ in 0..40 {
        ca.send_frame(&NULL_USERCMD);
        cb.send_frame(&use_);
        step(&mut sv, &mut ca, &mut cb);
        let s = cb.snapshots().newest().unwrap();
        if s.ps.field_i32(p, "pm_type") == 0 {
            respawn = Some(s.clone());
            break;
        }
    }
    let respawn = respawn.expect("B never respawned");
    assert_eq!(respawn.ps.health(), 100);
    assert_eq!(respawn.ps.field_i32(p, "eventSequence"), 0);
    assert_eq!(respawn.ps.field_i32(p, "legsAnim"), 634);
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
}

/// The retail capture's bullet death frame, as `key=value` pairs: the first
/// `!trace` of `mp_carentan-tdm-hit-target.txt` that reads health 0 with a
/// damage event behind it (combat doc, 8.4).
fn retail_bullet_death() -> std::collections::BTreeMap<String, String> {
    let text = include_str!("fixtures/playerstate/mp_carentan-tdm-hit-target.txt");
    let line = text
        .lines()
        .filter_map(|l| l.strip_prefix("!trace "))
        .find(|l| l.contains(" health=0 ") && l.contains(" damageEvent=1 "))
        .expect("the capture's bullet death");
    line.split_whitespace()
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// A bullet death frame against retail's. The shooter's packet runs ahead of
/// the victim's, as `SV_ExecuteClientMessage` runs packets in arrival order,
/// and the round and its damage callback run inside the shooter's cmd, so
/// the victim's cmd behind it still moves at `pm_type` 0 and disarms: the
/// capture reads `EV_PAIN` from the first hit, `EV_DEATH`, then
/// `EV_RAISE_WEAPON` (combat doc, 8.4 and 16).
#[test]
fn a_bullet_deaths_frame_is_retails() {
    use vcod_common::net::msg::{BUTTON_ADS, BUTTON_ATTACK, NULL_USERCMD, UserCmd};

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
    let carbine = vcod_server::configstrings::weapon_index("m1carbine_mp").unwrap() as u8;
    let num = |c: &vcod_common::net::NetClient<common::ClientEnd>| {
        c.snapshots().newest().unwrap().ps.field_i32(p, "clientNum") as usize
    };
    let (na, nb) = (num(&ca), num(&cb));
    let spot = ca.snapshots().newest().unwrap().ps.origin(p);
    assert!(
        sv.test_clear_line(spot, 0.0, 40.0),
        "no clear 40 units along +x from the spawn"
    );
    sv.place_client(na, spot, 0.0);
    sv.place_client(nb, [spot[0] + 40.0, spot[1], spot[2]], 180.0);
    // Both hold the carbine, as a retail client's cmd says (AGENTS.md).
    let sight = UserCmd {
        buttons: BUTTON_ADS,
        weapon: carbine,
        ..NULL_USERCMD
    };
    let fire = UserCmd {
        buttons: BUTTON_ADS | BUTTON_ATTACK,
        ..sight
    };
    let facing_a = UserCmd {
        angles: [0, 32768, 0],
        weapon: carbine,
        ..NULL_USERCMD
    };
    let mut step = |sv: &mut vcod_server::Server, a: &UserCmd, b: &UserCmd| {
        ca.send_frame(a);
        cb.send_frame(b);
        now += Duration::from_millis(50);
        common::step_pair(sv, (&qa, &mut ca), (&qb, &mut cb), now);
        (
            ca.snapshots().newest().unwrap().clone(),
            cb.snapshots().newest().unwrap().clone(),
        )
    };
    for _ in 0..40 {
        step(&mut sv, &sight, &facing_a);
    }
    // The seed `a_shot_takes_health_and_a_second_one_kills` puts both rounds
    // into B's head with: 67 and 67, the capture's own two hits.
    sv.test_seed_rng(1);
    let (_, hit) = step(&mut sv, &fire, &facing_a);
    assert_eq!(hit.ps.health(), 33, "the first round, the capture's 67");
    assert_eq!(hit.ps.field_i32(p, "eventSequence"), 1);
    for _ in 0..30 {
        step(&mut sv, &sight, &facing_a);
    }
    let (_, death) = step(&mut sv, &fire, &facing_a);
    let retail = retail_bullet_death();
    let ours = |name: &str| death.ps.field_i32(p, name).to_string();
    let list = |name: &str| {
        (0..4)
            .map(|i| death.ps.field_i32(p, &format!("{name}[{i}]")).to_string())
            .collect::<Vec<_>>()
            .join(",")
    };
    for (field, value) in [
        ("health", death.ps.health().to_string()),
        ("pm_type", ours("pm_type")),
        ("eventSequence", ours("eventSequence")),
        ("events", list("events")),
        ("eventParms", list("eventParms")),
        ("damageEvent", ours("damageEvent")),
        ("damageCount", ours("damageCount")),
        ("torsoAnim", ours("torsoAnim")),
    ] {
        assert_eq!(value, retail[field], "the death frame's {field}");
    }
    assert_eq!(death.ps.field_i32(p, "weapon"), 0, "the disarm ran");
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
}

/// A player one packet's round kills is out of the way of a later packet's
/// round in the same frame: `player_die` leaves the body `CONTENTS_CORPSE`,
/// which the shot mask leaves out (combat doc, 16). A kills B, then C's
/// round, fired along the same line from beyond B, reaches A. With every
/// shot traced after every move, B still stood in it and took the round.
#[test]
fn a_player_killed_earlier_in_the_frame_stops_no_later_round() {
    use vcod_common::net::msg::{BUTTON_ADS, BUTTON_ATTACK, NULL_USERCMD, UserCmd};

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
    let q: [Rc<RefCell<Queues>>; 3] = Default::default();
    // The thompson is no `rifleBullet`, so its round stops on the first
    // player it meets (combat doc 2.4, step 5).
    let [mut ca, mut cb, mut cc] = common::join_trio(
        &mut sv,
        [&q[0], &q[1], &q[2]],
        &mut now,
        [
            ("allies", "thompson_mp"),
            ("allies", "m1carbine_mp"),
            ("allies", "thompson_mp"),
        ],
    );
    let p = &PROTOCOL_V1;
    let num = |c: &vcod_common::net::NetClient<common::ClientEnd>| {
        c.snapshots().newest().unwrap().ps.field_i32(p, "clientNum") as usize
    };
    let (na, nb, nc) = (num(&ca), num(&cb), num(&cc));
    // `probe_passthru`'s flat brush floor, clear 300 units along +x.
    let spot = [1032.0, -376.0, -151.875];
    assert!(
        sv.test_clear_line(spot, 0.0, 80.0),
        "no clear 80 units along +x from the spawn"
    );
    sv.place_client(na, spot, 0.0);
    sv.place_client(nb, [spot[0] + 40.0, spot[1], spot[2]], 180.0);
    sv.place_client(nc, [spot[0] + 80.0, spot[1], spot[2]], 180.0);
    let thompson = vcod_server::configstrings::weapon_index("thompson_mp").unwrap() as u8;
    let carbine = vcod_server::configstrings::weapon_index("m1carbine_mp").unwrap() as u8;
    let a_sight = UserCmd {
        buttons: BUTTON_ADS,
        weapon: thompson,
        ..NULL_USERCMD
    };
    let c_sight = UserCmd {
        angles: [0, 32768, 0],
        ..a_sight
    };
    let fire = |c: UserCmd| UserCmd {
        buttons: BUTTON_ADS | BUTTON_ATTACK,
        ..c
    };
    let b_idle = UserCmd {
        angles: [0, 32768, 0],
        weapon: carbine,
        ..NULL_USERCMD
    };
    let mut step = |sv: &mut vcod_server::Server, a: UserCmd, c: UserCmd| {
        ca.send_frame(&a);
        cb.send_frame(&b_idle);
        cc.send_frame(&c);
        now += Duration::from_millis(50);
        common::step_trio(
            sv,
            (common::ADDR, &q[0], &mut ca),
            (common::ADDR_B, &q[1], &mut cb),
            (common::ADDR_C, &q[2], &mut cc),
            now,
        );
        [&ca, &cb, &cc].map(|c| c.snapshots().newest().unwrap().clone())
    };
    for _ in 0..40 {
        step(&mut sv, a_sight, c_sight);
    }
    sv.test_seed_rng(1);
    let [_, b, _] = step(&mut sv, fire(a_sight), c_sight);
    let hurt = b.ps.health();
    assert!((1..100).contains(&hurt), "A's first round left B at {hurt}");
    for _ in 0..30 {
        step(&mut sv, a_sight, c_sight);
    }
    // One frame: A's packet kills B, then C fires down the same line.
    let [a, b, _] = step(&mut sv, fire(a_sight), fire(c_sight));
    assert_eq!(b.ps.field_i32(p, "pm_type"), 6, "A's second round killed B");
    assert!(
        a.ps.health() < 100,
        "C's round stopped on B's body instead of reaching A"
    );
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
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
    use vcod_common::net::msg::{BUTTON_MELEE, NULL_USERCMD, UserCmd};

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
        sv.test_clear_line(spot, 0.0, 40.0),
        "no clear 40 units along +x from the spawn"
    );
    sv.place_client(na, spot, 0.0);
    // 30 is StuckInClient's exact touch threshold (2 * HALF_WIDTH); 40 stays clear of it and inside the 64-unit melee reach.
    sv.place_client(nb, [spot[0] + 40.0, spot[1], spot[2]], 180.0);
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
    // A temp entity rides every snapshot until it is freed, so a hit is the
    // snapshot its entity first appears on.
    let mut shown: Vec<u32> = Vec::new();
    for i in 0..80 {
        ca.send_frame(if i % 20 == 0 { &swing } else { &NULL_USERCMD });
        cb.send_frame(&facing_a);
        step(&mut sv, &mut ca, &mut cb);
        let sa = ca.snapshots().newest().unwrap();
        let now: Vec<u32> = sa
            .entities
            .iter()
            .filter(|(_, e)| {
                e.field_i32(p, "eType") == 12 + 166 && e.field_i32(p, "otherEntityNum") == nb as i32
            })
            .map(|(n, _)| *n)
            .collect();
        let landed = now.iter().any(|n| !shown.contains(n));
        shown = now;
        if !landed {
            continue;
        }
        hits += 1;
        if hits == 1 {
            let sb = cb.snapshots().newest().unwrap();
            after_first = Some(sb.ps.health());
            // The swing's weapon is the carbine, a `weaponType` bullet, so
            // `finishPlayerDamage` raises the flesh pair beside the melee hit,
            // as the 2026-09-27 retail melee run read (combat doc 4.5).
            let has = |snap: &vcod_common::net::snapshot::Snapshot, ev: i32| {
                snap.entities
                    .values()
                    .any(|e| e.field_i32(p, "eType") == 12 + ev)
            };
            assert!(has(sa, 174) && !has(sa, 176), "A is sent the plain copy");
            assert!(has(sb, 176) && !has(sb, 174), "B is sent the client copy");
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
/// the server's own f64 arithmetic (`Blast::hit`).
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
    use vcod_common::net::msg::{BUTTON_ATTACK, NULL_USERCMD, UserCmd};
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
    two_placed_under(None, b_at)
}

/// [`two_placed`] with `gametype`, `(name, source)`, run instead of stock
/// dm.
fn two_placed_under(
    gametype: Option<(&str, &str)>,
    b_at: impl Fn(&vcod_server::Server, [f32; 3]) -> [f32; 3],
) -> Option<Pair> {
    let fs = vcod_common::testing::game_fs()?;
    let bsp_path = fs.resolve_map(MAP).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut config = cfg();
    let mut sv = match gametype {
        Some((name, src)) => {
            config.gametype = name.into();
            let mut sv = vcod_server::Server::new(config, now);
            sv.overlay_script(&format!("maps/mp/gametypes/{name}"), src);
            sv
        }
        None => vcod_server::Server::new(config, now),
    };
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
/// scaled by `CanDamage`'s share of clear probes. The chair the frag rolls
/// behind on this spawn blocks probes through the same static-model clip a
/// bullet meets, and the thrower's own body, standing between the blast and
/// the target, blocks more of the target's (14.4).
/// Retail's own pair capture is the same arithmetic: a blast at
/// (1329, 3297, -22) left the target at (1192, 3296, -23.9) on health 26, 74
/// off a distance of 137, and the thrower 151 units out on 30.
#[test]
fn a_thrown_grenade_damages_a_player_in_its_blast() {
    use vcod_common::net::msg::{NULL_USERCMD, UserCmd};

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
    let num = |c: &Client| c.snapshots().newest().unwrap().ps.field_i32(p, "clientNum") as usize;
    for (who, feet, cl) in [("thrower", a_feet, &ca), ("target", b_feet, &cb)] {
        let fraction = sv.test_can_damage_among_players(at, feet, num(cl));
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
    assert!(
        sv.test_can_damage_among_players(at, b_feet, num(&cb)) < sv.test_can_damage(at, b_feet),
        "the thrower's body stood in the target's line"
    );
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
}

/// A grenade's walk measures each victim on its turn (combat doc 14.5):
/// the first victim's damage callback sets the other down out of reach,
/// and the walk passes it by. Retail, `client-probes/probe_blastmove`:
/// every grenade walk whose first callback parked the others logged no
/// later victim.
#[test]
fn a_grenade_walk_meets_a_victim_where_an_earlier_callback_moved_it() {
    use vcod_common::net::msg::{NULL_USERCMD, UserCmd};

    const PARK: &str = r#"
main()
{
	thread wrap();
	maps\mp\gametypes\dm::main();
}

wrap()
{
	wait 0.05;
	level.probe_damage = level.callbackPlayerDamage;
	level.callbackPlayerDamage = ::park;
}

park(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc)
{
	logPrint("PROBE gcb " + self getEntityNumber() + " " + iDamage + "\n");
	if (!isdefined(level.probe_parked))
	{
		level.probe_parked = 1;
		players = getentarray("player", "classname");
		for (i = 0; i < players.size; i++)
		{
			if (players[i] != self)
				players[i] setorigin((224, -1280, 1.86));
		}
	}
	[[level.probe_damage]](eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc);
}
"#;
    let Some(pair) = two_placed_under(Some(("probe_park", PARK)), |sv, spot| {
        assert!(
            sv.test_clear_line(spot, 0.0, 150.0),
            "no clear 150 units along +x from the spawn"
        );
        [spot[0] + 150.0, spot[1], spot[2]]
    }) else {
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
    cook_and_throw_down(
        &mut sv,
        &mut step,
        &mut ca,
        &mut cb,
        &facing_a,
        frag_index(),
        (180.0, 80.0),
    );
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
    let hits: Vec<&String> = sv
        .script_log()
        .iter()
        .filter(|l| l.contains("PROBE gcb"))
        .collect();
    assert_eq!(hits.len(), 1, "one victim, the other parked: {hits:?}");
}

/// A grenade goes off in `G_RunFrame`'s entity pass, after the frame's
/// threads, so its walk meets the links this frame's `setOrigin`s made
/// (combat doc 14.7). The thread glues both players over the grenade every
/// frame, side by side so one area node holds both, in an order that flips
/// with the frame's parity: the walk's first victim is the one linked last
/// on the frame it went off. Retail, `client-probes/probe_blastmove`: three
/// walks out of three.
#[test]
fn a_grenade_walk_meets_this_frame_s_script_links_first() {
    use vcod_common::net::msg::{NULL_USERCMD, UserCmd};

    const GLUE: &str = r#"
main()
{
	thread glue();
	maps\mp\gametypes\dm::main();
}

glue()
{
	wait 0.05;
	level.callbackPlayerDamage = ::hit;
	for (;;)
	{
		grenades = getentarray("grenade", "classname");
		if (grenades.size > 0)
		{
			players = getentarray("player", "classname");
			for (k = 0; k < players.size; k++)
			{
				i = k;
				if (gettime() % 100 != 0)
					i = players.size - 1 - k;
				players[i] setorigin(grenades[0].origin + (0, 32 * i - 16, 80));
			}
		}
		wait 0.05;
	}
}

hit(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc)
{
	logPrint("PROBE hit " + gettime() + " " + self getEntityNumber() + "\n");
}
"#;
    let Some(pair) = two_placed_under(Some(("probe_glue", GLUE)), |sv, spot| {
        assert!(
            sv.test_clear_line(spot, 0.0, 150.0),
            "no clear 150 units along +x from the spawn"
        );
        [spot[0] + 150.0, spot[1], spot[2]]
    }) else {
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
    cook_and_throw_down(
        &mut sv,
        &mut step,
        &mut ca,
        &mut cb,
        &facing_a,
        frag_index(),
        (180.0, 80.0),
    );
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
    let hits: Vec<(i32, usize)> = sv
        .script_log()
        .iter()
        .filter_map(|l| {
            let mut f = l.strip_prefix("PROBE hit ")?.split_whitespace();
            Some((f.next()?.parse().ok()?, f.next()?.parse().ok()?))
        })
        .collect();
    assert_eq!(hits.len(), 2, "both glued players in the walk: {hits:?}");
    let (time, first) = hits[0];
    assert_eq!(hits[1].0, time, "one walk: {hits:?}");
    // `getEntArray` lists the players by entity number; the loop links them
    // in that order on a frame at a multiple of 100 ms, reversed otherwise.
    let mut order = [hits[0].1.min(hits[1].1), hits[0].1.max(hits[1].1)];
    if time % 100 != 0 {
        order.reverse();
    }
    assert_eq!(
        first, order[1],
        "the walk starts at the player linked last on {time}: {hits:?}"
    );
}

/// Combat doc 14.7, "One entity pass": `G_RunFrame` runs items, links and
/// missiles in one loop by entity number, so a blast's callbacks read the
/// re-anchor of a child numbered below the grenade from this frame and that
/// of a child numbered above it from the last one. The parent moves 8 units
/// a frame. The map's spawn points sit below the grenade's number and the
/// children spawned once it is live above it.
#[test]
fn a_blast_meets_the_links_below_its_grenade_s_number_only() {
    use vcod_common::net::msg::NULL_USERCMD;

    const GLUE: &str = r#"
main()
{
	thread glue();
	maps\mp\gametypes\dm::main();
}

glue()
{
	wait 0.05;
	level.callbackPlayerDamage = ::hit;
	level.p = spawn("script_origin", (0, 0, 1000));
	level.kids = getentarray("mp_deathmatch_spawn", "classname");
	for (i = 0; i < level.kids.size; i++)
	{
		level.kids[i] enablelinkto();
		hang(level.kids[i]);
	}
	for (;;)
	{
		level.p.origin = level.p.origin + (0, 0, 8);
		grenades = getentarray("grenade", "classname");
		if (grenades.size > 0)
		{
			if (!isdefined(level.g))
			{
				level.g = grenades[0] getEntityNumber();
				for (i = 0; i < 16; i++)
					kid();
			}
			players = getentarray("player", "classname");
			for (i = 0; i < players.size; i++)
				players[i] setorigin(grenades[0].origin + (0, 32 * i - 16, 80));
		}
		wait 0.05;
	}
}

kid()
{
	k = spawn("script_origin", level.p.origin - (0, 0, 100));
	level.kids[level.kids.size] = k;
	hang(k);
}

hang(k)
{
	k linkto(level.p);
	k.gap = level.p.origin[2] - k.origin[2];
}

hit(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc)
{
	for (i = 0; i < level.kids.size; i++)
		logPrint("PROBE link " + gettime() + " " + level.g + " " + level.kids[i] getEntityNumber() + " " + (level.p.origin[2] - level.kids[i].origin[2] - level.kids[i].gap) + "\n");
}
"#;
    let Some(pair) = two_placed_under(Some(("probe_glue", GLUE)), |sv, spot| {
        assert!(
            sv.test_clear_line(spot, 0.0, 150.0),
            "no clear 150 units along +x from the spawn"
        );
        [spot[0] + 150.0, spot[1], spot[2]]
    }) else {
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
    let facing_a = vcod_common::net::msg::UserCmd {
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
    cook_and_throw_down(
        &mut sv,
        &mut step,
        &mut ca,
        &mut cb,
        &facing_a,
        frag_index(),
        (180.0, 80.0),
    );
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
    let rows: Vec<[i32; 4]> = sv
        .script_log()
        .iter()
        .filter_map(|l| {
            let mut f = l.strip_prefix("PROBE link ")?.split_whitespace();
            let mut n = || f.next()?.parse::<f32>().ok().map(|x| x.round() as i32);
            Some([n()?, n()?, n()?, n()?])
        })
        .collect();
    let first = rows.first().expect("a blast callback ran")[0];
    let walk: Vec<_> = rows.iter().filter(|r| r[0] == first).collect();
    assert!(
        walk.iter().any(|r| r[2] < r[1]) && walk.iter().any(|r| r[2] > r[1]),
        "children on both sides of the grenade: {walk:?}"
    );
    for [_, grenade, child, gap] in walk {
        let want = if child < grenade { 0 } else { 8 };
        assert_eq!(*gap, want, "child {child}, grenade {grenade}: {rows:?}");
    }
}

/// 11.2 to 11.4: a throw's first frame on the wire, pinned to retail's pair
/// capture `fixtures/playerstate/mp_carentan-tdm-grenade-shooter.txt`. Both
/// throws there stand still at z -23.9 and read `trBase` z 37: the muzzle is
/// the snapped origin plus 60, not the eye truncated, which would read 36.
/// `trTime` is the frame before the one the missile first reaches the wire
/// on, and `trDelta` is whole because the thrower stood still.
#[test]
fn a_throw_s_first_frame_matches_retail_s_capture() {
    use vcod_common::net::msg::{BUTTON_ATTACK, NULL_USERCMD, UserCmd};

    // (feet, view pitch and yaw, retail's trBase, retail's trDelta), the two
    // `!missile` lines that follow each throw's release.
    let throws = [
        (
            [1443.8, 3394.7, -23.9],
            (12.3, -158.6),
            [1443.0, 3394.0, 37.0],
            [-873.0, -342.0, -85.0],
        ),
        (
            [1449.0, 3399.2, -23.9],
            (45.0, -68.1),
            [1449.0, 3399.0, 37.0],
            [253.0, -629.0, -558.0],
        ),
    ];
    for (feet, (pitch, yaw), want_base, want_delta) in throws {
        let Some(pair) = two_placed(|_, _| [1192.0, 3296.0, -23.0]) else {
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
        let na = ca
            .snapshots()
            .newest()
            .unwrap()
            .ps
            .field_i32(p, "clientNum") as usize;
        sv.place_client(na, feet, yaw);
        let mut step = |sv: &mut vcod_server::Server, a: &mut Client, b: &mut Client| {
            now += Duration::from_millis(50);
            common::step_pair(sv, (&qa, a), (&qb, b), now);
        };
        let frag = frag_index();
        let aimed = UserCmd {
            angles: [angle_short(pitch), angle_short(yaw), 0],
            weapon: frag,
            ..NULL_USERCMD
        };
        raise_frag(
            &mut sv,
            &mut step,
            &mut ca,
            &mut cb,
            &NULL_USERCMD,
            &aimed,
            frag,
        );
        let cook = UserCmd {
            buttons: BUTTON_ATTACK,
            ..aimed
        };
        for _ in 0..20 {
            ca.send_frame(&cook);
            cb.send_frame(&NULL_USERCMD);
            step(&mut sv, &mut ca, &mut cb);
        }
        let stood = ca.snapshots().newest().unwrap().ps.origin(p);
        assert!(
            dist(stood, feet) < 0.2,
            "the thrower settled at {stood:?}, not retail's {feet:?}"
        );
        let mut first = None;
        for _ in 0..40 {
            ca.send_frame(&aimed);
            cb.send_frame(&NULL_USERCMD);
            step(&mut sv, &mut ca, &mut cb);
            let snap = ca.snapshots().newest().unwrap();
            if let Some(e) = snap
                .entities
                .values()
                .find(|e| e.field_i32(p, "eType") == ET_MISSILE)
            {
                let v = |f: &str| e.field_f32(p, f);
                first = Some((
                    snap.server_time,
                    e.field_i32(p, "pos.trTime"),
                    [v("pos.trBase[0]"), v("pos.trBase[1]"), v("pos.trBase[2]")],
                    [
                        v("pos.trDelta[0]"),
                        v("pos.trDelta[1]"),
                        v("pos.trDelta[2]"),
                    ],
                ));
                break;
            }
        }
        let (server_time, tr_time, base, delta) = first.expect("the release threw nothing");
        assert_eq!(
            tr_time,
            server_time - 50,
            "trTime is the level.time the throw cmd ran under"
        );
        assert_eq!(base, want_base, "trBase off the muzzle at {feet:?}");
        // The capture's view angles are printed to a tenth of a degree, which
        // at 960 units a second is most of a unit on each component.
        assert!(
            dist(delta, want_delta) < 2.0,
            "trDelta {delta:?} against retail's {want_delta:?}"
        );
        assert_eq!(sv.script_aborts(), Vec::<String>::new());
    }
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
/// delta inside those ranges, and an explode 3950 ms later. The `kill` runs
/// outside every cmd, so `r.currentOrigin` is the unsnapped `ps.origin`:
/// 1216.9 and 32.1 where a snap would read 1216 and 33 (5.5).
#[test]
fn a_player_killed_mid_cook_drops_a_live_grenade() {
    use vcod_common::net::msg::{BUTTON_ATTACK, NULL_USERCMD, UserCmd};

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
    // Off the unit grid on both horizontal axes, so the drop's origin tells
    // the unsnapped `ps.origin` from the snapped one (combat doc 5.5).
    let a_now = &ca.snapshots().newest().unwrap().ps;
    let a_spot = a_now.origin(p);
    sv.place_client(
        a_now.field_i32(p, "clientNum") as usize,
        [a_spot[0].trunc() + 0.6, a_spot[1].trunc() + 0.3, a_spot[2]],
        0.0,
    );
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
    assert!(
        death_spot[0].fract() != 0.0 && death_spot[1].fract() != 0.0,
        "A settled on the unit grid at {death_spot:?}, where a snap is invisible"
    );
    let want = [death_spot[0], death_spot[1], death_spot[2] + 40.0];
    assert!(
        dist(base, want) < 0.01,
        "the grenade left A's hand at {base:?}, not at the unsnapped {want:?}"
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
    use vcod_common::net::msg::{NULL_USERCMD, UserCmd};

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

/// A spectator in slot 0, connected first and never joined, following B, and
/// the A/B pair of the shot test beside it: A faces +x at B, 40 units away.
struct Followed {
    sv: vcod_server::Server,
    q: [Rc<RefCell<Queues>>; 3],
    cs: Client,
    ca: Client,
    cb: Client,
    now: Instant,
    nb: usize,
}

impl Followed {
    fn new() -> Option<Self> {
        let fs = vcod_common::testing::game_fs()?;
        let bsp_path = fs.resolve_map(MAP).expect("map in the mounted paks");
        let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
        let mut now = Instant::now();
        let mut sv = vcod_server::Server::new(cfg(), now);
        sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        let q: [Rc<RefCell<Queues>>; 3] = Default::default();
        let cs = common::connect_at(&mut sv, common::ADDR_C, &q[0], &mut now, 0x2000);
        let ca = Client::start_with_qport(common::ClientEnd(q[1].clone()), now, 0x2001);
        let cb = Client::start_with_qport(common::ClientEnd(q[2].clone()), now, 0x2002);
        let mut r = Followed {
            sv,
            q,
            cs,
            ca,
            cb,
            now,
            nb: 0,
        };
        let mut ja = common::Join::new("allies", "m1carbine_mp");
        let mut jb = common::Join::new("allies", "m1carbine_mp");
        let null = vcod_common::net::msg::NULL_USERCMD;
        for _ in 0..600 {
            let [_, ea, eb] = r.step(&null, &null, &null);
            for (events, join, cl) in [(ea, &mut ja, &mut r.ca), (eb, &mut jb, &mut r.cb)] {
                for e in events {
                    if let NetEvent::ServerCommand(tokens) = e {
                        join.on_server_command(&tokens, cl, r.now);
                    }
                }
            }
            if ja.settled(r.now) && jb.settled(r.now) {
                break;
            }
        }
        assert!(
            ja.settled(r.now) && jb.settled(r.now),
            "the pair never joined"
        );
        let num = |c: &Client| {
            c.snapshots()
                .newest()
                .unwrap()
                .ps
                .field_i32(&PROTOCOL_V1, "clientNum") as usize
        };
        assert_eq!(num(&r.cs), 0, "the spectator connected first");
        let (na, nb) = (num(&r.ca), num(&r.cb));
        r.nb = nb;
        // `probe_passthru`'s flat brush floor, whatever dm spawned the pair on.
        let spot = [1132.0, -376.0, -151.875];
        assert!(
            r.sv.test_clear_line(spot, 0.0, 40.0),
            "no clear 40 units along +x from the spawn"
        );
        r.sv.place_client(na, spot, 0.0);
        r.sv.place_client(nb, [spot[0] + 40.0, spot[1], spot[2]], 180.0);
        // Attack taps cycle the follow forward until it lands on B.
        let attack = vcod_common::net::msg::UserCmd {
            buttons: vcod_common::net::msg::BUTTON_ATTACK,
            ..null
        };
        for _ in 0..4 {
            if r.followed() == Some(nb) {
                break;
            }
            let (a, b) = r.holding();
            r.step(&attack, &a, &b);
            r.step(&null, &a, &b);
            r.step(&null, &a, &b);
        }
        assert_eq!(r.followed(), Some(nb), "the spectator never followed B");
        for _ in 0..20 {
            let (a, b) = r.holding();
            r.step(&null, &a, &b);
        }
        Some(r)
    }

    /// The slot the spectator's frame copies, `None` while it flies free.
    fn followed(&self) -> Option<usize> {
        let s = self.cs.snapshots().newest()?;
        (s.ps.field_i32(&PROTOCOL_V1, "pm_flags") & 0x10000 != 0)
            .then(|| s.ps.field_i32(&PROTOCOL_V1, "clientNum") as usize)
    }

    /// A still and B facing back at it, each sending the weapon it holds.
    fn holding(
        &self,
    ) -> (
        vcod_common::net::msg::UserCmd,
        vcod_common::net::msg::UserCmd,
    ) {
        let mut b = common::holding(&self.cb);
        b.angles = [0, angle_short(180.0), 0];
        (common::holding(&self.ca), b)
    }

    /// One frame, each client's server commands back in slot order.
    fn step(
        &mut self,
        s: &vcod_common::net::msg::UserCmd,
        a: &vcod_common::net::msg::UserCmd,
        b: &vcod_common::net::msg::UserCmd,
    ) -> [Vec<NetEvent>; 3] {
        self.now += Duration::from_millis(50);
        self.cs.send_frame(s);
        self.ca.send_frame(a);
        self.cb.send_frame(b);
        let (es, ea, eb) = common::step_trio(
            &mut self.sv,
            (common::ADDR_C, &self.q[0], &mut self.cs),
            (common::ADDR, &self.q[1], &mut self.ca),
            (common::ADDR_B, &self.q[2], &mut self.cb),
            self.now,
        );
        [es, ea, eb]
    }

    /// Frames until the spectator's copy of B reads health 0, with the
    /// scoreboards each client was sent on each: the death frame's index and
    /// every `b` by frame, client and tokens.
    fn until_dead(
        &mut self,
        mut cmds: impl FnMut(
            usize,
            &Self,
        ) -> (
            vcod_common::net::msg::UserCmd,
            vcod_common::net::msg::UserCmd,
        ),
    ) -> (usize, Vec<(usize, usize, Vec<String>)>) {
        let mut pushed = Vec::new();
        for frame in 0..80 {
            let (a, b) = cmds(frame, self);
            let events = self.step(&vcod_common::net::msg::NULL_USERCMD, &a, &b);
            for (client, events) in events.into_iter().enumerate() {
                for e in events {
                    if let NetEvent::ServerCommand(t) = e
                        && t.first().map(String::as_str) == Some("b")
                    {
                        pushed.push((frame, client, t));
                    }
                }
            }
            if self.cs.snapshots().newest().unwrap().ps.health() == 0 {
                assert_eq!(self.followed(), Some(self.nb));
                return (frame, pushed);
            }
        }
        panic!("B did not die");
    }

    /// B's `(score, deaths)` in a `b` scoreboard's rows.
    fn row_of_b(&self, b: &[String]) -> (i64, i64) {
        let cells: Vec<i64> = b[4..].iter().map(|s| s.parse().unwrap()).collect();
        let row = cells
            .chunks(5)
            .find(|c| c[0] == self.nb as i64)
            .expect("no row for B");
        (row[1], row[3])
    }
}

/// `player_die`'s walk (combat doc 5.1 step 9) on a bullet death: the
/// spectator following B is pushed the scoreboard once, in the packet of the
/// frame whose copy of B first reads health 0, with the kill already scored,
/// and the players are pushed none. The retail follow run read its `b` on that
/// frame for a head shot and for a `kill` (`cod11-spectator-follow.md` 9).
#[test]
fn a_shot_death_pushes_the_scoreboard_to_the_victims_follower() {
    use vcod_common::net::msg::{BUTTON_ADS, BUTTON_ATTACK};
    let Some(mut r) = Followed::new() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    // Two taps down the sight 30 frames apart: two head hits of 67.
    let (death, pushed) = r.until_dead(|frame, r| {
        let (mut a, b) = r.holding();
        a.buttons = BUTTON_ADS;
        if frame % 30 == 0 {
            a.buttons |= BUTTON_ATTACK;
        }
        (a, b)
    });
    assert_eq!(pushed.len(), 1, "{pushed:?}");
    let (frame, client, b) = &pushed[0];
    assert_eq!((*frame, *client), (death, 0), "{pushed:?}");
    assert_eq!(r.row_of_b(b), (0, 1), "the push reads the scored death");
    assert_eq!(r.sv.script_aborts(), Vec::<String>::new());
}

/// The same walk off `Cmd_Kill_f`, whose `player_die` runs inside B's own
/// packet: the push rides the death frame, B's row already `-1` and one death.
#[test]
fn a_kill_pushes_the_scoreboard_to_the_victims_follower() {
    let Some(mut r) = Followed::new() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    r.cb.send_reliable("kill");
    let (death, pushed) = r.until_dead(|_, r| r.holding());
    assert_eq!(pushed.len(), 1, "{pushed:?}");
    let (frame, client, b) = &pushed[0];
    assert_eq!((*frame, *client), (death, 0), "{pushed:?}");
    assert_eq!(r.row_of_b(b), (-1, 1), "the push reads the suicide scored");
    assert_eq!(r.sv.script_aborts(), Vec::<String>::new());
}
