//! The CoD2 1.3 soak, on the CoD4 soak's harness (`super::cod4_sweep`):
//! per stock map, vcod-server `--game cod2` loads it on a thread of the
//! process with six shooting bots and `sv_mapRotation` naming the map once
//! per gametype (dm, tdm, sd, ctf, hq; dm again last), each to its time limit and
//! `endMap`'s intermission, and one client
//! joins it through the session's own team and weapon answers and plays
//! it as `main`'s `Mode::Cod2` arm does: `Session::pump`, `scene`, the
//! events' sounds and effects, the view effects, `draw_view`, `ViewTail`
//! and the HUD quads, rendered at `VCOD_SOAK_RENDER_HZ` (10) into a 320 x
//! 240 target. The player walks the bots' nav graph, fires, throws a
//! frag, holds the scoreboard and opens the Esc menu
//! (`super::cod4_sweep::Player`). `soak.md` under `VCOD_SWEEP_OUT`
//! (default `tmp/cod2-soak`) counts per pair what the CoD4 soak counts
//! (docs/research/cod2-client-perf.md, "The soak").
//!
//! With `COD2_DIR=...` set:
//! `cargo run -p vcod --features testing --example client_tools -- cod2_soak`
//! (it installs the process's logger), under
//! lavapipe with `VK_DRIVER_FILES`, `WGPU_BACKEND=vulkan`,
//! `GALLIVM_PERF=nopt`. `tools/cod2_soak.sh` runs it a map a process.
//! `VCOD_SWEEP_MAPS`, `VCOD_SWEEP_GAMETYPES`, `VCOD_SOAK_MINUTES` (3),
//! `VCOD_SWEEP_LEVEL_SECS` as the CoD4 soak's. The client rig and its
//! `cod2_live_*` gates are `play/cod2_soak.rs`.

use std::net::SocketAddr;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use anyhow::Context;
use vcod_common::pk3::Pk3Fs;

use super::cod4_sweep::{
    BOTS, Game, Opts, Player, Sample, Shared, SoakOpts, SoakPair, SoakServer, Tally,
};
use crate::play::cod2_soak::{Cod2Pilot, Rig, rig, step};
use crate::play::cod4_sweep::{Pilot, warm_counts};

const GAMETYPES: [&str; 5] = ["dm", "tdm", "sd", "ctf", "hq"];

/// See the module doc.
pub fn cod2_soak() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    crate::tools::cod4_sweep::soak(&Cod2 { fs }, &GAMETYPES, "cod2-soak");
}

/// CoD2 1.3 under the run.
struct Cod2 {
    fs: Pk3Fs,
}

impl Game for Cod2 {
    /// Every `maps/mp/mp_*.d3dbsp` the stock iwds hold.
    fn stock_maps(&self) -> Vec<String> {
        let mut maps: Vec<String> = self
            .fs
            .find_maps()
            .into_iter()
            .filter(|m| m.starts_with("mp_"))
            .collect();
        maps.sort();
        maps
    }

    fn start(
        &self,
        map: &str,
        gametypes: &[String],
        opts: Opts,
    ) -> anyhow::Result<(Box<dyn SoakServer>, Vec<usize>)> {
        use vcod_server::cod2::{Cod2Config, Cod2Server};
        let fs = Rc::new(vcod_common::testing::cod2_fs().context("COD2_DIR")?);
        let mut sv = Cod2Server::new(
            Cod2Config {
                map: map.into(),
                gametype: gametypes[0].clone(),
                hostname: "soak".into(),
                max_clients: 12,
                gametype_script: None,
            },
            7,
            Instant::now(),
        );
        sv.exec_default_cfg(&*fs);
        let minutes = opts.soak.map_or(0.5, |s| s.minutes).to_string();
        let mut sets: Vec<(String, String)> = vec![("sv_pure".into(), "0".into())];
        for g in GAMETYPES {
            sets.push((format!("scr_{g}_timelimit"), minutes.clone()));
        }
        // One round of sd, as long as the level.
        sets.push(("scr_sd_roundlimit".into(), "1".into()));
        sets.push(("scr_sd_roundlength".into(), minutes));
        sets.push((
            "sv_mapRotation".into(),
            crate::tools::cod4_sweep::rotation(map, gametypes),
        ));
        for (k, v) in &sets {
            sv.set_cvar(k, v);
        }
        sv.load(Some(fs))?;
        let slots = sv.add_bots(BOTS, true);
        if let Some(b) = sv.bots_mut() {
            b.wait_for_graph();
        }
        Ok((Box::new(sv), slots))
    }

    fn play(&self, map: &str, addr: SocketAddr, shared: &Shared, opts: Opts) -> anyhow::Result<()> {
        let soak = opts
            .soak
            .context("CoD2 runs the soak only, not the sweep")?;
        play_soak(map, addr, shared, soak)
    }
}

impl SoakServer for vcod_server::cod2::Cod2Server {
    fn handle_packet(&mut self, from: SocketAddr, pkt: &[u8], now: Instant) {
        vcod_server::cod2::Cod2Server::handle_packet(self, from, pkt, now);
    }

    fn take_outgoing(&mut self) -> Vec<(SocketAddr, Vec<u8>)> {
        vcod_server::cod2::Cod2Server::take_outgoing(self)
    }

    fn tick(&mut self, now: Instant) {
        self.drain_console(now);
        self.tick_frames(now, 1);
    }

    fn frame_ms(&self) -> u64 {
        vcod_server::cod2::FRAME_MS as u64
    }

    fn server_id(&self) -> i32 {
        vcod_server::cod2::Cod2Server::server_id(self)
    }

    fn gametype(&self) -> String {
        self.cvar("g_gametype").to_string()
    }

    fn graph(&mut self) -> Option<Arc<vcod_server::nav::NavGraph>> {
        self.bots_mut().and_then(|b| b.graph().cloned())
    }

    fn sample(&mut self, slots: &[usize]) -> Option<Sample> {
        use vcod_cod2::pmove::pm_type;
        let holds: std::collections::HashMap<usize, bool> = self
            .bots_mut()
            .map(|b| {
                b.bots()
                    .iter()
                    .map(|(s, b)| {
                        let h = b.engaged()
                            || b.holds_cover()
                            || b.retreating() != 0
                            || b.on_objective();
                        (*s, h)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let level = self.level()?;
        // `endMap` puts every player in the intermission.
        let ended = level
            .players
            .iter()
            .flatten()
            .any(|p| p.ps.pm_type == pm_type::INTERMISSION);
        let bodies = slots
            .iter()
            .filter_map(|&slot| {
                let p = level.players.get(slot)?.as_ref()?;
                let live = p.ps.pm_type == pm_type::NORMAL && !ended;
                let at = live.then_some(p.ps.origin);
                Some((slot, at, holds.get(&slot).copied().unwrap_or(false)))
            })
            .collect();
        Some(Sample {
            state: if ended { "intermission" } else { "playing" }.into(),
            ended,
            errors: level.rt.aborts(),
            bodies,
        })
    }

    fn rotate(&mut self) {
        self.push_console("map_rotate");
    }
}

/// The soak's client: [`Player`] decides the frame's input, then [`step`]
/// runs the frame, `render` at most `render_hz` times a second.
fn play_soak(map: &str, addr: SocketAddr, shared: &Shared, opts: SoakOpts) -> anyhow::Result<()> {
    let mut rig = rig(map, addr)?;
    crate::frame_prof::init();
    let seed = map.bytes().fold(0x9e37_79b9_7f4a_7c15u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
    });
    let mut me = Player::new(seed);
    let start = Instant::now();
    let mut last = start;
    let every = Duration::from_secs_f32(1.0 / opts.render_hz.max(0.1));
    let mut next_render = start;
    let mut fx_ms = 0.0;
    let mut audio_was = rig.audio.stats();
    while !shared.done.load(Ordering::Relaxed) {
        let now = Instant::now();
        let dt = now.duration_since(last).as_secs_f32();
        last = now;
        let time = start.elapsed().as_secs_f64();
        let graph = shared.graph.lock().unwrap().clone();
        let mut frame = SoakPair::default();
        let Rig {
            sess,
            ui,
            shell,
            cam,
            ..
        } = &mut rig;
        let mut pilot = Cod2Pilot {
            c: sess.cod2_mut().unwrap(),
            ui,
            shell,
            start,
        };
        me.act(&mut pilot, cam, time as f32, graph.as_deref(), &mut frame);
        let warm = warm_counts(&rig.r);
        let render = now >= next_render;
        if render {
            next_render = now + every;
            frame.renders = 1;
        }
        let (parts, render_ms) = step(&mut rig, time, dt, render, &mut fx_ms)?;
        let gametype = rig.pilot(start).gametype();
        Tally {
            parts: &parts,
            render_ms,
            time: time as f32,
            level_at: me.level_at(&gametype, time as f32),
            errors: &rig.errors,
            audio: &rig.audio,
            warm: [warm, warm_counts(&rig.r)],
        }
        .add(&mut frame, &mut audio_was, shared, &gametype);
        let spent = now.elapsed();
        std::thread::sleep(Duration::from_millis(16).saturating_sub(spent));
    }
    crate::frame_prof::flush();
    rig.sess.shutdown();
    Ok(())
}
