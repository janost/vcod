//! Stock gametypes through a running CoD4 1.7 level: what their setup
//! threads build off the map's entities. Needs `COD4_DIR`.

use std::time::{Duration, Instant};

use super::*;

/// A level of `gametype` on `map`, `sv_pure 0`, run `ms` of frames with
/// nobody on, and its clock; `None` without an install.
fn level(map: &str, gametype: &str, ms: u64) -> Option<(Cod4Server, Instant)> {
    let install = vcod_cod4::testing::cod4_install()?;
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: map.into(),
            gametype: gametype.into(),
            hostname: "vcod".into(),
            max_clients: 8,
        },
        7,
    );
    sv.set_cvar("sv_pure", "0");
    sv.load(Some(&install)).unwrap();
    let mut now = Instant::now();
    run(&mut sv, &mut now, ms);
    Some((sv, now))
}

/// `ms` of frames, the console drained ahead of each.
fn run(sv: &mut Cod4Server, now: &mut Instant, ms: u64) {
    for _ in 0..ms / 50 {
        *now += Duration::from_millis(50);
        <Cod4Game as crate::engine::GameModule>::drain_console(sv, *now);
        sv.tick_frames(*now, 1);
        sv.take_outgoing();
    }
}

/// `koth::SetupRadios` finds every radio inside its `radiotrigger` brush
/// (`isTouching`, a point against the trigger's brushes), so mp_crash's
/// level runs koth instead of `AbortLevel`'s dm.
#[test]
fn koth_on_mp_crash_finds_its_radios() {
    let Some((mut sv, _)) = level("mp_crash", "koth", 1000) else {
        return;
    };
    let rt = &mut sv.game.level.as_mut().unwrap().rt;
    assert_eq!(rt.host.cvars.get("g_gametype"), "koth");
    assert!(
        !matches!(rt.level_field("radios"), vcod_gsc::Value::Undefined),
        "SetupRadios found map errors"
    );
    let aborts = rt.aborts();
    assert!(aborts.is_empty(), "script errors:\n{}", aborts.join("\n"));
}

/// `--set g_gametype=koth` before the load runs koth, not the config's
/// `war`, as retail's `+set g_gametype koth` does.
#[test]
fn a_set_gametype_before_the_load_runs_that_gametype() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: "vcod".into(),
            max_clients: 8,
        },
        7,
    );
    sv.set_cvar("sv_pure", "0");
    sv.set_cvar("g_gametype", "koth");
    sv.load(Some(&install)).unwrap();
    let mut now = Instant::now();
    run(&mut sv, &mut now, 500);
    assert_eq!(sv.cvar("g_gametype"), "koth");
    assert!(sv.configstring(0).contains("\\g_gametype\\koth"));
    let rt = &mut sv.game.level.as_mut().unwrap().rt;
    assert!(!matches!(
        rt.level_field("radios"),
        vcod_gsc::Value::Undefined
    ));
}

/// `dom`'s flags show their compass icons through `objective_state(id,
/// "active")`, a state 1.7 reads (0x80c210a): the setup runs clean.
#[test]
fn dom_flags_set_their_objectives_active() {
    let Some((mut sv, _)) = level("mp_crash", "dom", 1000) else {
        return;
    };
    let rt = &mut sv.game.level.as_mut().unwrap().rt;
    assert!(
        rt.host.objectives.iter().any(|o| o.state == 1),
        "no objective active"
    );
    let aborts = rt.aborts();
    assert!(aborts.is_empty(), "script errors:\n{}", aborts.join("\n"));
}

/// A `fast_restart` keeps the configstrings, so `_hardpoints::init`'s
/// second `precacheLocationSelector("map_artillery_selector")` finds its
/// name there, which 1.7 only prints at `developer` (0x80bc5f2): the
/// restarted `sd` level runs clean.
#[test]
fn an_sd_fast_restart_precaches_the_selector_again_without_an_error() {
    let Some((mut sv, mut now)) = level("mp_crash", "sd", 1000) else {
        return;
    };
    let id = sv.server_id();
    sv.core.console.push_back("fast_restart".into());
    run(&mut sv, &mut now, 1000);
    assert_ne!(sv.server_id(), id, "no restart");
    let rt = &sv.game.level.as_ref().unwrap().rt;
    let aborts = rt.aborts();
    assert!(aborts.is_empty(), "script errors:\n{}", aborts.join("\n"));
    assert_eq!(
        rt.host.configstrings[crate::game::cod4::CS_LOC_SEL_MTLS],
        "map_artillery_selector"
    );
}

/// mp_backlot's `misc_turret`s (`weapon_saw_MG_setup`) take their tags
/// (`tag_aim`, `tag_player`, `tag_flash`, ...) off the zones' xmodel, not
/// a pak's, where CoD4 has none.
#[test]
fn the_map_turrets_take_their_tags_from_the_zones() {
    let Some((sv, _)) = level("mp_backlot", "war", 200) else {
        return;
    };
    let host = &sv.game.level.as_ref().unwrap().rt.host;
    assert!(!host.turrets.is_empty(), "no turret on mp_backlot");
    for (id, rec) in &host.turrets {
        assert!(rec.tags.is_some(), "turret {id:?} has no tags");
    }
}

/// `play_sound_on_tag` on a destructible car links its sound origin to a
/// bone of the car's zone model, as a destroyed part's sound does: a CoD4
/// `linkTo` reads the zone's bind tags, which the search path lacks (the
/// map sweep's mp_backlot `sab`, cod4-gsc.md 3.4).
#[test]
fn a_sound_links_to_a_destructible_cars_tag() {
    let Some((mut sv, mut now)) = level("mp_backlot", "war", 500) else {
        return;
    };
    let rt = &mut sv.game.level.as_mut().unwrap().rt;
    let ids: Vec<_> = rt.host.ents.iter_inuse().map(|(id, _)| id).collect();
    let car = ids
        .into_iter()
        .find(|&id| rt.field_str(id, "model").contains("_destructible_mp"))
        .expect("no destructible car on mp_backlot");
    rt.start_script_thread_for_test("maps/mp/_utility", "play_sound_on_tag", Some(car), |cx| {
        vec![
            vcod_gsc::Value::String(cx.intern_exact("car_glass_break")),
            vcod_gsc::Value::String(cx.intern_exact("tag_glass_back_d")),
        ]
    });
    run(&mut sv, &mut now, 200);
    let rt = &mut sv.game.level.as_mut().unwrap().rt;
    let aborts = rt.aborts();
    assert!(aborts.is_empty(), "script errors:\n{}", aborts.join("\n"));
    assert!(
        rt.host
            .links
            .tagged_children(car)
            .iter()
            .any(|(_, t)| t.eq_ignore_ascii_case("tag_glass_back_d")),
        "nothing linked to the tag"
    );
}

/// The `sd` and `sab` bomb's `visuals[0] radiusDamage(origin, 512, 200,
/// 20, player)` names its attacker: 1.7's builtin (0x80bdab0) reads the
/// fifth argument, so the stock `Callback_PlayerKilled` reads the planter,
/// not the world (the map sweep's `Callback_PlayerKilled:4787` errors,
/// cod4-combat.md 10.10).
#[test]
fn a_bomb_blast_credits_its_attacker() {
    let Some((mut sv, mut now)) = level("mp_crash", "war", 100) else {
        return;
    };
    let slot = sv.add_bots(1, false)[0];
    run(&mut sv, &mut now, 8000);
    let rt = &mut sv.game.level.as_mut().unwrap().rt;
    assert_eq!(
        rt.client_field(slot, "sessionstate").as_deref(),
        Some("playing")
    );
    let me = rt.client_entity(slot).unwrap();
    rt.install_for_test(
        "blast(attacker)\n{\n\tbomb = spawn(\"script_model\", attacker.origin);\n\tbomb radiusDamage(attacker.origin + (0,0,30), 512, 400, 400, attacker);\n}\n",
    );
    let entry = rt.entry_for_test().to_string();
    rt.start_script_thread_for_test(&entry, "blast", None, |_| vec![vcod_gsc::Value::Entity(me)]);
    run(&mut sv, &mut now, 200);
    let rt = &mut sv.game.level.as_mut().unwrap().rt;
    assert_eq!(
        rt.client_field(slot, "sessionstate").as_deref(),
        Some("dead")
    );
    let aborts = rt.aborts();
    assert!(aborts.is_empty(), "script errors:\n{}", aborts.join("\n"));
}

/// A blast with no attacker (`sd.gsc` 807, `_destructible.gsc` 1064)
/// hands the callbacks the world, whose classname `SP_worldspawn`
/// (0x80c83ea) sets to `worldspawn`, so `Callback_PlayerKilled:4787`'s
/// `attacker.classname == "script_vehicle"` compares two strings
/// (cod4-gsc.md 3.5).
#[test]
fn a_blast_with_no_attacker_names_the_world() {
    let Some((mut sv, mut now)) = level("mp_crash", "war", 100) else {
        return;
    };
    let slot = sv.add_bots(1, false)[0];
    run(&mut sv, &mut now, 8000);
    let rt = &mut sv.game.level.as_mut().unwrap().rt;
    let me = rt.client_entity(slot).unwrap();
    rt.install_for_test(
        "blast(victim)\n{\n\tbomb = spawn(\"script_model\", victim.origin);\n\tbomb radiusDamage(victim.origin + (0,0,30), 512, 400, 400);\n}\n",
    );
    let entry = rt.entry_for_test().to_string();
    rt.start_script_thread_for_test(&entry, "blast", None, |_| vec![vcod_gsc::Value::Entity(me)]);
    run(&mut sv, &mut now, 200);
    let rt = &mut sv.game.level.as_mut().unwrap().rt;
    assert_eq!(
        rt.client_field(slot, "sessionstate").as_deref(),
        Some("dead")
    );
    let aborts = rt.aborts();
    assert!(aborts.is_empty(), "script errors:\n{}", aborts.join("\n"));
}

/// 1.7's `ClientEndFrame` (0x80a68b8) indexes every connected client's
/// attachment tags ahead of its session checks, so a tag a spectating
/// client's attachment names first takes its configstring there too
/// (cod4-player-anims.md 5).
#[test]
fn a_spectators_new_attachment_tag_takes_its_index() {
    let Some((mut sv, mut now)) = level("mp_crash", "sd", 100) else {
        return;
    };
    let slot = sv.add_bots(1, false)[0];
    run(&mut sv, &mut now, 8000);
    let rt = &mut sv.game.level.as_mut().unwrap().rt;
    let me = rt.client_entity(slot).unwrap();
    rt.install_for_test(
        "probe(p)\n{\n\tp.sessionstate = \"spectator\";\n\tp attach(\"prop_suitcase_bomb\", \"tag_inhand\", true);\n}\n",
    );
    let entry = rt.entry_for_test().to_string();
    rt.start_script_thread_for_test(&entry, "probe", None, |_| vec![vcod_gsc::Value::Entity(me)]);
    run(&mut sv, &mut now, 100);
    let rt = &mut sv.game.level.as_mut().unwrap().rt;
    let aborts = rt.aborts();
    assert!(aborts.is_empty(), "script errors:\n{}", aborts.join("\n"));
    let (model, tag) = *rt.client_attachments(slot).last().unwrap();
    assert_ne!(model, 0, "the bomb's model");
    assert_ne!(tag, 0, "tag_inhand took no index");
}
