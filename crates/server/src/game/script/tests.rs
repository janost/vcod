use super::*;

/// The per-tick copy from the sim keeps an op the sim has not applied
/// yet: the copy predates it, the mirror must not.
#[test]
fn the_per_tick_ammo_copy_keeps_a_queued_op() {
    use crate::game::host::WeaponOp;
    use vcod_common::pmove::weapon::NUM_AMMO;
    let mut rt = ScriptRuntime::for_test("main() {}");
    rt.host.weapon_op(
        0,
        WeaponOp::SetAmmo {
            ammo_index: 10,
            rounds: 400,
        },
    );
    rt.host.weapon_op(
        1,
        WeaponOp::SetClip {
            clip_index: 3,
            rounds: 7,
        },
    );
    let mut clip = [0; NUM_AMMO];
    clip[3] = 2;
    rt.set_client_ammo(0, [0; NUM_AMMO], clip);
    assert_eq!(rt.host.clients[0].ammo.ammo[10], 400);
    assert_eq!(rt.host.clients[0].ammo.clip[3], 2, "slot 1's op leaked");
}

/// `probe_delete`'s measurement: an entity deleted in one frame is out of
/// `getEntArray` for a thread that wakes past the deferred free, and
/// still in it for one that looks before. The packet pass ahead of both
/// must not step that thread early.
#[test]
fn a_deleted_entity_is_gone_by_the_thread_pass_that_looks_past_the_defer() {
    let mut rt = ScriptRuntime::for_test(
        "main() { e = spawn(\"script_origin\", (0, 0, 0)); e delete(); \
         level.now = getentarray(\"script_origin\", \"classname\").size; \
         wait 0.5; \
         level.later = getentarray(\"script_origin\", \"classname\").size; }",
    );
    assert_eq!(
        rt.level_field("now"),
        Value::Int(1),
        "delete() freed at once"
    );
    for frame in 1..=12 {
        rt.run_frame(frame * 50);
    }
    assert_eq!(rt.level_field("later"), Value::Int(0));
}

/// `delete()` notifies `"death"` with the entity (1.1d 0x5da14, UO
/// 0x68bda): a thread holding `point endon("death")` on a deleted
/// spawn point ends there, and one waiting on it wakes with the entity.
/// UO's `_spawnlogic::Player_WaitForDeath` wrote through the dead point
/// once its player died, a script error (bot-objectives.md 7.1).
#[test]
fn delete_notifies_death() {
    let mut rt = ScriptRuntime::for_test(
        "hold(p) { p endon(\"death\"); level waittill(\"go\"); level.held = 1; } \
         watch(p) { p waittill(\"death\", e); level.woke = (e == p); } \
         main() { p = spawn(\"script_origin\", (0, 0, 0)); level thread hold(p); \
         level thread watch(p); wait 0.05; p delete(); wait 0.05; level notify(\"go\"); }",
    );
    for frame in 1..=4 {
        rt.run_frame(frame * 50);
    }
    assert_eq!(rt.level_field("woke"), Value::Int(1));
    assert_eq!(rt.level_field("held"), Value::Undefined);
}

/// `delete()` on a connected client notifies `death` but leaves his
/// entity: the slot is the connection's until the drop.
#[test]
fn a_deleted_client_keeps_his_entity() {
    let mut rt = ScriptRuntime::for_test_at(
        CALLBACK_SETUP,
        "main() { level.callbackPlayerConnect = ::c; }\n\
         CodeCallback_PlayerConnect() { [[level.callbackPlayerConnect]](); }\n\
         c() { self thread d(); level thread w(self); }\n\
         d() { wait 0.05; self delete(); }\n\
         w(p) { p waittill(\"death\"); level.died = 1; }\n",
    );
    rt.reconnect_client(0, "vcod".into());
    let before = rt.client_entity(0).expect("connected");
    for frame in 1..=6 {
        rt.run_frame(frame * 50);
    }
    assert_eq!(rt.level_field("died"), Value::Int(1));
    assert_eq!(rt.client_entity(0), Some(before));
    assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
}

/// CoD4's `delete()` refuses a client with a script error before any
/// notify (cod4-combat.md 14).
#[test]
fn cod4_refuses_to_delete_a_client() {
    let mut rt = ScriptRuntime::for_test_at(
        CALLBACK_SETUP,
        "main() { level.callbackPlayerConnect = ::c; }\n\
         CodeCallback_PlayerConnect() { [[level.callbackPlayerConnect]](); }\n\
         c() { self thread d(); level thread w(self); }\n\
         d() { wait 0.05; self delete(); level.past = 1; }\n\
         w(p) { p waittill(\"death\"); level.died = 1; }\n",
    );
    rt.host.set_profile(&crate::game::profile::COD4);
    rt.reconnect_client(0, "vcod".into());
    let before = rt.client_entity(0).expect("connected");
    for frame in 1..=6 {
        rt.run_frame(frame * 50);
    }
    assert_eq!(rt.level_field("died"), Value::Undefined);
    assert_eq!(rt.level_field("past"), Value::Undefined);
    assert_eq!(rt.client_entity(0), Some(before));
    assert!(
        rt.aborts()
            .iter()
            .any(|a| a.contains("Cannot delete a client entity")),
        "{:?}",
        rt.aborts()
    );
}

/// `probe_entnum` on retail: the free a `delete()` schedules 100 ms out
/// runs in that frame's entity pass, after the frame's threads, so a
/// thread woken on it still reads the entity (combat doc 14.7, "Entity
/// numbers").
#[test]
fn a_delete_s_free_runs_after_the_threads_of_its_frame() {
    let mut rt = ScriptRuntime::for_test(
        "main() { wait 0.05; e = spawn(\"script_origin\", (0, 0, 0)); e delete(); \
         wait 0.1; level.at100 = isdefined(e); \
         wait 0.05; level.at150 = isdefined(e); }",
    );
    for frame in 1..=5 {
        rt.run_frame(frame * 50);
    }
    assert_eq!(rt.level_field("at100"), Value::Int(1));
    assert_eq!(rt.level_field("at150"), Value::Int(0));
}

/// `probe_entnum` on retail: a `playFx` temp entity takes the next
/// number off `G_Spawn` between two script spawns, and goes to the tail
/// of the free list on the first entity pass more than 300 ms past its
/// event, so the spawn on the frame after that reuses it.
#[test]
fn a_temp_entity_takes_a_spawn_number_and_frees_it_past_300_ms() {
    let mut rt = ScriptRuntime::for_test(
        "num(e) { return e getEntityNumber(); } \
         main() { wait 0.05; a = spawn(\"script_origin\", (0, 0, 0)); playfx(1, (0, 0, 0)); \
         b = spawn(\"script_origin\", (0, 0, 0)); level.a = num(a); level.b = num(b); \
         wait 0.3; level.c = num(spawn(\"script_origin\", (0, 0, 0))); \
         wait 0.05; level.d = num(spawn(\"script_origin\", (0, 0, 0))); \
         wait 0.05; level.e = num(spawn(\"script_origin\", (0, 0, 0))); }",
    );
    // From a frame's thread pass, as the probe's spawns were: there the
    // script clock runs a frame behind `level.time`.
    for frame in 1..=10 {
        rt.run_frame(frame * 50);
    }
    let n = |rt: &mut ScriptRuntime, k: &str| match rt.level_field(k) {
        Value::Int(n) => n,
        other => panic!("{k}: {other:?}"),
    };
    let a = n(&mut rt, "a");
    let got: Vec<i32> = ["b", "c", "d", "e"].map(|k| n(&mut rt, k) - a).to_vec();
    assert_eq!(got, [2, 3, 4, 1]);
}

/// `probe_concnum` on retail: `grenadeExplosionEffect` takes two numbers
/// off `G_Spawn`, the temp entity's and `Concussive_fx`'s. The first
/// comes back on the +350 pass and the second on the +700 one, so of
/// one spawn a frame after the call, the +400 one and the +750 one reuse
/// them (combat doc 13.4).
#[test]
fn grenade_explosion_effect_takes_a_temp_and_a_concussion_number() {
    let mut rt = ScriptRuntime::for_test(
        "n(a) { return (spawn(\"script_origin\", (0, 0, 0)) getEntityNumber()) - a; } \
         main() { wait 0.05; a = spawn(\"script_origin\", (0, 0, 0)) getEntityNumber(); \
         grenadeexplosioneffect((0, 0, 0)); level.s = \"\" + n(a); \
         for (i = 1; i <= 16; i++) { wait 0.05; level.s = level.s + \" \" + n(a); } }",
    );
    // From a frame's thread pass, as the probe's calls were (after `wait 1`).
    for frame in 1..=21 {
        rt.run_frame(frame * 50);
    }
    // Retail: the spawn after the call 3, the +400 one the temp's 1,
    // the +750 one the concussion's 2.
    assert_eq!(
        rt.level_field_str("s"),
        "3 4 5 6 7 8 9 10 1 11 12 13 14 15 16 2 17"
    );
}

/// Both closures load into one `Vm` through one `Loader`, which is what
/// keeps `Vm::install`'s duplicate rejection from firing on the first
/// file the two share (`_utility`, `_teams` and the rest).
#[test]
fn the_map_and_gametype_closures_both_load() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let rt = ScriptRuntime::load(
        Rc::new(fs),
        "mp_pavlov",
        "dm",
        vec![String::new(); 2048],
        crate::cvars::Cvars::new(),
        None,
        Rc::new(crate::weapons::WeaponTable::empty()),
        0,
        Carry::default(),
    );
    assert!(rt.is_ok(), "{:?}", rt.err());
}

/// A CoD1 load handed the last level's gametype compile resumes it and
/// ends with the compile a load from nothing makes.
#[test]
fn a_cod1_load_resumes_the_gametype_compile_where_a_fresh_one_lands() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let fs = Rc::new(fs);
    let load = |map: &str, carry: Carry| {
        ScriptRuntime::load(
            fs.clone(),
            map,
            "dm",
            vec![String::new(); 2048],
            crate::cvars::Cvars::new(),
            None,
            Rc::new(crate::weapons::WeaponTable::empty()),
            0,
            carry,
        )
        .unwrap()
    };
    let first = load("mp_pavlov", Carry::default());
    let carry = Carry {
        gametype: first.take_carry().gametype,
        ..Carry::default()
    };
    let resumed = load("mp_carentan", carry);
    let fresh = load("mp_carentan", Carry::default());
    let (a, b, c) = (
        first.compiled().unwrap(),
        resumed.compiled().unwrap(),
        fresh.compiled().unwrap(),
    );
    assert!(Rc::ptr_eq(a.gametype(), b.gametype()));
    assert!(!Rc::ptr_eq(b.gametype(), c.gametype()));
    assert!(b.same_compile(c));
    assert_eq!(resumed.configstrings(), fresh.configstrings());
}

/// mp_depot's `*1` is a `script_brushmodel` carrying `script_exploder`
/// and `targetname "exploder"`, which is the arm of `_load.gsc::main`
/// that calls `notsolid()`. The bootstrap therefore has to leave those
/// brushes out of the clip; `*2` carries `script_exploder` with no
/// targetname, so no arm reaches it and it stays solid. mp_powcamp
/// (`*3`, `*9`) and mp_rocket (`*3`) are the other two stock maps with
/// one. The gametype is `sd` because `*1` is also a `bombzone`, which
/// `_gameobjects::main` deletes under every other gametype, which
/// would be another reason to unlink it.
#[test]
fn the_bootstrap_unlinks_mp_depots_exploder_brush_model() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let bytes = fs.read("maps/mp/mp_depot.bsp").expect("mp_depot.bsp");
    let bsp = vcod_common::bsp::parse(&bytes).unwrap();
    // No paks for the props: only the submodel brushes matter here.
    let world = Rc::new(crate::world::World::from_bsp(&bsp, None));
    ScriptRuntime::load(
        Rc::new(fs),
        "mp_depot",
        "sd",
        vec![String::new(); 2048],
        crate::cvars::Cvars::new(),
        Some(world.clone()),
        Rc::new(crate::weapons::WeaponTable::empty()),
        0,
        Carry::default(),
    )
    .expect("load mp_depot on sd");
    assert!(!world.collision.model_linked(1), "exploder stays solid");
    assert!(world.collision.model_linked(2));
}

/// Stock `_utility::getPlant` from where retail's planter stood in the
/// plant capture, facing its view yaw: neither 18-unit trace meets the
/// clip-only floor, the fallback's `(+16, +16)` trace lands on bombzone_A's
/// flak88 `script_model`, and `objective_add` truncates that to the
/// `-176, 2473, -22` retail's slot 0 reads at 91800
/// (docs/research/cod11-gsc-object-model.md 23.3, 23.6).
#[test]
fn get_plant_puts_the_charge_where_retail_did_on_mp_carentan() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let bytes = fs.read("maps/mp/mp_carentan.bsp").expect("mp_carentan.bsp");
    let bsp = vcod_common::bsp::parse(&bytes).unwrap();
    let world = Rc::new(crate::world::World::from_bsp(&bsp, Some(&fs)));
    let mut rt = ScriptRuntime::load(
        Rc::new(fs),
        "mp_carentan",
        "sd",
        vec![String::new(); 2048],
        crate::cvars::Cvars::new(),
        Some(world),
        Rc::new(crate::weapons::WeaponTable::empty()),
        0,
        Carry::default(),
    )
    .expect("load mp_carentan on sd");
    rt.install_for_test(
        "plant() { self.angles = (0, 29.6, 0); p = self maps\\mp\\_utility::getPlant(); \
         level.charge = p.origin; objective_add(0, \"current\", p.origin); }",
    );
    let planter = rt.spawn_map_entity_for_test([-192.8, 2457.1, -21.9]);
    rt.start_thread_for_test(planter, "plant");
    rt.run_frame(50);
    assert_eq!(rt.aborts(), Vec::<String>::new());
    let Value::Vector(charge) = rt.level_field("charge") else {
        panic!("getPlant returned no origin");
    };
    // The slot is all retail measured of the charge, and it is truncated,
    // so each component is within a unit of it rather than the point.
    let retail = glam::Vec3::new(-176.0, 2473.0, -22.0);
    assert!(
        (glam::Vec3::from(charge) - retail).abs().max_element() < 1.0,
        "charge at {charge:?}"
    );
    assert_eq!(rt.host.objectives[0].origin_f32(), retail.to_array());
}

/// `mp_pavlov.gsc` sets `game["allies"] = "russian"`, and dm's
/// `Callback_StartGameType` turns that into `team_russiangerman`, so
/// this pins that the map's `main` ran before the code callback and that
/// all three precache families reached the table. It does **not** pin
/// the order of the two `main`s: `dm.gsc`'s own `main` never reads
/// `game["allies"]` and its `Callback_StartGameType` defaults the key
/// when unset, so 1180 reads the same either way. That order is pinned
/// by `probe_bootstrap_matches_retail` in
/// `crates/server/tests/semantics_ents.rs`, which drives this same
/// bootstrap with the probe as its gametype.
#[test]
fn the_bootstrap_precaches_the_russian_team_menu_on_mp_pavlov() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let rt = ScriptRuntime::load(
        Rc::new(fs),
        "mp_pavlov",
        "dm",
        vec![String::new(); 2048],
        crate::cvars::Cvars::new(),
        None,
        Rc::new(crate::weapons::WeaponTable::empty()),
        0,
        Carry::default(),
    )
    .expect("load mp_pavlov on dm");
    assert_eq!(rt.configstrings()[1180], "team_russiangerman");
    assert_eq!(rt.configstrings()[1181], "weapon_russian");
    assert_eq!(rt.configstrings()[21], "gfx/hud/hud@status_dead.tga");
    assert_eq!(
        rt.configstrings()[1501],
        "levelshots/layouts/hud@layout_mp_pavlov"
    );
}

/// A gametype script that resolves but defines no `main` reached
/// `Vm::start_thread`'s "target must be installed" panic. `--gametype` is
/// user input, so it has to come back as an error `main.rs` exits on,
/// same as a gametype whose file does not exist at all.
#[test]
fn a_gametype_without_a_main_is_an_error_not_a_panic() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    /// The paks with one gametype path answered by a script that has
    /// every part of a gametype except the entry point.
    struct NoMain(Rc<Pk3Fs>);
    impl ScriptSource for NoMain {
        fn read(&self, canonical: &str) -> Option<String> {
            if canonical == "maps/mp/gametypes/nomain" {
                return Some("notMain() {}\n".to_string());
            }
            PakScripts::new(self.0.clone(), None).read(canonical)
        }
    }
    let fs = Rc::new(fs);
    let err = ScriptRuntime::load_from(
        Box::new(NoMain(fs.clone())),
        fs,
        "mp_pavlov",
        "nomain",
        vec![String::new(); 2048],
        crate::cvars::Cvars::new(),
        None,
        Rc::new(crate::weapons::WeaponTable::empty()),
        0,
        Carry::default(),
    );
    let Err(err) = err else {
        panic!("a gametype with no main must not load");
    };
    assert!(
        err.to_string().contains("defines no main()"),
        "unexpected error: {err:#}"
    );
}

/// A call outside the version's builtin tables is retail's script
/// compile error, which fails the load; the same script loads under the
/// version that lists the name (`builtins::set`).
#[test]
fn a_builtin_another_version_lists_fails_the_load() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    struct UoCall(Rc<Pk3Fs>);
    impl ScriptSource for UoCall {
        fn read(&self, canonical: &str) -> Option<String> {
            if canonical == "maps/mp/gametypes/uocall" {
                return Some("main() { n = getNumVehicles(); }\n".to_string());
            }
            PakScripts::new(self.0.clone(), None).read(canonical)
        }
    }
    let fs = Rc::new(fs);
    let load = |protocol: &str| {
        let mut cvars = crate::cvars::Cvars::new();
        cvars.set("protocol", protocol);
        ScriptRuntime::load_from(
            Box::new(UoCall(fs.clone())),
            fs.clone(),
            "mp_pavlov",
            "uocall",
            vec![String::new(); 2048],
            cvars,
            None,
            Rc::new(crate::weapons::WeaponTable::empty()),
            0,
            Carry::default(),
        )
    };
    let Err(err) = load("1") else {
        panic!("getNumVehicles must not compile on 1.1");
    };
    assert!(
        err.to_string()
            .contains("unknown (builtin) function 'getnumvehicles'"),
        "unexpected error: {err:#}"
    );
    if let Err(e) = load("22") {
        assert!(!e.to_string().contains("unknown (builtin)"), "{e:#}");
    }
}

/// The engine's side of the connect handshake is one notify and its exact
/// spelling. A miss does not error, it hangs the thread, so this test
/// exists to fail loudly if the constant is ever edited.
#[test]
fn the_begin_notify_is_spelled_begin() {
    assert_eq!(BEGIN_NOTIFY, "begin");
}

/// The whole team-join state machine wakes on this one notify, and a
/// misspelling hangs the loop with nothing logged, so the spelling is
/// pinned the same way `begin` is.
#[test]
fn the_menu_notify_is_spelled_menuresponse() {
    assert_eq!(MENURESPONSE_NOTIFY, "menuresponse");
}

/// What `savePersist` keeps for a client across a restart
/// (docs/research/cod11-map-cycle.md section 1): the `pers[]` table, and
/// not the rest of the entity. The second level is a second
/// `ScriptRuntime` with its own `Vm`, so this only passes if the copy
/// travelled as text through the carry and was re-interned on the far
/// side. The two levels default the key differently, so the value read
/// after says which one wrote it.
/// 1.5's `maxspeed`: `g_speed`'s integer at the connect, floored at 0
/// by its setter, which copies it into the entity's `speed`; it is what
/// `ClientEndFrame` stores in `ps.speed`. 1.1 stores `g_speed` itself.
#[test]
fn maxspeed_seeds_from_g_speed_and_feeds_the_end_frame() {
    let src = "main() { level.callbackPlayerConnect = ::c; }\n\
               CodeCallback_PlayerConnect() { [[level.callbackPlayerConnect]](); }\n\
               c() { level.seeded = self.maxspeed; self.maxspeed = -5; \
                     level.floored = self.maxspeed; level.ent = self.speed; \
                     self.maxspeed = 220; }\n";
    let mut rt = ScriptRuntime::for_test_at(CALLBACK_SETUP, src);
    rt.host.set_profile(&crate::game::profile::COD1_15);
    rt.host.cvars.set("g_speed", "300.9");
    rt.reconnect_client(0, "vcod".into());
    assert_eq!(rt.level_field("seeded"), vcod_gsc::Value::Int(300));
    assert_eq!(rt.level_field("floored"), vcod_gsc::Value::Int(0));
    assert_eq!(rt.level_field("ent"), vcod_gsc::Value::Float(0.0));
    assert_eq!(rt.host.client_speed(0), 220);
    // A later `g_speed` waits for the next connect.
    rt.host.cvars.set("g_speed", "190");
    assert_eq!(rt.host.client_speed(0), 220);

    rt.host.set_profile(&crate::game::profile::COD1_11);
    assert_eq!(rt.host.client_speed(0), 190);
}

#[test]
fn a_carried_pers_is_back_on_the_client_the_next_level_connects() {
    fn src(default_team: &str) -> String {
        format!(
            "main() {{ level.callbackPlayerConnect = ::c; }}\n\
             CodeCallback_PlayerConnect() {{ [[level.callbackPlayerConnect]](); }}\n\
             c() {{ if (!isdefined(self.pers[\"team\"])) self.pers[\"team\"] = \"{default_team}\"; \
                   self.statusicon = self.pers[\"team\"]; }}\n"
        )
    }

    let mut before = ScriptRuntime::for_test_at(CALLBACK_SETUP, &src("axis"));
    before.reconnect_client(0, "vcod".into());
    assert_eq!(before.client_pers(0, "team").as_deref(), Some("axis"));
    // Something only the first level wrote, to show what does not carry.
    before.host.clients[0].vitals.health = 42;
    let carry = before.take_carry();

    let mut after = ScriptRuntime::for_test_at(CALLBACK_SETUP, &src("allies"));
    after.host.pers_carry = carry.pers;
    after.reconnect_client(0, "vcod".into());
    assert_eq!(
        after.client_pers(0, "team").as_deref(),
        Some("axis"),
        "the carried pers did not reach the reconnected client"
    );
    // The `gclient_t` itself does not carry: retail bzeroes it in
    // `ClientConnect` and the script-side `pers` is the whole of what
    // survives.
    assert_eq!(after.host.clients[0].vitals.health, 0);

    // The carry is spent by the connect that took it, so the next one
    // starts empty, which is every connect but a persisting
    // boundary's.
    after.reconnect_client(0, "vcod".into());
    assert_eq!(after.client_pers(0, "team").as_deref(), Some("allies"));
}

/// The carry a level boundary lifted is spent by a disconnect as well as
/// by a connect. `map_restart` sends its `n` through the reliable guard,
/// which can drop the client whose `pers[]` was just lifted, and the next
/// client to take that slot must not inherit the old one's team, score or
/// weapons.
#[test]
fn a_disconnect_spends_the_slot_s_carried_pers() {
    fn src(default_team: &str) -> String {
        format!(
            "main() {{ level.callbackPlayerConnect = ::c; \
                      level.callbackPlayerDisconnect = ::d; }}\n\
             CodeCallback_PlayerConnect() {{ [[level.callbackPlayerConnect]](); }}\n\
             CodeCallback_PlayerDisconnect() {{ [[level.callbackPlayerDisconnect]](); }}\n\
             c() {{ if (!isdefined(self.pers[\"team\"])) self.pers[\"team\"] = \"{default_team}\"; }}\n\
             d() {{ level.gone = 1; }}\n"
        )
    }

    let mut before = ScriptRuntime::for_test_at(CALLBACK_SETUP, &src("axis"));
    before.reconnect_client(0, "vcod".into());
    assert_eq!(before.client_pers(0, "team").as_deref(), Some("axis"));
    let carry = before.take_carry();

    let mut after = ScriptRuntime::for_test_at(CALLBACK_SETUP, &src("allies"));
    after.host.pers_carry = carry.pers;
    // The drop the restart's `n` can cause: the slot has no entity in
    // this runtime yet, its `ClientConnect` has not run.
    after.push_client_event(ClientEvent::Disconnect(0));
    after.run_frame(50);

    after.reconnect_client(0, "stranger".into());
    assert_eq!(
        after.client_pers(0, "team").as_deref(),
        Some("allies"),
        "the dropped client's carried pers reached the next occupant"
    );
    assert!(after.aborts().is_empty(), "{:?}", after.aborts());
}

/// A client connecting into a slot its last occupant left in the same
/// level reads a zeroed `gclient_t`: no health, no `maxhealth`, not
/// alive, no `allowSpectateTeam` bits.
#[test]
fn a_connect_into_a_used_slot_starts_with_no_vitals_or_spectate_mask() {
    use crate::game::host::Vitals;
    let mut rt = ScriptRuntime::for_test_at(
        CALLBACK_SETUP,
        "main() { level.callbackPlayerConnect = ::c; }\n\
         CodeCallback_PlayerConnect() { [[level.callbackPlayerConnect]](); }\n\
         CodeCallback_PlayerDisconnect() {}\n\
         c() { level.h = self.health; level.m = self.maxhealth; \
               level.alive = isAlive(self); }\n",
    );
    rt.reconnect_client(0, "first".into());
    rt.host.clients[0].vitals = Vitals {
        health: 100,
        max_health: 100,
        dead: false,
        takedamage: true,
    };
    rt.host.clients[0].no_spectate = 0x17;
    rt.client_disconnect(0);
    rt.run_frame(50);

    rt.reconnect_client(0, "second".into());
    assert_eq!(rt.level_field("h"), vcod_gsc::Value::Int(0));
    assert_eq!(rt.level_field("m"), vcod_gsc::Value::Int(0));
    assert_eq!(rt.level_field("alive"), vcod_gsc::Value::Int(0));
    assert_eq!(rt.host.clients[0].vitals, Vitals::default());
    assert_eq!(rt.host.clients[0].no_spectate, 0);
}

/// `ClientDisconnect`'s `menuresponse` ("-1", "disconnect") wakes a
/// level thread waiting on the player, which runs on the next frame's
/// clock, after the callback; the player's own waiter dies with it
/// (`client-probes/probe_dropclient`, all three retail servers).
#[test]
fn a_drop_notifies_menuresponse_for_the_next_frame() {
    let mut rt = ScriptRuntime::for_test_at(
        CALLBACK_SETUP,
        "main() { level.lvl = \"none\"; level.own = \"none\"; }\n\
         CodeCallback_PlayerConnect() { level thread lvl(self); self thread own(); }\n\
         CodeCallback_PlayerDisconnect() { level.cb = getTime(); }\n\
         lvl(p) { p waittill(\"menuresponse\", menu, response); \
                  level.lvl = menu + \" \" + response + \" \" + getTime(); }\n\
         own() { self waittill(\"menuresponse\", menu, response); level.own = menu; }\n",
    );
    rt.push_client_event(ClientEvent::Connect {
        slot: 0,
        name: "vcod".into(),
    });
    rt.run_frame(50);
    rt.client_disconnect(0);
    assert_eq!(rt.level_field("cb"), Value::Int(50));
    // `CalculateRanks` closes `ClientDisconnect`.
    assert!(rt.take_ranks_dirty());
    assert_eq!(rt.level_field_str("lvl"), "none");
    rt.run_frame(100);
    assert_eq!(rt.level_field_str("lvl"), "-1 disconnect 100");
    assert_eq!(rt.level_field_str("own"), "none");
    assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
}

/// Connect arms the callback's `waittill("begin")` and Begin releases it.
/// Order matters and the failure is silent: `Callback_PlayerConnect`
/// blocks on that notify as its second statement, so a Begin drained
/// before its Connect leaves the thread parked forever with nothing
/// logged.
#[test]
fn connect_arms_the_wait_and_begin_releases_it() {
    let mut rt = ScriptRuntime::for_test_at(
        CALLBACK_SETUP,
        "main() { level.callbackPlayerConnect = ::c; }\n\
         CodeCallback_PlayerConnect() { [[level.callbackPlayerConnect]](); }\n\
         c() { self.probe_out = \"connecting\"; self waittill(\"begin\"); \
               self.probe_out = \"begun\"; }\n",
    );
    rt.push_client_event(ClientEvent::Connect {
        slot: 0,
        name: "vcod".into(),
    });
    rt.run_frame(50);
    let ent = rt.client_entity(0).expect("connect allocated no entity");
    assert_eq!(rt.field_str(ent, "probe_out"), "connecting");

    rt.push_client_event(ClientEvent::Begin(0));
    rt.run_frame(100);
    assert_eq!(rt.field_str(ent, "probe_out"), "begun");
    assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
}

/// The `begin` waiter runs in the frame after the packet that entered
/// the client, on that frame's clock, as on all three retail servers
/// (`client-probes/probe_dropclient`: `PROBE begin` reads the first
/// snapshot's `serverTime`).
#[test]
fn the_begin_waiter_runs_on_the_next_frames_clock() {
    let mut rt = ScriptRuntime::for_test_at(
        CALLBACK_SETUP,
        "main() { level.callbackPlayerConnect = ::c; }\n\
         CodeCallback_PlayerConnect() { [[level.callbackPlayerConnect]](); }\n\
         c() { self waittill(\"begin\"); self.probe_out = \"\" + getTime(); }\n",
    );
    rt.push_client_event(ClientEvent::Connect {
        slot: 0,
        name: "vcod".into(),
    });
    rt.run_frame(50);
    let ent = rt.client_entity(0).expect("connect allocated no entity");
    rt.push_client_event(ClientEvent::Begin(0));
    rt.run_frame(100);
    assert_eq!(rt.field_str(ent, "probe_out"), "100");
    assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
}

/// Both events in one frame, which is what a client whose first usercmd
/// lands before the next `run_frame` produces. It holds because `start_thread`
/// steps a new thread to its first suspend before returning, so the
/// `waittill` is armed by the time the Begin behind it in the queue is
/// drained; nothing else pins that.
#[test]
fn a_connect_and_a_begin_in_one_frame_still_release_the_wait() {
    let mut rt = ScriptRuntime::for_test_at(
        CALLBACK_SETUP,
        "main() { level.callbackPlayerConnect = ::c; }\n\
         CodeCallback_PlayerConnect() { [[level.callbackPlayerConnect]](); }\n\
         c() { self.probe_out = \"connecting\"; self waittill(\"begin\"); \
               self.probe_out = \"begun\"; }\n",
    );
    rt.push_client_event(ClientEvent::Connect {
        slot: 0,
        name: "vcod".into(),
    });
    rt.push_client_event(ClientEvent::Begin(0));
    rt.run_frame(50);
    let ent = rt.client_entity(0).expect("connect allocated no entity");
    assert_eq!(rt.field_str(ent, "probe_out"), "begun");
    assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
}

/// The two things a client entity carries before any script touches it:
/// `.name` from the userinfo, and a `.pers` the gametype can index. Both
/// are measured on the retail 1.1d server: a probe gametype logged
/// `pers` defined and `self.name` correct inside
/// `Callback_PlayerConnect`, before its `waittill("begin")`. Without
/// either, `dm.gsc`'s connect callback dies before it opens a menu.
#[test]
fn a_fresh_client_entity_carries_its_name_and_an_indexable_pers() {
    let mut rt = ScriptRuntime::for_test_at(
        CALLBACK_SETUP,
        "main() { level.callbackPlayerConnect = ::c; }\n\
         CodeCallback_PlayerConnect() { [[level.callbackPlayerConnect]](); }\n\
         c() { if(!isdefined(self.pers[\"team\"])) self.pers[\"team\"] = \"spectator\"; \
               self.probe_out = self.name + \":\" + self.pers[\"team\"]; }\n",
    );
    rt.push_client_event(ClientEvent::Connect {
        slot: 0,
        name: "player".into(),
    });
    rt.run_frame(50);
    let ent = rt.client_entity(0).expect("connect allocated no entity");
    assert_eq!(rt.field_str(ent, "probe_out"), "player:spectator");
    assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
}

/// The `menuresponse` notify carries the menu's *name*, read back out of
/// its configstring slot, not the index the client sent. Every
/// gametype's join loop compares that first argument against
/// `game["menu_team"]` and hands it back to `openMenu`, so an index
/// would match nothing and the loop would spin forever.
#[test]
fn a_menu_response_notifies_the_menu_name_and_the_response() {
    let mut rt = ScriptRuntime::for_test_at(
        CALLBACK_SETUP,
        "main() { level.callbackPlayerConnect = ::c; }\n\
         CodeCallback_PlayerConnect() { [[level.callbackPlayerConnect]](); }\n\
         c() { self waittill(\"menuresponse\", menu, response); \
               self.probe_out = menu + \":\" + response; }\n",
    );
    let (lo, _) = crate::configstrings::CsRange::Menu.bounds();
    rt.host.configstrings.set(lo + 1, "weapon_russian");
    rt.push_client_event(ClientEvent::Connect {
        slot: 0,
        name: "vcod".into(),
    });
    rt.run_frame(50);
    let ent = rt.client_entity(0).expect("connect allocated no entity");

    rt.menu_response(0, &MenuRef::Index(1), "mosin_nagant_mp");
    rt.run_frame(100);
    assert_eq!(
        rt.field_str(ent, "probe_out"),
        "weapon_russian:mosin_nagant_mp"
    );
    assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
}

/// `logPrint` only reads its line's text (`Host::reads_text_only`), so
/// a level logging new lines every frame keeps its strings flat: the
/// lines are freed between frames (cod2-gsc-diff.md 3.7).
#[test]
fn logged_lines_are_freed_between_frames() {
    let mut rt = ScriptRuntime::for_test(
        "main() { for (n = 0; ; n++) { for (i = 0; i < 100; i++) \
         logPrint(\"K;\" + n + \";\" + i + \";a line of the log that never rotates\\n\"); \
         wait 0.05; } }",
    );
    for frame in 1..=600 {
        rt.run_frame(frame * 50);
    }
    assert_eq!(rt.host.game_log.len(), 60_000);
    let bytes = rt.vm.interner().bytes();
    assert!(bytes < 3 << 19, "{bytes} bytes of strings for 60,000 lines");
}

/// A client spraying distinct `mr` answers fills its own budget and no
/// more: the interner stays bounded, and an answer the scripts already
/// hold still goes through.
#[test]
fn distinct_menu_responses_stop_at_the_client_s_string_budget() {
    let mut rt = ScriptRuntime::for_test_at(
        CALLBACK_SETUP,
        "main() { level.callbackPlayerConnect = ::c; }\n\
         CodeCallback_PlayerConnect() { [[level.callbackPlayerConnect]](); }\n\
         c() { for (;;) { self waittill(\"menuresponse\", menu, response); \
               if (response == \"autoassign\") self.probe_out = menu; } }\n",
    );
    rt.push_client_event(ClientEvent::Connect {
        slot: 0,
        name: "vcod".into(),
    });
    rt.run_frame(50);
    let ent = rt.client_entity(0).expect("connect allocated no entity");
    let before = rt.vm.string_lengths().len();
    for i in 0..100_000 {
        rt.menu_response(0, &MenuRef::Name("team".into()), &format!("{i:0>1000}"));
    }
    let added = rt.vm.string_lengths().len() - before;
    assert!(added <= CLIENT_STRING_BUDGET / 1000 + 1, "{added} strings");
    rt.run_frame(75);
    rt.menu_response(0, &MenuRef::Name("team".into()), "autoassign");
    rt.run_frame(100);
    assert_eq!(rt.field_str(ent, "probe_out"), "team");
}

/// Disconnect runs its callback on the entity and only then frees the
/// slot, so the callback still has a `self` to read and the next frame
/// finds the slot empty.
#[test]
fn disconnect_runs_the_callback_before_it_frees_the_slot() {
    let mut rt = ScriptRuntime::for_test_at(
        CALLBACK_SETUP,
        "main() { level.callbackPlayerDisconnect = ::d; }\n\
         CodeCallback_PlayerConnect() {}\n\
         CodeCallback_PlayerDisconnect() { [[level.callbackPlayerDisconnect]](); }\n\
         d() { level.gone = self getEntityNumber(); }\n",
    );
    rt.push_client_event(ClientEvent::Connect {
        slot: 3,
        name: "vcod".into(),
    });
    rt.run_frame(50);
    assert!(rt.client_entity(3).is_some());

    rt.push_client_event(ClientEvent::Disconnect(3));
    rt.run_frame(100);
    assert_eq!(rt.level_field("gone"), vcod_gsc::Value::Int(3));
    assert!(rt.client_entity(3).is_none());
    assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
}

/// `kill` from a spectator who has played does nothing: no death, no
/// sim op, no killed callback (BUG-066: only `max_health` and `dead`
/// were tested).
#[test]
fn kill_from_a_spectator_who_played_is_ignored() {
    let mut rt = ScriptRuntime::for_test_at(
        CALLBACK_SETUP,
        "main() {}\n\
         CodeCallback_PlayerConnect() {}\n\
         CodeCallback_PlayerKilled(a, b, c, d, e, f, g) { level.killed = 1; }\n",
    );
    rt.spawn_client_for_test(0, [0.0; 3]);
    rt.host.clients[0].vitals.max_health = 100;
    rt.host.clients[0].vitals.health = 100;
    rt.set_client_state_for_test(0, "spectator");
    assert!(!rt.kill_client(0));
    rt.run_frame(50);
    assert!(!rt.host.clients[0].vitals.dead);
    assert!(rt.host.client_sim_ops.is_empty());
    assert_eq!(rt.level_field("killed"), Value::Undefined);
    rt.set_client_state_for_test(0, "playing");
    assert!(rt.kill_client(0));
    rt.run_frame(100);
    assert_eq!(rt.level_field("killed"), Value::Int(1));
}

/// A disconnect drops the leaver's pending seat ops, not the other
/// riders' (BUG-043: it dropped every one).
#[test]
fn a_disconnect_keeps_the_other_riders_seat_ops() {
    use crate::game::vehicle::seat::SeatOp;
    let mut rt = ScriptRuntime::for_test_at(
        CALLBACK_SETUP,
        "main() {}\n\
         CodeCallback_PlayerConnect() {}\n\
         CodeCallback_PlayerDisconnect() {}\n",
    );
    rt.push_client_event(ClientEvent::Connect {
        slot: 3,
        name: "vcod".into(),
    });
    rt.run_frame(50);
    let exit = |slot| SeatOp::Exit {
        slot,
        origin: None,
        velocity: None,
    };
    rt.host.vehicles.seat_ops = vec![exit(1), exit(3), exit(2)];
    rt.client_disconnect(3);
    let left: Vec<usize> = rt.host.vehicles.seat_ops.iter().map(SeatOp::slot).collect();
    assert_eq!(left, [1, 2]);
}

/// A gunner who leaves the server with the MG hot leaves no overheat
/// mark for the next client in the slot (BUG-142).
#[test]
fn a_disconnect_drops_the_overheated_gunner() {
    let mut rt = ScriptRuntime::for_test_at(
        CALLBACK_SETUP,
        "main() {}\n\
         CodeCallback_PlayerConnect() {}\n\
         CodeCallback_PlayerDisconnect() {}\n",
    );
    rt.push_client_event(ClientEvent::Connect {
        slot: 3,
        name: "vcod".into(),
    });
    rt.run_frame(50);
    rt.host.vehicles.overheated_gunners = vec![1, 3];
    rt.client_disconnect(3);
    assert_eq!(rt.host.vehicles.overheated_gunners, [1]);
}

/// A thread still polling the player when it disconnects dies with the
/// entity instead of erroring on a freed `self`, which is what the
/// stock `waitRespawnButton` did to a dead player who left.
#[test]
fn a_disconnect_kills_the_threads_running_on_the_player() {
    let mut rt = ScriptRuntime::for_test_at(
        CALLBACK_SETUP,
        "main() {}\n\
         CodeCallback_PlayerConnect() { self thread poll(); }\n\
         CodeCallback_PlayerDisconnect() {}\n\
         poll() { for(;;) { level.polls = self useButtonPressed(); wait .05; } }\n",
    );
    rt.push_client_event(ClientEvent::Connect {
        slot: 3,
        name: "vcod".into(),
    });
    rt.run_frame(50);
    rt.run_frame(100);
    assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());

    rt.push_client_event(ClientEvent::Disconnect(3));
    for f in 3..8 {
        rt.run_frame(f * 50);
    }
    assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
}

/// The missile `fire_grenade` just spawned.
fn live_missile(rt: &ScriptRuntime) -> EntId {
    rt.host
        .ents
        .iter_inuse()
        .map(|(id, _)| id)
        .find(|&id| rt.host.missiles.contains(id))
        .expect("a missile in the pool")
}

/// A script `delete` on a grenade in flight arms `G_FreeEntity` 100 ms
/// out, which `G_RunMissile`'s closing `G_RunThink` runs in place of
/// the fuse: it flies on until then and goes without exploding
/// (combat doc 13.8).
#[test]
fn a_deleted_grenade_flies_on_and_goes_unexploded() {
    let mut rt = ScriptRuntime::for_test("main() {}\ndel() { self delete(); }\n");
    let v = glam::Vec3::new(100.0, 0.0, 0.0);
    rt.fire_grenade(0, 1, 0, glam::Vec3::Z * 100.0, v, glam::Vec3::ZERO, 3000, 0);
    let g = live_missile(&rt);
    rt.run_frame(50);
    rt.start_thread_for_test(g, "del");
    assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
    rt.run_frame(100);
    assert!(rt.host.missiles.contains(g), "it flies on until the free");
    rt.run_frame(150);
    assert!(!rt.host.missiles.contains(g), "the free took it out");
    assert!(rt.host.ents.get(g).is_none());
}

/// 1.1's drop ring holds numbers: a grenade that took a held drop's
/// number is what a full ring's next drop evicts, and the eviction's
/// free a frame on replaces its fuse (items doc 8, combat doc 13.8).
#[test]
fn on_1_1_a_grenade_on_an_evicted_ring_number_goes_unexploded() {
    let mut rt = ScriptRuntime::for_test_at(
        CALLBACK_SETUP,
        "main() {}\n\
         CodeCallback_PlayerConnect() {}\n",
    );
    rt.push_client_event(ClientEvent::Connect {
        slot: 0,
        name: "vcod".into(),
    });
    rt.run_frame(50);
    let health = 68;
    rt.host.precache_items_for_test(&["item_health"]);
    let host = &mut rt.host;
    let drops: Vec<EntId> = rt.vm.with_cx(|cx| {
        (0..crate::game::item::DROP_RING)
            .map(|_| crate::game::item::drop_item(host, cx, 0, health).unwrap())
            .collect()
    });
    rt.host.free_entity(drops[0]);
    let v = glam::Vec3::new(100.0, 0.0, 0.0);
    rt.fire_grenade(
        0,
        1,
        0,
        glam::Vec3::Z * 100.0,
        v,
        glam::Vec3::ZERO,
        3000,
        50,
    );
    let g = live_missile(&rt);
    assert_eq!(
        g.0, drops[0].0,
        "the grenade took the picked-up drop's number"
    );
    let host = &mut rt.host;
    rt.vm
        .with_cx(|cx| crate::game::item::drop_item(host, cx, 0, health))
        .unwrap();
    rt.run_frame(100);
    assert!(!rt.host.missiles.contains(g), "the eviction freed it");
    assert!(rt.host.ents.get(g).is_none());
}

/// `G_FreeEntityRefs` sweeps a barrage's `r.ownerNum` with the rest: a
/// strike whose caller left hangs shells nobody owns, and so charges
/// no one (UO `fire_artillery` 0x6002d, combat doc 13.7).
#[test]
fn a_barrage_whose_caller_left_hangs_shells_nobody_owns() {
    let mut rt = ScriptRuntime::for_test_at(
        CALLBACK_SETUP,
        "main() {}\n\
         CodeCallback_PlayerConnect() {}\n\
         CodeCallback_PlayerDisconnect() {}\n",
    );
    rt.push_client_event(ClientEvent::Connect {
        slot: 3,
        name: "vcod".into(),
    });
    rt.run_frame(50);
    let def = vcod_common::weapon::WeaponDef {
        projectile_count: 4,
        projectile_radius: 100,
        projectile_delay: 100,
        projectile_spacing: (100, 100),
        projectile_speed: 10_000.0,
        damage: 2000,
        proj_explosion_type: "artillery".into(),
        ..Default::default()
    };
    let host = &mut rt.host;
    let id = rt.vm.with_cx(|cx| host.ents.spawn(cx)).unwrap();
    let strike = crate::game::artillery::Barrage::new(id, 3, 6, glam::Vec3::ZERO, &def, 50);
    rt.host.barrages.push(strike);
    rt.push_client_event(ClientEvent::Disconnect(3));
    for f in 2..12 {
        rt.run_frame(f * 50);
    }
    let shells: Vec<Option<usize>> = rt
        .host
        .ents
        .iter_inuse()
        .filter_map(|(id, _)| rt.host.missiles.owner_of(id))
        .collect();
    assert!(!shells.is_empty(), "no shell was hung");
    assert!(shells.iter().all(Option::is_none), "{shells:?}");
}

/// UO's `G_MissileImpact` with no owner hands `G_Damage` NULL for both
/// inflictor and attacker, and `Scr_PlayerDamage` pushes each as
/// undefined (combat doc 13.7).
#[test]
fn a_hit_with_no_inflictor_and_no_attacker_reaches_the_callback_undefined() {
    let mut rt = ScriptRuntime::for_test_at(
        CALLBACK_SETUP,
        "main() {}\n\
         CodeCallback_PlayerConnect() {}\n\
         CodeCallback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {\n\
             self.seen = isDefined(eInflictor) + \" \" + isDefined(eAttacker) + \" \" + sWeapon;\n\
         }\n",
    );
    rt.push_client_event(ClientEvent::Connect {
        slot: 0,
        name: "vcod".into(),
    });
    rt.run_frame(0);
    rt.host.clients[0].vitals.takedamage = true;
    rt.deliver_hits(vec![crate::game::combat::Hit {
        victim: 0,
        attacker: crate::game::host::ENTITYNUM_NONE as usize,
        inflictor: None,
        damage: 300,
        dflags: 0,
        mod_: "MOD_ARTILLERY",
        weapon: "none".into(),
        point: [0.0; 3],
        dir: [0.0, 0.0, -1.0],
        hitloc: "none",
    }]);
    rt.run_frame(50);
    assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
    assert_eq!(rt.client_field(0, "seen").as_deref(), Some("0 0 none"));
}

/// A UO vehicle shell's fuse blast names the vehicle as `eAttacker`
/// (`G_ExplodeMissile`'s `parent`, game.mp.uo 0x5e2fc, combat doc 13.7),
/// and `charged_owner` keeps its number.
#[test]
fn a_blast_charged_to_a_vehicle_names_it() {
    let mut rt = ScriptRuntime::for_test_at(
        CALLBACK_SETUP,
        "main() {}\n\
         CodeCallback_PlayerConnect() {}\n\
         CodeCallback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {\n\
             self.seen = eAttacker getEntityNumber() + \" \" + isPlayer(eAttacker);\n\
         }\n",
    );
    rt.push_client_event(ClientEvent::Connect {
        slot: 0,
        name: "vcod".into(),
    });
    rt.run_frame(0);
    rt.host.clients[0].vitals.takedamage = true;
    let tank = rt.vm.with_cx(|cx| rt.host.ents.spawn(cx)).unwrap();
    rt.host
        .vehicles
        .live
        .insert(tank, crate::game::vehicle::Vehicle::for_test(0));
    let attacker = rt.charged_owner(Some(tank.0 as usize)).unwrap();
    assert_eq!(attacker, tank.0 as usize);
    rt.deliver_hits(vec![crate::game::combat::Hit {
        victim: 0,
        attacker,
        inflictor: None,
        damage: 30,
        dflags: crate::game::combat::DFLAG_RADIUS,
        mod_: "MOD_PROJECTILE_SPLASH",
        weapon: "none".into(),
        point: [0.0; 3],
        dir: [0.0, 0.0, -1.0],
        hitloc: "none",
    }]);
    // The record has no info for the frame's vehicle think.
    rt.host.vehicles.live.remove(&tank);
    rt.run_frame(50);
    assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
    assert_eq!(rt.client_field(0, "seen"), Some(format!("{} 0", tank.0)));
}

/// A UO vehicle shell charges its driver's slot (`+0x330`) after he
/// left, and `Scr_AddEntity` reads the freed entity's number, 0 after
/// `G_FreeEntity`'s memset: the client in slot 0 is `eAttacker`
/// (game.mp.uo 0x5dcff, combat doc 13.7).
#[test]
fn a_hit_charged_to_an_empty_slot_names_entity_0() {
    let mut rt = ScriptRuntime::for_test_at(
        CALLBACK_SETUP,
        "main() {}\n\
         CodeCallback_PlayerConnect() {}\n\
         CodeCallback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {\n\
             self.seen = eAttacker getEntityNumber();\n\
         }\n",
    );
    for slot in [0, 1] {
        rt.push_client_event(ClientEvent::Connect {
            slot,
            name: "vcod".into(),
        });
    }
    rt.run_frame(0);
    rt.host.clients[1].vitals.takedamage = true;
    assert_eq!(rt.charged_owner(Some(0)), Some(0));
    let attacker = rt.charged_owner(Some(5)).unwrap();
    assert_eq!(attacker, crate::game::host::ZEROED_ENTITY);
    rt.deliver_hits(vec![crate::game::combat::Hit {
        victim: 1,
        attacker,
        inflictor: None,
        damage: 30,
        dflags: 0,
        mod_: "MOD_PROJECTILE",
        weapon: "none".into(),
        point: [0.0; 3],
        dir: [0.0, 0.0, -1.0],
        hitloc: "none",
    }]);
    rt.run_frame(50);
    assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
    assert_eq!(rt.client_field(1, "seen").as_deref(), Some("0"));
}

/// The object table has to survive past map load, so the runtime owns the
/// host. A thread that spawns an entity in one frame must find it in the
/// next.
#[test]
fn an_entity_spawned_in_one_frame_survives_into_the_next() {
    let mut rt = ScriptRuntime::for_test(
        "main() { level.e = spawn(\"script_origin\", (0,0,0)); wait 0.05; \
         level.n = level.e getEntityNumber(); }",
    );
    rt.run_frame(0);
    rt.run_frame(100);
    let n = rt.level_field("n");
    assert_eq!(n, vcod_gsc::Value::Int(72));
}

/// A client standing in a trigger wakes the thread parked on it, with the
/// toucher as the notify's argument. Retail raises this inside
/// `G_TouchTriggers` (0x3f88c) with `Scr_AddEntity` supplying `other`.
#[test]
fn touching_a_trigger_notifies_the_parked_thread() {
    let mut rt = ScriptRuntime::for_test(
        "main() { level.hits = 0; level thread watch(); }\n\
         watch() { for(;;) { level waittill(\"go\"); } }\n",
    );
    // The trigger's own thread, started on the entity the test registers.
    let src = "trigger_think() { for(;;) { self waittill(\"trigger\", other); \
               level.hits = level.hits + 1; level.who = other; } }";
    rt.install_for_test(src);

    let zone = rt.spawn_map_entity_for_test([0.0, 0.0, 0.0]);
    rt.triggers_mut().register(
        zone,
        crate::game::trigger::TriggerKind::Multiple,
        crate::game::trigger::TriggerShape::boxed([-64.0, -64.0, 0.0], [64.0, 64.0, 64.0]),
        crate::game::trigger::MULTIPLE_DEFAULT_WAIT_MS,
    );
    rt.link_trigger_for_test(zone);
    rt.start_thread_for_test(zone, "trigger_think");
    rt.run_frame(0);

    let player = rt.spawn_client_for_test(0, [10.0, 0.0, 0.0]);
    assert_eq!(rt.client_entity(0), Some(player));
    rt.set_client_state_for_test(0, "playing");

    rt.touch_triggers(0, 50);
    rt.run_frame(50);
    assert_eq!(rt.level_field("hits"), Value::Int(1), "one notify");
    assert_eq!(rt.level_field("who"), Value::Entity(player));

    // Out of the box: no second notify.
    rt.set_client_origin(0, [500.0, 0.0, 0.0]);
    rt.touch_triggers(0, 100);
    rt.run_frame(100);
    assert_eq!(rt.level_field("hits"), Value::Int(1));
}

/// The retrieval carrier is the client the compass record follows. A
/// death drop leaves the dead carrier's `hasobj` naming the objective
/// (bot-objectives.md 4.1), so a stale `hasobj` on a lower slot must not
/// win over the record.
#[test]
fn the_retrieval_carrier_is_who_the_compass_follows() {
    let mut rt = ScriptRuntime::for_test(
        "main() {
            game[\"re_attackers\"] = \"allies\";
            game[\"re_defenders\"] = \"axis\";
            obj = spawn(\"script_model\", (0, 0, 0));
            obj.trigger = spawn(\"script_origin\", (0, 0, 0));
            obj.goal = spawn(\"script_origin\", (500, 0, 0));
            obj.objnum = 1;
            objective_add(1, \"current\", (100, 0, 0));
            level.retrieval_objective = [];
            level.retrieval_objective[0] = obj;
        }
        stale() {
            self.hasobj = [];
            self.hasobj[1] = level.retrieval_objective[0];
            level.stale = 1;
        }
        carry() { objective_onEntity(1, self); }",
    );
    rt.run_frame(0);
    let o = rt.re_objectives(4);
    assert_eq!(o.objectives[0].carrier, None);
    assert_eq!(o.objectives[0].laid, Some([100.0, 0.0, 0.0]));
    let dead = rt.spawn_client_for_test(0, [0.0; 3]);
    let live = rt.spawn_client_for_test(1, [0.0; 3]);
    rt.start_thread_for_test(dead, "stale");
    rt.start_thread_for_test(live, "carry");
    rt.run_frame(50);
    assert_eq!(rt.level_field("stale"), Value::Int(1));
    let o = rt.re_objectives(4);
    assert_eq!(o.objectives[0].carrier, Some(1));
    assert_eq!(o.objectives[0].shown_to, 0);
    assert_eq!(o.objectives[0].laid, Some([100.0, 0.0, 0.0]));
}

/// A touch lands between frames, and the thread it wakes runs on the
/// next frame's clock: retail's plant bar carries the `scaleStartTime` of
/// the first snapshot it is on (object-model doc 23.1).
#[test]
fn a_triggered_thread_runs_on_the_next_frame_s_clock() {
    let mut rt = ScriptRuntime::for_test("main() {}");
    rt.install_for_test(
        "trigger_think() { self waittill(\"trigger\", other); level.at = getTime(); }",
    );
    let zone = rt.spawn_map_entity_for_test([0.0, 0.0, 0.0]);
    rt.triggers_mut().register(
        zone,
        crate::game::trigger::TriggerKind::Multiple,
        crate::game::trigger::TriggerShape::boxed([-64.0, -64.0, 0.0], [64.0, 64.0, 64.0]),
        crate::game::trigger::MULTIPLE_DEFAULT_WAIT_MS,
    );
    rt.link_trigger_for_test(zone);
    rt.start_thread_for_test(zone, "trigger_think");
    rt.run_frame(0);
    rt.spawn_client_for_test(0, [10.0, 0.0, 0.0]);
    rt.set_client_state_for_test(0, "playing");
    rt.touch_triggers(0, 0);
    rt.run_frame(50);
    assert_eq!(rt.level_field("at"), Value::Int(50));
}

/// The aim trace starts at the eye truncated toward zero, the way
/// `CalcMuzzlePoints` snaps the muzzle point (object-model doc 23.1): an
/// eye at z 60.9 aims level from z 60, under a box whose top is 60.5.
#[test]
fn the_aim_trace_starts_at_the_truncated_eye() {
    let mut rt = ScriptRuntime::for_test("main() { level.hits = 0; }");
    rt.install_for_test(
        "trigger_think() { for(;;) { self waittill(\"trigger\", other); \
         level.hits = level.hits + 1; } }",
    );
    let zone = rt.spawn_map_entity_for_test([100.0, 0.0, 44.5]);
    rt.triggers_mut().register(
        zone,
        crate::game::trigger::TriggerKind::LookAt,
        crate::game::trigger::TriggerShape::boxed([-8.0, -8.0, 0.0], [8.0, 8.0, 16.0]),
        0,
    );
    rt.link_trigger_for_test(zone);
    rt.start_thread_for_test(zone, "trigger_think");
    rt.run_frame(0);
    rt.spawn_client_for_test(0, [0.9, 0.0, 0.0]);
    rt.set_client_state_for_test(0, "playing");
    rt.set_client_pm_type(0, 0);
    rt.set_client_aim(0, [0.9, 0.0, 60.9], [0.0, 0.0], false);
    rt.aim_lookat(0, 50);
    rt.run_frame(100);
    assert_eq!(rt.level_field("hits"), Value::Int(1));
}

/// The aim trace fires a `trigger_lookat` the client's view enters, with
/// the aimer as `other`, and `isLookingAt` answers off the same trace
/// (docs/research/cod11-gsc-object-model.md 23.1). Aimed away, neither.
#[test]
fn aiming_at_a_lookat_trigger_notifies_it_and_backs_islookingat() {
    let mut rt = ScriptRuntime::for_test("main() { level.hits = 0; }");
    rt.install_for_test(
        "trigger_think() { for(;;) { self waittill(\"trigger\", other); \
         level.hits = level.hits + 1; level.who = other; } }\n\
         check() { level.looking = self islookingat(level.zone); }",
    );
    let zone = rt.spawn_map_entity_for_test([300.0, 0.0, 0.0]);
    rt.triggers_mut().register(
        zone,
        crate::game::trigger::TriggerKind::LookAt,
        crate::game::trigger::TriggerShape::boxed([-20.0, -20.0, 40.0], [20.0, 20.0, 80.0]),
        0,
    );
    rt.link_trigger_for_test(zone);
    rt.set_level_field_for_test("zone", Value::Entity(zone));
    rt.start_thread_for_test(zone, "trigger_think");
    rt.run_frame(0);

    let player = rt.spawn_client_for_test(0, [0.0, 0.0, 0.0]);
    rt.set_client_state_for_test(0, "playing");
    rt.set_client_pm_type(0, 0);

    rt.set_client_aim(0, [0.0, 0.0, 60.0], [0.0, 0.0], false);
    rt.aim_lookat(0, 50);
    rt.run_frame(50);
    assert_eq!(rt.level_field("hits"), Value::Int(1), "one notify");
    assert_eq!(rt.level_field("who"), Value::Entity(player));
    rt.start_thread_for_test(player, "check");
    rt.run_frame(100);
    assert_eq!(rt.level_field("looking"), Value::Int(1));

    // Aimed away: no fire, and the answer drops.
    rt.set_client_aim(0, [0.0, 0.0, 60.0], [0.0, 90.0], false);
    rt.aim_lookat(0, 150);
    rt.run_frame(150);
    assert_eq!(rt.level_field("hits"), Value::Int(1));
    rt.start_thread_for_test(player, "check");
    rt.run_frame(200);
    assert_eq!(rt.level_field("looking"), Value::Int(0));

    // Every frame it is aimed at, since `G_Trigger` gates nothing.
    rt.set_client_aim(0, [0.0, 0.0, 60.0], [0.0, 0.0], false);
    rt.aim_lookat(0, 250);
    rt.run_frame(250);
    rt.aim_lookat(0, 300);
    rt.run_frame(300);
    assert_eq!(rt.level_field("hits"), Value::Int(3));
}

/// A live body between the eye and a `trigger_lookat` stops the aim
/// trace's second pass (mask 0x22802001 carries BODY), so neither the
/// fire nor `isLookingAt` happens; the aimer's own body is the pass
/// entity and never does (object-model doc 23.1). Moved aside, it fires.
#[test]
fn a_body_between_the_eye_and_a_lookat_blocks_the_fire_and_islookingat() {
    let mut rt = ScriptRuntime::for_test("main() { level.hits = 0; }");
    rt.install_for_test(
        "trigger_think() { for(;;) { self waittill(\"trigger\", other); \
         level.hits = level.hits + 1; } }\n\
         check() { level.looking = self islookingat(level.zone); }",
    );
    let zone = rt.spawn_map_entity_for_test([300.0, 0.0, 0.0]);
    rt.triggers_mut().register(
        zone,
        crate::game::trigger::TriggerKind::LookAt,
        crate::game::trigger::TriggerShape::boxed([-20.0, -20.0, 40.0], [20.0, 20.0, 80.0]),
        0,
    );
    rt.link_trigger_for_test(zone);
    rt.set_level_field_for_test("zone", Value::Entity(zone));
    rt.start_thread_for_test(zone, "trigger_think");
    rt.run_frame(0);
    let player = rt.spawn_client_for_test(0, [0.0, 0.0, 0.0]);
    rt.set_client_state_for_test(0, "playing");
    rt.set_client_pm_type(0, 0);
    let body = |slot: usize, x: f32, y: f32| crate::game::combat::HitBody {
        slot,
        origin: glam::Vec3::new(x, y, 0.0),
        mins: crate::game::trigger::PLAYER_MINS.into(),
        maxs: crate::game::trigger::PLAYER_MAXS.into(),
        pose: Default::default(),
    };
    rt.set_client_body(0, Some(body(0, 0.0, 0.0)));
    rt.set_client_body(1, Some(body(1, 150.0, 0.0)));
    rt.set_client_aim(0, [0.0, 0.0, 60.0], [0.0, 0.0], false);
    rt.aim_lookat(0, 50);
    rt.run_frame(50);
    assert_eq!(rt.level_field("hits"), Value::Int(0), "the body blocks");
    rt.start_thread_for_test(player, "check");
    rt.run_frame(100);
    assert_eq!(rt.level_field("looking"), Value::Int(0));

    rt.set_client_body(1, Some(body(1, 150.0, 100.0)));
    rt.aim_lookat(0, 150);
    rt.run_frame(150);
    assert_eq!(rt.level_field("hits"), Value::Int(1), "moved aside");
    rt.start_thread_for_test(player, "check");
    rt.run_frame(200);
    assert_eq!(rt.level_field("looking"), Value::Int(1));
}

/// A script `delete()` takes the trigger row with the entity, and the
/// entity number it hands back carries no box into its next tenant.
/// `sd.gsc` deletes both bombzones at the plant, so a row that outlived
/// its entity would keep firing at the old brush, and, once the number
/// is reused, fire on a spawn that was never a trigger.
#[test]
fn a_deleted_trigger_takes_its_row_and_leaves_the_reused_number_clean() {
    let mut rt = ScriptRuntime::for_test("main() { level.hits = 0; }");
    rt.install_for_test(
        "trigger_think() { for(;;) { self waittill(\"trigger\", other); \
         level.hits = level.hits + 1; } }\n\
         remove() { self delete(); }",
    );
    let zone = rt.spawn_map_entity_for_test([0.0, 0.0, 0.0]);
    rt.triggers_mut().register(
        zone,
        crate::game::trigger::TriggerKind::Multiple,
        crate::game::trigger::TriggerShape::boxed([-64.0, -64.0, 0.0], [64.0, 64.0, 64.0]),
        crate::game::trigger::MULTIPLE_DEFAULT_WAIT_MS,
    );
    rt.link_trigger_for_test(zone);
    rt.start_thread_for_test(zone, "trigger_think");
    rt.spawn_client_for_test(0, [10.0, 0.0, 0.0]);
    rt.set_client_state_for_test(0, "playing");
    rt.touch_triggers(0, 0);
    rt.run_frame(0);
    assert_eq!(rt.level_field("hits"), Value::Int(1), "the row fires first");

    // `delete()` defers the free by `DELETE_DEFER_MS`; 200 is past it.
    rt.start_thread_for_test(zone, "remove");
    rt.run_frame(0);
    rt.run_frame(200);

    assert!(rt.host.ents.get(zone).is_none(), "the entity is gone");
    assert!(rt.host.triggers.get(zone).is_none(), "the row is gone");

    // The free list hands the number straight back out, and its new
    // tenant is a plain entity: a point box at its own origin, not the
    // dead trigger's brush.
    let reused = rt.spawn_map_entity_for_test([300.0, 0.0, 0.0]);
    assert_eq!(reused.0, zone.0, "the number is reused");
    let host = &mut rt.host;
    let bounds = rt
        .vm
        .with_cx(|cx| crate::game::trigger::entity_abs_bounds(host, cx, reused));
    assert_eq!(bounds, ([300.0, 0.0, 0.0], [300.0, 0.0, 0.0]));

    // And standing back in the old box notifies nobody.
    rt.set_client_origin(0, [10.0, 0.0, 0.0]);
    rt.touch_triggers(0, 250);
    rt.run_frame(250);
    assert_eq!(rt.level_field("hits"), Value::Int(1), "no further notify");
}

/// `G_TouchTriggers` runs its pass only for `ps.pm_type <= 1`
/// (docs/research/cod11-gsc-object-model.md 8.2), so of the four values a
/// client can carry on our wire only a living player's 0 gets through:
/// spectator 4, intermission 5 and dead 6 all fail the compare. The dead
/// case is the minefield bug: a corpse lying in the trigger re-armed
/// `minefield_kill` on every pass.
#[test]
fn only_a_living_client_touches_a_trigger() {
    let mut rt = ScriptRuntime::for_test("main() { level.hits = 0; }");
    rt.install_for_test(
        "trigger_think() { for(;;) { self waittill(\"trigger\", other); \
         level.hits = level.hits + 1; } }",
    );
    let zone = rt.spawn_map_entity_for_test([0.0, 0.0, 0.0]);
    rt.triggers_mut().register(
        zone,
        crate::game::trigger::TriggerKind::Multiple,
        crate::game::trigger::TriggerShape::boxed([-64.0, -64.0, 0.0], [64.0, 64.0, 64.0]),
        crate::game::trigger::MULTIPLE_DEFAULT_WAIT_MS,
    );
    rt.link_trigger_for_test(zone);
    rt.start_thread_for_test(zone, "trigger_think");
    rt.spawn_client_for_test(0, [10.0, 0.0, 0.0]);
    rt.run_frame(0);

    // A spectator, at `pm_type` 4. The old gate excluded it off
    // `sessionstate`; the measured rule excludes it off the compare, so
    // the behaviour is unchanged.
    rt.set_client_state_for_test(0, "spectator");
    rt.set_client_pm_type(0, crate::spectate::PM_SPECTATOR);
    rt.touch_triggers(0, 50);
    rt.run_frame(50);
    assert_eq!(rt.level_field("hits"), Value::Int(0), "a spectator touched");

    rt.set_client_state_for_test(0, "intermission");
    rt.set_client_pm_type(0, crate::spectate::PM_INTERMISSION);
    rt.touch_triggers(0, 75);
    rt.run_frame(75);
    assert_eq!(
        rt.level_field("hits"),
        Value::Int(0),
        "an intermission camera touched"
    );

    rt.set_client_state_for_test(0, "dead");
    rt.set_client_pm_type(0, crate::spectate::PM_DEAD);
    rt.touch_triggers(0, 100);
    rt.run_frame(100);
    assert_eq!(rt.level_field("hits"), Value::Int(0), "a corpse touched");

    // And the living player the gate is there to let through.
    rt.set_client_state_for_test(0, "playing");
    rt.set_client_pm_type(0, 0);
    rt.touch_triggers(0, 125);
    rt.run_frame(125);
    assert_eq!(
        rt.level_field("hits"),
        Value::Int(1),
        "a live player did not"
    );
}

/// A `trigger_hurt` damages the player standing in it through the stock
/// damage callback, so the victim's health drops on the path a bullet's
/// hit already takes, and the 100 ms cadence `arm_hurt` arms gates
/// the second helping.
#[test]
fn a_trigger_hurt_damages_the_player_in_it() {
    let mut rt = ScriptRuntime::for_test_at(
        CALLBACK_SETUP,
        "main() {}\n\
         CodeCallback_PlayerDamage(inflictor, attacker, damage, flags, mod, weapon, point, \
         dir, hitloc) { level.damage = damage; level.mod = mod; \
         self.health = self.health - damage; }\n",
    );
    rt.spawn_client_for_test(0, [0.0, 0.0, 0.0]);
    rt.set_client_state_for_test(0, "playing");
    rt.set_client_health_for_test(0, 100);

    let hurt = rt.spawn_map_entity_for_test([0.0, 0.0, 0.0]);
    rt.register_hurt_for_test(
        hurt,
        crate::game::trigger::TriggerShape::boxed([-64.0, -64.0, 0.0], [64.0, 64.0, 64.0]),
        5,
    );

    rt.touch_triggers(0, 100);
    rt.run_frame(100);
    let expected_mod = rt
        .vm
        .with_cx(|cx| cx.intern_exact(crate::game::trigger::MOD_TRIGGER_HURT));
    assert_eq!(rt.level_field("damage"), Value::Int(5));
    assert_eq!(rt.level_field("mod"), Value::String(expected_mod));
    assert_eq!(rt.client_vitals(0).health, 95);

    rt.touch_triggers(0, 150);
    rt.run_frame(150);
    assert_eq!(rt.client_vitals(0).health, 95, "inside the 100 ms window");

    rt.touch_triggers(0, 200);
    rt.run_frame(200);
    assert_eq!(rt.client_vitals(0).health, 90);
}

/// A script `spawn("trigger_hurt", ...)` runs `SP_trigger_hurt`: it is a
/// hurt trigger and its `dmg` reads 5 (cod11-combat.md 5.9).
#[test]
fn a_script_spawned_trigger_hurt_reads_dmg_5() {
    let mut rt = ScriptRuntime::for_test(
        "main() { t = spawn(\"trigger_hurt\", (0, 0, 0), 16); level.t = t; \
         level.dmg = t.dmg; }",
    );
    rt.run_frame(50);
    assert_eq!(rt.level_field("dmg"), Value::Int(5));
    let Value::Entity(t) = rt.level_field("t") else {
        panic!("no trigger");
    };
    assert_eq!(
        rt.triggers_mut().get(t).map(|t| t.kind),
        Some(crate::game::trigger::TriggerKind::Hurt)
    );
}

/// A `trigger_use` answers the use key through the aim pick, the way
/// `Cmd_Activate_f` reaches it, and never through contact
/// (docs/research/cod11-gametypes-re-bel.md 4): standing in one with the
/// key down raises nothing, a press while aimed at it from outside fires
/// it, and the hint pass shows `HINT_ACTIVATE` with the string
/// `setHintString` stored.
#[test]
fn a_trigger_use_fires_off_the_aim_and_never_off_contact() {
    let mut rt = pickup_rig();
    rt.install_for_test(
        "trigger_think() { level.hits = 0; for(;;) { self waittill(\"trigger\", other); \
         level.hits = level.hits + 1; } } \
         hint() { self setHintString(&\"RE_PRESS_TO_PICKUP_GENERIC\"); } \
         unhint() { self setHintString(\"\"); }",
    );
    let pickup = rt.spawn_map_entity_for_test([40.0, 0.0, 30.0]);
    rt.triggers_mut().register(
        pickup,
        crate::game::trigger::TriggerKind::Use,
        crate::game::trigger::TriggerShape::boxed([-8.0, -8.0, -8.0], [8.0, 8.0, 8.0]),
        0,
    );
    rt.link_trigger_for_test(pickup);
    rt.start_thread_for_test(pickup, "trigger_think");
    rt.run_frame(0);
    let use_key = vcod_common::net::msg::BUTTON_USE;

    rt.set_client_origin(0, [40.0, 0.0, 0.0]);
    rt.item_pass(0, 0, EYE, [-36.0, 0.0, 0.0]);
    rt.touch_triggers(0, 50);
    rt.item_pass(0, use_key, EYE, [-36.0, 0.0, 0.0]);
    rt.run_frame(50);
    assert_eq!(rt.level_field("hits"), Value::Int(0), "inside, aimed away");

    rt.item_pass(0, 0, EYE, AT_ITEM);
    rt.item_pass(0, use_key, EYE, AT_ITEM);
    rt.run_frame(100);
    assert_eq!(rt.level_field("hits"), Value::Int(1), "aimed at it");

    assert_eq!(rt.cursor_hint_pass(0, EYE, AT_ITEM), (2, Some(-1)));
    rt.start_thread_for_test(pickup, "hint");
    let slot = rt
        .cursor_hint_pass(0, EYE, AT_ITEM)
        .1
        .expect("a hint string");
    assert_eq!(
        rt.host.configstrings[1212 + slot as usize],
        "RE_PRESS_TO_PICKUP_GENERIC\u{15}"
    );
    rt.start_thread_for_test(pickup, "unhint");
    assert_eq!(rt.cursor_hint_pass(0, EYE, AT_ITEM), (2, Some(-1)));
}

/// The link re-anchor reads the parent through its handle, so a parent
/// freed and replaced in its slot between two frames releases the link
/// rather than handing the planter to the new occupant.
#[test]
fn a_freed_parent_has_no_origin_once_its_slot_is_reused() {
    let mut rt = ScriptRuntime::for_test("main() {}");
    let parent = rt.spawn_map_entity_for_test([1.0, 2.0, 3.0]);
    assert_eq!(rt.entity_origin_of(parent), Some([1.0, 2.0, 3.0]));
    rt.host.free_entity(parent);
    let next = rt.spawn_map_entity_for_test([9.0, 9.0, 9.0]);
    assert_eq!(next.0, parent.0, "the slot was not reused");
    assert_eq!(rt.entity_origin_of(parent), None);
}

/// `sd.gsc`'s `bombzone_think` with two attackers: the first aborts and
/// destroys its bar, the second starts at the other zone and its bar
/// takes the freed record, the first retries. Its
/// `isdefined(other.progressbackground)` has to read 0, as a destroyed
/// hudelem does on retail (`# probe_stale_handle`), so it makes a bar of
/// its own and its `setShader` lands there rather than on the second's.
#[test]
fn a_retried_plant_gets_its_own_bar_after_another_took_the_freed_one() {
    let mut rt = ScriptRuntime::for_test("main() {}");
    // Past the first wait `setShader` only finds a precached shader.
    rt.host
        .allocators
        .index(
            &mut rt.host.configstrings,
            crate::configstrings::CsRange::Shader,
            "black",
        )
        .unwrap();
    rt.install_for_test(
        "plant() { \
           if(!isdefined(self.progressbackground)) \
             self.progressbackground = newClientHudElem(self); \
           self.progressbackground setShader(\"black\", 124, 12); } \
         abort() { self.progressbackground destroy(); }",
    );
    let a = rt.spawn_client_for_test(0, [0.0; 3]);
    let b = rt.spawn_client_for_test(1, [0.0; 3]);
    let bar = |rt: &mut ScriptRuntime, p: EntId| {
        use vcod_gsc::Host;
        let host = &mut rt.host;
        rt.vm.with_cx(|cx| {
            let f = cx.intern_folded("progressbackground");
            host.get_field(cx, p, f)
        })
    };
    rt.start_thread_for_test(a, "plant");
    let Value::Entity(first) = bar(&mut rt, a) else {
        panic!("no bar for the first attempt");
    };
    rt.start_thread_for_test(a, "abort");
    rt.start_thread_for_test(b, "plant");
    let Value::Entity(b_bar) = bar(&mut rt, b) else {
        panic!("no bar for the second attacker");
    };
    assert_eq!(b_bar.0, first.0, "the second bar did not reuse the record");
    rt.start_thread_for_test(a, "plant");
    assert_eq!(rt.aborts(), Vec::<String>::new());

    let Value::Entity(a_bar) = bar(&mut rt, a) else {
        panic!("the retry made no bar");
    };
    assert_ne!(a_bar, b_bar, "the retry reused the other attacker's bar");
    let owner = |rt: &ScriptRuntime, id| rt.host.ents.get(id).and_then(|e| e.hud).map(|h| h.owner);
    assert_eq!(owner(&rt, a_bar), Some(a.0));
    assert_eq!(owner(&rt, b_bar), Some(b.0));
}

/// A connect callback that returns at once, so a test client gets its
/// entity without the stock menus.
const PICKUP_CALLBACKS: &str = "main() {}\nCodeCallback_PlayerConnect() {}\n";

/// One client at the origin with 100/100 health, the allied loadout on
/// the host mirrors, and a table built from the stock numbers.
fn pickup_rig() -> ScriptRuntime {
    pickup_rig_with(PICKUP_CALLBACKS)
}

fn pickup_rig_with(callbacks: &str) -> ScriptRuntime {
    use crate::game::host::Vitals;
    let mut rt = ScriptRuntime::for_test_at(CALLBACK_SETUP, callbacks);
    rt.host.weapons = Rc::new(crate::game::pickup::tests_table());
    rt.push_client_event(ClientEvent::Connect {
        slot: 0,
        name: "p".into(),
    });
    rt.run_frame(0);
    rt.host.clients[0].vitals = Vitals {
        health: 100,
        max_health: 100,
        dead: false,
        takedamage: true,
    };
    for (name, s) in [("m1carbine_mp", 1), ("colt_mp", 3), ("fraggrenade_mp", 4)] {
        rt.host.precache_items_for_test(&[name]);
        let w = crate::defs::V1.weapon_index(name).unwrap();
        let d = rt.host.weapons.get(w).unwrap().clone();
        rt.host.clients[0].weapons.give(w, s);
        rt.host.clients[0].ammo.clip[d.clip_index] = d.clip_size as i16;
        if !d.clip_only {
            rt.host.clients[0].ammo.ammo[d.ammo_index] = d.max_ammo as i16;
        }
    }
    rt.host.clients[0].weapons.current =
        crate::defs::V1.weapon_index("m1carbine_mp").unwrap() as u8;
    rt.set_client_origin(0, [0.0; 3]);
    rt
}

const DOWN: [f32; 3] = [87.9, 0.0, 0.0];

/// One cmd's touch pass and use key, in `ClientThink_real`'s order.
fn cmd(rt: &mut ScriptRuntime, slot: usize, buttons: u8, eye: [f32; 3], view: [f32; 3]) {
    let now = rt.host.level_time_ms;
    rt.touch_triggers(slot, now);
    rt.item_pass(slot, buttons, eye, view);
}

#[test]
fn a_health_pack_underfoot_heals_and_leaves_the_wire() {
    let mut rt = pickup_rig();
    rt.host.clients[0].vitals.health = 50;
    let id = rt.place_item("item_health", [0.0, 0.0, 1.0], 0);
    cmd(&mut rt, 0, 0, [0.0, 0.0, 60.0], DOWN);
    assert_eq!(rt.host.clients[0].vitals.health, 75);
    assert!(rt.host.ents.get(id).unwrap().item.unwrap().taken);
    assert_eq!(
        rt.take_sim_ops(),
        vec![(
            0,
            crate::game::host::SimOp::Event {
                event: 146,
                parm: 68
            }
        )]
    );
    assert_eq!(
        rt.take_client_commands(),
        vec![(0, "f \"GAME_PICKUP_HEALTH\u{15}25\"".to_string())]
    );
    assert!(rt.script_log().iter().any(|l| l == "Item: 0 item_health"));
}

/// A grabbed drop is freed 100 ms on; a grabbed placed item stays
/// allocated, off the wire (section 7, removal).
#[test]
fn a_grabbed_drop_is_freed_and_a_grabbed_placed_item_stays_hidden() {
    let mut rt = pickup_rig();
    rt.host.clients[0].vitals.health = 10;
    let placed = rt.place_item("item_health", [0.0, 0.0, 1.0], 0);
    let drop = rt.place_item("item_health", [4.0, 0.0, 1.0], 0);
    rt.host
        .ents
        .get_mut(drop)
        .unwrap()
        .item
        .as_mut()
        .unwrap()
        .dropped = true;
    cmd(&mut rt, 0, 0, [0.0, 0.0, 60.0], DOWN);
    rt.run_frame(50);
    assert!(rt.host.ents.get(drop).is_some(), "freed before 100 ms");
    rt.run_frame(100);
    assert!(rt.host.ents.get(drop).is_none(), "the drop was not freed");
    rt.run_frame(1000);
    assert!(rt.host.ents.get(placed).unwrap().item.unwrap().taken);
    let p = &vcod_common::net::protocol::PROTOCOL_V1;
    assert!(!rt.packet_entities(p).contains_key(&placed.0));
}

#[test]
fn a_health_pack_at_full_health_stays() {
    let mut rt = pickup_rig();
    let id = rt.place_item("item_health", [0.0, 0.0, 1.0], 0);
    cmd(&mut rt, 0, 0, [0.0, 0.0, 60.0], DOWN);
    assert!(!rt.host.ents.get(id).unwrap().item.unwrap().taken);
    assert!(rt.take_sim_ops().is_empty());
}

#[test]
fn a_dead_or_spectating_player_takes_nothing() {
    use crate::game::host::Vitals;
    let mut rt = pickup_rig();
    rt.host.clients[0].vitals.health = 50;
    let id = rt.place_item("item_health", [0.0, 0.0, 1.0], 0);
    rt.set_client_pm_type(0, 4);
    cmd(
        &mut rt,
        0,
        vcod_common::net::msg::BUTTON_USE,
        [0.0, 0.0, 60.0],
        DOWN,
    );
    rt.set_client_pm_type(0, 0);
    rt.host.clients[0].vitals = Vitals {
        health: 0,
        max_health: 100,
        dead: true,
        takedamage: true,
    };
    cmd(&mut rt, 0, 0, [0.0, 0.0, 60.0], DOWN);
    assert!(!rt.host.ents.get(id).unwrap().item.unwrap().taken);
}

/// The use key acts on its rising edge only: a use held across ten cmds
/// in front of two health packs, both out of the touch box and each
/// still grabbable after the other, takes one.
#[test]
fn a_held_use_key_grabs_once() {
    let mut rt = pickup_rig();
    rt.host.clients[0].vitals.health = 10;
    let a = rt.place_item("item_health", [40.0, 0.0, 30.0], 0);
    let b = rt.place_item("item_health", [40.0, 4.0, 30.0], 0);
    for _ in 0..10 {
        cmd(
            &mut rt,
            0,
            vcod_common::net::msg::BUTTON_USE,
            [0.0, 0.0, 60.0],
            [36.0, 0.0, 0.0],
        );
    }
    let taken = [a, b]
        .iter()
        .filter(|id| rt.host.ents.get(**id).unwrap().item.unwrap().taken)
        .count();
    assert_eq!(taken, 1);
    assert_eq!(rt.host.clients[0].vitals.health, 35);
}

/// Two fg42s underfoot on one cmd, the player holding one short of full:
/// the first grab, in the area tree's order, fills the reserve and the
/// second, reading the mirror the first moved, finds nothing to take.
#[test]
fn two_items_in_reach_on_one_cmd_see_each_others_ammo() {
    let mut rt = pickup_rig();
    let fg = crate::defs::V1.weapon_index("fg42_mp").unwrap();
    let d = rt.host.weapons.get(fg).unwrap().clone();
    rt.host.clients[0].weapons.give(fg, 2);
    rt.host.clients[0].ammo.clip[d.clip_index] = 20;
    rt.host.clients[0].ammo.ammo[d.ammo_index] = 300;
    let a = rt.place_item("mpweapon_fg42", [0.0, 0.0, 1.0], 90);
    let b = rt.place_item("mpweapon_fg42", [4.0, 0.0, 1.0], 90);
    // The later link heads the list, so b is met first, not a.
    let walk = rt.host.area.entities_in_box([-50.0; 3], [50.0; 3], -1);
    assert_eq!(walk, [b.0, a.0]);
    cmd(&mut rt, 0, 0, [0.0, 0.0, 60.0], DOWN);
    assert!(rt.host.ents.get(b).unwrap().item.unwrap().taken);
    assert!(!rt.host.ents.get(a).unwrap().item.unwrap().taken);
    assert_eq!(rt.host.clients[0].ammo.ammo[d.ammo_index], 320);
}

/// The swap with the use key: the carbine lands on the panzerfaust's own
/// origin, carries its rounds and its dropper, and the `"trigger"` notify
/// names the player and the carbine, in that order.
/// A drop with an empty clip and a reserve still has rounds to take;
/// only a count and a clip both written empty (-1) read as empty.
#[test]
fn a_weapon_item_with_a_reserve_behind_an_empty_clip_is_not_empty() {
    let mut rt = pickup_rig();
    let placed = rt.place_item("mpweapon_m1carbine", [0.0, 0.0, 30.0], 0);
    let reserve = rt.place_item("mpweapon_m1carbine", [100.0, 0.0, 30.0], 30);
    let empty = rt.place_item("mpweapon_m1carbine", [200.0, 0.0, 30.0], -1);
    for id in [reserve, empty] {
        rt.host
            .ents
            .get_mut(id)
            .unwrap()
            .item
            .as_mut()
            .unwrap()
            .clip = -1;
    }
    let items = rt.weapon_items();
    let at = |x: f32| items.iter().find(|i| i.1[0] == x).unwrap().2;
    assert!(!at(0.0), "{placed:?} unset counts are filled at the pickup");
    assert!(!at(100.0), "a reserve behind an empty clip");
    assert!(at(200.0));
}

#[test]
fn the_use_key_swaps_and_the_trigger_notify_names_the_drop() {
    let mut rt = pickup_rig();
    let fg = crate::defs::V1.weapon_index("fg42_mp").unwrap();
    rt.host.clients[0].weapons.give(fg, 2);
    let pf = rt.place_item("mpweapon_panzerfaust", [40.0, 0.0, 30.0], 0);
    cmd(&mut rt, 0, 0, [0.0, 0.0, 60.0], [36.0, 0.0, 0.0]);
    cmd(
        &mut rt,
        0,
        vcod_common::net::msg::BUTTON_USE,
        [0.0, 0.0, 60.0],
        [36.0, 0.0, 0.0],
    );
    assert!(rt.host.ents.get(pf).unwrap().item.unwrap().taken);
    let carbine = crate::defs::V1.weapon_index("m1carbine_mp").unwrap();
    let (drop, state) = rt
        .host
        .ents
        .iter_inuse()
        .find_map(|(id, e)| {
            e.item
                .filter(|i| i.index as usize == carbine)
                .map(|i| (id, i))
        })
        .expect("the carbine was dropped");
    assert_eq!(state.owner, Some(0));
    assert_eq!(state.clip, 15);
    assert_eq!(rt.entity_origin_of(drop), Some([40.0, 0.0, 30.0]));
    let client = rt.client_entity(0).unwrap();
    assert!(
        rt.host
            .engine_notifies
            .iter()
            .any(|(id, ev, args)| *id == pf
                && *ev == "trigger"
                && args == &vec![Value::Entity(client), Value::Entity(drop)])
    );
    // `Cmd_Activate_f` notifies the item alone, never the player.
    assert!(
        !rt.host
            .engine_notifies
            .iter()
            .any(|(id, ev, _)| *id == client && *ev == "touch")
    );
    let panzerfaust = crate::defs::V1.weapon_index("panzerfaust_mp").unwrap();
    assert!(
        rt.take_client_commands()
            .contains(&(0, format!("a {panzerfaust}")))
    );
}

/// A stock stand turret at (40, 0, 0) facing +x, and `pickup_rig`'s
/// client plus a second one, both on the ground behind it.
fn turret_rig() -> (ScriptRuntime, EntId) {
    use vcod_gsc::Host;
    let mut rt = pickup_rig();
    rt.push_client_event(ClientEvent::Connect {
        slot: 1,
        name: "q".into(),
    });
    rt.run_frame(0);
    rt.host.clients[1].vitals = rt.host.clients[0].vitals;
    rt.set_client_origin(1, [0.0, 4.0, 0.0]);
    rt.set_client_on_ground(0, true);
    rt.set_client_on_ground(1, true);
    rt.host.configstrings.set(1212, "CGAME_USEMG42");
    let host = &mut rt.host;
    let gun = rt.vm.with_cx(|cx| {
        let id = host.ents.spawn(cx).unwrap();
        let a = cx.intern_folded("origin");
        host.set_field(cx, id, a, Value::Vector([40.0, 0.0, 0.0]))
            .unwrap();
        id
    });
    let def = crate::game::turret::TurretDef::parse(
        "WEAPONFILE\\weaponClass\\turret\\leftArc\\45\\rightArc\\45\\topArc\\40\\bottomArc\\40\\useHintString\\CGAME_USEMG42",
    )
    .unwrap();
    rt.host.turrets.insert(
        gun,
        crate::game::turret::TurretRecord::new("mg42_bipod_stand_mp", def, Default::default(), 0.0),
    );
    // `G_SpawnTurret`'s link: the use key's walk lists it off the tree.
    let host = &mut rt.host;
    rt.vm.with_cx(|cx| host.link_turret(cx, gun));
    (rt, gun)
}

const AT_GUN: [f32; 3] = [30.0, 0.0, 0.0];

/// How many mounts the drain after `slot`'s pass applied.
fn mount_queued(rt: &mut ScriptRuntime, slot: usize) -> usize {
    rt.take_turret_mounts(slot, [0.0; 3], vcod_common::pmove::Stance::Stand, AT_GUN)
        .len()
}

#[test]
fn a_usable_turret_hints_mg42_with_its_hint_string_slot() {
    let (mut rt, _) = turret_rig();
    assert_eq!(rt.cursor_hint_pass(0, EYE, AT_GUN), (6, Some(0)));
}

/// `G_FreeTurret`'s `TeleportPlayer` runs in the freeing thread (combat
/// doc 14.7): the gunner is linked a unit above his mount spot and the
/// script reads him there before his end frame applies the sim half.
#[test]
fn freeing_a_manned_gun_links_its_gunner_at_once() {
    use vcod_gsc::Host;
    let (mut rt, gun) = turret_rig();
    let rec = rt.host.turrets.get_mut(&gun).unwrap();
    rec.owner = Some(0);
    rec.busy = 1;
    rec.mount_origin = [10.0, 20.0, 0.0];
    let gunner_box = rt.host.client_box(0);
    rt.host
        .link_client(0, [30.0, 0.0, 0.0], gunner_box, 0x0200_0000, true);
    rt.host.free_entity(gun);
    let link = rt.host.area.last_link(0).expect("linked");
    assert_eq!(
        (link.absmin, link.contents),
        ([-6.0, 4.0, 0.0], 0x0200_0000),
        "a unit up, box and contents kept"
    );
    let me = rt.client_entity(0).unwrap();
    let host = &mut rt.host;
    let origin = rt.vm.with_cx(|cx| {
        let f = cx.intern_folded("origin");
        host.get_field(cx, me, f)
    });
    assert_eq!(origin, Value::Vector([10.0, 20.0, 1.0]));
    assert_eq!(rt.host.clients[0].link_origin, [10.0, 20.0, 1.0]);
}

/// A gun whose record names a slot whose playerstate is not mounted, as
/// a 1.1 reconnect leaves it (`ClientConnect`'s memset), gets no think
/// and no release: `ClientEndFrame` tests the mounted bits first
/// (0x41494), so the gun stays owned (cod11-server-handshake.md, "A
/// reconnect into a held slot").
#[test]
fn an_unmounted_owner_leaves_the_gun_owned() {
    let (mut rt, gun) = turret_rig();
    let rec = rt.host.turrets.get_mut(&gun).unwrap();
    rec.owner = Some(0);
    rec.busy = 1;
    let angles = vcod_common::net::msg::NULL_USERCMD.angles;
    let mut sim = crate::spectate::ClientSim::spectator([0.0, 0.0, 8.0], 0.0, angles);
    sim.become_player([0.0, 0.0, 8.0], 0.0, angles);
    assert!(rt.turret_think_client(0, &mut sim, true, None).is_none());
    assert_eq!(rt.host.turrets[&gun].owner, Some(0));
    assert_eq!(rt.host.turrets[&gun].busy, 1);
    assert_eq!(rt.host.turrets[&gun].angles2, [0.0; 3], "no aim");
    assert_eq!(sim.viewlocked, 0);
}

/// The server drains each cmd's mount before the next cmd's pass, so the
/// second client's press in the same frame finds the gun manned.
#[test]
fn two_presses_on_one_gun_mount_only_the_first() {
    let (mut rt, gun) = turret_rig();
    cmd(&mut rt, 0, vcod_common::net::msg::BUTTON_USE, EYE, AT_GUN);
    assert_eq!(mount_queued(&mut rt, 0), 1);
    cmd(&mut rt, 1, vcod_common::net::msg::BUTTON_USE, EYE, AT_GUN);
    assert_eq!(mount_queued(&mut rt, 1), 0);
    assert_eq!(rt.host.turrets[&gun].owner, Some(0));
}

/// A gunner's press is the release request and nothing else, and a
/// gunner reads hint 0 with the string left where the mount left it.
#[test]
fn a_gunners_use_press_asks_for_the_release_and_mounts_nothing() {
    let (mut rt, gun) = turret_rig();
    cmd(&mut rt, 0, vcod_common::net::msg::BUTTON_USE, EYE, AT_GUN);
    mount_queued(&mut rt, 0);
    assert_eq!(rt.cursor_hint_pass(0, EYE, AT_GUN), (0, None));
    let fg42 = rt.place_item("mpweapon_fg42", [40.0, 0.0, 40.0], 90);
    cmd(&mut rt, 0, 0, EYE, AT_GUN);
    cmd(&mut rt, 0, vcod_common::net::msg::BUTTON_USE, EYE, AT_GUN);
    assert!(rt.host.turret_ops.is_empty());
    assert_eq!(rt.host.turrets[&gun].busy, 2);
    assert!(!rt.host.ents.get(fg42).unwrap().item.unwrap().taken);
}

/// `Cmd_Activate_f` has no `pm_type` gate (turrets doc 4.1): a gunner
/// past the touch pass's gate still asks for the release.
#[test]
fn a_gunners_press_asks_for_the_release_whatever_its_pm_type() {
    let (mut rt, gun) = turret_rig();
    cmd(&mut rt, 0, vcod_common::net::msg::BUTTON_USE, EYE, AT_GUN);
    mount_queued(&mut rt, 0);
    cmd(&mut rt, 0, 0, EYE, AT_GUN);
    rt.set_client_pm_type(0, 6);
    cmd(&mut rt, 0, vcod_common::net::msg::BUTTON_USE, EYE, AT_GUN);
    assert_eq!(rt.host.turrets[&gun].busy, 2);
}

const EYE: [f32; 3] = [0.0, 0.0, 60.0];

const AT_ITEM: [f32; 3] = [36.0, 0.0, 0.0];

/// `spawnTriggerMount`'s box from two unsorted corners.
fn spawn_mount(rt: &mut ScriptRuntime, a: [f32; 3], b: [f32; 3], crouch: bool) -> EntId {
    let args = [
        Value::Vector(a),
        Value::Vector(b),
        Value::Int(i32::from(crouch)),
    ];
    let made = rt.with_host(|host, cx| {
        crate::game::builtins::uo::spawn_trigger_mount(host, cx, None, &args)
    });
    match made {
        Ok(Value::Entity(id)) => id,
        v => panic!("{v:?}"),
    }
}

/// `G_TouchTriggers` finds a mount through the area query's mask
/// 0x405c0008 alone, so `setContents` takes it out of the touch pass
/// or puts it back: retail UO's `probe_mounttouch` fired 57 times a
/// second spawned (0x401c4008), 2 under 0x4000 and under 0, 55 under
/// 0x400000 and under 0x40000000 (coduo-movement.md 11).
#[test]
fn set_contents_moves_a_mount_in_and_out_of_the_touch_pass() {
    let mut rt = pickup_rig();
    let mount = spawn_mount(&mut rt, [-40.0, -40.0, -10.0], [40.0, 40.0, 80.0], false);
    let touched = |rt: &mut ScriptRuntime, contents: Option<i32>| {
        rt.with_host(|host, cx| {
            if let Some(c) = contents {
                let recv = Some(Target::Entity(mount));
                crate::game::builtins::uo::set_contents(host, cx, recv, &[Value::Int(c)]).unwrap();
            }
            crate::game::trigger::touched(host, cx, 0, [0.0; 3]).contains(&mount)
        })
    };
    assert!(touched(&mut rt, None));
    assert!(!touched(&mut rt, Some(0x4000)));
    assert!(touched(&mut rt, Some(0x40_0000)));
    assert!(!touched(&mut rt, Some(0)));
    assert!(touched(&mut rt, Some(0x4000_0000)));
    assert!(touched(&mut rt, Some(0x401c_4008)));
}

/// A held LMG hints 7 where its probe, 15 ahead and 1 up, is inside a
/// script mount, 0 when it faces away or has the bipod down; the
/// value is 0 off the mount's contents and the spawnflag only off the
/// link's margin (coduo-movement.md 11).
#[test]
fn a_held_lmg_hints_7_at_a_script_mount() {
    let mut rt = pickup_rig();
    let away = [0.0, 180.0, 0.0];
    let stand = spawn_mount(&mut rt, [20.0, 5.0, 10.0], [10.0, -5.0, -10.0], false);
    assert_eq!(
        rt.with_host(|host, cx| {
            let c = cx.intern_folded("classname");
            match vcod_gsc::Host::get_field(host, cx, stand, c) {
                Value::String(a) => cx.resolve(a).to_string(),
                v => format!("{v:?}"),
            }
        }),
        "trigger_mount"
    );
    let boxes = rt.mount_boxes();
    assert_eq!(boxes.len(), 1);
    assert_eq!(boxes[0].mins, glam::Vec3::new(10.0, -5.0, -10.0));
    assert_eq!(boxes[0].contents, 0x401c_4008);
    let lmg = |yaw_deg, sighted| LmgHint {
        origin: [0.0; 3],
        yaw_deg,
        sighted,
        hint_string: None,
        boxes: &boxes,
    };
    let at = |hint, val| CursorHint {
        hint,
        val,
        string: Some(-1),
    };
    assert_eq!(
        rt.cursor_hints(0, EYE, away, Some(lmg(0.0, false))),
        at(7, 0)
    );
    assert_eq!(
        rt.cursor_hints(0, EYE, away, Some(lmg(0.0, true))),
        at(0, 0)
    );
    assert_eq!(
        rt.cursor_hints(0, EYE, away, Some(lmg(180.0, false))),
        at(0, 0)
    );
    assert_eq!(rt.cursor_hints(0, EYE, away, None), at(0, 0));

    // A crouched one whose box stops half a unit short of the probe.
    let mut rt = pickup_rig();
    spawn_mount(&mut rt, [15.5, -5.0, -10.0], [25.0, 5.0, 10.0], true);
    let boxes = rt.mount_boxes();
    assert_eq!(boxes[0].contents, 0x405c_0008);
    let lmg = |x: f32| LmgHint {
        origin: [x, 0.0, 0.0],
        yaw_deg: 0.0,
        sighted: false,
        hint_string: None,
        boxes: &boxes,
    };
    assert_eq!(rt.cursor_hints(0, EYE, away, Some(lmg(0.0))), at(7, 1));
    assert_eq!(rt.cursor_hints(0, EYE, away, Some(lmg(1.0))), at(7, 0));
    assert_eq!(rt.cursor_hints(0, EYE, away, Some(lmg(-2.0))), at(0, 0));
}

#[test]
fn aiming_at_an_unowned_fg42_hints_nine_past_its_index() {
    let mut rt = pickup_rig();
    rt.place_item("mpweapon_fg42", [40.0, 0.0, 30.0], 90);
    // fg42 is index 6: the capture's 15 (docs/research/cod11-items.md 12.3).
    assert_eq!(rt.cursor_hint_pass(0, EYE, AT_ITEM).0, 15);
    assert_eq!(rt.cursor_hint_pass(0, EYE, [-36.0, 0.0, 0.0]).0, 0);
    rt.host.clients[0].vitals = crate::game::host::Vitals {
        health: 0,
        max_health: 100,
        dead: true,
        takedamage: true,
    };
    assert_eq!(rt.cursor_hint_pass(0, EYE, AT_ITEM).0, 0);
}

/// An owned fg42 short of full hints 73 past its index, the capture's
/// 79, and once taken the same aim hints nothing.
#[test]
fn an_owned_weapon_hints_73_past_its_index_until_taken() {
    let mut rt = pickup_rig();
    let fg = crate::defs::V1.weapon_index("fg42_mp").unwrap();
    let d = rt.host.weapons.get(fg).unwrap().clone();
    rt.host.clients[0].weapons.give(fg, 2);
    rt.host.clients[0].ammo.ammo[d.ammo_index] = 100;
    rt.place_item("mpweapon_fg42", [40.0, 0.0, 30.0], 90);
    assert_eq!(rt.cursor_hint_pass(0, EYE, AT_ITEM).0, 79);
    cmd(&mut rt, 0, vcod_common::net::msg::BUTTON_USE, EYE, AT_ITEM);
    assert_eq!(rt.cursor_hint_pass(0, EYE, AT_ITEM).0, 0);
}

/// The capture's swap: 32 on the panzerfaust, 0 on the carbine it
/// dropped while its dropper is locked out, 21 once the lock clears.
#[test]
fn an_owner_locked_drop_hints_nothing_until_its_lock_clears() {
    let mut rt = pickup_rig();
    let fg = crate::defs::V1.weapon_index("fg42_mp").unwrap();
    rt.host.clients[0].weapons.give(fg, 2);
    rt.place_item("mpweapon_panzerfaust", [40.0, 0.0, 30.0], 0);
    assert_eq!(rt.cursor_hint_pass(0, EYE, AT_ITEM).0, 32);
    cmd(&mut rt, 0, vcod_common::net::msg::BUTTON_USE, EYE, AT_ITEM);
    let carbine = crate::defs::V1.weapon_index("m1carbine_mp").unwrap();
    let drop = rt
        .host
        .ents
        .iter_inuse()
        .find_map(|(id, e)| e.item.filter(|i| i.index as usize == carbine).map(|_| id))
        .expect("the carbine was dropped");
    assert_eq!(rt.cursor_hint_pass(0, EYE, AT_ITEM).0, 0);
    rt.host
        .ents
        .get_mut(drop)
        .unwrap()
        .item
        .as_mut()
        .unwrap()
        .owner = None;
    assert_eq!(rt.cursor_hint_pass(0, EYE, AT_ITEM).0, 21);
}

/// The item pass's notifies reach script at the next frame: a player
/// parked on `"touch"` wakes with the item, before the grab it refused.
#[test]
fn a_touch_notify_wakes_a_waiting_player_thread() {
    let mut rt = pickup_rig_with(
        "main() {}\nCodeCallback_PlayerConnect() { self waittill(\"touch\", item); logPrint(\"touched \" + item.classname); }\n",
    );
    let id = rt.place_item("item_health", [0.0, 0.0, 1.0], 0);
    cmd(&mut rt, 0, 0, [0.0, 0.0, 60.0], DOWN);
    assert!(!rt.host.ents.get(id).unwrap().item.unwrap().taken);
    rt.run_frame(50);
    assert!(rt.script_log().iter().any(|l| l == "touched item_health"));
}

/// A thread an item notify wakes runs before the frame's `wait`s come
/// due, whatever the two threads' ages: an older loop polling a flag the
/// `"trigger"` waiter sets sees it on the grab's own frame, the way
/// retail's `probe_pickup` teleport did (docs/research/cod11-items.md,
/// 13.1).
#[test]
fn an_item_notified_thread_runs_before_the_frames_waits() {
    let mut rt = pickup_rig_with(
        "main() { level.flag = 0; thread poll(); }\n\
         poll() { for (;;) { wait 0.05; if (level.flag) { logPrint(\"seen \" + getTime()); return; } } }\n\
         watch() { self waittill(\"trigger\", player); level.flag = 1; }\n\
         CodeCallback_PlayerConnect() {}\n",
    );
    rt.host.clients[0].vitals.health = 50;
    let id = rt.place_item("item_health", [0.0, 0.0, 1.0], 0);
    rt.start_thread_for_test(id, "watch");
    // The loop's first `wait` from the load falls due on the first
    // frame and runs on the second; from there it polls every frame.
    rt.run_frame(50);
    cmd(&mut rt, 0, 0, [0.0, 0.0, 60.0], DOWN);
    rt.run_frame(100);
    rt.run_frame(150);
    let seen: Vec<&String> = rt
        .script_log()
        .iter()
        .filter(|l| l.starts_with("seen"))
        .collect();
    assert_eq!(seen, vec!["seen 100"]);
}
