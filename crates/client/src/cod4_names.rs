//! CoD4's names over heads, as 1.7's 2D pass draws them (`iw3mp.exe`
//! 0x42f570): the client under the crosshair (`CG_DrawCrosshairNames`,
//! 0x42e2a0) and every teammate in sight (`CG_DrawFriendlyNames`,
//! 0x42e0d0), each over its `j_head`, sized by its distance, in its team
//! colour, with its rank's icon and level to the left
//! (docs/research/cod4-hud.md 17.1).

use crate::cod4_crosshair::{CrosshairTarget, NameFades};
use crate::cod4_waypoints::Eye;
use crate::hud::HudQuad;
use crate::hud::font::{self, Font};
use crate::play::cod4::{ET_PLAYER, client_team, ent_u32};
use glam::Vec3;
use vcod_cod4::net::parse::Snapshot;

/// The names' dvars at 1.7's defaults (`iw3mp.exe` 0x43b1d8, 0x43d4e5..
/// 0x43d959; cod4-hud.md 17.1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Dvars {
    /// `cg_drawCrosshairNames`.
    pub crosshair: bool,
    /// `cg_drawFriendlyNames`.
    pub friendly: bool,
    /// `cg_drawThroughWalls`.
    pub through_walls: bool,
    /// `cg_overheadNamesMaxDist`, `NearDist`, `FarDist`, units.
    pub max_dist: f32,
    pub near_dist: f32,
    pub far_dist: f32,
    /// `cg_overheadNamesFarScale`, `Size`, `Font`.
    pub far_scale: f32,
    pub size: f32,
    pub font: i32,
    /// `cg_overheadIconSize`, `cg_overheadRankSize`.
    pub icon_size: f32,
    pub rank_size: f32,
    /// `cg_overheadNamesGlow`.
    pub glow: [f32; 4],
    pub fades: NameFades,
}

impl Default for Dvars {
    fn default() -> Self {
        Dvars {
            crosshair: true,
            friendly: true,
            through_walls: false,
            max_dist: 10000.0,
            near_dist: 256.0,
            far_dist: 1024.0,
            far_scale: 0.6,
            size: 0.5,
            font: 2,
            icon_size: 0.7,
            rank_size: 0.5,
            glow: [0.0, 0.0, 0.0, 1.0],
            fades: NameFades::default(),
        }
    }
}

impl Dvars {
    /// The shell's values over the defaults.
    pub fn read(shell: &crate::console::shell::Shell) -> Dvars {
        let d = Dvars::default();
        let flag = |n: &str, def: bool| shell.cvar(n).map_or(def, |v| v.trim() != "0");
        let num = |n: &str, def: f32| {
            shell
                .cvar(n)
                .and_then(|v| v.trim().parse::<f32>().ok())
                .map_or(def, |v| v.max(0.0))
        };
        let glow = shell.cvar("cg_overheadNamesGlow").map_or(d.glow, |v| {
            let mut c = v
                .split_whitespace()
                .map(|x| x.parse::<f32>().unwrap_or(0.0));
            [0; 4].map(|_| c.next().unwrap_or(0.0).clamp(0.0, 1.0))
        });
        Dvars {
            crosshair: flag("cg_drawCrosshairNames", d.crosshair),
            friendly: flag("cg_drawFriendlyNames", d.friendly),
            through_walls: flag("cg_drawThroughWalls", d.through_walls),
            max_dist: num("cg_overheadNamesMaxDist", d.max_dist),
            near_dist: num("cg_overheadNamesNearDist", d.near_dist),
            far_dist: num("cg_overheadNamesFarDist", d.far_dist),
            far_scale: num("cg_overheadNamesFarScale", d.far_scale),
            size: num("cg_overheadNamesSize", d.size).min(100.0),
            font: shell
                .cvar("cg_overheadNamesFont")
                .map_or(d.font, |v| vcod_common::text::atoi(v).clamp(0, 6)),
            icon_size: num("cg_overheadIconSize", d.icon_size).min(100.0),
            rank_size: num("cg_overheadRankSize", d.rank_size),
            glow,
            fades: NameFades::read(shell),
        }
    }
}

/// A name's alpha (`iw3mp.exe` 0x42de60): 0 once `fade_out` has passed
/// since `last` or while the sighting from `start` to `last` is under
/// `fade_in`, else what is left of the fade-out.
pub(crate) fn alpha(now: i32, start: i32, last: i32, fade_in: i32, fade_out: i32) -> f32 {
    let since = now - last;
    if since >= fade_out || last - start < fade_in {
        return 0.0;
    }
    (fade_out - since) as f32 / fade_out as f32
}

/// A teammate's sighting (`iw3mp.exe` 0x7439d0 + 12 a client): when he
/// was last seen, when this sighting began, and whether he was seen last
/// frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Sighting {
    last: i32,
    start: i32,
    seen: bool,
}

/// The friendly names' sightings, kept from frame to frame.
#[derive(Clone, Debug)]
pub(crate) struct OverheadNames {
    friends: [Sighting; 64],
}

impl Default for OverheadNames {
    fn default() -> Self {
        OverheadNames {
            friends: [Sighting::default(); 64],
        }
    }
}

/// What one frame's names read.
pub(crate) struct Frame<'a> {
    pub snap: &'a Snapshot,
    /// `cg.time`, ms.
    pub now: i32,
    /// The drawn view's origin and forward axis (`cg.refdef`).
    pub eye: Vec3,
    pub forward: Vec3,
    pub third_person: bool,
    /// A flashbang's shock runs (`play::cod4_shock::flashbanged`).
    pub flashed: bool,
    pub crosshair: &'a CrosshairTarget,
    /// A player entity's posed `j_head`, if his body was posed.
    pub head: &'a dyn Fn(u32) -> Option<Vec3>,
    /// A player entity's drawn origin.
    pub origin: &'a dyn Fn(u32) -> Vec3,
    /// The entity the eye to a point meets under 0x2803001, passing the
    /// drawn playerstate's client (`CG_TraceCapsule`): 1022 for the
    /// world, 1023 for none. `None` without a world: nothing is seen.
    pub trace: Option<&'a dyn Fn(Vec3, Vec3) -> u32>,
}

/// A name to draw: its client, alpha and where its body stands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Name {
    pub client: u32,
    pub alpha: f32,
    /// The posed `j_head`, else `None`.
    pub head: Option<Vec3>,
    pub origin: Vec3,
    /// The crosshair's name rather than a teammate's.
    pub crosshair: bool,
}

impl Name {
    /// Where it draws: 10 over `j_head`, else 82 over the origin
    /// (0x42d801, 0x42d81e).
    pub fn at(&self) -> Vec3 {
        self.head
            .map_or(self.origin + Vec3::Z * 82.0, |h| h + Vec3::Z * 10.0)
    }
}

/// `ENTITYNUM_NONE`, a trace that met nothing.
const NONE: u32 = 1023;

impl OverheadNames {
    /// One frame of 0x42f570: the crosshair's name, then each teammate's
    /// (a spectator's every player's), those with an alpha over 0.001
    /// (0x42d779). Teammates' sightings step whether drawn or not.
    pub fn frame(&mut self, f: &Frame, dv: &Dvars) -> Vec<Name> {
        let snap = f.snap;
        let team = |n: u32| snap.clients.iter().find(|c| c.0[0] == n).map(client_team);
        let own = snap.ps.get("clientNum").unwrap_or(0);
        let players = || snap.entities.iter().filter(|e| e.etype() == ET_PLAYER);
        let name = |n: u32, client: u32, alpha: f32, crosshair: bool| Name {
            client,
            alpha,
            head: (f.head)(n),
            origin: (f.origin)(n),
            crosshair,
        };
        let mut out = Vec::new();
        let fades = &dv.fades;
        // `CG_DrawCrosshairNames`: not in third person; the client under
        // the crosshair, both client infos valid, standing in the
        // snapshot.
        let (target, start, last) = f.crosshair.sighting();
        if dv.crosshair
            && !f.third_person
            && target <= 64
            && let (Some(mine), Some(theirs)) = (team(own), team(target))
            && let Some(e) = players().find(|e| ent_u32(e, "clientNum") == target)
        {
            let friendly = mine == 3 || (mine != 0 && mine == theirs);
            let (fade_in, fade_out) = match friendly {
                true => (fades.friendly_in, fades.friendly_out),
                false => (fades.enemy_in, fades.enemy_out),
            };
            let a = alpha(f.now, start, last, fade_in, fade_out);
            if a > 0.001 {
                out.push(name(e.number(), target, a, true));
            }
        }
        // `CG_DrawFriendlyNames`: our team's players (team 0 has none, a
        // spectator sees all), less the one we follow in first person.
        let mine = team(own).unwrap_or(0);
        if !dv.friendly || mine == 0 {
            return out;
        }
        let following = snap.ps.get("otherFlags").unwrap_or(0) & 6 != 0;
        for e in players() {
            if following && e.number() == own && !f.third_person {
                continue;
            }
            let client = ent_u32(e, "clientNum");
            if mine != 3 && team(client).unwrap_or(0) != mine {
                continue;
            }
            let seen = !f.flashed && self.visible(f, dv, e.number(), client);
            let s = &mut self.friends[client as usize % 64];
            if seen {
                if !s.seen {
                    s.start = f.now;
                }
                s.last = f.now;
            }
            s.seen = seen;
            let a = alpha(
                f.now,
                s.start,
                s.last,
                fades.friendly_in,
                fades.friendly_out,
            );
            if a > 0.001 {
                out.push(name(e.number(), client, a, false));
            }
        }
        out
    }

    /// `iw3mp.exe` 0x42de90: under `cg_drawThroughWalls` always; else the
    /// head (or the origin) ahead of the eye, within
    /// `cg_overheadNamesMaxDist`, and the trace to it meeting nothing or
    /// the player himself. The smoke's rating of the trace's end (0x4a2b20,
    /// under 0.2 hides) vcod does not run.
    fn visible(&self, f: &Frame, dv: &Dvars, number: u32, client: u32) -> bool {
        if dv.through_walls {
            return true;
        }
        let at = (f.head)(number).unwrap_or_else(|| (f.origin)(number));
        let d = at - f.eye;
        if d.dot(f.forward) <= 0.0 || d.length_squared() > dv.max_dist * dv.max_dist {
            return false;
        }
        f.trace.is_some_and(|trace| {
            let hit = trace(f.eye, at);
            hit == client || hit == NONE
        })
    }
}

/// The drawn view a session's names read.
#[derive(Clone, Copy, Debug)]
pub(crate) struct View {
    /// `cg.time`, ms.
    pub now: i32,
    pub eye: Vec3,
    pub forward: Vec3,
    pub third_person: bool,
    /// The drawn playerstate's `pm_type`.
    pub pm_type: u32,
}

/// [`OverheadNames::frame`] off a session's newest snapshot, its world
/// and solids and its shellshock, as `CG_Draw2D` runs it (`iw3mp.exe`
/// 0x42f656, 0x42f6c9): for a spectator (`pm_type` 4) or a player
/// before death (under 7), not at the intermission (5) nor under a
/// script menu. `head` and `origin` place a player entity.
pub(crate) fn session_frame(
    names: &mut OverheadNames,
    s: &crate::play::cod4::Cod4Session,
    view: View,
    crosshair: &CrosshairTarget,
    head: &dyn Fn(u32) -> Option<Vec3>,
    origin: &dyn Fn(u32) -> Vec3,
    dv: &Dvars,
) -> Vec<Name> {
    let Some(snap) = s.client().snapshots.last() else {
        return Vec::new();
    };
    if view.pm_type == 5 || view.pm_type >= 7 || s.menus.open().is_some() {
        return Vec::new();
    }
    let (parms, start, duration) = crate::play::cod4_shock::of_session(s);
    let pass = snap.ps.get("clientNum").unwrap_or(0);
    let trace = s.world().map(|w| {
        move |a: Vec3, b: Vec3| {
            use vcod_cod4::pmove::PmWorld;
            let world = crate::play::cod4_predict::Solids {
                clip: &w.clip,
                solids: &s.shot_solids,
            };
            world
                .trace(a, b, Vec3::ZERO, Vec3::ZERO, pass, 0x280_3001)
                .entity_num
        }
    });
    let f = Frame {
        snap,
        now: view.now,
        eye: view.eye,
        forward: view.forward,
        third_person: view.third_person,
        flashed: crate::play::cod4_shock::flashbanged(parms.as_ref(), start, duration, view.now),
        crosshair,
        head,
        origin,
        trace: trace.as_ref().map(|t| t as &dyn Fn(Vec3, Vec3) -> u32),
    };
    names.frame(&f, dv)
}

/// How a name's client stands to us (`CG_RelativeTeamColor`, `iw3mp.exe`
/// 0x431f40): its `g_TeamColor_*` dvar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Relation {
    Spectator,
    MyTeam,
    EnemyTeam,
}

impl Relation {
    /// A spectator's client is `Spectator`; ourselves (`cg.clientNum`) or
    /// one sharing our team, team 0 excepted, `MyTeam`; else `EnemyTeam`.
    pub fn of(client: u32, theirs: i32, me: u32, mine: i32) -> Relation {
        if theirs == 3 {
            Relation::Spectator
        } else if client == me || (theirs != 0 && theirs == mine) {
            Relation::MyTeam
        } else {
            Relation::EnemyTeam
        }
    }

    pub fn dvar(self) -> &'static str {
        match self {
            Relation::Spectator => "g_TeamColor_Spectator",
            Relation::MyTeam => "g_TeamColor_MyTeam",
            Relation::EnemyTeam => "g_TeamColor_EnemyTeam",
        }
    }
}

/// One name's look: its text and colour, and its rank's icon (an image)
/// and level.
pub(crate) struct Look<'a> {
    pub font: &'a Font,
    pub text: &'a str,
    pub rgb: [f32; 3],
    pub rank_icon: Option<String>,
    pub rank_level: Option<String>,
}

/// The size factor for a name `dist_sq` from the eye (0x42d8cb..0x42d952):
/// 1 inside `near`, `far_scale` past `far`, the straight line between.
pub(crate) fn distance_scale(dist_sq: f32, dv: &Dvars) -> f32 {
    let (near, far) = (dv.near_dist, dv.far_dist);
    if dist_sq < near * near {
        1.0
    } else if dist_sq > far * far {
        dv.far_scale
    } else {
        let t = (dist_sq.sqrt() - near) / (far - near);
        1.0 - t + t * dv.far_scale
    }
}

/// `iw3mp.exe` 0x42d770: the name centred on where `n` projects, in real
/// pixels, its baseline there, `cg_overheadNamesSize` times the distance
/// scale (a line 48 pixels at 1), style 3 with `cg_overheadNamesGlow`;
/// left of it the rank's icon, `cg_overheadIconSize` of the line square
/// and centred on its middle, then the level at `cg_overheadRankSize`, a
/// quarter of its line low, ending twice the distance scale short of the
/// name, both white.
pub(crate) fn quads(n: &Name, look: &Look, eye: &Eye, dv: &Dvars, out: &mut Vec<HudQuad>) {
    use crate::cod4_hud::{glow_text, quad, text};
    if n.alpha <= 0.001 {
        return;
    }
    let at = n.at();
    let Some(p) = eye.project(at) else {
        return;
    };
    let f = look.font;
    let scale = distance_scale((at - eye.origin).length_squared(), dv);
    // A line `48 * size` real pixels tall; `font::layout` sets its top
    // `max_height` glyph pixels over the baseline.
    let size = dv.size * scale;
    let px = size / f.unit_scale();
    let w = font::measure(f, look.text, px);
    let x = (p.x - w * 0.5 + 0.5).floor();
    let base = (p.y + 0.5).floor();
    let top = |size: f32| base - f.max_height as f32 * f.glyph_scale * size;
    let [r, g, b] = look.rgb;
    let mut glow = dv.glow;
    glow[3] *= n.alpha;
    glow_text(
        f,
        look.text,
        x,
        top(size),
        px,
        [r, g, b, n.alpha],
        glow,
        out,
    );
    let Some(icon) = &look.rank_icon else {
        return;
    };
    let line = 48.0 * size;
    let icon_px = dv.icon_size * line;
    let rank = dv.rank_size * scale;
    let rank_px = rank / f.unit_scale();
    let level = look.rank_level.as_deref().unwrap_or("");
    let icon_x = x - icon_px - font::measure(f, level, rank_px) - 2.0 * scale;
    let icon_y = base - (icon_px + line) * 0.5;
    let white = [1.0, 1.0, 1.0, n.alpha];
    out.push(quad(
        icon_x,
        icon_y,
        icon_px,
        icon_px,
        white,
        icon.clone(),
        false,
    ));
    let rank_top = top(rank) + 0.25 * 48.0 * rank;
    text(f, level, icon_x + icon_px, rank_top, rank_px, white, out);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 0x42de60 at the crosshair's enemy fades (250 in, 250 out): nothing
    /// while the sighting is younger than the fade-in, full on the frame
    /// it is seen past it, then down to 0 over the fade-out.
    #[test]
    fn a_name_waits_out_its_fade_in_then_fades_out() {
        assert_eq!(alpha(1249, 1000, 1249, 250, 250), 0.0);
        assert_eq!(alpha(1250, 1000, 1250, 250, 250), 1.0);
        assert_eq!(alpha(1350, 1000, 1250, 250, 250), 0.6);
        assert_eq!(alpha(1500, 1000, 1250, 250, 250), 0.0);
    }

    /// 0x42d8cb..0x42d952: full size inside 256, 0.6 past 1024, between
    /// on the straight line.
    #[test]
    fn names_shrink_between_the_near_and_far_distances() {
        let dv = Dvars::default();
        assert_eq!(distance_scale(255.0 * 255.0, &dv), 1.0);
        assert_eq!(distance_scale(640.0 * 640.0, &dv), 0.8);
        assert_eq!(distance_scale(1024.0 * 1024.0, &dv), 0.6);
        assert_eq!(distance_scale(4096.0 * 4096.0, &dv), 0.6);
    }
}
