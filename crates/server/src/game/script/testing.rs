//! Seams the tests and the `testing` feature drive the runtime through.

use super::*;

impl ScriptRuntime {
    /// A client's `sessionteam`, as a test sets it (`Server::test_set_client_team`).
    #[cfg(any(test, feature = "testing"))]
    pub fn set_client_team_for_test(&mut self, slot: usize, team: &str) {
        self.set_client_string_for_test(slot, "sessionteam", team);
    }

    /// Writes one string field of a client's entity, as a script would.
    #[cfg(any(test, feature = "testing"))]
    pub fn set_client_string_for_test(&mut self, slot: usize, name: &str, value: &str) {
        use vcod_gsc::Host;
        let Some(ent) = self.client_entity(slot) else {
            return;
        };
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            let field = cx.intern_folded(name);
            let v = Value::String(cx.intern_exact(value));
            host.set_field(cx, ent, field, v).unwrap();
        });
    }
}

#[cfg(test)]
impl ScriptRuntime {
    /// A playing client standing at `at`, linked there the way its cmd's
    /// link would: what a blast's walk finds it by.
    pub fn place_client(&mut self, slot: usize, at: [f32; 3]) {
        self.set_client_origin(slot, at);
        let bounds = self.host.client_box(slot);
        let body = vcod_common::movetrace::CONTENTS_BODY as i32;
        self.host.link_client(slot, at, bounds, body, true);
    }

    /// A `misc_mg42` at `at` with `G_SpawnTurret`'s health and a record
    /// off a bare weapon file.
    pub fn place_turret(&mut self, at: [f32; 3]) -> EntId {
        use vcod_gsc::Host;
        let host = &mut self.host;
        let id = self.vm.with_cx(|cx| {
            let id = host.ents.spawn(cx).unwrap();
            for (f, v) in [
                ("classname", Value::String(cx.intern_exact("misc_mg42"))),
                ("origin", Value::Vector(at)),
                ("health", Value::Int(crate::game::turret::TURRET_HEALTH)),
            ] {
                let a = cx.intern_folded(f);
                host.set_field(cx, id, a, v).unwrap();
            }
            id
        });
        let def = crate::game::turret::TurretDef::parse("WEAPONFILE\\weaponClass\\turret").unwrap();
        let rec = crate::game::turret::TurretRecord::new(
            "mg42_bipod_stand_mp",
            def,
            Default::default(),
            0.0,
        );
        self.host.turrets.insert(id, rec);
        let host = &mut self.host;
        self.vm.with_cx(|cx| host.link_turret(cx, id));
        id
    }

    /// A placed item at `at`, the way the map load makes one.
    pub fn place_item(&mut self, classname: &str, at: [f32; 3], count: i32) -> EntId {
        use vcod_gsc::Host;
        let host = &mut self.host;
        let id = self.vm.with_cx(|cx| {
            let id = host.ents.spawn(cx).unwrap();
            for (f, v) in [
                ("classname", Value::String(cx.intern_exact(classname))),
                ("origin", Value::Vector(at)),
                ("count", Value::Int(count)),
            ] {
                let a = cx.intern_folded(f);
                host.set_field(cx, id, a, v).unwrap();
            }
            id
        });
        let row = crate::items::classname_index(self.host.defs, classname).unwrap();
        crate::game::item::attach(&mut self.host, id, row);
        let host = &mut self.host;
        self.vm
            .with_cx(|cx| crate::game::item::link(host, cx, id, crate::game::item::CONTENTS_ITEM));
        id
    }

    /// A `trigger_hurt` of `shape` on `id` doing `dmg` a touch, linked as
    /// its spawn would link it.
    #[cfg(any(test, feature = "testing"))]
    pub fn register_hurt_for_test(
        &mut self,
        id: EntId,
        shape: crate::game::trigger::TriggerShape,
        dmg: i32,
    ) {
        self.host.triggers.register_hurt(id, shape, 0, 0, 0.0);
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            let atom = cx.intern_folded("dmg");
            let _ = vcod_gsc::Host::set_field(host, cx, id, atom, Value::Int(dmg));
            host.link_trigger(cx, id);
        });
    }

    /// A trigger the test registered, linked as its spawn would link it.
    #[cfg(any(test, feature = "testing"))]
    pub fn link_trigger_for_test(&mut self, id: EntId) {
        let host = &mut self.host;
        self.vm.with_cx(|cx| host.link_trigger(cx, id));
    }

    /// Compiles `src` as `maps/mp/test`, builds a host with an empty object
    /// table (no paks, no bsp, no missing-builtin pre-scan), and starts
    /// `main`. For this module's own tests.
    pub fn for_test(src: &str) -> ScriptRuntime {
        Self::for_test_at("maps/mp/test", src)
    }

    /// `for_test` with the file path spelled out, for a test whose script has
    /// to answer to a path the runtime dispatches into by name:
    /// `CALLBACK_SETUP` and its `CodeCallback_*` entry points.
    pub fn for_test_at(path: &str, src: &str) -> ScriptRuntime {
        let mut vm = Vm::new();
        let ast = vcod_gsc::parse::parse_file(src).expect("test script parses");
        let fns = vcod_gsc::compile::compile_file(&ast, path, vm.interner_mut())
            .expect("test script compiles");
        vm.install(fns).expect("test script installs");
        let host = GameHost::new(&crate::game::profile::COD1_11, vec![String::new(); 2048]);
        let mut rt = ScriptRuntime {
            vm,
            host,
            entry: path.to_string(),
            gametype_entry: String::new(),
            map: None,
            scripts: None,
            script_lag_ms: 0,
            developer: false,
            client_strings: HashMap::new(),
        };
        let main = rt.vm.func_ref(&rt.entry, "main");
        rt.vm.start_thread(&mut rt.host, main, None, vec![]);
        rt.host.initializing = false;
        rt
    }

    /// The path [`Self::install_for_test`] installs under.
    #[cfg(test)]
    pub fn entry_for_test(&self) -> &str {
        &self.entry
    }

    /// Compile and install another file's worth of functions into a test
    /// runtime, so a test can add a thread body after `for_test`.
    #[cfg(any(test, feature = "testing"))]
    pub fn install_for_test(&mut self, src: &str) {
        let ast = vcod_gsc::parse::parse_file(src).expect("test script parses");
        let fns = vcod_gsc::compile::compile_file(&ast, &self.entry, self.vm.interner_mut())
            .expect("test script compiles");
        self.vm.install(fns).expect("test script installs");
    }

    /// A map entity at `origin`, the way the entity lump makes one.
    #[cfg(any(test, feature = "testing"))]
    pub fn spawn_map_entity_for_test(&mut self, origin: [f32; 3]) -> EntId {
        use vcod_gsc::Host;
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            let id = host.ents.spawn(cx).unwrap();
            let at = cx.intern_folded("origin");
            host.set_field(cx, id, at, Value::Vector(origin)).unwrap();
            id
        })
    }

    /// A client entity in `slot` at `origin`.
    #[cfg(any(test, feature = "testing"))]
    pub fn spawn_client_for_test(&mut self, slot: usize, origin: [f32; 3]) -> EntId {
        let host = &mut self.host;
        let id = self
            .vm
            .with_cx(|cx| host.ents.spawn_client(cx, slot, None).unwrap());
        self.set_client_origin(slot, origin);
        id
    }

    /// Writes a client field through the field setter, the way script does.
    #[cfg(any(test, feature = "testing"))]
    pub fn set_client_field_for_test(&mut self, slot: usize, name: &str, v: Value) {
        use vcod_gsc::Host;
        let Some(ent) = self.client_entity(slot) else {
            return;
        };
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            let field = cx.intern_folded(name);
            host.set_field(cx, ent, field, v).unwrap();
        });
    }

    /// A client's `sessionstate`, which `spawn_client` leaves at
    /// `"spectator"`; the four legal strings are in
    /// docs/research/cod11-map-cycle.md 6.1. Also the `takedamage` the
    /// client's next end frame would write for it.
    #[cfg(any(test, feature = "testing"))]
    pub fn set_client_state_for_test(&mut self, slot: usize, state: &str) {
        use vcod_gsc::Host;
        self.set_client_takedamage(slot, state == "playing");
        let Some(ent) = self.client_entity(slot) else {
            return;
        };
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            let field = cx.intern_folded("sessionstate");
            let v = Value::String(cx.intern_exact(state));
            host.set_field(cx, ent, field, v).unwrap();
        });
    }

    /// Start `name` as a thread on `ent`, the way a script's `thread` does.
    #[cfg(any(test, feature = "testing"))]
    pub fn start_thread_for_test(&mut self, ent: EntId, name: &str) {
        let f = self.vm.func_ref(&self.entry, name);
        self.vm
            .start_thread(&mut self.host, f, Some(Target::Entity(ent)), vec![]);
    }

    /// Start `path::name` as a thread on `recv` with the arguments `args`
    /// builds, the way a script's `thread` does.
    #[cfg(test)]
    pub fn start_script_thread_for_test(
        &mut self,
        path: &str,
        name: &str,
        recv: Option<EntId>,
        args: impl FnOnce(&mut vcod_gsc::Cx) -> Vec<Value>,
    ) {
        let f = self.vm.func_ref(path, name);
        let args = self.vm.with_cx(args);
        let recv = Some(match recv {
            Some(e) => Target::Entity(e),
            None => Target::Struct(self.vm.level_id()),
        });
        self.vm.start_thread(&mut self.host, f, recv, args);
    }

    /// Set a test client's health and max health, the way a spawn does.
    #[cfg(any(test, feature = "testing"))]
    pub fn set_client_health_for_test(&mut self, slot: usize, health: i32) {
        if let Some(v) = self.host.clients.get_mut(slot).map(|c| &mut c.vitals) {
            v.health = health;
            v.max_health = health;
        }
    }

    /// Writes a folded field on `level`.
    #[cfg(any(test, feature = "testing"))]
    pub fn set_level_field_for_test(&mut self, name: &str, v: Value) {
        let level = self.vm.level_id();
        self.vm.with_cx(|cx| {
            let atom = cx.intern_folded(name);
            cx.set_field(level, atom, v);
        });
    }

    /// Reads a folded field off `level`.
    pub fn level_field(&mut self, name: &str) -> vcod_gsc::Value {
        let level = self.vm.level_id();
        self.vm.with_cx(|cx| {
            let atom = cx.intern_folded(name);
            cx.get_field(level, atom)
        })
    }

    /// [`ScriptRuntime::level_field`] rendered the way `client_field`
    /// renders.
    pub fn level_field_str(&mut self, name: &str) -> String {
        let v = self.level_field(name);
        self.vm.with_cx(|cx| render(cx, v))
    }
}
