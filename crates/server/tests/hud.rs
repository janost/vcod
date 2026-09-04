//! Script HUD elements on the wire: what dm's bootstrap puts on every
//! client's screen, and what a death puts on the dead client's alone.
//!
//! Needs `COD_DIR`; without the paks it returns early.

mod common;

use common::Queues;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::msg::{hud_field as f, NULL_USERCMD};
use vcod_common::net::protocol::PROTOCOL_V1;

const MAP: &str = "mp_carentan";
/// `hudelem_t` type 4, `setTimer`'s countdown (`crate::game::builtins::hud`).
const TYPE_TIMER_DOWN: i32 = 4;
/// Type 1, `setText`.
const TYPE_TEXT: i32 = 1;
/// The configstring a localized-string index `n` resolves through
/// (`docs/research/clientstate-wire-format.md`, "Configstring map").
const LOCALIZED_BASE: usize = 1244;

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

fn server(now: Instant) -> Option<vcod_server::Server> {
    let fs = vcod_common::testing::game_fs()?;
    let bsp_path = fs.resolve_map(MAP).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut sv = vcod_server::Server::new(cfg(), now);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    Some(sv)
}

/// `startGame`'s round clock: `newHudElem` with no owner, so every client is
/// sent it, and `archived` left at the allocator's 1, so it rides the first
/// of the two arrays. The fields are dm.gsc's own (`x` 320, `y` 460, both
/// alignments centred, the `bigfixed` font) and the retail connect-time
/// frame in `docs/protocol-1.1.md` carries exactly these.
#[test]
fn the_round_clock_reaches_a_fresh_client() {
    let mut now = Instant::now();
    let Some(mut sv) = server(now) else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl = common::connect(&mut sv, &q, &mut now);
    for _ in 0..10 {
        now += Duration::from_millis(50);
        cl.send_frame(&NULL_USERCMD);
        common::step(&mut sv, &q, &mut cl, now);
    }
    let snap = cl.snapshots().newest().expect("a snapshot");
    assert!(
        snap.ps.arrays.hud_current.is_empty(),
        "the bootstrap makes no unarchived element"
    );
    let clock = snap
        .ps
        .arrays
        .hud_archived
        .first()
        .expect("no round clock on the wire");
    assert_eq!(clock.get(f::TYPE), TYPE_TIMER_DOWN);
    assert_eq!(clock.get(f::X), 320);
    assert_eq!(clock.get(f::Y), 460);
    assert_eq!(clock.get(f::ALIGN_X), 1, "\"center\"");
    assert_eq!(clock.get(f::ALIGN_Y), 1, "\"middle\"");
    assert_eq!(clock.get(f::FONT), 1, "\"bigfixed\"");
    assert_eq!(clock.get_f32(f::FONT_SCALE), 1.0);
    assert_eq!(clock.get(f::COLOR), -1, "the allocator's opaque white");
    // `scr_dm_timelimit` defaults to 30 minutes, and the deadline is
    // absolute: `level.time + 1800000` at the moment `startGame` ran.
    let deadline = clock.get(f::TIME);
    assert!(
        (1_800_000..1_800_000 + 60_000).contains(&deadline),
        "the clock's deadline is {deadline}"
    );
}

/// `waitRespawnButton`: the killed client is sent the "press activate"
/// element and the killer is sent nothing, because `newClientHudElem(self)`
/// owns it to one client. `archived false` puts it in the second array, and
/// `setText` puts a localized-string index on the wire, not a string.
#[test]
fn only_the_killed_client_is_sent_the_respawn_text() {
    use vcod_common::net::msg::{UserCmd, BUTTON_ADS, BUTTON_ATTACK};

    let mut now = Instant::now();
    let Some(mut sv) = server(now) else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
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
    // Neither client has a HUD element of its own before the kill.
    for (who, cl) in [("A", &ca), ("B", &cb)] {
        assert!(
            cl.snapshots()
                .newest()
                .unwrap()
                .ps
                .arrays
                .hud_current
                .is_empty(),
            "{who} was sent an unarchived element before anyone died"
        );
    }

    // Two taps down the sight, the carbine being semi-automatic: a head hit
    // takes 67 and the second one kills.
    let ads = UserCmd {
        buttons: BUTTON_ADS,
        ..NULL_USERCMD
    };
    let fire = UserCmd {
        buttons: BUTTON_ADS | BUTTON_ATTACK,
        ..NULL_USERCMD
    };
    for shot in 0..2 {
        ca.send_frame(&fire);
        cb.send_frame(&NULL_USERCMD);
        step(&mut sv, &mut ca, &mut cb);
        if shot == 0 {
            for _ in 0..10 {
                ca.send_frame(&ads);
                cb.send_frame(&NULL_USERCMD);
                step(&mut sv, &mut ca, &mut cb);
            }
        }
    }
    assert_eq!(cb.snapshots().newest().unwrap().ps.health(), 0, "B is dead");
    // The text is 2.05 s behind the death: `Callback_PlayerKilled` waits 2 s
    // before it threads the killcam, the killcam waits another 0.05 s and
    // then bounces straight into `respawn()` because `archivetime` is 0
    // here, and `waitRespawnButton` opens with a `wait 0` of its own.
    for _ in 0..50 {
        ca.send_frame(&ads);
        cb.send_frame(&NULL_USERCMD);
        step(&mut sv, &mut ca, &mut cb);
    }
    assert_eq!(sv.script_aborts(), Vec::<String>::new());

    let sa = ca.snapshots().newest().unwrap();
    let sb = cb.snapshots().newest().unwrap();
    assert!(
        sa.ps.arrays.hud_current.is_empty(),
        "the killer is sent the dead client's element"
    );
    let text = sb
        .ps
        .arrays
        .hud_current
        .first()
        .expect("the killed client is sent no respawn text");
    assert_eq!(text.get(f::TYPE), TYPE_TEXT);
    assert_eq!(text.get(f::X), 320);
    assert_eq!(text.get(f::Y), 70);
    assert_eq!(text.get(f::ALIGN_X), 1, "\"center\"");
    assert_eq!(text.get(f::ALIGN_Y), 1, "\"middle\"");
    let index = text.get(f::TEXT);
    assert!(index > 0, "setText put no localized index on the wire");
    assert_eq!(
        sv.configstring(LOCALIZED_BASE + index as usize),
        "MPSCRIPT_PRESS_ACTIVATE_TO_RESPAWN"
    );
    // The clock is still on the other array, and still on both wires.
    for (who, snap) in [("A", &sa), ("B", &sb)] {
        assert_eq!(
            snap.ps.arrays.hud_archived.len(),
            1,
            "{who} lost the round clock"
        );
    }

    // The use key respawns B, and `removeRespawnText` destroys the element:
    // the array goes back to empty rather than keeping a stale slot.
    let use_key = UserCmd {
        buttons: vcod_common::net::msg::BUTTON_USE,
        ..NULL_USERCMD
    };
    for _ in 0..20 {
        ca.send_frame(&ads);
        cb.send_frame(&use_key);
        step(&mut sv, &mut ca, &mut cb);
    }
    let sb = cb.snapshots().newest().unwrap();
    assert!(sb.ps.health() > 0, "B did not respawn");
    assert!(
        sb.ps.arrays.hud_current.is_empty(),
        "the respawn text outlived the respawn"
    );
}
