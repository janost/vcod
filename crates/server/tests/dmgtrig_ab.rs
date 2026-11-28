//! `trigger_damage` under script blasts, against retail UO 1.51b
//! (`client-probes/probe_dmgtrig.gsc` on mp_uo_stanjel, `COD_VERSION=uo
//! tools/run_probe.sh`, 2026-10-18; docs/research/cod11-combat.md 14.8).
//! Every blast activates its bush, but a `"trigger"` a script thread
//! queues is dropped with `G_RunFrame`'s queue, so none is ever notified:
//! the activations show in the wait they arm (the second and third blasts
//! take damage and fire nothing) and in `health`, back at 32000 after
//! each. A breakpoint on retail's `G_Trigger` counted one call per
//! activation the gate expects.
//!
//! Needs `COD_DIR` at a UO install; without one it returns early.

use crate::common;

/// What retail's `games_mp.log` held.
const RETAIL: &[&str] = &[
    "PROBE count 17",
    "PROBE ent 86 (-1976.00, 3240.00, 24.00) 32000",
    "PROBE ent 87 (-1352.00, 3656.00, -312.00) 32000",
    "PROBE ent 88 (-816.00, 4520.00, -448.00) 32000",
    "PROBE ent 89 (1240.00, 3856.00, -384.00) 32000",
    "PROBE ent 90 (1952.00, 3336.00, -312.00) 32000",
    "PROBE ent 91 (-1944.00, 2136.00, 560.00) 32000",
    "PROBE ent 92 (-1120.00, 3184.00, -112.00) 32000",
    "PROBE ent 93 (1640.00, 3472.00, -96.00) 32000",
    "PROBE ent 94 (368.00, 2448.00, 16.00) 32000",
    "PROBE ent 414 (-2000.00, 2104.00, 272.00) 32000",
    "PROBE ent 416 (528.00, 4904.00, -464.00) 32000",
    "PROBE ent 417 (-200.00, 4888.00, -464.00) 32000",
    "PROBE ent 418 (-696.00, 4888.00, -464.00) 32000",
    "PROBE ent 420 (-48.00, 2888.00, -144.00) 32000",
    "PROBE ent 422 (0.00, 0.00, 0.00) 32000",
    "PROBE ent 425 (1504.00, 2536.00, -56.00) 32000",
    "PROBE ent 493 (-2072.00, 2920.00, 72.00) 32000",
    "PROBE phase 1550 single",
    "PROBE health 1550 86 32000",
    "PROBE damage 1550 86 100 world MOD_EXPLOSIVE",
    "PROBE phase 1650 busy",
    "PROBE health 1650 86 32000",
    "PROBE damage 1650 86 100 world MOD_EXPLOSIVE",
    "PROBE phase 1900 still_busy",
    "PROBE damage 1900 86 100 world MOD_EXPLOSIVE",
    "PROBE phase 2000 rearmed",
    "PROBE damage 2000 86 100 world MOD_EXPLOSIVE",
    "PROBE phase 3000 edge",
    "PROBE health 3000 86 32000",
    "PROBE damage 3000 86 77 world MOD_EXPLOSIVE",
    "PROBE phase 4000 out_of_range",
    "PROBE phase 5000 kill",
    "PROBE health 5000 86 32000",
    "PROBE death 5000 86 world",
    "PROBE damage 5000 86 40000 world MOD_EXPLOSIVE",
    "PROBE phase 6000 same_frame",
    "PROBE health 6000 87 32000",
    "PROBE damage 6000 87 100 world MOD_EXPLOSIVE",
    "PROBE phase 7000 attacker",
    "PROBE damage 7000 87 100 312 MOD_EXPLOSIVE",
    "PROBE phase 8000 gone_attacker",
    "PROBE damage 8000 86 100 313 MOD_EXPLOSIVE",
    "PROBE phase 9000 wait_loop",
    "PROBE damage 9000 87 100 world MOD_EXPLOSIVE",
    "PROBE damage 9050 87 100 world MOD_EXPLOSIVE",
    "PROBE damage 9100 87 100 world MOD_EXPLOSIVE",
    "PROBE damage 9150 87 100 world MOD_EXPLOSIVE",
    "PROBE damage 9200 87 100 world MOD_EXPLOSIVE",
    "PROBE damage 9250 87 100 world MOD_EXPLOSIVE",
    "PROBE damage 9300 87 100 world MOD_EXPLOSIVE",
    "PROBE damage 9350 87 100 world MOD_EXPLOSIVE",
    "PROBE damage 9400 87 100 world MOD_EXPLOSIVE",
    "PROBE damage 9450 87 100 world MOD_EXPLOSIVE",
    "PROBE damage 9500 87 100 world MOD_EXPLOSIVE",
    "PROBE damage 9550 87 100 world MOD_EXPLOSIVE",
    "PROBE end",
];

#[test]
fn script_blasts_on_trigger_damage_match_retail_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let fs = std::rc::Rc::new(fs);
    let src =
        std::fs::read_to_string("../gsc/tests/fixtures/semantics/client-probes/probe_dmgtrig.gsc")
            .expect("the probe");
    let (mut sv, mut now) = common::Boot {
        proto: Some(&vcod_common::net::protocol::PROTOCOL_V22),
        script: Some(&src),
        ..common::Boot::new("mp_uo_stanjel", "probe_dmgtrig")
    }
    .start(fs);
    common::run_alone(&mut sv, &mut now, 11000);
    let ours: Vec<&str> = sv
        .script_log()
        .iter()
        .map(|l| l.trim())
        .filter(|l| l.starts_with("PROBE"))
        .collect();
    if std::env::var_os("DMGTRIG_REPORT").is_some() {
        for l in &ours {
            eprintln!("{l}");
        }
    }
    assert_eq!(ours, RETAIL);
}

/// `client-probes/probe_dmgshot.gsc` on retail UO 1.51b, mp_uo_stanjel,
/// 2026-10-18: one axis `--probe-uo-weapon direct` client with
/// `--probe-weapon kar98k_mp`, held 160 off the first bush and aimed at it.
/// A round and a swing notify `"damage"` with two arguments (no mod) and
/// queue a `"trigger"`, whose waiter runs first; a miss does nothing; the
/// grenade's blast takes `G_Damage`'s six-argument arm and reaches a second
/// bush. Two retail runs read the round and swing lines identically and
/// the grenade's 100/16 and 98/18: where a throw lands rides the client's
/// cmd timing, so [`landing_free`] drops those two numbers and the landing.
const RETAIL_SHOT: &[&str] = &[
    "PROBE phase shot",
    "PROBE trigger 86 player -",
    "PROBE damage 86 120 player -",
    "PROBE phase shot_again",
    "PROBE trigger 86 player 2100",
    "PROBE damage 86 120 player -",
    "PROBE phase miss",
    "PROBE phase melee",
    "PROBE trigger 86 player 5850",
    "PROBE damage 86 150 player -",
    "PROBE phase grenade",
    "PROBE weapon stielhandgranate_mp",
    "PROBE trigger 86 player 8450",
    "PROBE trigger 92 player -",
    "PROBE damage 86 98 player MOD_GRENADE_SPLASH",
    "PROBE damage 92 18 player MOD_GRENADE_SPLASH",
    "PROBE nade (-2021.00, 3339.00, 3.00)",
    "PROBE done",
];

/// A line with what the grenade's landing decides taken out: the blast's
/// charge on a `MOD_GRENADE_SPLASH` line, and the `nade` origin.
fn landing_free(l: &str) -> String {
    if l.starts_with("PROBE nade") {
        return "PROBE nade".to_string();
    }
    if l.ends_with("MOD_GRENADE_SPLASH") {
        let t: Vec<&str> = l.split(' ').collect();
        return format!("{} {} {} _ {} {}", t[0], t[1], t[2], t[4], t[5]);
    }
    l.to_string()
}

/// The buttons of a `f "PROBE_IN ..."` command.
fn direct_buttons(tokens: &[String]) -> Option<u8> {
    if tokens.first().map(String::as_str) != Some("f") {
        return None;
    }
    let mut t = tokens.get(1)?.strip_prefix("PROBE_IN ")?.split_whitespace();
    t.next()?.parse().ok()
}

/// Runs `client-probes/<probe>.gsc` on ours on UO's `map` with one axis
/// `--probe-uo-weapon direct` client holding `kar98k_mp`, until the probe
/// logs `PROBE done`, and returns its `PROBE` lines. `None` without a UO
/// install.
fn direct_run(map: &str, probe: &str) -> Option<Vec<String>> {
    direct_run_seeded(map, probe, None)
}

/// [`direct_run`] with `srand(seed)` on the server's `rand()` once the
/// level is up.
fn direct_run_seeded(map: &str, probe: &str, seed: Option<u64>) -> Option<Vec<String>> {
    let fs = vcod_common::testing::uo_fs()?;
    Some(direct_run_on(
        fs,
        &vcod_common::net::protocol::PROTOCOL_V22,
        map,
        probe,
        seed,
        &[],
    ))
}

/// [`direct_run`] on `fs` under `proto` with `cvars` set.
fn direct_run_on(
    fs: vcod_common::pk3::Pk3Fs,
    proto: &'static vcod_common::net::protocol::Cod1Wire,
    map: &str,
    probe: &str,
    seed: Option<u64>,
    cvars: &[(&str, &str)],
) -> Vec<String> {
    use common::{Join, Queues};
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;
    use vcod_common::net::msg::UserCmd;
    use vcod_common::net::{NetClient, NetEvent};
    let fs = Rc::new(fs);
    let src = std::fs::read_to_string(format!(
        "../gsc/tests/fixtures/semantics/client-probes/{probe}.gsc"
    ))
    .expect("the probe");
    let (mut sv, mut now) = common::Boot {
        proto: Some(proto),
        cvars,
        script: Some(&src),
        ..common::Boot::new(map, probe)
    }
    .start(fs);
    if let Some(seed) = seed {
        sv.test_seed_rng(seed);
    }
    common::run_alone(&mut sv, &mut now, 3000);
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl = NetClient::start_with_qport(common::ClientEnd(q.clone()), now, 0x2001);
    let mut join = Join::new("axis", "kar98k_mp");
    let mut hold = common::ProbeHold {
        aim: true,
        ..Default::default()
    };
    let (mut buttons, mut selected) = (0u8, None::<u8>);
    let frame = Duration::from_millis(common::FRAME_MS as u64);
    for _ in 0..3000 {
        if sv.script_log().iter().any(|l| l.contains("PROBE done")) {
            break;
        }
        now += frame;
        let live = cl
            .snapshots()
            .newest()
            .is_some_and(|s| s.ps.field_i32(cl.protocol(), "pm_type") == 0);
        if live {
            let mut cmds = hold.cmds(&cl, buttons);
            if let Some(w) = selected {
                for c in &mut cmds {
                    c.weapon = w;
                }
            }
            cl.send_cmds(&cmds);
        } else if let Some(c) = cl.send_frame(&UserCmd::default()) {
            hold.last = c.server_time;
        }
        let pending: Vec<Vec<u8>> = q.borrow_mut().to_server.drain(..).collect();
        for p in pending {
            sv.handle_packet(common::ADDR, &p, now);
        }
        sv.tick(now);
        for (_, p) in sv.take_outgoing() {
            q.borrow_mut().to_client.push_back(p);
        }
        for e in cl.pump_at(now) {
            if let NetEvent::ServerCommand(tokens) = e {
                if tokens.first().map(String::as_str) == Some("a") {
                    selected = tokens.get(1).and_then(|w| w.parse().ok());
                }
                if let Some(b) = direct_buttons(&tokens) {
                    buttons = b;
                }
                join.on_server_command(&tokens, &mut cl, now);
            }
        }
    }
    let ours: Vec<String> = sv
        .script_log()
        .iter()
        .map(|l| l.trim().to_string())
        .filter(|l| l.starts_with("PROBE"))
        .collect();
    if std::env::var_os("DMGTRIG_REPORT").is_some() {
        for l in &ours {
            eprintln!("{l}");
        }
    }
    ours
}

#[test]
fn a_players_rounds_swing_and_grenade_on_trigger_damage_match_retail_uo() {
    let Some(ours) = direct_run("mp_uo_stanjel", "probe_dmgshot") else {
        return;
    };
    let free = |v: &[&str]| v.iter().map(|l| landing_free(l)).collect::<Vec<_>>();
    let ours: Vec<&str> = ours.iter().map(String::as_str).collect();
    assert_eq!(free(&ours), free(RETAIL_SHOT));
}

/// `client-probes/probe_dmgtouch.gsc` on retail UO 1.51b, mp_uo_stanjel,
/// 2026-10-18, set up as [`RETAIL_SHOT`]. A frag through bush 86 after
/// `enableGrenadeTouchDamage` queues one `"trigger"` with the grenade and
/// notifies `"damage"` with the grenade and its `explosionInnerDamage`
/// (120) on every frame of the flight inside the bush; with the walk
/// closed it raises nothing. A panzerschreck shell into the ground inside
/// the bush notifies `"damage"` 1000 with no mod; one 300 units the other
/// way raises nothing off its contact. A gdb breakpoint on
/// `G_CheckHitTriggerDamage` read both shells' segments as the contact
/// point alone (cod11-combat.md 14.8). Of four retail runs, the frames
/// inside the bush were 4, 5, 4, 4 and the blasts' landings, charges and
/// victim order differed: [`touch_walks`] keeps the walks' lines.
const RETAIL_TOUCH: &[&str] = &[
    "PROBE weapon stielhandgranate_mp",
    "PROBE phase touch",
    "PROBE trigger 86 grenade -",
    "PROBE damage 86 120 grenade -",
    "PROBE damage 86 120 grenade -",
    "PROBE damage 86 120 grenade -",
    "PROBE damage 86 120 grenade -",
    "PROBE trigger 86 player 3750",
    "PROBE trigger 493 player -",
    "PROBE damage 86 84 player MOD_GRENADE_SPLASH",
    "PROBE damage 493 18 player MOD_GRENADE_SPLASH",
    "PROBE nade (-2133.00, 3264.00, 25.00)",
    "PROBE phase touch_off",
    "PROBE trigger 493 player 6400",
    "PROBE trigger 86 player 6400",
    "PROBE trigger 92 player -",
    "PROBE damage 86 120 player MOD_GRENADE_SPLASH",
    "PROBE damage 493 13 player MOD_GRENADE_SPLASH",
    "PROBE damage 92 47 player MOD_GRENADE_SPLASH",
    "PROBE nade (-1963.00, 3272.00, 0.00)",
    "PROBE phase shell",
    "PROBE spot (-1852.00, 3232.00, -16.35) (-1972.00, 3232.00, 9.27) (15.99, 180.00, 0.00)",
    "PROBE weapon panzerschreck_mp",
    "PROBE aim (15.99, 180.00, 0.00) (-1852.00, 3232.00, -15.35) (15.99, 180.00, 0.00)",
    "PROBE trigger 92 player 4950",
    "PROBE trigger 86 player 4950",
    "PROBE damage 92 26 player MOD_PROJECTILE_SPLASH",
    "PROBE damage 86 1000 player -",
    "PROBE rocket (-1963.00, 3248.00, 5.00)",
    "PROBE phase shell_wide",
    "PROBE aim (34.99, 0.00, 0.00) (-1852.00, 3232.00, -15.35) (34.99, 0.00, 0.00)",
    "PROBE trigger 92 player 11150",
    "PROBE damage 92 59 player MOD_PROJECTILE_SPLASH",
    "PROBE rocket (-1570.00, 3251.00, -83.00)",
    "PROBE done",
];

/// The lines the two walks decide: every blast line, landing and missile
/// origin dropped, a phase's `"trigger"` kept only for a bush that took a
/// walk's `"damage"` in it (its wait in ms as `_`), and a run of equal
/// lines (the frames a frag spends inside a bush) as one.
fn touch_walks(v: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let phases = v.split_inclusive(|l| l.starts_with("PROBE phase") || *l == "PROBE done");
    for chunk in phases {
        let walked: Vec<&str> = chunk
            .iter()
            .filter(|l| l.starts_with("PROBE damage") && !l.contains("SPLASH"))
            .filter_map(|l| l.split(' ').nth(2))
            .collect();
        for l in chunk {
            let t: Vec<&str> = l.split(' ').collect();
            let keep = match t[1] {
                "nade" | "rocket" => None,
                "damage" if l.contains("SPLASH") => None,
                "trigger" if !walked.contains(&t[2]) => None,
                "trigger" if t[4] != "-" => Some(format!("{} {} {} {} _", t[0], t[1], t[2], t[3])),
                _ => Some(l.to_string()),
            };
            if let Some(k) = keep
                && out.last() != Some(&k)
            {
                out.push(k);
            }
        }
    }
    out
}

const TOUCH_SEED: u64 = 1;

#[test]
fn grenade_touch_and_shell_walks_on_trigger_damage_match_retail_uo() {
    // A `rand()` seed whose rocket spread takes the shell into the bush
    // retail's took.
    let Some(ours) = direct_run_seeded("mp_uo_stanjel", "probe_dmgtouch", Some(TOUCH_SEED)) else {
        return;
    };
    let ours: Vec<&str> = ours.iter().map(String::as_str).collect();
    assert_eq!(touch_walks(&ours), touch_walks(RETAIL_TOUCH));
}

/// `client-probes/probe_belltower.gsc` on retail UO 1.51b, mp_foy,
/// 2026-10-18, one axis client as [`RETAIL_SHOT`]'s. mp_foy's belltower
/// (spawnflags 115) notifies a round's `"damage"` and refuses it; a frag
/// landing below does nothing; a panzerschreck shell from inside the
/// belfry into its floor fires it: the shell's splash hurts the shooter,
/// `belltower_damage()`'s `radiusDamage` hurts him again with the world
/// as attacker, the probe's `"trigger"` waiter runs after both and its
/// `"damage"` waiter never does (`_load.gsc`'s exploder thread deleted the
/// trigger first), and the exploder deletes the tower's `delete` brush and
/// keeps the effect model. Four runs read alike but for the shell's
/// splash on the shooter (78 to 84), which rides the spread.
const RETAIL_BELLTOWER: &[&str] = &[
    "PROBE state 1 1 1",
    "PROBE spot (495.00, -2108.00, 475.48)",
    "PROBE phase round",
    "PROBE damage 120 player -",
    "PROBE phase frag",
    "PROBE nade (495.00, -1925.00, 1.00)",
    "PROBE state 1 1 1",
    "PROBE phase shell",
    "PROBE weapon panzerschreck_mp",
    "PROBE inside (495.00, -1508.00, 416.12) (51.34, 90.00, 0.00)",
    "PROBE hurt 83 MOD_PROJECTILE_SPLASH player",
    "PROBE hurt 198 MOD_EXPLOSIVE world",
    "PROBE trigger player",
    "PROBE rocket (496.00, -1460.00, 417.00)",
    "PROBE state 0 0 1",
    "PROBE done",
];

#[test]
fn mp_foys_belltower_matches_retail_uo() {
    let Some(ours) = direct_run("mp_foy", "probe_belltower") else {
        return;
    };
    let spread_free = |v: &[&str]| {
        v.iter()
            .filter(|l| !l.starts_with("PROBE rocket"))
            .map(|l| match l.strip_prefix("PROBE hurt ") {
                Some(r) if r.contains("PROJECTILE_SPLASH") => "PROBE hurt _ splash".to_string(),
                _ => l.to_string(),
            })
            .collect::<Vec<_>>()
    };
    let ours: Vec<&str> = ours.iter().map(String::as_str).collect();
    assert_eq!(spread_free(&ours), spread_free(RETAIL_BELLTOWER));
}

/// `client-probes/probe_dmgspawn.gsc` on retail 1.1d and 1.5b, mp_carentan,
/// 2026-10-18, no client: both servers logged these lines alike. A
/// `spawn("trigger_damage", origin)` takes inline model 0, the world's box
/// moved to its origin, and `CanDamage` probes from that box's centre: a
/// blast at the origin is cut off from it and raises nothing, one in the sky
/// or at the centre takes the whole charge, and one past the box's top is
/// out of range. Moved 1500 down, the centre goes under the ground; deleted,
/// nothing reaches it (docs/research/cod11-combat.md 14.8).
const RETAIL_SPAWNED: &[&str] = &[
    "PROBE spawned trigger_damage 32000 (844.00, 3480.00, -21.25)",
    "PROBE phase 1550 origin",
    "PROBE health 1550 32000",
    "PROBE phase 2550 above",
    "PROBE health 2550 32000",
    "PROBE damage 100 - 2550",
    "PROBE phase 3550 centre",
    "PROBE health 3550 32000",
    "PROBE damage 100 - 3550",
    "PROBE phase 4550 under_centre",
    "PROBE health 4550 32000",
    "PROBE damage 100 - 4550",
    "PROBE phase 5550 past_top",
    "PROBE health 5550 32000",
    "PROBE phase 6550 moved_old_centre",
    "PROBE health 6550 32000",
    "PROBE phase 7550 moved_new_centre",
    "PROBE health 7550 32000",
    "PROBE damage 100 - 7550",
    "PROBE phase 8550 back",
    "PROBE health 8550 32000",
    "PROBE damage 100 - 8550",
    "PROBE phase 9550 deleted",
    "PROBE done",
];

/// The same probe on retail UO, 2026-11-14, no client: the same lines but
/// for the mod UO's `"damage"` notify carries.
const RETAIL_SPAWNED_UO: &[&str] = &[
    "PROBE spawned trigger_damage 32000 (844.00, 3480.00, -21.25)",
    "PROBE phase 1550 origin",
    "PROBE health 1550 32000",
    "PROBE phase 2550 above",
    "PROBE health 2550 32000",
    "PROBE damage 100 MOD_EXPLOSIVE 2550",
    "PROBE phase 3550 centre",
    "PROBE health 3550 32000",
    "PROBE damage 100 MOD_EXPLOSIVE 3550",
    "PROBE phase 4550 under_centre",
    "PROBE health 4550 32000",
    "PROBE damage 100 MOD_EXPLOSIVE 4550",
    "PROBE phase 5550 past_top",
    "PROBE health 5550 32000",
    "PROBE phase 6550 moved_old_centre",
    "PROBE health 6550 32000",
    "PROBE phase 7550 moved_new_centre",
    "PROBE health 7550 32000",
    "PROBE damage 100 MOD_EXPLOSIVE 7550",
    "PROBE phase 8550 back",
    "PROBE health 8550 32000",
    "PROBE damage 100 MOD_EXPLOSIVE 8550",
    "PROBE phase 9550 deleted",
    "PROBE done",
];

/// Ours' `PROBE` lines of `client-probes/probe_dmgspawn.gsc` on `fs`.
fn spawned_run(fs: vcod_common::pk3::Pk3Fs) -> Vec<String> {
    let proto = vcod_server::server::install_protocol(&fs);
    let fs = std::rc::Rc::new(fs);
    let src =
        std::fs::read_to_string("../gsc/tests/fixtures/semantics/client-probes/probe_dmgspawn.gsc")
            .expect("the probe");
    let (mut sv, mut now) = common::Boot {
        proto: Some(proto),
        script: Some(&src),
        ..common::Boot::new("mp_carentan", "probe_dmgspawn")
    }
    .start(fs);
    common::run_alone(&mut sv, &mut now, 11000);
    let ours: Vec<String> = sv
        .script_log()
        .iter()
        .map(|l| l.trim().to_string())
        .filter(|l| l.starts_with("PROBE"))
        .collect();
    if std::env::var_os("DMGTRIG_REPORT").is_some() {
        for l in &ours {
            eprintln!("{l}");
        }
    }
    ours
}

/// Needs `COD_DIR` at a 1.1 or 1.5 install; a UO one returns early.
#[test]
fn a_script_spawned_trigger_damage_matches_retail() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let proto = vcod_server::server::install_protocol(&fs);
    if proto.transport.version == vcod_common::net::protocol::PROTOCOL_V22.transport.version {
        return;
    }
    assert_eq!(spawned_run(fs), RETAIL_SPAWNED);
}

/// Needs `COD_DIR` at a UO install.
#[test]
fn a_script_spawned_trigger_damage_matches_retail_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    assert_eq!(spawned_run(fs), RETAIL_SPAWNED_UO);
}

/// `client-probes/probe_dmgspawnshot.gsc` on retail 1.1d, mp_carentan,
/// 2026-10-18 (two runs), one axis `--probe-uo-weapon direct` client with
/// `--probe-weapon kar98k_mp` firing into the ground 160 units ahead. A
/// round's walk sight-traces its segment against the spawned trigger's
/// world copy: with the copy's ground 20 or 1 unit over the world's the
/// round ends inside it and fires the trigger, 20 or 1 under or level it
/// ends in the air or on the face and fires nothing (cod11-combat.md 14.8).
const RETAIL_SPAWNED_SHOT: &[&str] = &[
    "PROBE site (844.00, 3480.00, -23.88) (1004.00, 3480.00, -23.88)",
    "PROBE phase up (0.00, 0.00, 20.00)",
    "PROBE trigger player",
    "PROBE damage 120 player -",
    "PROBE phase down (0.00, 0.00, -20.00)",
    "PROBE phase level (0.00, 0.00, 0.00)",
    "PROBE phase up1 (0.00, 0.00, 1.00)",
    "PROBE trigger player",
    "PROBE damage 120 player -",
    "PROBE phase down1 (0.00, 0.00, -1.00)",
    "PROBE done",
];

/// 1.5b logged the same lines. Needs `COD_DIR` at a 1.1 or 1.5 install; a
/// UO one returns early.
#[test]
fn a_round_on_a_script_spawned_trigger_damage_matches_retail() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let proto = vcod_server::server::install_protocol(&fs);
    if proto.transport.version == vcod_common::net::protocol::PROTOCOL_V22.transport.version {
        return;
    }
    let ours = direct_run_on(fs, proto, "mp_carentan", "probe_dmgspawnshot", None, &[]);
    let ours: Vec<&str> = ours.iter().map(String::as_str).collect();
    assert_eq!(ours, RETAIL_SPAWNED_SHOT);
}

/// Retail UO logged the same lines, 2026-11-14 (cod11-combat.md 14.8).
/// Needs `COD_DIR` at a UO install.
#[test]
fn a_round_on_a_script_spawned_trigger_damage_matches_retail_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let proto = vcod_server::server::install_protocol(&fs);
    let ours = direct_run_on(fs, proto, "mp_carentan", "probe_dmgspawnshot", None, &[]);
    let ours: Vec<&str> = ours.iter().map(String::as_str).collect();
    assert_eq!(ours, RETAIL_SPAWNED_SHOT);
}

/// The same probe with `probe_fine 1` on retail 1.1d, 2026-11-26: the copy
/// moved by sixteenths of a unit about the round's end, 0.125 over the
/// site's terrain. Only a copy whose ground stands over the end fires, as
/// the terrain arm's bare segment has it (cod11-combat.md 14.8.1).
const RETAIL_SPAWNED_FINE: &[&str] = &[
    "PROBE site (844.00, 3480.00, -23.88) (1004.00, 3480.00, -23.88)",
    "PROBE phase up1_16 (0.00, 0.00, 0.06)",
    "PROBE phase up1_8 (0.00, 0.00, 0.12)",
    "PROBE phase up3_16 (0.00, 0.00, 0.19)",
    "PROBE trigger player",
    "PROBE damage 120 player -",
    "PROBE phase up1_4 (0.00, 0.00, 0.25)",
    "PROBE trigger player",
    "PROBE damage 120 player -",
    "PROBE phase down1_16 (0.00, 0.00, -0.06)",
    "PROBE done",
];

/// Needs `COD_DIR` at a 1.1 or 1.5 install; a UO one returns early.
#[test]
fn a_round_on_a_world_copy_a_sixteenth_off_matches_retail() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let proto = vcod_server::server::install_protocol(&fs);
    if proto.transport.version == vcod_common::net::protocol::PROTOCOL_V22.transport.version {
        return;
    }
    let fine = [("probe_fine", "1")];
    // A `rand()` seed whose sight spread (0.1) keeps each round's end on
    // the side of the sixteenth retail's took: 4 of 12 seeds tip one over.
    let ours = direct_run_on(
        fs,
        proto,
        "mp_carentan",
        "probe_dmgspawnshot",
        Some(1),
        &fine,
    );
    let ours: Vec<&str> = ours.iter().map(String::as_str).collect();
    assert_eq!(ours, RETAIL_SPAWNED_FINE);
}

/// `probe_fine 2` on retail 1.1d and UO, 2026-11-26, both logging these
/// lines: the round strikes a wall's brush square on, its end 0.125 off the
/// face, and the copy's wall is moved along the normal. A copy whose face
/// stands a sixteenth or an eighth nearer than the world's, the end a
/// sixteenth short of it or on it, fires nothing, where a trace's clip
/// epsilon would meet it (cod11-combat.md 14.8.1).
const RETAIL_SPAWNED_WALL: &[&str] = &[
    "PROBE wall (-1.00, 0.00, 0.00)",
    "PROBE site (1368.00, 2608.00, -23.88) (1831.88, 2608.00, 36.12)",
    "PROBE phase near1_16 (-0.06, 0.00, 0.00)",
    "PROBE phase near1_8 (-0.12, 0.00, 0.00)",
    "PROBE phase near3_16 (-0.19, 0.00, 0.00)",
    "PROBE trigger player",
    "PROBE damage 120 player -",
    "PROBE phase far1_16 (0.06, -0.00, 0.00)",
    "PROBE phase near1 (-1.00, 0.00, 0.00)",
    "PROBE trigger player",
    "PROBE damage 120 player -",
    "PROBE done",
];

/// Ours' `probe_dmgspawnshot` lines with `probe_fine 2` on `fs`.
fn wall_run(fs: vcod_common::pk3::Pk3Fs) -> Vec<String> {
    let proto = vcod_server::server::install_protocol(&fs);
    let fine = [("probe_fine", "2")];
    direct_run_on(fs, proto, "mp_carentan", "probe_dmgspawnshot", None, &fine)
}

/// Needs `COD_DIR` at a 1.1 or 1.5 install; a UO one returns early.
#[test]
fn a_round_on_a_world_copys_wall_matches_retail() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let proto = vcod_server::server::install_protocol(&fs);
    if proto.transport.version == vcod_common::net::protocol::PROTOCOL_V22.transport.version {
        return;
    }
    assert_eq!(wall_run(fs), RETAIL_SPAWNED_WALL);
}

/// Needs `COD_DIR` at a UO install.
#[test]
fn a_round_on_a_world_copys_wall_matches_retail_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    assert_eq!(wall_run(fs), RETAIL_SPAWNED_WALL);
}

/// `client-probes/probe_dmguse.gsc` on retail 1.1d, 1.5b and UO, no client,
/// mp_carentan with three submodels made `trigger_damage`s by
/// `fixtures/triggers/mp_carentan-dmguse.ent`. `useBy` activates with
/// `accumulate + 1` as the damage: after a 100 blast the use clears
/// `threshold` 101 and puts `health` back, not 102; the count comes off no
/// `health`; `useBy`'s own `"trigger"` is notified on every use
/// (cod11-combat.md 14.8.2). Runs on whichever install `COD_DIR` names.
#[test]
fn use_by_on_trigger_damage_counts_accumulate_plus_one_like_retail() {
    let Some(mut fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let path = fs.resolve_map("mp_carentan").expect("mp_carentan");
    let bytes = fs.read(&path).unwrap();
    fs.overlay(
        &path,
        vcod_common::testing::bsp_replacing_models(
            &bytes,
            include_str!("fixtures/triggers/mp_carentan-dmguse.ent"),
        ),
    );
    let proto = vcod_server::server::install_protocol(&fs);
    let src =
        std::fs::read_to_string("../gsc/tests/fixtures/semantics/client-probes/probe_dmguse.gsc")
            .expect("the probe");
    let (mut sv, mut now) = common::Boot {
        proto: Some(proto),
        script: Some(&src),
        ..common::Boot::new("mp_carentan", "probe_dmguse")
    }
    .start(std::rc::Rc::new(fs));
    common::run_alone(&mut sv, &mut now, 11000);
    let ours: Vec<&str> = sv
        .script_log()
        .iter()
        .map(|l| l.trim())
        .filter(|l| l.starts_with("PROBE"))
        .collect();
    let retail: Vec<&str> = include_str!("fixtures/triggers/mp_carentan-dmguse.txt")
        .lines()
        .filter(|l| l.starts_with("PROBE"))
        .collect();
    assert_eq!(ours, retail);
}

/// `client-probes/probe_dmgshot.gsc` on retail UO 1.51b retaken under
/// `tools/re/gdb_muzzle.py` (`fixtures/triggers/mp_uo_stanjel-dmgshot-muzzle-uo.txt`).
/// The script's hold never lands the thrower, so the spawn's landing
/// velocity grows through every phase and the bob byte moves only on the
/// frames he touches down: handed retail's `SPAWNED` velocity, ours'
/// throw reads retail's `bobCycle` 241, origin and velocity, every frag
/// snapshot sits on retail's base to the bit and every line holds, the
/// blast's charges and landing included (cod11-combat.md 14.8).
#[test]
fn a_uo_throw_under_the_hold_leaves_from_retails_muzzle() {
    let text = include_str!("fixtures/triggers/mp_uo_stanjel-dmgshot-muzzle-uo.txt");
    if vcod_common::testing::uo_fs().is_none() {
        return;
    }
    let proto = &vcod_common::net::protocol::PROTOCOL_V22;
    let (lines, frames) = common::direct_replay_spawned(
        vcod_common::testing::uo_fs,
        proto,
        "mp_uo_stanjel",
        "probe_dmgshot",
        "kar98k_mp",
        text,
    );
    let retail: Vec<&str> = text.lines().filter(|l| l.starts_with("PROBE")).collect();
    assert_eq!(lines, retail);
    // (snapshot time, trType, base bits) of each live frag snapshot.
    let ours: Vec<(i32, i32, [i32; 3])> = frames
        .iter()
        .filter_map(|(t, ents)| {
            let e = ents
                .values()
                .find(|e| e.field_i32(proto, "eType") == vcod_common::net::flags::ET_MISSILE)?;
            let b =
                ["pos.trBase[0]", "pos.trBase[1]", "pos.trBase[2]"].map(|n| e.field_i32(proto, n));
            Some((*t, e.field_i32(proto, "pos.trType"), b))
        })
        .collect();
    let hex = |t: &str| u32::from_str_radix(t.trim_start_matches("0x"), 16).unwrap() as i32;
    let retail: Vec<(i32, i32, [i32; 3])> = text
        .lines()
        .filter(|l| l.starts_with("VEH ") && l.contains(" missile ") && l.contains(" etype 4 "))
        .map(|l| {
            let t: Vec<&str> = l.split_whitespace().collect();
            let at = |k: &str| t.iter().position(|w| *w == k).unwrap() + 1;
            let b = at("bits");
            (
                t[1].parse().unwrap(),
                t[at("type")].parse().unwrap(),
                [hex(t[b]), hex(t[b + 1]), hex(t[b + 2])],
            )
        })
        .collect();
    assert!(!retail.is_empty());
    assert_eq!(ours, retail);
}

/// Ours' log of `client-probes/probe_belltower_tank.gsc` on mp_foy with
/// `fixtures/triggers/mp_foy-belltower-tank.ent` appended, the client
/// driven as `vehicle_probe.rs`'s `SIT` drives `--probe-vehicle sit`: on
/// foot 10 s from the placement, the use tap, then the seat.
fn belltower_tank_run() -> Option<Vec<String>> {
    use common::{Join, Queues, step};
    use std::cell::RefCell;
    use std::rc::Rc;
    use vcod_common::net::msg::{BUTTON_USE, UserCmd};
    const SETTLE_MS: i64 = 10_000;
    const TAP_MS: i64 = 150;
    let mut fs = vcod_common::testing::uo_fs()?;
    let path = fs.resolve_map("mp_foy").expect("mp_foy");
    let bytes = fs.read(&path).unwrap();
    fs.overlay(
        &path,
        vcod_common::testing::bsp_with_entities(
            &bytes,
            include_str!("fixtures/triggers/mp_foy-belltower-tank.ent"),
        ),
    );
    let (mut sv, mut now) = common::Boot {
        proto: Some(&vcod_common::net::protocol::PROTOCOL_V22),
        script: Some(include_str!(
            "../../gsc/tests/fixtures/semantics/client-probes/probe_belltower_tank.gsc"
        )),
        ..common::Boot::new("mp_foy", "probe_belltower_tank")
    }
    .start(fs);
    common::run_alone(&mut sv, &mut now, common::PROBE_JOIN_MS);
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl =
        vcod_common::net::NetClient::start_with_qport(common::ClientEnd(q.clone()), now, 0x2001);
    let mut join = Join::new("axis", "kar98k_mp");
    let frame = std::time::Duration::from_millis(common::FRAME_MS as u64);
    let mut hold = common::ProbeHold {
        follow: true,
        ..Default::default()
    };
    let mut placed_at: Option<i64> = None;
    for i in 0..3000i64 {
        if sv.script_log().iter().any(|l| l.contains("PROBE done")) {
            break;
        }
        now += frame;
        let since = placed_at.map(|at| (i - at) * common::FRAME_MS);
        let tap = since.is_some_and(|ms| (SETTLE_MS..SETTLE_MS + TAP_MS).contains(&ms));
        if tap {
            hold.follow = false;
        }
        if placed_at.is_some() && cl.state() == vcod_common::net::NetState::Active {
            let buttons = if tap { BUTTON_USE } else { 0 };
            cl.send_cmds(&hold.cmds(&cl, buttons));
        } else if let Some(c) = cl.send_frame(&UserCmd::default()) {
            hold.last = c.server_time;
        }
        for e in step(&mut sv, &q, &mut cl, now) {
            if let vcod_common::net::NetEvent::ServerCommand(tokens) = e {
                join.on_server_command(&tokens, &mut cl, now);
            }
        }
        if placed_at.is_none() && sv.script_log().iter().any(|l| l.contains("PROBE place")) {
            placed_at = Some(i);
        }
    }
    Some(
        sv.script_log()
            .iter()
            .map(|l| l.trim().to_string())
            .filter(|l| l.starts_with("PROBE"))
            .collect(),
    )
}

/// `fixtures/triggers/mp_foy-belltower-tank-uo.txt`, retail UO 1.51b: the
/// seated driver's `fireTurret` shell strikes the belfry's north wall inside
/// `*44` and walks the trigger_damages at the contact with damage 700 and
/// mod 5, which the belltower refuses no more than the panzerschreck's. It
/// fires (the activator is the driver), `_load.gsc`'s exploder deletes the
/// trigger before the probe's `"damage"` waiter runs and takes the tower's
/// `delete` brush (cod11-combat.md 14.8.3).
#[test]
fn a_tank_shell_sets_off_mp_foys_belltower_like_retail_uo() {
    let Some(ours) = belltower_tank_run() else {
        return;
    };
    let retail: Vec<&str> = include_str!("fixtures/triggers/mp_foy-belltower-tank-uo.txt")
        .lines()
        .filter(|l| l.starts_with("PROBE"))
        .collect();
    assert_eq!(ours, retail);
}
