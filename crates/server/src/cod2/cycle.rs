//! The map cycle of a running CoD2 server (cod2-map-cycle.md): `map` and
//! `devmap`, `map_restart`, `fast_restart` (the script's
//! `map_restart(persist)`), `map_rotate` over `sv_mapRotation` (the
//! script's `exitLevel`). Addresses are cod2_lnxded's.

use std::rc::Rc;
use vcod_common::asset_source::{Asset, AssetSource, RawFile};

use vcod_common::pk3::Pk3Fs;

use super::{Cod2Server, State};
use crate::game::cod2::{Cod2Init, Cod2Level};

/// `sv_serverid` after `SV_SpawnServer` (0x8093520): the high nibble steps,
/// and steps again past a wrap to 0, the low nibble kept. VERIFIED.
pub fn spawn_server_id(id: i32) -> i32 {
    let mut b = (id as u8).wrapping_add(0x10);
    if b & 0xf0 == 0 {
        b = b.wrapping_add(0x10);
    }
    i32::from(b)
}

/// `sv_serverid` after a `fast_restart` (`SV_MapRestart` 0x808be3e): the
/// low nibble steps, the high one kept. VERIFIED.
pub fn restart_server_id(id: i32) -> i32 {
    ((id + 1) & 0xf) + (id & 0xf0)
}

/// What `SV_MapRotate_f` (0x808c132) does with `sv_mapRotationCurrent`
/// (refilled from `sv_mapRotation` when empty or used up): the gametypes it
/// sets on the way and the map it ends in, `None` for a `fast_restart`.
/// The rest of the rotation is what `sv_mapRotationCurrent` keeps.
#[derive(Debug, PartialEq)]
pub struct Rotation {
    pub gametype: Option<String>,
    pub map: Option<String>,
    pub rest: String,
    /// The ending's line when there is no map (cod2-map-cycle.md 1.3).
    pub warning: Option<&'static str>,
}

/// `SV_MapRotate_f`'s walk (`crate::engine::rotation`), each skipped word
/// logged. VERIFIED, the branches and the strings.
pub fn rotate(rotation: &str, current: &str) -> Rotation {
    use crate::engine::rotation::{End, Words};
    let mut rest = current.to_string();
    let walk = crate::engine::rotation::rotate(rotation, &mut rest, Words::Cod2);
    for w in &walk.unknown {
        log::info!("Unknown keyword '{w}' in sv_mapRotation.");
    }
    Rotation {
        gametype: walk.gametype,
        warning: walk.end.warning(),
        map: match walk.end {
            End::Map(m) => Some(m),
            _ => None,
        },
        rest,
    }
}

/// `SV_SpawnServer`'s `NET_Sleep(0xfa)` after `loadingnewmap`. VERIFIED.
pub const SPAWN_SLEEP_MS: u64 = 250;

impl Cod2Server {
    /// `default_mp.cfg`'s `set` lines into the dvars, as the engine execs
    /// it at startup before the command line's: the stock `scr_*` values
    /// (`scr_dm_scorelimit` 50 where `dm.gsc` defaults to 100; retail's
    /// dm sent `MP_GAIN_POINTS_BY_ELIMINATING` with 50, cod2-map-cycle.md
    /// 2). Its `g_gametype random` gives way to the config's gametype.
    pub fn exec_default_cfg(&mut self, fs: &dyn AssetSource) {
        let Ok(raw) = fs.load::<RawFile>("default_mp.cfg") else {
            log::warn!("default_mp.cfg not in the iwds; stock scr_* defaults lost");
            return;
        };
        let n = self.game.cvars.exec_cfg(&raw.text());
        log::debug!("default_mp.cfg: {n} dvars set");
        let gametype = self.game.cfg.gametype.clone();
        self.set_cvar("g_gametype", &gametype);
    }

    /// `SV_SpawnServer` over the running server (0x8093520): the OOB
    /// `loadingnewmap` to every client with a gamestate, the level's
    /// `savePersist` carried, a new checksum feed, `sv_serverid`'s high
    /// nibble stepped, the load, and every client back to `CS_CONNECTED`
    /// through `ClientConnect`. Each gets its gamestate when its next
    /// message carries the old id (`client_packet`).
    pub(super) fn spawn_server(&mut self, map: &str) -> anyhow::Result<()> {
        let (carry, rigs) = match self.game.level.as_ref() {
            Some(level) => {
                // One dvar table on retail: what the script set stays set.
                self.game.cvars = level.rt.host.cvars.clone();
                (
                    level.take_carry(false),
                    level.carried_rigs(self.game.fs.as_ref()),
                )
            }
            None => Default::default(),
        };
        let gametype = self.game.cvars.get("g_gametype").to_string();
        let line = format!("loadingnewmap\n{map}\n{gametype}");
        let to: Vec<_> = self
            .core
            .clients
            .iter()
            .flatten()
            // A bot has no address to tell (`NA_BOT`).
            .filter(|c| c.state > State::Connected && !c.bot)
            .map(|c| c.link.addr)
            .collect();
        for addr in to {
            self.send_oob(addr, &line);
        }
        // `NET_Sleep(250)` past the loop, with or without a client to tell
        // (0x8093520). The lines go out first, as `NET_OutOfBandPrint`
        // writes the socket.
        if let Some(net) = self.game.net_now.as_mut() {
            for (to, pkt) in self.core.outbox.drain(..) {
                (net.send)(to, &pkt);
            }
            (net.sleep)(std::time::Duration::from_millis(SPAWN_SLEEP_MS));
        }
        let new_map = self.game.cfg.map != map;
        self.game.cfg.map = map.to_string();
        self.game.cvars.set("mapname", map);
        self.shutdown_game_log();
        self.game.level = None;
        self.core.configstrings = vec![String::new(); vcod_cod2::net::ops::MAX_CONFIGSTRINGS];
        self.game.baselines.clear();
        self.core.checksum_feed = self.rand();
        let fs = self.game.fs.clone();
        self.load_with(
            fs,
            Cod2Init {
                carry,
                rigs,
                ..Default::default()
            },
        )?;
        for slot in 0..self.core.clients.len() {
            // A zombie is no client to carry over (`state < CS_CONNECTED`).
            if self.core.clients[slot]
                .as_ref()
                .is_none_or(|c| c.state == State::Zombie)
            {
                continue;
            }
            self.core.clients[slot].as_mut().unwrap().state = State::Connected;
            self.connect_level(slot);
        }
        if let Some(b) = self.game.bots.as_mut() {
            b.new_level(new_map);
        }
        Ok(())
    }

    /// `SV_MapRestart(1)` (0x808be3e), `fast_restart` and the script's
    /// `map_restart`: with the gametype unchanged the level restarts in
    /// place, `sv_serverid`'s low nibble stepped, three 100 ms frames, then
    /// for every client `B` (no `savePersist`) or `n`, `ClientConnect` and
    /// for one in the world `ClientBegin`. No gamestate: the configstrings
    /// the restart wrote go out as `d` and its server commands as they
    /// are, ahead of the `B`, the new id as `d 1` on the next frame. A changed gametype spawns the server
    /// instead, `savePersist` dropped.
    pub(super) fn fast_restart(&mut self) -> anyhow::Result<()> {
        let Some(level) = self.game.level.as_ref() else {
            let map = self.game.cfg.map.clone();
            return self.spawn_server(&map);
        };
        self.game.cvars = level.rt.host.cvars.clone();
        if !self
            .game
            .cvars
            .get("g_gametype")
            .eq_ignore_ascii_case(&self.game.cfg.gametype)
        {
            if let Some(level) = self.game.level.as_mut() {
                level.rt.host.save_persist = false;
            }
            let map = self.game.cfg.map.clone();
            return self.spawn_server(&map);
        }
        let persist = level.save_persist();
        let carry = level.take_carry(true);
        let rigs = level.carried_rigs(self.game.fs.as_ref());
        let configstrings = level.rt.configstrings().to_vec();
        self.game.server_id = restart_server_id(self.game.server_id);
        self.game.snap_flags ^= 4;
        self.shutdown_game_log();
        self.game.level = None;
        let fs = self.game.fs.clone();
        self.load_level(
            fs,
            Cod2Init {
                carry,
                configstrings: Some(configstrings),
                rigs,
                ..Default::default()
            },
        )?;
        // What the restart's `G_InitGame` and frames wrote and sent goes
        // out ahead of the `n` (retail's sd capture: `d 13`, `d 3`, `d 12`,
        // `H 0`, `G 0`, `n`).
        self.flush_game_commands();
        if let Some(b) = self.game.bots.as_mut() {
            b.new_level(false);
        }
        let cmd = if persist { "n" } else { "B" };
        for slot in 0..self.core.clients.len() {
            // Only a client past `CS_CONNECTED`: a zombie (a drop still
            // draining) gets no `ClientConnect`, or the new level's
            // `connecting` waiters wake for it first (cod2-protocol.md 15.7).
            let Some(c) = self.core.clients[slot]
                .as_ref()
                .filter(|c| c.state != State::Zombie)
            else {
                continue;
            };
            let active = c.state == State::Active;
            self.send_server_command(slot, cmd);
            self.connect_level(slot);
            if active && let Some(level) = self.game.level.as_mut() {
                level.begin(slot);
            }
        }
        self.write_engine_configstrings();
        Ok(())
    }

    /// `map_rotate` (`SV_MapRotate_f`): the rotation's gametype into
    /// `g_gametype` (dropping `savePersist` when it changes), then `map`
    /// of its map, else a `fast_restart`.
    pub(super) fn map_rotate(&mut self) -> anyhow::Result<()> {
        let r = rotate(
            self.game.cvars.get("sv_mapRotation"),
            self.game.cvars.get("sv_mapRotationCurrent"),
        );
        self.set_cvar("sv_mapRotationCurrent", &r.rest);
        if let Some(g) = &r.gametype {
            if !g.eq_ignore_ascii_case(self.game.cvars.get("g_gametype"))
                && let Some(level) = self.game.level.as_mut()
            {
                level.rt.host.save_persist = false;
            }
            self.set_cvar("g_gametype", g);
        }
        match r.map {
            Some(map) => self.map_command(&map, false),
            None => {
                log::info!("{}", r.warning.unwrap_or_default());
                self.fast_restart()
            }
        }
    }

    /// `map` / `devmap` (`SV_Map_f` 0x808bd46): the lowered name's
    /// `.d3dbsp` must read, then `SV_SpawnServer`, then `sv_cheats` 1 for
    /// `devmap` and 0 for `map`. VERIFIED.
    pub(super) fn map_command(&mut self, name: &str, devmap: bool) -> anyhow::Result<()> {
        let map = name.to_ascii_lowercase();
        let path = format!("maps/mp/{map}.d3dbsp");
        if !self
            .game
            .fs
            .as_ref()
            .is_some_and(|fs| fs.read(&path).is_some())
        {
            log::info!("Can't find map {path}");
            return Ok(());
        }
        self.spawn_server(&map)?;
        self.set_cvar("sv_cheats", if devmap { "1" } else { "0" });
        Ok(())
    }

    /// `map_restart` (`SV_MapRestart(0)`): `savePersist` dropped and the
    /// current map spawned again, a full load with a new gamestate.
    pub(super) fn map_restart(&mut self) -> anyhow::Result<()> {
        if let Some(level) = self.game.level.as_mut() {
            level.rt.host.save_persist = false;
        }
        let map = self.game.cfg.map.clone();
        self.spawn_server(&map)
    }

    /// `ClientConnect` for `slot` on the loaded level: its name off the
    /// userinfo, the spectator at the intermission spot.
    pub(super) fn connect_level(&mut self, slot: usize) {
        let (origin, angles) = self.game.spot;
        let Some(c) = self.core.clients[slot].as_ref() else {
            return;
        };
        let name = vcod_common::net::info_value_for_key(&c.link.userinfo, "name")
            .unwrap_or("")
            .to_string();
        let bot = c.bot;
        if let Some(level) = self.game.level.as_mut() {
            level.rt.host.client_names[slot] = name.clone();
            level.connect_at(slot, &name, origin, angles);
            if bot {
                level.mark_bot(slot);
            }
        }
    }

    /// The level under `self.game.cfg` with `init`, its spawn frames run, the
    /// aborts logged; what [`Cod2Server::load`] and a `fast_restart` share.
    pub(super) fn load_level(
        &mut self,
        fs: Option<Rc<Pk3Fs>>,
        init: Cod2Init,
    ) -> anyhow::Result<()> {
        use anyhow::Context;
        let Some(fs) = fs else {
            return Ok(());
        };
        let mut laps = vcod_common::laps::Laps::new(format!("cod2 level {}", self.game.cfg.map));
        let file = vcod_cod2::map::MapFile::find(&*fs, &self.game.cfg.map)
            .with_context(|| format!("map {} not loaded", self.game.cfg.map))?;
        laps.lap("read and parse");
        let world = file.world();
        laps.lap("world");
        self.game.spot = super::intermission_spot(&world);
        let overlay =
            self.game.cfg.gametype_script.clone().filter(|(stem, _)| {
                format!("maps/mp/gametypes/{}", self.game.cfg.gametype) == *stem
            });
        // `G_InitGame`'s open, at the new `level.time` (cod11-map-cycle.md 9).
        let game = match self.game.cvars.get("fs_game") {
            "" => fs
                .dirs()
                .last()
                .and_then(|d| d.file_name())
                .map_or("main".to_string(), |n| n.to_string_lossy().into_owned()),
            g => g.to_string(),
        };
        let info = self.serverinfo();
        self.game
            .game_log
            .init(&self.game.cvars, &game, &info, self.core.sv_time_ms);
        // `G_InitGame`'s `srand` and the script's generator share one seed:
        // retail's wall clock in ms, here a draw (cod2-combat.md 9.6).
        let mut init = init;
        init.carry.seed = Some(u64::from(self.game_seed()));
        let mut level = Cod2Level::load_map_with(
            fs,
            &file,
            &world,
            &self.game.cfg.map,
            &self.game.cfg.gametype,
            self.game.cvars.clone(),
            self.core.sv_time_ms,
            Cod2Init { overlay, ..init },
        )
        .context("loading the level")?;
        laps.lap("level");
        for _ in 0..super::SPAWN_FRAMES {
            self.core.sv_time_ms += super::SPAWN_FRAME_MS;
            level.rt.script_lag_ms += super::SPAWN_FRAME_MS - super::FRAME_MS;
            level.run_frame(self.core.sv_time_ms);
        }
        laps.lap("spawn frames");
        laps.finish();
        for e in level.rt.aborts() {
            log::warn!("cod2 script: {e}");
        }
        self.game.level = Some(level);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_server_id_steps_as_retail_s() {
        assert_eq!(spawn_server_id(0), 16);
        assert_eq!(spawn_server_id(17), 33);
        assert_eq!(spawn_server_id(0xf3), 0x13);
        assert_eq!(restart_server_id(16), 17);
        assert_eq!(restart_server_id(0x1f), 0x10);
    }

    #[test]
    fn the_rotation_walks_gametypes_to_a_map() {
        let rot = "gametype tdm map mp_toujane gametype sd map mp_carentan";
        let r = rotate(rot, "");
        assert_eq!(r.gametype.as_deref(), Some("tdm"));
        assert_eq!(r.map.as_deref(), Some("mp_toujane"));
        let r2 = rotate(rot, &r.rest);
        assert_eq!(
            (r2.gametype.as_deref(), r2.map.as_deref()),
            (Some("sd"), Some("mp_carentan"))
        );
        // Used up: the rotation starts over.
        let r3 = rotate(rot, &r2.rest);
        assert_eq!(r3.map.as_deref(), Some("mp_toujane"));
        // No rotation: a fast_restart.
        assert_eq!(rotate("", "").map, None);
        assert_eq!(rotate("bogus map", "").map, None);
    }

    /// Each ending logs its own line (cod2_lnxded 0x81494e0, 0x8149540,
    /// 0x81495c0); a map logs none.
    #[test]
    fn each_ending_has_retail_s_line() {
        let warn = |rot: &str| rotate(rot, "").warning;
        assert_eq!(
            warn("bogus"),
            Some("No map specified in sv_mapRotation - forcing map_restart.")
        );
        assert_eq!(
            warn("gametype"),
            Some(
                "No gametype specified after 'gametype' keyword in sv_mapRotation - forcing map_restart."
            )
        );
        assert_eq!(
            warn("gametype tdm map"),
            Some("No map specified after 'map' keyword in sv_mapRotation - forcing map_restart.")
        );
        assert_eq!(warn("map mp_toujane"), None);
    }
}
