//! The map script's own body writes configstrings 3 and 12, and they match
//! what retail writes for the same map.

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
