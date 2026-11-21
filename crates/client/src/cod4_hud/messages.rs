//! The game messages: obituaries and kill lines, the chat ring and its
//! wrap, the message windows, the centre print, the say line, the vote and
//! the killcam's text.

use vcod_cod4::hud::{self as h, MessageWindow};
use vcod_cod4::net::delta::EntityState;
use vcod_cod4::net::parse::Snapshot;

use super::assets::Loaded;
use super::text::text;
use super::text_height;
use super::{
    CENTER_MS, CENTER_SCALE, CENTER_Y, CHAT_MS, CHAT_POS, CHAT_SCALE, CHAT_WIDTH, CS_VOTE_NO,
    CS_VOTE_STRING, CS_VOTE_TIME, CS_VOTE_YES, Cod4Hud, EF_VOTED, FADE_IN_MS, FADE_OUT_MS,
    ICON_SIZE, Line, MSG_LINES, MSG_MS, Piece, VOTE_POSITION, image_of, in_killcam, quad,
};
use crate::console::shell::Shell;
use crate::hud::HudQuad;
use crate::hud::cod2elem::ScreenPlacement;
use crate::hud::font::{self, Font};
use crate::play::cod4::{Cod4Session, client_name, ent_u32};

impl Cod4Hud {
    /// `CG_Obituary`: a kill line in
    /// game message window 0, `^8` for the drawn player's team and `^9`
    /// for the other, and a centre print for the drawn player's kill or
    /// death; nothing in the killcam (`ps.deltaTime` set).
    pub(super) fn obituary(&mut self, snap: &Snapshot, e: &EntityState, own: u32, now: i32) {
        let victim = ent_u32(e, "otherEntityNum");
        let attacker = ent_u32(e, "attackerEntityNum");
        let parm = ent_u32(e, "eventParm");
        let in_killcam = in_killcam(snap);
        let client = |n: u32| snap.clients.iter().find(|c| c.0[0] == n);
        let Some(v) = client(victim) else { return };
        let team = |c| crate::play::cod4::client_u32(c, "team");
        let me = client(own).map_or(0, team);
        let colour = |t: u32| match (t, me) {
            (1 | 2, 1 | 2) if t == me => '8',
            (1 | 2, 1 | 2) => '9',
            _ => '7',
        };
        let victim_name = client_name(v);
        let attacker_c = (attacker < 64).then(|| client(attacker)).flatten();
        if attacker < 64 && attacker_c.is_none() {
            return;
        }
        let attacker_name = match attacker_c {
            Some(a) if attacker != victim => Some((client_name(a), colour(team(a)))),
            _ => None,
        };
        let icon = self.kill_icon(parm);
        if !in_killcam {
            let teammate = attacker_c.is_some_and(|a| team(a) != 0 && team(a) == team(v));
            let tail = if teammate { "\u{14}CGAME_TEAMMATE" } else { "" };
            let print = if attacker == own && attacker != victim {
                Some(format!("CGAME_YOUKILLED\u{15}{victim_name}{tail}"))
            } else if victim == own && attacker_c.is_some() && attacker != victim {
                let name = attacker_name.as_ref().map_or("", |(n, _)| n.as_str());
                Some(format!("CGAME_YOUWEREKILLED\u{15}{name}{tail}"))
            } else {
                None
            };
            if let Some(p) = print {
                self.counts.center_prints += 1;
                self.center = Some((self.message(&p), now));
            }
            let mut pieces = Vec::new();
            if let Some((n, c)) = attacker_name {
                pieces.push(Piece::Text(format!("^{c}{n}^7 ")));
            }
            pieces.push(icon);
            pieces.push(Piece::Text(format!(" ^{}{victim_name}", colour(team(v)))));
            self.counts.kill_lines += 1;
            push_line(&mut self.windows[0], pieces, now, MSG_LINES[0]);
        }
    }

    /// The obituary's icon: a means of death (`eventParm & 0x80`) picks one
    /// of the `killicon*` materials, else the weapon's `killIcon`.
    fn kill_icon(&self, parm: u32) -> Piece {
        let icon = |m: &str, w: f32| Piece::Icon {
            material: m.to_string(),
            w,
            h: ICON_SIZE,
            flip: false,
        };
        if parm & 0x80 != 0 {
            return icon(
                crate::hud::killfeed::iw_mod_icon(parm & 0x7f, true),
                ICON_SIZE,
            );
        }
        let weapon = self.weapon_name(parm);
        let Some(k) = self
            .loaded
            .as_ref()
            .and_then(|l| l.as_ref())
            .and_then(|l| l.kill_icons.get(weapon.as_deref()?))
        else {
            return icon("killicondied", ICON_SIZE);
        };
        let (w, h) = match k.ratio {
            1 => (2.0 * ICON_SIZE, ICON_SIZE),
            2 => (2.0 * ICON_SIZE, 0.5 * ICON_SIZE),
            _ => (ICON_SIZE, ICON_SIZE),
        };
        Piece::Icon {
            material: k.material.clone(),
            w,
            h,
            flip: k.flip,
        }
    }

    fn weapon_name(&self, n: u32) -> Option<String> {
        self.weapons.get((n as usize).checked_sub(1)?).cloned()
    }
}

/// A chat line broken as `CG_AddToTeamChat` breaks it (cod4-hud.md 4):
/// once a line holds more than [`CHAT_WIDTH`] characters, colour codes
/// not counted, the next character starts a new line, which takes along
/// whatever followed the line's last space. A carried tail counts every
/// character it holds, colour codes too.
pub(super) fn wrap_chat(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut counted = 0;
    for (piece, width) in chat_pieces(text) {
        if counted > CHAT_WIDTH {
            let tail = line
                .rfind(' ')
                .map_or_else(String::new, |sp| line.split_off(sp + 1));
            counted = tail.chars().count();
            lines.push(std::mem::replace(&mut line, tail));
        }
        line.push_str(piece);
        counted += width;
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// `text` as colour codes (`^` and a digit, no width) and single
/// characters (width 1).
fn chat_pieces(text: &str) -> impl Iterator<Item = (&str, usize)> {
    let mut rest = text;
    std::iter::from_fn(move || {
        let mut chars = rest.chars();
        let first = chars.next()?;
        let code = first == '^' && chars.next().is_some_and(|d| d.is_ascii_digit());
        let len = if code { 2 } else { first.len_utf8() };
        let (piece, tail) = rest.split_at(len);
        rest = tail;
        Some((piece, usize::from(!code)))
    })
}

pub(super) fn push_line(lines: &mut Vec<Line>, pieces: Vec<Piece>, at: i32, keep: usize) {
    lines.push(Line { pieces, at });
    if lines.len() > keep {
        lines.remove(0);
    }
}

/// `cg_hudSayPosition`'s default (5, 180).
const SAY_POS: (f32, f32) = (5.0, 180.0);

/// `Con_DrawSay`: `EXE_SAY: ` or `EXE_SAYTEAM: ` and the typed
/// line with its cursor, white, font 0 at 16 px over a 48 px em (a 768
/// line screen or less), its top 24 under `cg_hudSayPosition`.
pub(super) fn draw_say(
    l: &Loaded,
    field: &crate::hud::ChatField,
    sp: &ScreenPlacement,
    out: &mut Vec<HudQuad>,
) {
    const SCALE: f32 = 16.0 / 48.0;
    let key = if field.team { "EXE_SAYTEAM" } else { "EXE_SAY" };
    let label = format!("{}: ", l.localized.get(key).unwrap_or(key));
    let f = l.fonts.pick(0, SCALE, sp);
    let px = SCALE / f.unit_scale();
    let th = (48.0 * SCALE).trunc();
    let (x, _) = sp.axis(0, SAY_POS.0, 0.0, 1);
    let (y, _) = sp.axis(1, SAY_POS.1 + 24.0, th, 1);
    let k = sp.virtual_to_real[1];
    let w = font::layout(f, &label, x, y, px * k, [1.0; 4], out);
    font::layout(
        f,
        &format!("{}_", field.text),
        x + w,
        y,
        px * k,
        [1.0; 4],
        out,
    );
}

impl Cod4Hud {
    /// `CG_DrawVote` (`iw3mp.exe` 0x42ed50), while configstring 13 names
    /// a vote of this server (`%d %d`, its end and `sv_serverid`; 15 and
    /// 16 the counts, 14 the localized text; `CG_ConfigStringModified`,
    /// 0x44afd0): at `cg_hudVotePosition` (5, 220) two lines a font size
    /// apart, size 10 over a 768-line window, else 16, scale size / 48,
    /// `CGAME_VOTE(<seconds left>):<text>` and the tallies, each with the
    /// key bound to `vote yes` or `vote no` (the command itself when none
    /// is) until the player voted (`eFlags` 0x100000). cod4-hud.md 14.
    pub(super) fn draw_vote(
        &self,
        l: &Loaded,
        s: &Cod4Session,
        snap: &Snapshot,
        sp: &ScreenPlacement,
        now: i32,
        shell: &Shell,
        out: &mut Vec<HudQuad>,
    ) {
        let Some(end) = self.vote_end(s) else {
            return;
        };
        let loc = |k: &str| l.localized.get(k).unwrap_or(k).to_string();
        let count = |i: u16| h::atoi(&self.configstring(s, i));
        let what = self.message(&self.configstring(s, CS_VOTE_STRING));
        let secs = ((end - now) / 1000).max(0);
        let first = format!("{}({secs}):{what}", loc("CGAME_VOTE"));
        let (yes, no) = (count(CS_VOTE_YES), count(CS_VOTE_NO));
        let second = if snap.ps.word(vcod_cod4::ps_word!("eFlags")) & EF_VOTED != 0 {
            format!("{}:{yes}, {}:{no}", loc("CGAME_YES"), loc("CGAME_NO"))
        } else {
            let key = |cmd: &str| match shell.key_text(cmd, &l.localized) {
                Some(_) => self.key_binding(shell, cmd),
                None => cmd.to_string(),
            };
            format!(
                "{}({}):{yes}, {}({}):{no}",
                loc("CGAME_YES"),
                key("vote yes"),
                loc("CGAME_NO"),
                key("vote no")
            )
        };
        let size = if sp.real_size[1] > 768.0 { 10.0 } else { 16.0 };
        let scale = size / 48.0;
        let f = l.fonts.pick(0, scale, sp);
        let px = scale / f.unit_scale();
        let [x, y] = VOTE_POSITION;
        for (k, line) in [first, second].iter().enumerate() {
            // UI_DrawText's y is the baseline, a font size under the last.
            let baseline = y + size * (k + 1) as f32;
            let h = text_height(scale);
            let (rx, _) = sp.axis(0, x, 0.0, 1);
            let (ry, _) = sp.axis(1, baseline - h, h, 1);
            text(f, line, rx, ry, px * sp.virtual_to_real[1], [1.0; 4], out);
        }
    }

    /// The running vote's end in server time, when configstring 13 names
    /// one of this server's (`sv_serverid` in the system info).
    fn vote_end(&self, s: &Cod4Session) -> Option<i32> {
        let cs = self.configstring(s, CS_VOTE_TIME);
        let mut w = cs.split_whitespace().map(h::atoi);
        let (end, id) = (w.next()?, w.next()?);
        let ours = crate::play::cod4::info_value(self.configstring(s, 1).as_bytes(), "sv_serverid")
            .map_or(0, |v| h::atoi(&v));
        (end != 0 && id == ours).then_some(end)
    }

    /// The obituary's centre print for `cg_centertime`, 150 under the
    /// screen's middle.
    pub(super) fn draw_center(
        &self,
        l: &Loaded,
        sp: &ScreenPlacement,
        now: i32,
        out: &mut Vec<HudQuad>,
    ) {
        let Some((line, at)) = &self.center else {
            return;
        };
        if now - at >= CENTER_MS {
            return;
        }
        let f = l.fonts.pick(0, CENTER_SCALE, sp);
        let px = CENTER_SCALE / f.unit_scale();
        let w = font::measure(f, line, px);
        let h = text_height(CENTER_SCALE);
        let (x, _) = sp.axis(0, -0.5 * w, w, 2);
        let (y, _) = sp.axis(1, CENTER_Y - h, h, 2);
        text(f, line, x, y, px * sp.virtual_to_real[1], [1.0; 4], out);
    }

    /// The `killcam` menu: its letterbox bars, the `white` material in
    /// their colour, and its title centred on the item's x at its
    /// baseline (cod4-hud.md 11).
    pub(super) fn draw_killcam(&self, l: &Loaded, sp: &ScreenPlacement, out: &mut Vec<HudQuad>) {
        for it in &l.killcam {
            let [x, y, w, h] = it.rect.rect;
            if let Some(t) = &it.text {
                let t = l.localized.get(t).unwrap_or(t);
                let f = l.fonts.pick(it.font, it.text_scale, sp);
                let px = it.text_scale / f.unit_scale();
                let tw = font::measure(f, t, px);
                let fh = text_height(it.text_scale);
                let tx = match it.text_align {
                    1 => x + 0.5 * w - 0.5 * tw,
                    2 => x + w - tw,
                    _ => x,
                };
                let (rx, _) = sp.axis(0, tx, tw, it.rect.horz_align);
                let (ry, _) = sp.axis(1, y + h - fh, fh, it.rect.vert_align);
                text(f, t, rx, ry, px * sp.virtual_to_real[1], it.fore_color, out);
            } else if let Some(tex) = it.rect.background.as_deref().and_then(|m| image_of(l, m)) {
                let (rx, rw) = sp.axis(0, x, w, it.rect.horz_align);
                let (ry, rh) = sp.axis(1, y, h, it.rect.vert_align);
                out.push(quad(rx, ry, rw, rh, it.fore_color, tex, false));
            }
        }
    }

    /// A game message window (`gamemessages` 0, `boldgamemessages` 1) at
    /// its menu item's rect: lines that have lived less than the window's
    /// message time, fading in and out, growing up from the rect in mode
    /// 1 and down in mode 0.
    pub(super) fn draw_window(
        &self,
        l: &Loaded,
        i: usize,
        sp: &ScreenPlacement,
        now: i32,
        out: &mut Vec<HudQuad>,
    ) {
        let Some(w) = l.windows.get(&(i as i32)) else {
            return;
        };
        let life = MSG_MS[i];
        let lines: Vec<&Line> = self.windows[i]
            .iter()
            .filter(|x| now - x.at < life)
            .collect();
        let f = l.fonts.pick(w.font, w.text_scale, sp);
        let n = lines.len();
        for (row, line) in lines.iter().enumerate() {
            let age = now - line.at;
            let a = (age as f32 / FADE_IN_MS[i] as f32)
                .min((life - age) as f32 / FADE_OUT_MS[i] as f32)
                .clamp(0.0, 1.0);
            let step = match w.mode {
                1 => -((n - 1 - row) as f32),
                _ => row as f32,
            };
            self.draw_line(f, w, w.y + step * 48.0 * w.text_scale, line, a, sp, out);
        }
    }

    /// One message line on the window's grid line `y`, its pieces left to
    /// right; an icon is font heights wide and tall, centred on the text.
    fn draw_line(
        &self,
        f: &Font,
        w: &MessageWindow,
        y: f32,
        line: &Line,
        alpha: f32,
        sp: &ScreenPlacement,
        out: &mut Vec<HudQuad>,
    ) {
        let Some(Some(l)) = &self.loaded else { return };
        let scale = w.text_scale;
        let px = scale / f.unit_scale();
        let h = text_height(scale);
        let width: f32 = line
            .pieces
            .iter()
            .map(|p| match p {
                Piece::Text(t) => font::measure(f, t, px),
                Piece::Icon { w, .. } => w * h,
            })
            .sum();
        let x = match w.alignment {
            1 => w.x - 0.5 * width,
            2 => w.x - width,
            _ => w.x,
        };
        let (mut rx, _) = sp.axis(0, x, width, w.horz_align);
        let (ry, rh) = sp.axis(1, y - h, h, w.vert_align);
        let k = sp.virtual_to_real[1];
        for p in &line.pieces {
            match p {
                Piece::Text(t) => {
                    rx += text(f, t, rx, ry, px * k, [1.0, 1.0, 1.0, alpha], out);
                }
                Piece::Icon {
                    material,
                    w: iw,
                    h: ih,
                    flip,
                } => {
                    let (qw, qh) = (iw * rh, ih * rh);
                    if let Some(tex) = image_of(l, material) {
                        let qy = ry + 0.5 * (rh - qh);
                        out.push(quad(rx, qy, qw, qh, [1.0, 1.0, 1.0, alpha], tex, *flip));
                    }
                    rx += qw;
                }
            }
        }
    }

    /// The chat ring at `cg_hudChatPosition`, oldest on top, each line
    /// for `cg_chatTime`.
    pub(super) fn draw_chat(
        &self,
        l: &Loaded,
        sp: &ScreenPlacement,
        now: i32,
        out: &mut Vec<HudQuad>,
    ) {
        let f = l.fonts.pick(0, CHAT_SCALE, sp);
        let px = CHAT_SCALE / f.unit_scale();
        let h = text_height(CHAT_SCALE);
        let live = self.chat.iter().filter(|c| now - c.at < CHAT_MS);
        for (row, line) in live.enumerate() {
            let Some(Piece::Text(t)) = line.pieces.first() else {
                continue;
            };
            let (x, _) = sp.axis(0, CHAT_POS.0, 0.0, 1);
            let (y, _) = sp.axis(1, CHAT_POS.1 + row as f32 * h, h, 1);
            text(f, t, x, y, px * sp.virtual_to_real[1], [1.0; 4], out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::super::tests::{replay, wrap_corpus};

    /// A vote of this server's (configstring 13's second number is
    /// `sv_serverid`) draws its two lines until its end; one naming
    /// another server id draws nothing.
    #[test]
    fn cod4_vote_draws_while_it_runs() {
        if vcod_cod4::testing::cod4_install().is_none() {
            return;
        }
        let mut last = 0;
        let (mut hud, s) = replay("play-1.7-kill-target.txt", |_, _, now| last = now);
        let shell = Shell::new();
        let size = (1280.0, 960.0);
        let without = hud.build(&s, size, last, None, &shell).len();
        let id = crate::play::cod4::info_value(hud.configstring(&s, 1).as_bytes(), "sv_serverid")
            .unwrap();
        hud.cs.insert(13, format!("{} {id}", last + 20_000));
        hud.cs.insert(14, "GAME_VOTE_MAPRESTART".into());
        hud.cs.insert(15, "1".into());
        hud.cs.insert(16, "0".into());
        let with = hud.build(&s, size, last, None, &shell).len();
        assert!(with > without + 20, "{without} quads, {with} with the vote");
        hud.cs
            .insert(13, format!("{} {}", last + 20_000, h::atoi(&id) + 1));
        assert_eq!(hud.build(&s, size, last, None, &shell).len(), without);
    }

    #[test]
    fn kill_capture_prints_one_kill_line_and_the_victims_centre_print() {
        let mut most = 0;
        let mut textures = std::collections::BTreeSet::new();
        let (hud, s) = replay("play-1.7-kill-target.txt", |hud, s, now| {
            let q = hud.build(
                s,
                (1280.0, 960.0),
                now,
                Some((glam::Vec3::ZERO, 90.0)),
                &Shell::new(),
            );
            most = most.max(q.len());
            textures.extend(q.into_iter().map(|q| q.texture));
        });
        // `^8` and `^9`'s colours off the configstring mirror (cod4-hud.md 3).
        assert_eq!(
            hud.server_dvar(&s, "g_TeamColor_MyTeam").as_deref(),
            Some("0.6 0.8 0.6 0")
        );
        assert_eq!(
            hud.server_dvar(&s, "g_TeamColor_EnemyTeam").as_deref(),
            Some("1 0.45098 0.501961 0")
        );
        let c = &hud.counts;
        // The killcam replays the obituary; only the live one prints
        // (`ps.deltaTime`, cod4-hud.md 3).
        assert_eq!(c.kill_lines, 1, "{c:?}");
        assert_eq!(c.center_prints, 1, "{c:?}");
        assert_eq!((c.chat_lines, c.game_messages, c.bold_messages), (0, 0, 0));
        assert_eq!(c.scores, 0, "the capture carries no `b`");
        assert_eq!(c.config_strings, 10, "{c:?}");
        assert!(c.dvars > 50, "{c:?}");
        let line = &hud.windows[0][0].pieces;
        assert_eq!(line[0], Piece::Text("^9shooter^7 ".into()));
        assert!(matches!(&line[1], Piece::Icon { .. }), "{line:?}");
        assert_eq!(line[2], Piece::Text(" ^8target".into()));
        if hud.loaded.is_some() {
            let (center, _) = hud.center.as_ref().unwrap();
            assert_eq!(center, "Killed by shooter");
            assert!(
                matches!(&line[1], Piece::Icon { material, flip: true, .. } if material == "hud_icon_m16a4_grenade")
            );
            println!("most quads {most}, textures {textures:?}");
            assert!(textures.contains("images/gamefonts_pc.iwi"), "{textures:?}");
            assert!(textures.iter().any(|t| t.contains("m16a4")), "{textures:?}");
            assert!(most > 100, "{most} quads");
            assert!(
                textures.contains("images/compass_map_mp_crash.iwi"),
                "{textures:?}"
            );
        }
    }

    /// The target's killcam draws the `killcam` menu over the killer's
    /// view: both letterbox bars and the 7 letters of `KILLCAM` on every
    /// frame of it, never outside; the script's tenths timer counts down
    /// beside it (cod4-hud.md 11).
    #[test]
    fn kill_capture_draws_the_killcam_letterbox_title_and_timer() {
        let (mut frames, mut drawn, mut outside) = (0, 0, 0);
        let mut timers = std::collections::BTreeSet::new();
        let shell = Shell::new();
        let (hud, _) = replay("play-1.7-kill-target.txt", |hud, s, now| {
            let (Some(Some(l)), Some(snap)) = (&hud.loaded, s.client().snapshots.last()) else {
                return;
            };
            let sp = ScreenPlacement::new(1280.0, 960.0);
            let mut q = Vec::new();
            if in_killcam(snap) {
                frames += 1;
                hud.draw_killcam(l, &sp, &mut q);
                for e in h::hud_elems(&snap.ps).iter().filter(|e| e.kind == 9) {
                    timers.extend(hud.elem_text(s, e, now, &shell));
                }
            }
            let all = hud.build(s, (1280.0, 960.0), now, None, &shell);
            let bars = |q: &[HudQuad]| {
                q.iter()
                    .filter(|q| q.rgba == [0.49, 0.05, 0.03, 0.5])
                    .count()
            };
            if in_killcam(snap) {
                let title = q.iter().filter(|q| q.rgba == [1.0; 4]).count();
                if bars(&q) == 2 && title == 7 && bars(&all) == 2 {
                    drawn += 1;
                    // The bars span the window: 112 of 480 rows each.
                    assert_eq!(
                        q[0].verts,
                        [[0.0, 0.0], [1280.0, 0.0], [1280.0, 224.0], [0.0, 224.0]]
                    );
                }
            } else if bars(&all) != 0 {
                outside += 1;
            }
        });
        if hud.loaded.is_none() {
            return;
        }
        println!("{frames} killcam frames, timers {timers:?}");
        assert!(frames > 200, "{frames}");
        assert_eq!(drawn, frames);
        assert_eq!(outside, 0);
        assert!(
            timers.contains("0:02.5") && timers.contains("0:00.0"),
            "{timers:?}"
        );
    }

    /// The obituary reaches the screen: the kill line's M16 icon draws
    /// for the message window's 5 s and the centre print's letters, 150
    /// under the middle, for `cg_centertime`'s, both from the death on,
    /// on into the killcam, which starts 2 s on (cod4-hud.md 3).
    #[test]
    fn kill_capture_draws_the_obituary_line_and_centre_print() {
        let (mut icon, mut centre) = (Vec::new(), Vec::new());
        let (hud, _) = replay("play-1.7-kill-target.txt", |hud, s, now| {
            let q = hud.build(s, (1280.0, 960.0), now, None, &Shell::new());
            // `hud_icon_m16a4_grenade`'s image.
            if q.iter().any(|q| q.texture == "images/hud_m16a4_m203.iwi") {
                icon.push(now);
            }
            let Some(Some(l)) = &hud.loaded else { return };
            let mut c = Vec::new();
            hud.draw_center(l, &ScreenPlacement::new(1280.0, 960.0), now, &mut c);
            // Every quad of it in the frame's HUD.
            if !c.is_empty()
                && c.iter().all(|c| {
                    q.iter()
                        .any(|x| x.verts == c.verts && x.texture == c.texture)
                })
            {
                centre.push(now);
            }
        });
        if hud.loaded.is_none() {
            return;
        }
        let span = |v: &[i32]| v.last().unwrap() - v.first().unwrap();
        println!(
            "icon {:?}..{:?}, centre {:?}..{:?}",
            icon.first(),
            icon.last(),
            centre.first(),
            centre.last()
        );
        assert!((4900..=5100).contains(&span(&icon)), "{}", span(&icon));
        assert!((4900..=5100).contains(&span(&centre)), "{}", span(&centre));
        assert!(icon[0] < 50_000 && centre[0] < 50_000);
    }

    #[test]
    fn spectator_capture_carries_the_walkers_chat_line() {
        let (hud, _) = replay("play-1.7-spec.txt", |_, _, _| {});
        assert_eq!(hud.counts.chat_lines, 1, "{:?}", hud.counts);
        let Some(Piece::Text(t)) = hud.chat[0].pieces.first() else {
            panic!()
        };
        assert_eq!(t, "^9walker^7: ^7vcod probe here");
        assert_eq!(hud.counts.kill_lines, 0);
    }

    /// Fails with the whole new text when `actual` moved off the pin.
    fn pin(tag: &str, actual: &str, expected: &str) {
        assert!(actual == expected, "{tag} moved:\n{actual}");
    }

    #[test]
    fn chat_wrap_pins() {
        let s: String = wrap_corpus()
            .iter()
            .map(|t| format!("{t:?} => {:?}\n", wrap_chat(t)))
            .collect();
        pin(
            "chat",
            &s,
            r##""" => []
"short" => ["short"]
"^" => ["^"]
"trailing caret^" => ["trailing caret^"]
"^^1 caret pair ^x not a code" => ["^^1 caret pair ^x not a code"]
"  leading and trailing spaces  " => ["  leading and trailing spaces  "]
"^1word word word word word word word word word word word word  tail" => ["^1word word word word word word word word word word ", "word word  tail"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "a"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "aaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "aaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "aaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "aaaaaaaaaaaaaa"]
"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx yyyyyyyyyyyyyyyyyyyyyyyyyyyyyy" => ["xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx ", "yyyyyyyyyyyyyyyyyyyyyyyyyyyyyy"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa ^3bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa ", "^3bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "bbbbbbbbb"]
"^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j " => ["^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ", "^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j "]
"ccccccccccccccccccccccccccccccccccccccccccccc   ddddddddddddddddddddddddddddddddddddddddddddd" => ["ccccccccccccccccccccccccccccccccccccccccccccc   ", "ddddddddddddddddddddddddddddddddddddddddddddd"]
"cccccccccccccccccccccccccccccc dddddddddddddddddddddddddddddddddddddddd" => ["cccccccccccccccccccccccccccccc ", "dddddddddddddddddddddddddddddddddddddddd"]
"Привет мир, это длинное сообщение Привет мир, это длинное сообщение Привет мир, это длинное сообщение " => ["Привет мир, это длинное сообщение Привет мир, это ", "длинное сообщение Привет мир, это длинное сообщение "]
"日本語のテキストは空白がない日本語のテキストは空白がない日本語のテキストは空白がない日本語のテキストは空白がない" => ["日本語のテキストは空白がない日本語のテキストは空白がない日本語のテキストは空白がない日本語のテキストは空白", "がない"]
"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeäffffffffff" => ["eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee", "eeeeäffffffffff"]
"^2Shooter^7: ^7gg, nice shot from across the whole map, really nice one" => ["^2Shooter^7: ^7gg, nice shot from across the whole map, ", "really nice one"]
"##,
        );
    }

    #[test]
    fn chat_wraps_past_52_characters_at_a_space() {
        let long = format!("^1{} tail", "word ".repeat(12));
        let lines = wrap_chat(&long);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].ends_with(' '), "{lines:?}");
        assert!(lines[1].ends_with("tail"));
    }
}
