//! The map cycle of a running CoD4 1.7 server (docs/research/
//! cod4-map-cycle.md): `map` and `devmap` (`SV_Map_f`), `map_restart`
//! and `fast_restart` (`SV_MapRestart` 0x816c306), `map_rotate` over
//! `sv_mapRotation` (`SV_MapRotate_f` 0x816dae2, the script's
//! `exitLevel`) and the spawn they end in (`SV_SpawnServer` 0x81748da).

use super::{Cod4Server, SNAPFLAG_SERVERCOUNT, SPAWN_FRAME_MS, SPAWN_FRAMES, State, client_name};
use crate::rcon::Redirect;

/// `SV_SpawnServer`'s `NET_Sleep(0xfa)` after `loadingnewmap`
/// (0x8175253). VERIFIED.
const SPAWN_SLEEP_MS: u64 = 250;

/// `sv_serverid` after a `fast_restart`: the low nibble steps, the high
/// one kept (0x816c466).
pub(super) fn restart_server_id(id: i32) -> i32 {
    ((id + 1) & 0xf) + (id & 0xf0)
}

impl Cod4Server {
    /// `SV_SpawnServer` over the running server: `loadingnewmap\n<map>\n
    /// <gametype>` to every client past `CS_CONNECTED` (0x81751f0), on the
    /// socket at once, then a 250 ms wait, the
    /// level's `savePersist` carried, the table emptied, a new checksum
    /// feed, the load, then every client back to `CS_CONNECTED` through
    /// `ClientConnect`, its stats asked for again with `requeststats`
    /// (0x8174ee7). Each gets the gamestate once its seven stats packets
    /// are in.
    pub(super) fn spawn_server(&mut self, map: &str) -> anyhow::Result<()> {
        let carry = self
            .game
            .level
            .as_ref()
            .map(|l| l.take_carry(false))
            .unwrap_or_default();
        // `SV_SetGametype`: the latched `g_gametype` takes.
        let gametype = self.game.cvars().get("g_gametype").to_string();
        if !gametype.is_empty() {
            self.game.cfg.gametype = gametype.clone();
        }
        let line = format!("loadingnewmap\n{map}\n{}", self.game.cfg.gametype);
        let to: Vec<_> = self
            .core
            .clients
            .iter()
            .flatten()
            .filter(|c| c.state >= State::Primed)
            .map(|c| c.link.addr)
            .collect();
        for addr in to {
            self.send_oob(addr, &line);
        }
        // `NET_Sleep(250)` past the loop, with or without a client to tell
        // (0x817525a). The lines go out first, as `NET_OutOfBandPrint`
        // writes the socket (0x813d066).
        if let Some(net) = self.game.net_now.as_mut() {
            for (to, pkt) in self.core.outbox.drain(..) {
                (net.send)(to, &pkt);
            }
            (net.sleep)(std::time::Duration::from_millis(SPAWN_SLEEP_MS));
        }
        log::info!("cod4: spawning {map} {}", self.game.cfg.gametype);
        self.game.cfg.map = map.to_string();
        self.game.cvars_mut().set("mapname", map);
        self.core.configstrings = vec![String::new(); self.core.configstrings.len()];
        self.game.baselines.clear();
        self.core.checksum_feed = self.rand();
        let carry = crate::game::script::Carry {
            seed: Some(self.rand() as u32 as u64),
            ..carry
        };
        self.load_level(carry)?;
        for slot in 0..self.core.clients.len() {
            // A zombie is no client to carry over (`state < CS_CONNECTED`).
            let Some(c) = self.core.clients[slot]
                .as_mut()
                .filter(|c| c.state != State::Zombie)
            else {
                continue;
            };
            c.state = State::Connected;
            c.stat_packets = 0;
            c.pure = None;
            let (addr, name, guid) = (c.link.addr, client_name(&c.link.userinfo), c.guid.clone());
            let spot = self.game.spot;
            if let Some(level) = self.game.level.as_mut() {
                level.connect(slot, &name, spot);
                level.set_guid(slot, &guid);
            }
            self.send_oob(addr, "requeststats\n");
        }
        Ok(())
    }

    /// `SV_MapRestart(1)`, `fast_restart` and the script's `map_restart`:
    /// with the gametype unchanged the level restarts in place (0x816c3cf),
    /// once a frame (0x816c3dc): `fastrestart` to every client past
    /// `CS_ZOMBIE`, the snapshot bit and `sv_serverid`'s low nibble
    /// stepped, the scripts run again over the kept configstrings, three
    /// 100 ms frames, then per client `n` (`savePersist`) or `B`,
    /// `ClientConnect` and, for one in the world, `ClientBegin`. No
    /// gamestate: the restart's configstrings go out as `d` ahead of the
    /// `B`, the new id as `d 1` on the next frame. A changed gametype, or
    /// `fast` false (`map_restart`), spawns the map again with
    /// `savePersist` dropped.
    pub(super) fn map_restart(&mut self, fast: bool) -> anyhow::Result<()> {
        let gametype = self.game.cvars().get("g_gametype").to_string();
        let same_type = gametype.eq_ignore_ascii_case(&self.game.cfg.gametype);
        let (Some(zones), Some(install)) = (self.game.zones4.clone(), self.game.install.clone())
        else {
            let map = self.game.cfg.map.clone();
            return self.spawn_server(&map);
        };
        if !fast || !same_type || self.game.level.is_none() {
            if let Some(level) = self.game.level.as_mut() {
                level.rt.host.save_persist = false;
            }
            let map = self.game.cfg.map.clone();
            return self.spawn_server(&map);
        }
        if self.game.start_frame == Some(self.core.sv_time_ms) {
            return Ok(());
        }
        let to: Vec<_> = self
            .core
            .clients
            .iter()
            .flatten()
            .filter(|c| c.state > State::Zombie)
            .map(|c| c.link.addr)
            .collect();
        for addr in to {
            self.send_oob(addr, "fastrestart");
        }
        // `G_ShutdownGame` and `G_InitGame` both run, so the log reads the
        // pair as at a map change.
        self.shutdown_game_log();
        let mut old = self.game.level.take().unwrap();
        let persist = old.rt.host.save_persist;
        let carry = crate::game::script::Carry {
            seed: Some(self.rand() as u32 as u64),
            ..old.take_carry(true)
        };
        let configstrings = old.rt.configstrings().to_vec();
        let cvars = std::mem::take(&mut old.rt.host.cvars);
        drop(old);
        self.game.snap_flags ^= SNAPFLAG_SERVERCOUNT;
        self.game.server_id = restart_server_id(self.game.server_id);
        self.game.start_frame = Some(self.core.sv_time_ms);
        let (map, gametype) = (self.game.cfg.map.clone(), self.game.cfg.gametype.clone());
        let seed = self.game_seed();
        self.game.cvars = cvars;
        let info = self.serverinfo();
        self.init_game_log(&info, self.core.sv_time_ms);
        let cvars = std::mem::take(&mut self.game.cvars);
        let mut level = crate::game::cod4::level::Cod4Level::load_with(
            &zones,
            &install,
            &map,
            &gametype,
            cvars,
            false,
            self.game.script_overrides.clone(),
            self.core.sv_time_ms,
            carry,
            Some(configstrings),
            seed,
        )?;
        level
            .rt
            .host
            .cvars
            .set("sv_serverid", &self.game.server_id.to_string());
        let frames = crate::tick_profile::phase("console/restart frames");
        for _ in 0..SPAWN_FRAMES {
            self.core.sv_time_ms += SPAWN_FRAME_MS;
            level.rt.set_network_time(self.core.sv_time_ms);
            level.run_frame(self.core.sv_time_ms);
        }
        for e in level.rt.aborts() {
            log::warn!("cod4 script: {e}");
        }
        drop(frames);
        self.game.level = Some(level);
        // What the restart's `G_InitGame` and frames wrote and sent goes
        // out ahead of the `B`.
        self.flush_game_commands();
        let cmd = if persist { "n" } else { "B" };
        let spot = self.game.spot;
        for slot in 0..self.core.clients.len() {
            let Some(c) = self.core.clients[slot]
                .as_ref()
                .filter(|c| c.state > State::Zombie)
            else {
                continue;
            };
            let (active, name, stats, guid) = (
                c.state == State::Active,
                client_name(&c.link.userinfo),
                (c.stat_packets == 0x7f).then(|| c.stats.clone()),
                c.guid.clone(),
            );
            self.send_server_command(slot, cmd);
            if let Some(level) = self.game.level.as_mut() {
                level.connect(slot, &name, spot);
                level.set_guid(slot, &guid);
                if let Some(stats) = &stats {
                    level.set_stats(slot, stats);
                }
                if active {
                    level.begin(slot);
                }
            }
        }
        log::info!("cod4: {map} {gametype} restarted");
        self.write_engine_configstrings();
        Ok(())
    }

    /// `map_rotate` (`SV_MapRotate_f` 0x816dae2): the rotation's
    /// gametype into `g_gametype` (dropping `savePersist` when it
    /// changes), then `map` of its map, else a `fast_restart`. The walk
    /// and its lines are CoD2's (`crate::engine::rotation`), the strings
    /// 1.7's own.
    pub(super) fn map_rotate(&mut self, out: &mut Redirect) -> anyhow::Result<()> {
        use crate::engine::rotation::{End, Words};
        let rotation = self.game.cvars().get("sv_mapRotation").to_string();
        let mut current = self.game.cvars().get("sv_mapRotationCurrent").to_string();
        out.print("map_rotate...\n\n");
        out.print(&format!("\"sv_mapRotation\" is:\"{rotation}\"\n\n"));
        out.print(&format!("\"sv_mapRotationCurrent\" is:\"{current}\"\n\n"));
        let walk = crate::engine::rotation::rotate(&rotation, &mut current, Words::Cod2);
        self.set_cvar("sv_mapRotationCurrent", &current);
        for w in &walk.unknown {
            out.print(&format!("Unknown keyword '{w}' in sv_mapRotation.\n"));
        }
        if let Some(g) = &walk.gametype {
            out.print(&format!("Setting g_gametype: {g}.\n"));
            if !g.eq_ignore_ascii_case(self.game.cvars().get("g_gametype"))
                && let Some(level) = self.game.level.as_mut()
            {
                level.rt.host.save_persist = false;
            }
            self.set_cvar("g_gametype", g);
        }
        match walk.end {
            End::Map(map) => {
                out.print(&format!("Setting map: {map}.\n"));
                self.map_command(&map, false, out)
            }
            end => {
                out.print(&format!("{}\n", end.warning().unwrap_or_default()));
                self.map_restart(true)
            }
        }
    }

    /// `map` / `devmap` (`SV_Map_f`): the lowered name's zone must exist,
    /// then `SV_SpawnServer`, then `sv_cheats` 1 for `devmap` and 0 for
    /// `map`.
    pub(super) fn map_command(
        &mut self,
        name: &str,
        devmap: bool,
        out: &mut Redirect,
    ) -> anyhow::Result<()> {
        if name.is_empty() {
            return Ok(());
        }
        let map = name.to_ascii_lowercase();
        let exists = self
            .game
            .install
            .as_ref()
            .is_some_and(|i| i.zone("english", &map).is_file());
        if !exists {
            out.print(&format!("Can't find map \"{map}\".\n"));
            return Ok(());
        }
        self.spawn_server(&map)?;
        self.set_cvar("sv_cheats", if devmap { "1" } else { "0" });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fast_restart_steps_the_low_nibble() {
        assert_eq!(restart_server_id(16), 17);
        assert_eq!(restart_server_id(0x1f), 0x10);
        assert_eq!(super::super::spawn_server_id(0x1f), 0x2f);
    }
}
