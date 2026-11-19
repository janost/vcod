//! Level load, scripts and cvars, serverinfo.

use super::*;

impl Server {
    /// Builds the bots' navigation graph off the tick thread: the server
    /// keeps real time through the build and the bots wander until it lands.
    /// Off by default, so a test's bots find the graph on their first tick.
    pub fn build_nav_in_background(&mut self, on: bool) {
        self.game.module.driver.nav_background = on;
    }

    /// Swap in the map built by the binary; tests run without one.
    pub fn load_world(&mut self, world: World) {
        world
            .collision
            .set_arith(vcod_common::patch::Arith::for_protocol(self.game.proto));
        self.game.world = Some(Rc::new(world));
        self.game.module.driver.new_level(false);
        self.game.module.nav_relink = None;
        self.game.module.nav_rebuild = None;
        self.game.module.bot_breach_stands.clear();
        self.game.module.bot_parked = Default::default();
        self.game.module.bot_drives = Default::default();
    }

    /// The cvar table a gametype script starts with: the engine defaults,
    /// then `default_mp.cfg`, then `g_gametype` and `sv_maxclients`
    /// mirroring this run's `ServerConfig` (`sv_hostname` too, at the first
    /// load only), and the engine
    /// cvars `Server` keeps in its own fields. None of
    /// them are flagged into the 140/204 mirror; only `makeCvarServerInfo`
    /// does that.
    ///
    /// `default_mp.cfg` is where the stock `scr_*` values come from, and
    /// its `scr_allow_fg42 0` is the one the scripts' defaults disagree with
    /// (cod11-gsc-object-model.md 17). It is not cosmetic: it makes
    /// `_teams::restrictPlacedWeapons` `delete()` the map's placed fg42s, so
    /// the value moves entity numbering as well as the mirror.
    ///
    /// Past the first level the table comes from the one the outgoing level
    /// left ([`Self::carried_cvars`]) rather than from `default_mp.cfg`
    /// again: retail's is process-global and a script's `setCvar` survives
    /// both boundaries. The `--set`s go in at the first load only, as
    /// `Com_StartupVariable` runs once; [`Self::set_cvar`] writes the
    /// running table after that.
    fn cvars(&self, fs: &dyn vcod_common::asset_source::AssetSource) -> crate::cvars::Cvars {
        let mut cvars = match self.game.carried_cvars.clone() {
            Some(c) => c,
            None => {
                let mut cvars =
                    crate::cvars::Cvars::for_protocol(self.game.proto.transport.version);
                match fs.load::<RawFile>("default_mp.cfg").ok() {
                    Some(raw) => {
                        let n = cvars.exec_cfg(&raw.text());
                        log::debug!("default_mp.cfg: {n} cvars set");
                    }
                    // A mount set without the localized pak loses every stock
                    // `scr_*` value, so say so rather than run on script defaults.
                    None => log::warn!(
                        "default_mp.cfg not in the mounted paks; stock scr_* defaults lost"
                    ),
                }
                cvars
            }
        };
        // A level load is where a latched value goes live.
        cvars.apply_latched();
        cvars.set("g_gametype", &self.game.cfg.gametype);
        // `SV_SpawnServer`'s `Cvar_Set("mapname", ...)` (map-cycle doc,
        // section 3 step 15), which a script's `getCvar("mapname")` reads.
        cvars.set("mapname", &self.game.cfg.map);
        // Latched: a `set` while a level ran waits in the overrides.
        cvars.set(
            "sv_maxclients",
            &self.pending_cvar("sv_maxclients", &self.game.cfg.max_clients.to_string()),
        );
        cvars.set("dedicated", &self.game.dedicated.to_string());
        cvars.set("sv_running", "1");
        cvars.set("sv_serverid", &self.game.server_id.to_string());
        cvars.set("sv_cheats", if self.game.cheats { "1" } else { "0" });
        // `Com_StartupVariable` runs from `Com_Init` only, never from
        // `SV_SpawnServer`, so the command line's sets go in once, over
        // `default_mp.cfg`; past that a script's `setCvar` stands
        // (map-cycle doc 4.5).
        if self.game.carried_cvars.is_none() {
            cvars.set("sv_hostname", &self.game.cfg.hostname);
            for (name, value) in &self.game.cvar_overrides {
                cvars.set(name, value);
            }
        }
        if self.v15() {
            cvars.set_ui_cvars();
            // 1.5b's `map` and `map_restart` (0x80876aa, 0x8087777) put a
            // `timescale` at or under 0 back to 1, which is what ends a
            // match timeout a level boundary cut short.
            if crate::game::builtins::cvar::atof(cvars.get("timescale")) <= 0.0 {
                cvars.set("timescale", "1");
            }
        }
        cvars
    }

    /// The movement rules this server's players and bots move by.
    pub fn move_rules(&self) -> vcod_common::pmove::MoveRules {
        vcod_common::pmove::MoveRules::for_protocol(self.game.proto)
    }

    /// The protocol this server speaks: one per process, as each retail
    /// binary speaks one (docs/research/cod15-protocol-diff.md, "Server
    /// side"). Set before the first level loads.
    pub fn set_protocol(&mut self, p: &'static Cod1Wire) {
        self.game.proto = p;
        self.core.transport = &p.transport;
        // `sv_master1` defaults to the binary's own master unless set.
        if !self
            .game
            .cvar_overrides
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case("sv_master1"))
        {
            self.core.masters.set(0, p.transport.master().host);
        }
        self.core.masters.set_port(p.transport.master().port);
        if let Some(w) = &self.game.world {
            w.collision
                .set_arith(vcod_common::patch::Arith::for_protocol(p));
        }
        // The static slots move with the layout (UO's start time, winner
        // and hint strings).
        self.core.configstrings = configstrings::static_configstrings(
            &self.game.cfg,
            self.game.server_id,
            self.game.module.fall_heights,
            self.game.cheats,
            &self.game.paks,
            self.systeminfo_v6().as_deref(),
            p,
        );
        self.core.sent_configstrings = self.core.configstrings.clone();
    }

    pub fn protocol(&self) -> &'static Cod1Wire {
        self.game.proto
    }

    /// Whether this server speaks 1.5 or United Offensive, whose engine and
    /// game module are 1.5's with UO's additions (coduo-diff.md, "Server").
    pub fn v15(&self) -> bool {
        !self.game.proto.is_v1()
    }

    /// The game module's tables for this protocol.
    pub fn defs(&self) -> &'static crate::defs::Defs {
        crate::defs::Defs::of(self.game.proto)
    }

    /// Configstring 1's 1.5 rows (`registry::SYSTEMINFO_V6`), `None` on a
    /// 1.1 server.
    pub(super) fn systeminfo_v6(&self) -> Option<Vec<(String, String)>> {
        if !self.v15() {
            return None;
        }
        let fresh;
        let cvars = match self.game.module.script.as_ref() {
            Some(rt) => rt.cvars(),
            None => {
                let mut c = crate::cvars::Cvars::for_protocol(self.game.proto.transport.version);
                c.set_ui_cvars();
                fresh = c;
                &fresh
            }
        };
        Some(
            cvars
                .info_pairs(crate::cvars::flag::SYSTEMINFO)
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    /// Loads and runs the gametype and map scripts. Called once per level,
    /// before any client is let back in. The script keeps allocating
    /// configstrings after that (any `setModel`, `loadFX`, `playSound` or
    /// `ambientPlay` from a thread that has passed a `wait`), so the table is
    /// not final at gamestate time; `tick` copies the script's table back
    /// every frame and [`Self::broadcast_configstring_changes`] is what
    /// carries a later allocation to a client that already has its gamestate.
    ///
    /// The table is cloned in, not moved: `ScriptRuntime::load` fails on a
    /// missing `.gsc`, an unresolvable map, a bad BSP or a failed entity
    /// spawn, and the error path has to leave `self.core.configstrings` exactly as
    /// it was, so the error the caller reports is about the script rather
    /// than about a half-cleared table. `main.rs` exits on it.
    pub fn load_scripts(&mut self, fs: Rc<vcod_common::pk3::Pk3Fs>) -> anyhow::Result<()> {
        self.load_scripts_with(fs, false, crate::game::script::Carry::default(), false)?;
        // Configstring 0 from the cvar table the scripts now own, as the
        // first frame's flush would write it.
        self.flush_serverinfo();
        // The first map's `SV_SpawnServer` steps 20 and 21, as a map change
        // runs them.
        self.settle_frames();
        self.rebuild_baselines();
        Ok(())
    }

    /// The test seam for a gametype script that ships in no pak: `path` is a
    /// canonical script path (no extension) and `text` its source, answered
    /// instead of the paks on every later level load. `--gametype-script`
    /// uses it to run a client probe as the gametype, and the tests that need
    /// the real console and restart paths under a probe script call it
    /// directly.
    pub fn overlay_script(&mut self, path: &str, text: &str) {
        self.game.module.script_overlay = Some((path.to_string(), text.to_string()));
        // A restart reuses the running level's compile; new text needs a new one.
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.forget_compiled();
        }
    }

    /// Every line the running level's script has passed to `logPrint`.
    /// Test-facing: a new level starts a new log, so a caller spanning a
    /// restart collects this per level.
    pub fn script_log(&self) -> &[String] {
        self.game
            .module
            .script
            .as_ref()
            .map_or(&self.game.shutdown_log, |rt| rt.script_log())
    }

    /// Whether `killserver` or a `G_Error` drop has shut the level down.
    pub fn is_killed(&self) -> bool {
        self.game.killed
    }

    /// `SV_InitGameProgs(savePersist)` (docs/research/cod11-map-cycle.md,
    /// section 3 step 19): [`Self::load_scripts`] with retail's
    /// `G_InitGame(levelTime, seed, restart, savePersist)` arguments and the
    /// two tables `savePersist` decides the fate of, `game[]` and one
    /// `pers[]` per client slot. `save_persist` is the whole of that
    /// decision: both boundaries lift the two tables and both drop them
    /// here when the outgoing level did not ask for them (section 1, and
    /// the three `probe_persist_*` captures).
    ///
    /// `restart` skips re-reading what a restart cannot have changed: the
    /// paks are the same and so is the map, so the animtree, the weapon
    /// files and the hit-location table stay as they are. That is the whole
    /// of the difference between the two paths here; retail's is the game
    /// module it keeps loaded (section 4.1).
    pub fn load_scripts_with(
        &mut self,
        fs: Rc<vcod_common::pk3::Pk3Fs>,
        restart: bool,
        carry: crate::game::script::Carry,
        save_persist: bool,
    ) -> anyhow::Result<()> {
        let cvars = self.cvars(&*fs);
        self.game.level_cvars = Some((
            cvars.get("g_gametype").to_string(),
            cvars.get("sv_maxclients").to_string(),
        ));
        self.game.fs = Some(fs.clone());
        // `sv_pure`: retail's default 1, a `--set` over it, kept across levels.
        let pure = cvars.get("sv_pure").trim().parse::<i32>().unwrap_or(0) != 0;
        if let Some(cs0) = self.core.configstrings.get_mut(0) {
            *cs0 = configstrings::with_sv_pure(cs0, pure);
        }
        self.game.pure_check = Some(PureCheck::new(
            &fs,
            self.core.checksum_feed,
            self.game.proto,
        ));
        if !restart {
            self.game.module.anims = match vcod_common::animtree::PlayerAnims::load(&*fs) {
                Ok(a) => Some(Rc::new(a)),
                Err(e) => {
                    log::warn!("player anims: {e:#}, players will not animate");
                    None
                }
            };
            self.game.module.weapon_table =
                Rc::new(crate::weapons::WeaponTable::load(&fs, self.defs()));
            self.game.module.hitlocs = crate::game::combat::HitLocTable::load(&fs);
        }
        // `game[]` and `pers[]` are the script's and go only under
        // `savePersist`; the item registry is the engine's and survives a
        // restart either way, and a map change builds a fresh one (the
        // spawn path hands an empty `Carry`).
        let carry = crate::game::script::Carry {
            game: carry.game.filter(|_| save_persist),
            pers: if save_persist { carry.pers } else { Vec::new() },
            items: carry.items.filter(|_| restart),
            area: carry.area.filter(|_| restart),
            map: carry.map.filter(|_| restart),
            scripts: carry.scripts.filter(|_| restart),
            models: carry.models.filter(|_| restart),
            gametype: carry.gametype,
            seed: Some(self.game.module.level_seed),
            spawn_pins: self.game.module.spawn_pins.clone(),
            weapon_files: None,
        };
        self.game.module.level_seed =
            vcod_common::rng::xorshift(&mut self.game.module.level_seed) | 1;
        // `SV_MapRestart`'s `G_ShutdownGame(1)`: the running level's log
        // closes ahead of the new one's `G_InitGame`.
        self.close_game_log(if restart {
            crate::engine::gamelog::RESTART
        } else {
            crate::engine::gamelog::SHUTDOWN
        });
        let fs_paks = fs.clone();
        let source = crate::game::script::PakScripts::new(
            fs.clone(),
            self.game.module.script_overlay.clone(),
        );
        let mut rt = crate::game::script::ScriptRuntime::load_from(
            Box::new(source),
            fs,
            &self.game.cfg.map,
            &self.game.cfg.gametype,
            self.core.configstrings.clone(),
            cvars,
            self.game.world.clone(),
            self.game.module.weapon_table.clone(),
            self.core.sv_time_ms,
            carry,
        )?;
        rt.set_player_anims(self.game.module.anims.clone());
        // `SV_SpawnServer` step 23, once the level has read its files; a
        // restart keeps the lists, as `SV_MapRestart` never sets them.
        if !restart {
            self.game.paks = self.level_paks(&fs_paks, &rt, pure);
        }
        let mut configstrings = rt.configstrings().to_vec();
        rt.cvars()
            .write_mirror(&mut configstrings)
            .map_err(|e| anyhow::anyhow!("writing the cvar mirror: {e:?}"))?;
        self.core.configstrings = configstrings;
        // `G_InitGame`'s open, at the new `level.time` (cod11-map-cycle.md 9).
        let game = if rt.cvars().get("fs_game").is_empty() {
            fs_paks
                .dirs()
                .last()
                .and_then(|d| d.file_name())
                .map_or("main".to_string(), |n| n.to_string_lossy().into_owned())
        } else {
            rt.cvars().get("fs_game").to_string()
        };
        let mut info = Info::new();
        for (k, v) in rt.cvars().info_pairs(crate::cvars::flag::SERVERINFO) {
            info.set(k, v);
        }
        info.set("sv_pure", u8::from(self.game.paks.pure));
        let info = info.to_string();
        self.game
            .game_log
            .init(rt.cvars(), &game, &info, self.core.sv_time_ms);
        self.game.module.script = Some(rt);
        self.refresh_fall_heights();
        self.sync_sent_configstrings();
        // `SV_SpawnServer` and `SV_MapRestart` both clear it, the flag
        // included, and the new level's `main` turns it back on.
        self.game.archive.clear();
        Ok(())
    }

    /// The four pak lists for a level `rt` has just loaded off `fs`
    /// (cod11-server-handshake.md, "The four cvars").
    fn level_paks(
        &self,
        fs: &vcod_common::pk3::Pk3Fs,
        rt: &crate::game::script::ScriptRuntime,
        pure: bool,
    ) -> configstrings::PakLists {
        use vcod_common::net::protocol::PROTOCOL_V6;
        // 1.1 lists every pak, read or not.
        if !self.game.proto.is_v1() {
            // `G_ModelIndex` precaches each model as it indexes it, which
            // reads the descriptor and LOD 0's `xmodelparts`; vcod reads
            // them when it needs them, so the reads that flag their paks
            // are made here.
            let (first, last) = rt
                .host
                .allocators
                .layout()
                .bounds(configstrings::CsRange::Model);
            for name in &rt.configstrings()[first..=last] {
                if let Some(name) = name.strip_prefix("xmodel/") {
                    let _ = vcod_common::model::load_bones(fs, name);
                }
            }
            // So does the load's sound alias pass: every `soundaliases/*.csv`
            // on 1.5; on UO the map's sound load spec, or the default one
            // when the map ships none, whose aliases UO leaves unflagged.
            if self.game.proto.transport.version == PROTOCOL_V6.transport.version {
                for name in fs.list_prefix("soundaliases/") {
                    if name.ends_with(".csv") {
                        fs.touch(&name);
                    }
                }
            } else {
                let spec = format!("soundloadspecs/mp/{}.csv", self.game.cfg.map);
                if fs.contains(&spec) {
                    fs.touch(&spec);
                } else {
                    fs.touch("soundloadspecs/mp/default.csv");
                }
            }
            // And the script load reads each `#using_animtree`'s tree.
            for tree in rt.animtrees() {
                fs.touch(&format!("animtrees/{tree}.atr"));
            }
        }
        let cvars = rt.cvars();
        let mut paks = configstrings::PakLists::of(
            pure,
            fs.paks_read(),
            self.game.proto,
            cvars.get("fs_basegame"),
        );
        paks.fs_game = cvars.get("fs_game").to_string();
        paks
    }

    /// Every script thread that has died of an error, rendered
    /// `file::func:line: Kind`. Empty is the healthy reading: a thread that
    /// aborts stops running, silently as far as the wire is concerned, so
    /// this is what a test checks a clean bootstrap against.
    pub fn script_aborts(&self) -> Vec<String> {
        self.game
            .module
            .script
            .as_ref()
            .map_or_else(Vec::new, |rt| rt.aborts())
    }

    /// One field off a client's script entity, rendered as text. `None` when
    /// no script is loaded or the slot holds no client entity. This is the
    /// only way into a joined client's script state from outside the crate,
    /// and the tests that assert what the stock scripts left there are its
    /// only callers.
    pub fn client_field(&mut self, slot: usize, name: &str) -> Option<String> {
        self.game.module.script.as_mut()?.client_field(slot, name)
    }

    /// One key out of a client's `.pers`, rendered the same way.
    pub fn client_pers(&mut self, slot: usize, key: &str) -> Option<String> {
        self.game.module.script.as_mut()?.client_pers(slot, key)
    }

    /// One cvar as the running script reads it. `None` before
    /// `load_scripts`. Test-facing: `cfg.gametype` picks which gametype
    /// script loads and this table is what that script's own `getCvar`
    /// answers from, so a rotation that wrote only one of the two is
    /// invisible everywhere else.
    pub fn script_cvar(&self, name: &str) -> Option<String> {
        Some(
            self.game
                .module
                .script
                .as_ref()?
                .cvars()
                .get(name)
                .to_string(),
        )
    }

    /// `Cvar_Set`: before the first load a `+set` for it to stamp
    /// (`Com_StartupVariable`), after it a write to the running table, or to
    /// the carried one between levels. `g_gametype` and `sv_maxclients` are
    /// latched and wait for the next load. `sv_mapRotation` and `g_gametype`
    /// also mirror into their own fields, so `--set` and `map_rotate`'s
    /// `gametype` token both work through one value
    /// (docs/research/cod11-map-cycle.md section 5).
    pub fn set_cvar(&mut self, name: &str, value: &str) {
        // 1.5b registers `sv_punkbuster` read-only ahead of the command
        // line's `+set`s, so the server runs without PunkBuster
        // (cod15-protocol-diff.md, "PunkBuster").
        if self.v15() && name.eq_ignore_ascii_case("sv_punkbuster") {
            log::info!("{name} is read only.");
            return;
        }
        // In place when the name is already there, so the last write wins
        // whether it came from `--set` or from a rotation token, and a
        // rotation running for days does not grow the list a map at a time.
        match self.game.cvar_overrides.iter_mut().find(|(n, _)| n == name) {
            Some((_, v)) => *v = value.to_string(),
            None => self
                .game
                .cvar_overrides
                .push((name.to_string(), value.to_string())),
        }
        let latched =
            name.eq_ignore_ascii_case("g_gametype") || name.eq_ignore_ascii_case("sv_maxclients");
        if !latched {
            if let Some(rt) = self.game.module.script.as_mut() {
                rt.host.cvars.set(name, value);
            } else if let Some(c) = self.game.carried_cvars.as_mut() {
                c.set(name, value);
            }
        }
        self.apply_engine_cvar(name, value);
        // Latched: serverinfo keeps the running level's value until a load,
        // and takes this one at once only while no level has loaded.
        if name == "g_gametype" {
            self.game.cfg.gametype = value.to_string();
            self.refresh_serverinfo();
        }
    }

    /// The engine cvars `Server` keeps in its own fields. Names fold case.
    pub(super) fn apply_engine_cvar(&mut self, name: &str, value: &str) {
        let lower = name.to_ascii_lowercase();
        if let Some(i) = crate::master::Masters::index(&lower) {
            self.core.masters.set(i, value);
        }
        match lower.as_str() {
            "dedicated" => self.game.dedicated = value.trim().parse().unwrap_or(0),
            "rconpassword" => self.game.rcon_password = value.to_string(),
            "sv_maprotation" => self.game.sv_map_rotation = value.to_string(),
            "sv_hostname" => {
                self.game.cfg.hostname = value.to_string();
                self.refresh_serverinfo();
            }
            "sv_privateclients" => self.refresh_serverinfo(),
            _ => {}
        }
    }

    /// `Cvar_InfoString(CVAR_SERVERINFO)`: the running level's table, so a
    /// script `setCvar` of a serverinfo cvar shows as a console write does.
    /// With no level, the config and the `--set`s the first load stamps.
    /// `sv_pure` is the one the paks were listed under.
    pub(super) fn serverinfo(&self) -> Info {
        let mut i = match self.game.module.script.as_ref() {
            Some(rt) => {
                let mut i = Info::new();
                for (k, v) in rt.cvars().info_pairs(crate::cvars::flag::SERVERINFO) {
                    i.set(k, v);
                }
                i
            }
            None => {
                let mut i = configstrings::serverinfo(&self.live_cfg());
                i.set("sv_privateClients", self.private_clients())
                    .set("protocol", self.game.proto.transport.version)
                    .set("shortversion", self.game.proto.transport.short_version);
                i
            }
        };
        i.set("sv_pure", u8::from(self.game.paks.pure));
        i
    }

    /// `SV_Frame`'s `cvar_modifiedFlags & CVAR_SERVERINFO` test, as a
    /// compare: whatever wrote a serverinfo cvar this frame, configstring 0
    /// follows before the snapshots go out.
    pub(super) fn flush_serverinfo(&mut self) {
        if self.core.configstrings.first() != Some(&self.serverinfo().to_string()) {
            self.refresh_serverinfo();
        }
    }

    /// `SV_Frame`'s cvar flush, the serverinfo half: a write to a cvar the
    /// serverinfo string carries is followed by
    /// `SV_SetConfigstring(0, Cvar_InfoString(CVAR_SERVERINFO))`
    /// (docs/research/cod11-map-cycle.md, 4.3). Both tables, because the
    /// script owns its own copy between level loads and `tick` reads that one
    /// back over this one every frame.
    fn refresh_serverinfo(&mut self) {
        let info = self.serverinfo().to_string();
        if let Some(slot) = self.core.configstrings.get_mut(0) {
            *slot = info.clone();
        }
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.host.configstrings.set(0, info);
        }
    }

    /// `G_UpdateCvars` for the two fall bounds and `SV_Frame`'s systeminfo
    /// flush behind them: the moves read the values the script's table holds
    /// now, and slot 1 carries them to the clients that predict with them.
    /// Both tables, as [`Self::refresh_serverinfo`] writes both.
    pub(super) fn refresh_fall_heights(&mut self) {
        use crate::game::builtins::cvar::atof;
        let v6 = self.systeminfo_v6();
        let Some(rt) = self.game.module.script.as_mut() else {
            return;
        };
        let cvars = rt.cvars();
        let fall = FallHeights {
            min: atof(cvars.get("bg_fallDamageMinHeight")),
            max: atof(cvars.get("bg_fallDamageMaxHeight")),
        };
        self.game.module.fall_heights = fall;
        // Rebuilt only when an input moved or something else wrote slot 1.
        if let Some((_, info)) = self.game.systeminfo.as_ref().filter(|(k, info)| {
            k.0 == self.game.server_id
                && k.1 == [fall.min.to_bits(), fall.max.to_bits()]
                && k.2 == self.game.cheats
                && k.3 == self.game.paks
                && k.4 == v6
                && rt.configstrings().get(1) == Some(info)
        }) {
            if let Some(slot) = self.core.configstrings.get_mut(1)
                && slot != info
            {
                slot.clone_from(info);
            }
            return;
        }
        let info = configstrings::systeminfo(
            self.game.server_id,
            fall,
            self.game.cheats,
            &self.game.paks,
            v6.as_deref(),
        )
        .to_string();
        let key = (
            self.game.server_id,
            [fall.min.to_bits(), fall.max.to_bits()],
            self.game.cheats,
            self.game.paks.clone(),
            v6,
        );
        self.game.systeminfo = Some((key, info.clone()));
        rt.host.configstrings.set(1, info.clone());
        if let Some(slot) = self.core.configstrings.get_mut(1) {
            *slot = info;
        }
    }

    /// What the next level load would stamp for `name`, given the config's
    /// own value: a `+set` override if one names it, since [`Self::cvars`]
    /// replays the override list last.
    pub(super) fn pending_cvar(&self, name: &str, from_cfg: &str) -> String {
        self.game
            .cvar_overrides
            .iter()
            .find(|(n, _)| n == name)
            .map_or_else(|| from_cfg.to_string(), |(_, v)| v.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::*;

    /// The 1.5 `ui_*` rows ride configstring 1 in folded-name order, with
    /// no `timescale`, as the 1.5b gamestate's does.
    #[test]
    fn protocol_6_systeminfo_carries_the_15_rows() {
        let sv = v6_server(Instant::now());
        let rows = sv.systeminfo_v6().unwrap();
        let info = configstrings::systeminfo(
            0x10,
            FallHeights::default(),
            false,
            &Default::default(),
            Some(&rows),
        )
        .to_string();
        assert!(
            info.starts_with(
                "\\bg_fallDamageMaxHeight\\480\\bg_fallDamageMinHeight\\256\\g_synchronousClients\\0\\pmove_fixed\\0\\pmove_msec\\8\\sv_cheats\\0\\sv_console_lockout\\0\\sv_disableClientConsole\\0\\sv_pure\\0\\sv_serverid\\16\\ui_allowVote\\1\\ui_allowVoteClientKick\\0"
            ),
            "{info}"
        );
        assert!(
            info.ends_with("\\ui_killcam\\1\\ui_timeoutBank\\180000\\ui_timeoutLength\\90000\\ui_timeoutRecovery\\10000\\ui_timeoutsAllowed\\0"),
            "{info}"
        );
        assert!(!info.contains("timescale"));
    }

    #[test]
    fn serverinfo_is_configstring_zero() {
        let sv = Server::new(cfg(), Instant::now());
        let cs0 = sv.configstring(0);
        assert_eq!(info_value_for_key(cs0, "mapname"), Some("mp_carentan"));
        assert_eq!(
            info_value_for_key(sv.configstring(1), "sv_serverid"),
            Some("16")
        );
        assert!(sv.configstring(7).contains("kar98k_mp"));
    }

    /// The `weaponClass` table the animation machine reads every frame, and
    /// the seam it is built across: `WEAPON_LIST` is 1-based on the wire, so
    /// `items::item_name` subtracts one to name it. Misaligned by one, every
    /// pistol player animates as a rifleman and nothing says so. Needs the
    /// paks: the classes come from the weapon files.
    #[test]
    fn the_weapon_class_table_lines_up_with_the_wire_index() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            return;
        };
        let mut sv = Server::new(cfg(), Instant::now());
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        for (weapon, class) in [
            ("colt_mp", "pistol"),
            ("thompson_mp", "smg"),
            ("m1carbine_mp", "rifle"),
        ] {
            let index = crate::defs::V1
                .weapon_index(weapon)
                .expect("in WEAPON_LIST");
            assert_eq!(
                sv.game.module.weapon_table.class(index),
                class,
                "{weapon} at wire index {index}"
            );
            // The same index the frame loop hands the animscript.
            assert_eq!(
                crate::items::item_name(&crate::defs::V1, index),
                Some(weapon)
            );
        }
        assert_eq!(
            sv.game.module.weapon_table.class(0),
            "",
            "slot 0 is the wire's no-weapon"
        );
    }

    /// A failed load reports the error and leaves the table untouched; the
    /// caller (`main.rs`) exits on it rather than serving on: the table is
    /// mostly script output.
    #[test]
    fn a_failed_script_load_reports_and_changes_nothing() {
        let mut sv = Server::new(cfg(), Instant::now());
        let before: Vec<String> = sv.core.configstrings.clone();
        // No paks, so the map script does not resolve and `load` fails at its
        // first step.
        let err = sv.load_scripts(Rc::new(vcod_common::pk3::Pk3Fs::empty()));
        assert!(err.is_err());
        assert_eq!(sv.core.configstrings, before);
        assert!(sv.configstring(7).contains("kar98k_mp"));
    }

    /// The level script's cvar table carries `mapname`, the probes' only way
    /// to tell which map they run on; it used to read "".
    #[test]
    fn a_level_script_reads_the_map_name_cvar() {
        let sv = Server::new(cfg(), Instant::now());
        let cvars = sv.cvars(&vcod_common::pk3::Pk3Fs::empty());
        assert_eq!(cvars.get("mapname"), "mp_carentan");
    }

    /// Doc 4.5: `Com_StartupVariable` runs from `Com_Init` only, so a
    /// `--set` goes into the first level's table and a script's later
    /// `setCvar` of it survives a restart and a map change. Each load of
    /// this gametype appends to the cvar; a replay of the `--set` at each
    /// load would keep resetting it to one `x`.
    #[test]
    fn a_set_applies_once_and_a_scripts_write_outlives_later_loads() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let mut now = Instant::now();
        let mut sv = Server::new(
            ServerConfig {
                gametype: "trail".into(),
                ..cfg()
            },
            now,
        );
        sv.overlay_script(
            "maps/mp/gametypes/trail",
            "main() { maps\\mp\\gametypes\\_callbacksetup::SetupCallbacks(); \
             setCvar(\"vc_trail\", getCvar(\"vc_trail\") + \"x\"); }",
        );
        sv.set_cvar("vc_trail", "a");
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        assert_eq!(sv.script_cvar("vc_trail").as_deref(), Some("ax"));
        now += Duration::from_millis(50);
        sv.push_console("map_restart");
        sv.tick(now);
        assert_eq!(sv.script_cvar("vc_trail").as_deref(), Some("axx"));
        sv.spawn_server("mp_brecourt").expect("the map load failed");
        assert_eq!(sv.script_cvar("vc_trail").as_deref(), Some("axxx"));

        // A set while the level runs is `Cvar_Set` on the live table, and
        // the next load builds on it.
        sv.set_cvar("vc_trail", "b");
        assert_eq!(sv.script_cvar("vc_trail").as_deref(), Some("b"));
        sv.spawn_server("mp_carentan").expect("the map load failed");
        assert_eq!(sv.script_cvar("vc_trail").as_deref(), Some("bx"));
    }

    /// `sv_hostname` is a cvar like any other: the config's goes in at the
    /// first load and a script's `setCvar` of it outlives a restart and a
    /// map change (map-cycle doc 4.5).
    #[test]
    fn a_scripts_hostname_outlives_later_loads() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let mut now = Instant::now();
        let mut sv = Server::new(
            ServerConfig {
                gametype: "trail".into(),
                ..cfg()
            },
            now,
        );
        sv.overlay_script(
            "maps/mp/gametypes/trail",
            "main() { maps\\mp\\gametypes\\_callbacksetup::SetupCallbacks(); \
             setCvar(\"sv_hostname\", getCvar(\"sv_hostname\") + \"x\"); }",
        );
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        assert_eq!(sv.script_cvar("sv_hostname").as_deref(), Some("vcod testx"));
        now += Duration::from_millis(50);
        sv.push_console("map_restart");
        sv.tick(now);
        assert_eq!(
            sv.script_cvar("sv_hostname").as_deref(),
            Some("vcod testxx")
        );
        sv.spawn_server("mp_brecourt").expect("the map load failed");
        assert_eq!(
            sv.script_cvar("sv_hostname").as_deref(),
            Some("vcod testxxx")
        );
        let cs0 = sv.configstring(0).to_string();
        assert_eq!(
            info_value_for_key(&cs0, "sv_hostname"),
            Some("vcod testxxx")
        );
    }

    /// Doc 4.5: a `+set` after `+map` runs again once the level is up
    /// (`Com_AddStartupCommands`), so it outranks the script's load-time
    /// write, as `scr_allow_vote 0` read back on 1.1d over `dm.gsc`'s
    /// `setCvar("scr_allow_vote", level.allowvote)`.
    #[test]
    fn a_set_after_the_map_outranks_the_scripts_load_time_write() {
        // 1.1's dm.gsc writes scr_allow_vote at load; UO's does not.
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR is not a 1.1 install: skipping");
            return;
        };
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.set_cvar("scr_allow_vote", "0");
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        assert_eq!(
            sv.script_cvar("scr_allow_vote").as_deref(),
            Some("1"),
            "dm.gsc no longer writes scr_allow_vote at load"
        );
        sv.push_set("scr_allow_vote", "0");
        sv.drain_console(now);
        assert_eq!(sv.script_cvar("scr_allow_vote").as_deref(), Some("0"));
    }

    /// `map_rotate`'s `gametype` token is a `Cvar_Set` (doc section 5.2), so
    /// it has to outrank an earlier `--set g_gametype`: the token picks which
    /// gametype script loads, and the cvar table is what that script's own
    /// `getCvar` answers from. Writing only `cfg.gametype` splits the two.
    #[test]
    fn a_rotation_gametype_token_outranks_an_earlier_set() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let fs = Rc::new(fs);
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.set_cvar("g_gametype", "dm");
        sv.set_cvar("sv_mapRotation", "gametype tdm map mp_brecourt");
        let path = fs.resolve_map("mp_carentan").expect("map in the paks");
        let bsp =
            vcod_common::bsp::parse(&fs.read(&path).expect("read the bsp")).expect("parse the bsp");
        sv.load_world(World::from_bsp(&bsp, Some(&fs)));
        sv.load_scripts(fs).expect("load the scripts");
        assert_eq!(sv.script_cvar("g_gametype").as_deref(), Some("dm"));

        sv.push_console("map_rotate");
        sv.tick(now);
        assert_eq!(sv.game.cfg.map, "mp_brecourt", "the rotation did not load");
        assert_eq!(sv.game.cfg.gametype, "tdm");
        assert!(
            sv.configstring(0).contains("g_gametype\\tdm"),
            "serverinfo: {:?}",
            sv.configstring(0)
        );
        assert_eq!(
            sv.script_cvar("g_gametype").as_deref(),
            Some("tdm"),
            "the rotated level's script still reads the value `--set` left"
        );
    }

    /// Doc section 5.2: a script's `setCvar("sv_mapRotation", ...)` is the
    /// rotation the next `map_rotate` runs, and the queue it leaves is
    /// `sv_mapRotationCurrent` in the same table.
    #[test]
    fn a_rotation_a_script_set_drives_map_rotate() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR not at a 1.1 install: skipping");
            return;
        };
        let fs = Rc::new(fs);
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.set_cvar("g_gametype", "dm");
        let path = fs.resolve_map("mp_carentan").expect("map in the paks");
        let bsp =
            vcod_common::bsp::parse(&fs.read(&path).expect("read the bsp")).expect("parse the bsp");
        sv.load_world(World::from_bsp(&bsp, Some(&fs)));
        sv.load_scripts(fs).expect("load the scripts");
        sv.game.module.script.as_mut().unwrap().host.cvars.set(
            "sv_mapRotation",
            "gametype tdm map mp_brecourt map mp_carentan",
        );

        sv.push_console("map_rotate");
        sv.tick(now);
        assert_eq!(
            sv.game.cfg.map, "mp_brecourt",
            "the script's rotation did not load"
        );
        assert_eq!(sv.game.cfg.gametype, "tdm");
        assert_eq!(
            sv.script_cvar("sv_mapRotationCurrent").as_deref(),
            // Retail keeps the blank that ended `mp_brecourt` (doc 5.1).
            Some(" map mp_carentan"),
        );
    }

    /// `sv_pure` is retail's 1 on the binary's server (cod11-server-handshake.md,
    /// "The four cvars", "Defaults and order"), 0 on the test constructor, and
    /// a set holds past the first level, as the carried table keeps it.
    #[test]
    fn sv_pure_defaults_to_retails_and_a_set_outlives_the_level() {
        let fs = vcod_common::pk3::Pk3Fs::empty();
        let now = Instant::now();
        assert_eq!(
            Server::with_seed(cfg(), now, 7).cvars(&fs).get("sv_pure"),
            "1"
        );
        let mut sv = Server::new(cfg(), now);
        assert_eq!(sv.cvars(&fs).get("sv_pure"), "0");
        sv.set_cvar("sv_pure", "1");
        let first = sv.cvars(&fs);
        assert_eq!(first.get("sv_pure"), "1");
        sv.game.carried_cvars = Some(first);
        assert_eq!(sv.cvars(&fs).get("sv_pure"), "1");
    }
}
