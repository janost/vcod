//! `VCOD_SOAK=<path>`: the CoD1 (1.1, 1.5, UO) client soak's player
//! (docs/research/cod11-client-soak.md). A joined client plays as a
//! person would through `main`'s whole frame: it wanders (forward, a new
//! heading every few seconds or when held up, a jump when stuck), fires
//! in bursts, aims down the sight, reloads, crouches and goes prone now
//! and then, switches weapons, taps use (a UO vehicle's seat), opens the
//! scoreboard for 1.5 s every 15 to 30 s and the Esc menu for 2 s every
//! 25 to 45 s, and taps fire while dead. It writes one line per event to
//! `<path>`, each stamped with `frame_prof`'s frame number so
//! `tools/cod1_soak.py` can join it with `VCOD_FRAME_PROFILE`'s stages:
//! the level (map and gametype) and the resident set at its start and
//! every 5 s, spawns, deaths, the first in-game frame of a level, what the
//! player did, and audio misses. `tools/cod1_soak.sh` runs it beside a
//! vcod-server with six shooting bots.

use std::io::Write as _;

use glam::Vec3;

use super::input::{Action, PlayInput};
use vcod_common::net::msg::PlayerState;
use vcod_common::net::protocol::Protocol;

/// What the soak asks of `App` once the frame's borrows end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    /// `+scores` / `-scores`.
    Scores(bool),
    /// Esc: opens the in-game menu, or closes the one up.
    Esc,
}

/// The soak's player and its log.
pub struct Soak {
    out: std::io::BufWriter<std::fs::File>,
    rng: u64,
    /// `mapname gametype` of the level under way.
    level: String,
    level_at: f32,
    /// A snapshot seen since the level's gamestate.
    in_game: bool,
    alive: bool,
    /// Where the feet were when they last moved, and when.
    moved: (Vec3, f32),
    next_turn: f32,
    /// Degrees of yaw still to turn, spent a little each frame.
    turn_left: f32,
    next_rss: f32,
    next_scores: f32,
    scores_until: f32,
    next_menu: f32,
    menu_until: Option<f32>,
    next_stance: f32,
    stance_until: f32,
    stance: Action,
    next_switch: f32,
    next_reload: f32,
    next_use: f32,
    ads_until: f32,
    next_ads: f32,
    asks: Vec<Ask>,
    audio_misses: u64,
}

/// pmove's `pm_type`s the soak reads (CoD1's numbering).
const PM_NORMAL_LINKED: i32 = 1;
const PM_DEAD: i32 = 6;

impl Soak {
    /// The soak when `VCOD_SOAK` names a file it can create.
    pub fn from_env() -> Option<Soak> {
        let path = std::env::var_os("VCOD_SOAK")?;
        let f = match std::fs::File::create(&path) {
            Ok(f) => f,
            Err(e) => {
                log::warn!("VCOD_SOAK {}: {e}", path.to_string_lossy());
                return None;
            }
        };
        let seed = std::env::var("VCOD_SOAK_SEED")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0x9e37_79b9_7f4a_7c15u64);
        Some(Soak {
            out: std::io::BufWriter::new(f),
            rng: seed | 1,
            level: String::new(),
            level_at: 0.0,
            in_game: false,
            alive: false,
            moved: (Vec3::ZERO, 0.0),
            next_turn: 0.0,
            turn_left: 0.0,
            next_rss: 0.0,
            next_scores: 0.0,
            scores_until: 0.0,
            next_menu: 0.0,
            menu_until: None,
            next_stance: 0.0,
            stance_until: 0.0,
            stance: Action::Crouch,
            next_switch: 0.0,
            next_reload: 0.0,
            next_use: 0.0,
            ads_until: 0.0,
            next_ads: 0.0,
            asks: Vec::new(),
            audio_misses: 0,
        })
    }

    fn rand(&mut self) -> u32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 32) as u32
    }

    /// `lo..hi` seconds.
    fn secs(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * (self.rand() % 1000) as f32 / 1000.0
    }

    fn line(&mut self, what: &str) {
        let _ = writeln!(self.out, "{} {what}", crate::frame_prof::frame());
    }

    /// The frame's input, decided before the cmds are built. `ps` is the
    /// newest snapshot's playerstate, `serverinfo` configstring 0, `feet`
    /// the drawn origin, `menu_up` whether a menu holds the keys.
    #[allow(clippy::too_many_arguments)]
    pub fn act(
        &mut self,
        input: &mut PlayInput,
        ps: Option<&PlayerState>,
        p: &Protocol,
        serverinfo: &str,
        feet: Vec3,
        time: f32,
        menu_up: bool,
    ) {
        let info = |k| vcod_common::net::info_value_for_key(serverinfo, k).unwrap_or("");
        let level = format!("{} {}", info("mapname"), info("g_gametype"));
        if level != self.level && !info("mapname").is_empty() {
            self.level = level;
            self.level_at = time;
            self.in_game = false;
            self.alive = false;
            input.release_all();
            let wall = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0.0, |d| d.as_secs_f64());
            let line = format!(
                "level {} {:.1} rss {} wall {wall:.1}",
                self.level,
                time,
                rss_mb()
            );
            self.line(&line);
        }
        if time >= self.next_rss {
            self.next_rss = time + 5.0;
            let line = format!("rss {:.1} {}", time, rss_mb());
            self.line(&line);
            let _ = self.out.flush();
        }
        let Some(ps) = ps else {
            return;
        };
        if !self.in_game {
            self.in_game = true;
            let line = format!("first {:.1}", time - self.level_at);
            self.line(&line);
        }
        // The Esc menu goes again after two seconds.
        if let Some(until) = self.menu_until {
            if time >= until {
                self.menu_until = None;
                if menu_up {
                    self.asks.push(Ask::Esc);
                }
            }
            return;
        }
        let pm_type = ps.field_i32(p, "pm_type");
        let alive = pm_type <= PM_NORMAL_LINKED && ps.health() > 0;
        if alive && !self.alive {
            self.alive = true;
            self.line("spawn");
            self.moved = (feet, time);
            self.next_turn = time + self.secs(2.0, 6.0);
            self.next_scores = time + self.secs(6.0, 20.0);
            self.next_menu = time + self.secs(10.0, 40.0);
            self.next_stance = time + self.secs(5.0, 15.0);
            self.next_switch = time + self.secs(8.0, 20.0);
            self.next_reload = time + self.secs(10.0, 25.0);
            self.next_use = time + self.secs(5.0, 15.0);
            self.next_ads = time + self.secs(3.0, 10.0);
        }
        if !alive {
            if self.alive {
                self.alive = false;
                if pm_type >= PM_DEAD || ps.health() <= 0 {
                    self.line("death");
                }
                input.release_all();
                if self.scores_until > 0.0 {
                    self.scores_until = 0.0;
                    self.asks.push(Ask::Scores(false));
                }
            }
            // A respawn, or the next player followed.
            if !menu_up {
                input.key(Action::Attack, (time * 1000.0) as u32 % 1000 < 100);
            }
            return;
        }
        if menu_up {
            input.release_all();
            return;
        }
        if feet.distance(self.moved.0) > 16.0 {
            self.moved = (feet, time);
        }
        let still = time - self.moved.1;
        // A new heading now and then, and when held up.
        if time >= self.next_turn || (still > 1.5 && self.turn_left == 0.0) {
            self.next_turn = time + self.secs(2.0, 6.0);
            self.turn_left = self.secs(-180.0, 180.0);
            if still > 1.5 {
                self.moved = (feet, time);
            }
        }
        let step = self.turn_left.clamp(-6.0, 6.0);
        self.turn_left -= step;
        if self.turn_left.abs() < 0.01 {
            self.turn_left = 0.0;
        }
        // Level the view slowly while turning.
        input.mouse([0.0, step]);
        let ms = (time * 1000.0) as u32;
        input.key(Action::Forward, true);
        input.key(Action::Jump, still > 1.0 && ms % 700 < 50);
        input.key(Action::Attack, ms % 1500 < 250);
        if time >= self.next_ads {
            self.next_ads = time + self.secs(6.0, 15.0);
            self.ads_until = time + self.secs(1.0, 3.0);
            self.line("ads");
        }
        input.key(Action::Ads, time < self.ads_until);
        if time >= self.next_stance {
            self.next_stance = time + self.secs(10.0, 25.0);
            self.stance_until = time + self.secs(1.0, 4.0);
            self.stance = if self.rand().is_multiple_of(3) {
                Action::Prone
            } else {
                Action::Crouch
            };
            input.key(self.stance, true);
            input.key(self.stance, false);
            self.line("stance");
        } else if self.stance_until > 0.0 && time >= self.stance_until {
            self.stance_until = 0.0;
            // `+gostand`.
            input.key(Action::Jump, true);
            input.key(Action::Jump, false);
        }
        if time >= self.next_switch {
            self.next_switch = time + self.secs(8.0, 20.0);
            let slot = 1 + (self.rand() % 4) as usize;
            input.key(Action::Slot(slot), true);
            input.key(Action::Slot(slot), false);
            self.line("switch");
        }
        input.key(Action::Reload, time >= self.next_reload);
        if time >= self.next_reload + 0.1 {
            self.next_reload = time + self.secs(10.0, 25.0);
            self.line("reload");
        }
        input.key(
            Action::Use,
            time >= self.next_use && time < self.next_use + 0.2,
        );
        if time >= self.next_use + 0.2 {
            self.next_use = time + self.secs(5.0, 15.0);
            self.line("use");
        }
        if time >= self.next_scores {
            self.next_scores = time + self.secs(15.0, 30.0);
            self.scores_until = time + 1.5;
            self.asks.push(Ask::Scores(true));
            self.line("board");
        } else if self.scores_until > 0.0 && time >= self.scores_until {
            self.scores_until = 0.0;
            self.asks.push(Ask::Scores(false));
        }
        if time >= self.next_menu {
            self.next_menu = time + self.secs(25.0, 45.0);
            input.release_all();
            self.menu_until = Some(time + 2.0);
            self.asks.push(Ask::Esc);
            self.line("esc");
        }
    }

    /// What `App` should do now that the frame's borrows ended.
    pub fn take_asks(&mut self) -> Vec<Ask> {
        std::mem::take(&mut self.asks)
    }

    /// The frame's end: the audio misses since the last.
    pub fn end_frame(&mut self, audio_misses: u64) {
        if audio_misses > self.audio_misses {
            let line = format!("audio_miss {}", audio_misses - self.audio_misses);
            self.line(&line);
        }
        self.audio_misses = audio_misses;
    }

    /// At exit.
    pub fn flush(&mut self) {
        let _ = self.out.flush();
    }
}

/// The process's resident set in MB.
fn rss_mb() -> u64 {
    std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok())
        .map_or(0, |pages| pages * 4096 / (1 << 20))
}
