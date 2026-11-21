//! The score list: its scroll keys, the `score` requests, the fade and the
//! board's rows and ping graph.

use vcod_cod4::hud::{self as h, Scores};
use vcod_cod4::net::parse::Snapshot;

use super::assets::Loaded;
use super::text::text;
use super::weapon_info::{VOICE_OFF, VOICE_ON};
use super::{Cod4Hud, image_of, quad};
use crate::console::shell::Shell;
use crate::hud::HudQuad;
use crate::hud::cod2elem::ScreenPlacement;
use crate::play::cod4::Cod4Session;

/// `cg_scoreboardPingGraph` and the `cg_ScoresPing_*` dvars off the
/// shell, 1.7's defaults where unset.
fn ping_graph(shell: &Shell) -> crate::cod4_scoreboard::PingGraph {
    let d = crate::cod4_scoreboard::PingGraph::default();
    let int = |n: &str, def: i32| shell.cvar(n).map_or(def, vcod_cod4::menuexp::atoi);
    let colour = |n: &str, def: [f32; 4]| {
        shell.cvar(n).map_or(def, |v| {
            let mut c = def;
            for (k, x) in v.split_whitespace().take(4).enumerate() {
                c[k] = x.parse().unwrap_or(def[k]);
            }
            c
        })
    };
    crate::cod4_scoreboard::PingGraph {
        enabled: int("cg_scoreboardPingGraph", 0) != 0,
        max_bars: int("cg_ScoresPing_MaxBars", d.max_bars).clamp(1, 10),
        interval: int("cg_ScoresPing_Interval", d.interval).clamp(1, 500),
        high: colour("cg_ScoresPing_HighColor", d.high),
        med: colour("cg_ScoresPing_MedColor", d.med),
        low: colour("cg_ScoresPing_LowColor", d.low),
        bg: colour("cg_ScoresPing_BgColor", d.bg),
    }
}

impl Cod4Hud {
    /// `+scores` / `-scores` at drawn time `now` (`CG_ScoresDown`,
    /// `CG_ScoresUp`); true when a `score` request is due.
    pub(crate) fn set_scores(&mut self, down: bool, now: i32) -> bool {
        if !down {
            if self.show_scores {
                self.show_scores = false;
                self.scores_top.set(0);
                self.score_fade_at = Some(now);
            }
            return false;
        }
        if !self.show_scores {
            self.scores_top.set(0);
        }
        self.show_scores = true;
        self.score_request(now)
    }

    /// `Scoreboard_HandleInput` (`iw3mp.exe` 0x44a2c0): while `+scores`
    /// is held a press of PGDN, KP_PGDN or MWHEELDOWN scrolls the board
    /// `cg_scoreboardScrollStep` lines down while it runs off the bottom,
    /// PGUP, KP_PGUP or MWHEELUP up to the first line; true when the key
    /// is the board's, which then reaches no bind (`CL_KeyEvent`).
    pub(crate) fn scoreboard_key(&mut self, key: &str, step: i32) -> bool {
        if !self.show_scores {
            return false;
        }
        let top = self.scores_top.get();
        match key {
            "PGDN" | "KP_PGDN" | "MWHEELDOWN" => {
                let (off_bottom, total) = self.scores_off_bottom.get();
                if off_bottom {
                    self.scores_top.set((top + step).min(total));
                }
            }
            "PGUP" | "KP_PGUP" | "MWHEELUP" => {
                if top > 1 {
                    self.scores_top.set((top - step).max(1));
                }
            }
            _ => return false,
        }
        true
    }

    /// `UpdateScores`: while the board draws, a `score` every 2 s.
    pub(crate) fn score_request(&mut self, now: i32) -> bool {
        if self.scores_asked.is_some_and(|t| t + 2000 >= now) {
            return false;
        }
        self.scores_asked = Some(now);
        true
    }

    /// The board's alpha at `now`: 1 while shown or at intermission,
    /// then a 100 ms fade (`CG_FadeColor(time, scoreFadeTime, 100, 100)`);
    /// `None` once gone.
    pub(crate) fn scoreboard_fade(&self, snap: &Snapshot, now: i32) -> Option<f32> {
        if self.show_scores || snap.ps.word(vcod_cod4::ps_word!("pm_type")) == h::PM_INTERMISSION {
            return Some(1.0);
        }
        let t = now - self.score_fade_at?;
        (0..100).contains(&t).then(|| 1.0 - t as f32 / 100.0)
    }
}

impl Cod4Hud {
    /// The board's pieces on the grid (`cod4_scoreboard::layout`).
    pub(super) fn scoreboard(
        &self,
        l: &Loaded,
        s: &Cod4Session,
        snap: &Snapshot,
        sp: &ScreenPlacement,
        fade: f32,
        shell: &Shell,
    ) -> Vec<crate::cod4_scoreboard::Piece> {
        // `CG_DrawScoreboard` draws the backdrop whether or not a `b`
        // came: until one does, the board has no rows.
        let none = Scores::default();
        let scores = self.scores.as_ref().unwrap_or(&none);
        let gs = s.gamestate();
        let cs = |i: u16| self.configstring(s, i);
        let dvar = |n: &str| self.server_dvar(s, n);
        let localize = |k: &str| match self.localized() {
            Some(loc) => loc.get(k).map_or_else(|| k.to_string(), str::to_string),
            None => k.to_string(),
        };
        let font = |scale: f32| l.fonts.pick(h::scoreboard::FONT, scale, sp);
        let rank_icon =
            |r: i32, p: i32| l.rank_icons.get(&(r, p)).filter(|m| !m.is_empty()).cloned();
        let rank_level = |r: i32| l.rank_levels.get(&r).cloned();
        let talk_icon = |c: u32| {
            if s.menus.is_muted(c) {
                Some(VOICE_OFF)
            } else {
                s.is_talking(c).then_some(VOICE_ON)
            }
        };
        let host = gs
            .and_then(|g| crate::play::cod4::info_value(g.configstring(0), "sv_hostname"))
            .unwrap_or_default();
        let mut inputs = crate::cod4_scoreboard::Inputs {
            scores,
            snap,
            own: gs.map_or(0, |g| g.client_num as u32),
            grid_width: sp.real_size[0] / sp.virtual_to_real[0],
            fade,
            host_name: host,
            configstring: &cs,
            dvar: &dvar,
            localize: &localize,
            font: &font,
            rank_icon: &rank_icon,
            rank_level: &rank_level,
            talk_icon: &talk_icon,
            top: self.scores_top.get(),
            ping_graph: ping_graph(shell),
        };
        // `CG_DrawScoreboard`: a board not yet placed opens on our row.
        if inputs.top <= 0 {
            inputs.top = crate::cod4_scoreboard::centred_top(&inputs);
            self.scores_top.set(inputs.top);
        }
        let board = crate::cod4_scoreboard::layout(&inputs);
        let total = crate::cod4_scoreboard::total_lines(&inputs);
        self.scores_off_bottom.set((board.off_bottom, total));
        board.pieces
    }

    /// The board's pieces on the newest snapshot, as `build` draws them.
    #[cfg(test)]
    pub(crate) fn board_pieces(
        &self,
        s: &Cod4Session,
        shell: &Shell,
    ) -> Vec<crate::cod4_scoreboard::Piece> {
        let (Some(Some(l)), Some(snap)) = (&self.loaded, s.client().snapshots.last()) else {
            return Vec::new();
        };
        let sp = ScreenPlacement::new(1280.0, 960.0);
        self.scoreboard(l, s, snap, &sp, 1.0, shell)
    }

    /// The board's pieces as `build` puts them on a `size` window, text
    /// and pictures alike.
    #[cfg(test)]
    pub(crate) fn board_quads(
        &self,
        s: &Cod4Session,
        size: (f32, f32),
        shell: &Shell,
    ) -> Vec<HudQuad> {
        let (Some(Some(l)), Some(snap)) = (&self.loaded, s.client().snapshots.last()) else {
            return Vec::new();
        };
        let sp = ScreenPlacement::cod4(size.0, size.1);
        let mut out = Vec::new();
        for p in self.scoreboard(l, s, snap, &sp, 1.0, shell) {
            self.draw_piece(l, &sp, &p, &mut out);
        }
        out
    }

    /// A piece on the grid, left and top aligned (`UI_DrawText`,
    /// `UI_DrawHandlePic` with `horzAlign` 1, `vertAlign` 0).
    pub(super) fn draw_piece(
        &self,
        l: &Loaded,
        sp: &ScreenPlacement,
        p: &crate::cod4_scoreboard::Piece,
        out: &mut Vec<HudQuad>,
    ) {
        use crate::cod4_scoreboard::Piece as P;
        match p {
            P::Text {
                text: t,
                x,
                y,
                scale,
                rgba,
            } => {
                let f = l.fonts.pick(h::scoreboard::FONT, *scale, sp);
                let px = scale / f.unit_scale();
                let h = (48.0 * scale) as i32 as f32;
                let (rx, _) = sp.axis(0, *x, 0.0, 1);
                let (ry, _) = sp.axis(1, y - h, h, 0);
                text(f, t, rx, ry, px * sp.virtual_to_real[1], *rgba, out);
            }
            P::Pic {
                material,
                x,
                y,
                w,
                h,
                rgba,
            } => {
                if let Some(tex) = image_of(l, material) {
                    let (rx, rw) = sp.axis(0, *x, *w, 1);
                    let (ry, rh) = sp.axis(1, *y, *h, 0);
                    out.push(quad(rx, ry, rw, rh, *rgba, tex, false));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::super::tests::{FIXTURES, replay};

    /// `Scoreboard_HandleInput`: the board's keys reach it only while it
    /// is held; down scrolls a step while lines run off the bottom, to the
    /// last; up stops at the first line; other keys pass to the binds.
    #[test]
    fn the_scoreboard_scrolls_only_while_held_and_within_its_lines() {
        let mut hud = Cod4Hud::default();
        assert!(!hud.scoreboard_key("PGDN", 4), "a hidden board");
        hud.show_scores = true;
        hud.scores_off_bottom.set((false, 20));
        assert!(hud.scoreboard_key("MWHEELDOWN", 4));
        assert_eq!(hud.scores_top.get(), 0, "nothing below");
        hud.scores_off_bottom.set((true, 6));
        assert!(hud.scoreboard_key("KP_PGDN", 4));
        assert_eq!(hud.scores_top.get(), 4);
        assert!(hud.scoreboard_key("PGDN", 4));
        assert_eq!(hud.scores_top.get(), 6, "to the last line");
        assert!(hud.scoreboard_key("PGUP", 4));
        assert_eq!(hud.scores_top.get(), 2);
        assert!(hud.scoreboard_key("MWHEELUP", 4));
        assert_eq!(hud.scores_top.get(), 1, "the first line");
        assert!(!hud.scoreboard_key("TAB", 4));
    }

    /// `+scores` before any `b` still draws the board's backdrop, as a
    /// live server that has not answered `score` leaves it; the bars hide
    /// under the board's menu (`uiActive()`).
    #[test]
    fn cod4_scores_held_with_no_list_draws_the_board() {
        if vcod_cod4::testing::cod4_install().is_none() {
            return;
        }
        let mut last = 0;
        let (mut hud, s) = replay("play-1.7-kill-target.txt", |_, _, now| last = now);
        assert!(hud.scores.is_none(), "the capture carries no `b`");
        let shell = Shell::new();
        let size = (1280.0, 960.0);
        let bars = hud.bar_quads(&s, &shell, last);
        let without = hud.build(&s, size, last, None, &shell).len() - bars;
        hud.set_scores(true, last);
        let with = hud.build(&s, size, last, None, &shell).len();
        assert_eq!(
            hud.bar_quads(&s, &shell, last),
            0,
            "the bars under the board"
        );
        assert!(with > without + 4, "{without} quads, {with} with the board");
    }

    /// The score capture's target holds `+scores` from the first frame:
    /// its `b` replies fill the board with both players, the shooter's
    /// kill and the target's death in their rows (cod4-hud.md 7).
    #[test]
    fn score_capture_draws_both_players_on_the_board() {
        let mut held = false;
        let mut board: Vec<crate::cod4_scoreboard::Piece> = Vec::new();
        let mut quads = 0;
        let text =
            std::fs::read_to_string(format!("{FIXTURES}/play-1.7-score-target.txt")).unwrap();
        let mut s = Cod4Session::open_capture(&text).unwrap();
        let mut hud = Cod4Hud::default();
        if let Some(install) = vcod_cod4::testing::cod4_install() {
            hud.load(&install.root().join("main"), "mp_crash");
        }
        let mut ms = 0.0;
        while !s.finished() {
            ms += 1000.0 / 60.0;
            s.step(ms).unwrap();
            let fired = s.take_fired();
            let Some(scene) = s.scene() else { continue };
            if !held {
                held = true;
                hud.set_scores(true, scene.render_time);
            }
            hud.update(&mut s, &fired, scene.server_time);
            if let (Some(Some(l)), Some(snap)) = (&hud.loaded, s.client().snapshots.last()) {
                let sp = ScreenPlacement::new(1280.0, 960.0);
                board = hud.scoreboard(l, &s, snap, &sp, 1.0, &Shell::new());
                quads = hud
                    .build(&s, (1280.0, 960.0), scene.render_time, None, &Shell::new())
                    .len();
            }
        }
        // `b 2 10 0 750 1 10 <ping> 0 0 1 0 0 0 <ping> 1 0 0 0`: the
        // shooter (client 1) 10 points and a kill, the target a death.
        let sc = hud.scores.as_ref().expect("a `b` came");
        assert!(hud.counts.scores >= 10, "{:?}", hud.counts);
        assert_eq!((sc.team_scores, sc.score_limit), ([10, 0], 750));
        let row = |c: i32| sc.rows.iter().find(|r| r.client == c).unwrap();
        assert_eq!((row(1).score, row(1).kills, row(1).deaths), (10, 1, 0));
        assert_eq!((row(0).score, row(0).kills, row(0).deaths), (0, 0, 1));
        if hud.loaded.is_none() {
            return;
        }
        let texts: Vec<(&str, f32, f32)> = board
            .iter()
            .filter_map(|p| match p {
                crate::cod4_scoreboard::Piece::Text { text, x, y, .. } => {
                    Some((text.as_str(), *x, *y))
                }
                _ => None,
            })
            .collect();
        println!("{texts:?}, {quads} quads");
        let at = |t: &str| {
            texts
                .iter()
                .find(|x| x.0 == t)
                .unwrap_or_else(|| panic!("no {t}: {texts:?}"))
        };
        // Both banners (`MPUI_MARINES_SHORT`, `MPUI_OPFOR_SHORT`), our
        // own (allies) first, each with its one player.
        let (allies, opfor) = (at("Marines"), at("OpFor"));
        assert!(allies.2 < opfor.2, "{texts:?}");
        let (target, shooter) = (at("target"), at("shooter"));
        assert!(allies.2 < target.1.max(target.2) && target.2 < opfor.2 && opfor.2 < shooter.2);
        assert!(
            texts.iter().any(|t| t.0 == "10" && t.2 == shooter.2),
            "{texts:?}"
        );
        assert!(
            texts.iter().filter(|t| t.0 == "( 1 )").count() == 2,
            "{texts:?}"
        );
        for h in ["Score", "Kills", "Assists", "Deaths", "Ping"] {
            at(h);
        }
        let pics = board
            .iter()
            .filter(|p| matches!(p, crate::cod4_scoreboard::Piece::Pic { .. }))
            .count();
        assert!(pics >= 4, "{board:?}");
        assert!(quads > 100, "{quads}");
    }
}
