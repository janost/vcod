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

/// 800 ms on, the frozen `slot` stood on the item it dropped, wherever the
/// drop's `crandom`s put it, and 1200 ms more.
fn stand_on_the_drop(sv: &mut Cod4Server, now: &mut Instant, slot: usize) {
    run(sv, now, 800);
    let level = sv.game.level.as_mut().unwrap();
    // The eye 59 over the item puts the feet of a standing body on it.
    let at = level.rt.host.cod4().items[0].origin + glam::Vec3::Z * 59.0;
    level.set_view_pos(slot, at.to_array(), [0.0; 3]);
    run(sv, now, 1200);
}

/// `G_TouchTriggers` (0x80a5352) on a dropped weapon: every frame the
/// dropper stands on it the item and the dropper are notified `"touch"`,
/// though his own item is not his for 1000 ms; then, the same weapon held
/// with an empty reserve, he takes its rounds, the item is notified
/// `"trigger"` with him and no drop, and goes (cod4-combat.md 6.2.1).
#[test]
fn a_dropped_weapon_is_notified_touch_then_trigger() {
    let Some((mut sv, mut now)) = level("mp_crash", "war", 100) else {
        return;
    };
    let slot = sv.add_bots(1, false)[0];
    run(&mut sv, &mut now, 8000);
    let rt = &mut sv.game.level.as_mut().unwrap().rt;
    let me = rt.client_entity(slot).unwrap();
    rt.install_for_test(
        "drop(p)\n{\n\tw = p getCurrentWeapon();\n\tp freezeControls(true);\n\titem = p dropItem(w);\n\titem itemWeaponSetAmmo(30, 60);\n\tp giveWeapon(w);\n\tp setWeaponAmmoStock(w, 0);\n\tlevel.wakes = 0;\n\tlevel.touches = 0;\n\titem thread once();\n\titem thread count();\n\titem thread taken();\n\tp thread mine(item);\n}\n\
         once()\n{\n\tself waittill(\"touch\", who);\n\tlevel.wakes++;\n\tlevel.toucher = who;\n}\n\
         count()\n{\n\tfor(;;)\n\t{\n\t\tself waittill(\"touch\", who);\n\t\tlevel.touches++;\n\t}\n}\n\
         taken()\n{\n\tself waittill(\"trigger\", who, dropped);\n\tlevel.taker = who;\n\tlevel.no_drop = !isDefined(dropped);\n\tlevel.touches_at_take = level.touches;\n}\n\
         mine(item)\n{\n\tself waittill(\"touch\", other);\n\tlevel.mine = other == item;\n}\n",
    );
    let entry = rt.entry_for_test().to_string();
    rt.start_script_thread_for_test(&entry, "drop", None, |_| vec![vcod_gsc::Value::Entity(me)]);
    stand_on_the_drop(&mut sv, &mut now, slot);
    let level = sv.game.level.as_mut().unwrap();
    let rt = &mut level.rt;
    let aborts = rt.aborts();
    assert!(aborts.is_empty(), "script errors:\n{}", aborts.join("\n"));
    let int = |rt: &mut crate::game::script::ScriptRuntime, n: &str| match rt.level_field(n) {
        vcod_gsc::Value::Int(i) => i,
        v => panic!("level.{n}: {v:?}"),
    };
    assert_eq!(int(rt, "wakes"), 1);
    assert!(matches!(rt.level_field("toucher"), vcod_gsc::Value::Entity(e) if e == me));
    assert_eq!(int(rt, "mine"), 1, "the dropper's touch names the item");
    assert!(matches!(rt.level_field("taker"), vcod_gsc::Value::Entity(e) if e == me));
    assert_eq!(int(rt, "no_drop"), 1);
    // A touch each frame he stood on it while it was not his to take.
    let touches = int(rt, "touches_at_take");
    assert!(touches >= 4, "{touches} touches");
    assert!(rt.host.cod4().items.is_empty(), "the item is still there");
    assert!(level.pm_state(slot).unwrap().ammo.iter().sum::<i32>() > 0);
}

/// `Touch_Item`'s ammo-only take (0x8060dd4, 0x8091d44): a dropped weapon
/// not held, with a held one on its ammo slot, gives its stock and not its
/// clip, is notified `"trigger"`, and stays with the rest (cod4-combat.md
/// 6.2.1).
#[test]
fn a_weapon_on_the_same_ammo_slot_takes_the_stock_only() {
    let Some((mut sv, mut now)) = level("mp_crash", "war", 100) else {
        return;
    };
    let slot = sv.add_bots(1, false)[0];
    run(&mut sv, &mut now, 8000);
    let level = sv.game.level.as_mut().unwrap();
    let w = level.pm_state(slot).unwrap().weapon as usize;
    let defs = &level.rt.host.cod4().table.defs;
    let twin = (1..defs.len())
        .find(|&k| k != w && defs[k].ammo_slot == defs[w].ammo_slot && defs[k].alt != w)
        .map(|k| defs[k].name.clone())
        .expect("a weapon sharing the ammo slot");
    let rt = &mut level.rt;
    let me = rt.client_entity(slot).unwrap();
    rt.install_for_test(
        "drop(p, twin)\n{\n\tw = p getCurrentWeapon();\n\tp freezeControls(true);\n\titem = p dropItem(w);\n\titem itemWeaponSetAmmo(30, 60);\n\tp takeWeapon(w);\n\tp giveWeapon(twin);\n\tp setWeaponAmmoStock(twin, 0);\n\tlevel.item = item;\n\titem thread taken();\n}\n\
         taken()\n{\n\tself waittill(\"trigger\", who);\n\tlevel.taken = 1;\n}\n",
    );
    let entry = rt.entry_for_test().to_string();
    rt.start_script_thread_for_test(&entry, "drop", None, |cx| {
        vec![
            vcod_gsc::Value::Entity(me),
            vcod_gsc::Value::String(cx.intern_exact(&twin)),
        ]
    });
    stand_on_the_drop(&mut sv, &mut now, slot);
    let level = sv.game.level.as_mut().unwrap();
    let aborts = level.rt.aborts();
    assert!(aborts.is_empty(), "script errors:\n{}", aborts.join("\n"));
    assert!(matches!(
        level.rt.level_field("taken"),
        vcod_gsc::Value::Int(1)
    ));
    let items = &level.rt.host.cod4().items;
    assert_eq!(items.len(), 1, "the item went");
    assert_eq!((items[0].clip, items[0].stock), (30, 0));
    let ammo = level.pm_state(slot).unwrap().ammo;
    assert_eq!(ammo[level.rt.host.cod4().table.defs[w].ammo_slot], 60);
}

/// A restart into a gametype with no script tears the level down and fails:
/// the fatal is kept for the loop, the cvar table (`+set`s, rcon's
/// password) survives, and a good map loads over it.
#[test]
fn a_restart_into_a_missing_gametype_keeps_the_cvars_and_reports_fatal() {
    let install = vcod_cod4::testing::cod4_install();
    let Some((mut sv, mut now)) = level("mp_crash", "war", 100) else {
        return;
    };
    sv.set_cvar("rcon_password", "hunter2");
    sv.core.console.push_back("g_gametype bogus".into());
    sv.core.console.push_back("map_restart".into());
    run(&mut sv, &mut now, 100);
    assert!(sv.game.level.is_none());
    let fatal = <Cod4Game as crate::engine::GameModule>::take_fatal(&mut sv);
    assert!(fatal.is_some(), "no fatal");
    assert_eq!(sv.game.cvars().get("rcon_password"), "hunter2");
    assert_eq!(sv.game.cvars().get("sv_pure"), "0");
    sv.set_cvar("g_gametype", "war");
    sv.core.console.push_back("map mp_crash".into());
    run(&mut sv, &mut now, 100);
    assert!(sv.game.level.is_some() && install.is_some());
    assert_eq!(sv.game.cvars().get("rcon_password"), "hunter2");
}
