//! The map script's own body writes configstrings 3 and 12, and they match
//! what retail writes for the same map. This is also where the production
//! wiring that hands the mounted paks to `GameHost` is exercised: nothing
//! else notices when it goes away.

fn cfg(map: &str) -> vcod_server::ServerConfig {
    vcod_server::ServerConfig {
        map: map.into(),
        hostname: "vcod test".into(),
        max_clients: 4,
        gametype: "dm".into(),
        test_entities: 0,
        trace: false,
    }
}

#[test]
fn the_map_script_writes_the_fog_and_ambient_configstrings() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let mut sv = vcod_server::server::Server::new(cfg("mp_pavlov"), std::time::Instant::now());
    sv.load_scripts(std::rc::Rc::new(fs)).unwrap();

    // Captured from the retail server on mp_pavlov via the headless probe
    // at debug level. Slot 12 carries seven fields for setCullFog's six
    // arguments: the engine inserts a 1 after the far distance.
    assert_eq!(sv.configstring(12), "0 6000 1 0.8 0.8 0.8 0");
    assert_eq!(sv.configstring(3), "n\\ambient_mp_pavlov\\t\\0");
}

/// `GameHost.fs` is what lets `RegisterItem` read a weapon's `worldModel`
/// and `projectileModel` out of its weapon file, and `ScriptRuntime::load`
/// is the only place that sets it. Drop that one line and every
/// weapon-file-sourced model silently leaves the model block while every
/// other test stays green -- `precacheItem`'s own test uses `item_health`,
/// whose model is compiled into the binary, and the spawn-side test mounts
/// its own filesystem. These two names can come from nowhere else on
/// mp_pavlov: `weapon_MP40` is no entity's `model` key and no script
/// precaches it by name, and a `projectile_*` model is only ever a weapon
/// file's `projectileModel`.
#[test]
fn the_model_block_carries_the_models_that_come_from_weapon_files() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let mut sv = vcod_server::server::Server::new(cfg("mp_pavlov"), std::time::Instant::now());
    sv.load_scripts(std::rc::Rc::new(fs)).unwrap();

    let models: Vec<&str> = (269..=523).map(|i| sv.configstring(i)).collect();
    for name in ["xmodel/weapon_MP40", "xmodel/projectile_GermanGrenade"] {
        assert!(
            models.contains(&name),
            "{name} is not in the model configstring block"
        );
    }
}
