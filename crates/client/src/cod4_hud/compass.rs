//! The compass and radar: the minimap, the sweep, the ticker, objective
//! marks and the players' and vehicles' icons.

use vcod_cod4::hud as h;
use vcod_cod4::net::parse::Snapshot;

use super::assets::Loaded;
use super::{
    Cod4Hud, EF_COMPASS_PING, EF_PING, EF_TURRET_ACTIVE, ET_HELICOPTER, ET_PLANE, client_u32,
    image_of, quad,
};
use crate::cod4_radar::{COMPASS_RANGE, MapAxes, SWEEP_HALF};
use crate::hud::HudQuad;
use crate::hud::cod2elem::ScreenPlacement;
use crate::play::cod4::{Cod4Session, Fired, ent_u32};

impl Cod4Hud {
    /// One frame of the compass's radar at `now`: the newest snapshot's
    /// client states, players, helicopters and planes, `iCompassPlayerInfo`
    /// and the frame's shots (cod4-hud.md 15).
    pub(super) fn step_radar(&mut self, s: &Cod4Session, fired: &[Fired], now: i32) {
        use crate::cod4_radar::{Body, Frame, Info, Vehicle};
        let Some(snap) = s.client().snapshots.last() else {
            return;
        };
        // Every helicopter and plane in the snapshot (cod4-hud.md 15.7).
        for e in snap
            .entities
            .iter()
            .filter(|e| matches!(e.etype(), ET_HELICOPTER | ET_PLANE))
        {
            let o = crate::play::cod4::position_at(e, now);
            let tao = ent_u32(e, "lerp.u.vehicle.teamAndOwnerIndex");
            self.vehicles.seen(
                now,
                Vehicle {
                    entity: e.number(),
                    etype: e.etype(),
                    last_update: now,
                    pos: [o.x, o.y],
                    yaw: crate::play::cod4::yaw_at(e, now),
                    team: tao & 3,
                    owner: tao >> 2,
                },
            );
        }
        let shots = self.shots(snap, fired);
        let mut infos = [Info::default(); 64];
        for c in &snap.clients {
            if let Some(i) = infos.get_mut(c.0[0] as usize) {
                *i = Info {
                    valid: true,
                    team: client_u32(c, "team"),
                    perks: client_u32(c, "perks"),
                };
            }
        }
        let bodies: Vec<Body> = snap
            .entities
            .iter()
            .filter(|e| e.etype() == crate::play::cod4::ET_PLAYER)
            .map(|e| {
                let o = crate::play::cod4::origin_of(e);
                let client = ent_u32(e, "clientNum");
                let eflags = ent_u32(e, "lerp.eFlags");
                Body {
                    client: client as usize,
                    pos: [o.x, o.y],
                    turret: eflags & EF_TURRET_ACTIVE != 0,
                    yaw: crate::play::cod4::ent_f32(e, "lerp.apos.trBase[1]"),
                    pinged: eflags & EF_PING != 0,
                    talking: s.is_talking(client),
                }
            })
            .collect();
        let ps = &snap.ps;
        let f = |n: &str| ps.get_f32(n).unwrap_or(0.0);
        let map = self.minimap(s).map(|(_, m)| m);
        self.radar.step(&Frame {
            time: now,
            radar: ps.get("radarEnabled").unwrap_or(0) != 0,
            map,
            me: ps.get("clientNum").unwrap_or(0) as usize,
            infos: &infos,
            bodies: &bodies,
            compass_info: ps.get("iCompassPlayerInfo").unwrap_or(0),
            compass_pinged: ps.get("eFlags").unwrap_or(0) & EF_COMPASS_PING != 0,
            view: [f("origin[0]"), f("origin[1]")],
            shots: &shots,
        });
    }

    /// The fire sound's length the audio gave a shot from `client` this
    /// frame ([`crate::cod4_radar::Radar::hold_shot`]).
    pub(crate) fn hold_shot(&mut self, client: usize, ms: i32) {
        self.radar.hold_shot(client, ms);
    }

    /// The shots the compass hears (cod4-hud.md 15.7): another
    /// player's unsilenced, non-grenade fire event where he stands, and a
    /// turret's at the turret, its gunner the event's parm. A player's
    /// shot holds 0 ms here; the audio then hands the fire sound's length
    /// to [`Self::hold_shot`].
    fn shots(&self, snap: &Snapshot, fired: &[Fired]) -> Vec<crate::cod4_radar::Shot> {
        use crate::play::cod4::ET_PLAYER;
        let mut out = Vec::new();
        let Some(Some(l)) = &self.loaded else {
            return out;
        };
        let me = snap.ps.get("clientNum").unwrap_or(0);
        let player = |n: u32| {
            snap.entities
                .iter()
                .find(|e| e.number() == n && e.etype() == ET_PLAYER)
        };
        for f in fired {
            let Some(n) = f.entity else { continue };
            match f.name {
                "EV_FIRE_WEAPON" | "EV_FIRE_WEAPON_LASTSHOT" => {
                    let Some(e) = f.state.as_ref().filter(|e| e.etype() == ET_PLAYER) else {
                        continue;
                    };
                    let client = ent_u32(e, "clientNum");
                    let def = (f.weapon as usize)
                        .checked_sub(1)
                        .and_then(|w| self.weapons.get(w))
                        .and_then(|w| l.weapon_hud.get(w));
                    if client == me || def.is_none_or(|d| d.silenced || d.def.weap_type == 1) {
                        continue;
                    }
                    out.push(crate::cod4_radar::Shot {
                        client: client as usize,
                        pos: [f.origin.x, f.origin.y],
                        hold_ms: 0,
                    });
                }
                "EV_FIRE_WEAPON_MG42" => {
                    let Some(gunner) = player(f.parm) else {
                        continue;
                    };
                    if snap.ps.get("viewlocked_entNum") == Some(n) {
                        continue;
                    }
                    out.push(crate::cod4_radar::Shot {
                        client: ent_u32(gunner, "clientNum") as usize,
                        pos: [f.origin.x, f.origin.y],
                        hold_ms: 50,
                    });
                }
                _ => {}
            }
        }
        out
    }
}

impl Cod4Hud {
    /// configstring 822, the north yaw in degrees (0 when unset).
    pub(super) fn north_yaw(&self, s: &Cod4Session) -> f32 {
        self.configstring(s, h::CS_NORTH_YAW)
            .trim()
            .parse()
            .unwrap_or(0.0)
    }

    /// configstring 823, `compass_map_<map> x0 y0 x1 y1`: the minimap's
    /// material and where it lies (cod4-hud.md 6). A corner that does not
    /// parse reads 0; `None` while the string is empty.
    pub(crate) fn minimap(&self, s: &Cod4Session) -> Option<(String, MapAxes)> {
        let cs = self.configstring(s, h::CS_MINIMAP);
        let mut words = h::tokenize(&cs).into_iter();
        let material = words.next()?;
        let mut n = [0.0f32; 4];
        for (slot, w) in n.iter_mut().zip(words) {
            *slot = w.parse().unwrap_or(0.0);
        }
        let map = MapAxes::new(self.north_yaw(s), [n[0], n[1]], [n[2], n[3]]);
        Some((material, map))
    }

    /// Each active or current objective of the newest snapshot (cod4-hud.md
    /// 10, 22): its icon (configstring 2002 + `icon`), where it is (its
    /// entity's place, else its own origin) and whether it is current.
    pub(super) fn objective_marks(&self, s: &Cod4Session, snap: &Snapshot) -> Vec<ObjectiveMark> {
        let field = |o: &vcod_cod4::net::delta::Objective, name: &str| {
            vcod_cod4::net::fields::OBJECTIVE
                .iter()
                .find(|f| f.name == name)
                .map_or(0, |f| o.words[f.word()])
        };
        snap.ps
            .objectives
            .iter()
            .filter(|o| o.state == OBJST_ACTIVE || o.state == OBJST_CURRENT)
            .map(|o| {
                let ent = field(o, "entNum");
                let entity = (ent != ENTITYNUM_NONE)
                    .then(|| snap.entities.iter().find(|e| e.number() == ent))
                    .flatten();
                let at = entity.map_or_else(
                    || {
                        let axis = |k: usize| f32::from_bits(field(o, &format!("origin[{k}]")));
                        glam::Vec3::new(axis(0), axis(1), axis(2))
                    },
                    crate::play::cod4::origin_of,
                );
                ObjectiveMark {
                    material: self.configstring(s, h::CS_MATERIALS + field(o, "icon") as u16),
                    at,
                    current: o.state == OBJST_CURRENT,
                }
            })
            .collect()
    }

    /// `compass_old` (cod4-hud.md 6, 15.7): its background, the
    /// tickertape, the minimap turned with the view, the UAV's sweep over
    /// it, the pointers, friendlies, enemies, helicopters and planes, and
    /// last the player's arrow.
    pub(super) fn draw_compass(
        &self,
        l: &Loaded,
        s: &Cod4Session,
        (eye, yaw): (glam::Vec3, f32),
        now: i32,
        sp: &ScreenPlacement,
        out: &mut Vec<HudQuad>,
    ) {
        let to_screen = |item: &h::MenuRect, [x, y, w, h]: [f32; 4]| {
            let (sx, sw) = sp.axis(0, x, w, item.horz_align);
            let (sy, sh) = sp.axis(1, y, h, item.vert_align);
            (sx, sy, sw, sh)
        };
        let backed = |name: &str| {
            let item = l.compass.get(name)?;
            let tex = item.background.as_deref().and_then(|m| image_of(l, m))?;
            Some((item, tex))
        };
        if let Some((bg, tex)) = backed("minimapBG") {
            let (x, y, w, h) = to_screen(bg, bg.rect);
            out.push(quad(x, y, w, h, [1.0; 4], tex, false));
        }
        self.draw_ticker(l, s, sp, (eye, yaw), out);
        let Some(map_item) = l.compass.get("mini_map") else {
            return;
        };
        let (mx, my, mw, mh) = to_screen(map_item, map_item.rect);
        let over_map = |uvs, texture: String| HudQuad {
            uvs,
            ..HudQuad::rect([mx, my, mw, mh], [1.0; 4], texture)
        };
        let minimap = self.minimap(s);
        if let Some((material, map)) = &minimap
            && let Some(tex) = image_of(l, material)
        {
            out.push(over_map(minimap_uvs(map, [eye.x, eye.y], yaw), tex));
        }
        let uav = s
            .client()
            .snapshots
            .last()
            .is_some_and(|snap| snap.ps.get("radarEnabled").unwrap_or(0) != 0);
        if uav
            && let Some((_, map)) = &minimap
            && let Some(tex) = image_of(l, "compass_radarline")
        {
            let progress = self.radar.progress_at(now);
            out.push(over_map(sweep_uvs(map, progress, [eye.x, eye.y], yaw), tex));
        }
        if let Some(icons_item) = l.compass.get("compassfriendlies") {
            let crosshair = backed("compasspointers").map(|(_, tex)| tex);
            for icon in self.compass_icons(l, s, eye, yaw) {
                let Some(tex) = image_of(l, &icon.material) else {
                    continue;
                };
                let [cx, cy] = icon.centre;
                if icon.lines
                    && let Some(line) = &crosshair
                {
                    let [rx, ry, rw, rh] = icons_item.rect;
                    for r in [[rx, cy - 1.0, rw, 2.0], [cx - 1.0, ry, 2.0, rh]] {
                        let (x, y, w, h) = to_screen(icons_item, r);
                        out.push(quad(x, y, w, h, [1.0; 4], line.clone(), false));
                    }
                }
                let (x, y, _, _) = to_screen(icons_item, [cx, cy, 0.0, 0.0]);
                let (_, _, w, h) = to_screen(icons_item, [0.0, 0.0, icon.size, icon.size]);
                out.push(spun_quad([x, y], [w, h], -icon.turn, icon.alpha, tex));
            }
        }
        if let Some((arrow, tex)) = backed("compassplayer") {
            let [rx, ry, rw, rh] = arrow.rect;
            let at = [
                rx + 0.5 * (rw - PLAYER_ARROW),
                ry + 0.5 * (rh - PLAYER_ARROW),
                PLAYER_ARROW,
                PLAYER_ARROW,
            ];
            let (x, y, w, h) = to_screen(arrow, at);
            out.push(quad(x, y, w, h, [1.0; 4], tex, false));
        }
    }

    /// The compass's icons on the `compassfriendlies` rect (cod4-hud.md 10,
    /// 15): each active or current objective's, each teammate's arrow
    /// turned by his yaw, each enemy the radar pinged, and the
    /// helicopters and planes. Friendlies need us on a team. An icon past
    /// the compass's edge sits on the edge (`compassClampIcons` 1).
    fn compass_icons(
        &self,
        l: &Loaded,
        s: &Cod4Session,
        eye: glam::Vec3,
        yaw: f32,
    ) -> Vec<CompassIcon> {
        let mut out = Vec::new();
        let (Some(r), Some(snap)) = (
            l.compass.get("compassfriendlies"),
            s.client().snapshots.last(),
        ) else {
            return out;
        };
        let rect = r.rect;
        let centre = [rect[0] + rect[2] * 0.5, rect[1] + rect[3] * 0.5];
        let (ys, yc) = yaw.to_radians().sin_cos();
        let at = |x: f32, y: f32| {
            let o = world_to_compass(rect, [yc, ys], eye, glam::Vec3::new(x, y, 0.0));
            [centre[0] + o[0], centre[1] + o[1]]
        };
        let icon =
            |material: String, centre: [f32; 2], size: f32, turn: f32, alpha: f32| CompassIcon {
                material,
                centre,
                size,
                turn,
                lines: false,
                alpha,
            };
        let me = snap.ps.get("clientNum").unwrap_or(0);
        let team_of = |n: u32| {
            snap.clients
                .iter()
                .find(|c| c.0[0] == n)
                .map_or(0, |c| client_u32(c, "team"))
        };
        let my_team = team_of(me);
        for mark in self.objective_marks(s, snap) {
            out.push(CompassIcon {
                lines: mark.current,
                ..icon(
                    mark.material,
                    at(mark.at.x, mark.at.y),
                    OBJECTIVE_SIZE,
                    0.0,
                    1.0,
                )
            });
        }
        // The radar's teammates (0x429880). Under a shot's or a talker's
        // ping at full alpha the arrow itself is not drawn.
        if matches!(my_team, 1 | 2) {
            for f in self.radar.friendlies(self.radar.time(), me as usize) {
                let spot = at(f.pos[0], f.pos[1]);
                if f.chat_icon {
                    out.push(icon(
                        "objective_friendly_chat".into(),
                        spot,
                        FRIENDLY_SIZE,
                        0.0,
                        1.0,
                    ));
                    continue;
                }
                let arrow = (f.firing != Some(1.0) && f.voice != Some(1.0))
                    .then_some(("compassping_friendly_mp", 1.0));
                let voice = f.voice.map(|a| ("compassping_friendlyyelling_mp", a));
                let shot = f.firing.map(|a| ("compassping_friendlyfiring_mp", a));
                for (material, alpha) in arrow.into_iter().chain(voice).chain(shot) {
                    out.push(icon(
                        material.into(),
                        spot,
                        FRIENDLY_SIZE,
                        f.yaw - yaw,
                        alpha,
                    ));
                }
            }
        }
        // The radar's enemy pings, upright; none for a spectator.
        if my_team != 3 {
            let show_all = self
                .server_dvar(s, "g_compassShowEnemies")
                .is_some_and(|v| vcod_cod4::menuexp::atoi(&v) != 0);
            for p in self.radar.pings(self.radar.time(), show_all) {
                let material = match p.firing {
                    true => "compassping_enemyfiring",
                    false => "compassping_enemy",
                };
                out.push(icon(
                    material.into(),
                    at(p.pos[0], p.pos[1]),
                    FRIENDLY_SIZE,
                    0.0,
                    p.alpha,
                ));
            }
        }
        // Helicopters and planes (owner draws 155 and 156, cod4-hud.md
        // 15.7): `_busy` for another team's (in free for all another
        // owner's), `_friendly` for ours, turned by their yaw.
        let own = s.gamestate().map_or(me, |g| g.client_num as u32);
        let own_team = team_of(own);
        for (etype, kind, size) in [
            (ET_HELICOPTER, "helicopter", HELICOPTER_SIZE),
            (ET_PLANE, "airstrike", PLANE_SIZE),
        ] {
            for v in self.vehicles.drawn(self.radar.time(), etype) {
                let theirs = match own_team {
                    0 => own != v.owner,
                    team => team != v.team,
                };
                let side = if theirs { "busy" } else { "friendly" };
                let material = format!("compass_objpoint_{kind}_{side}");
                out.push(icon(
                    material,
                    at(v.pos[0], v.pos[1]),
                    size,
                    v.yaw - yaw,
                    1.0,
                ));
            }
        }
        out
    }
}

/// `compassPlayerWidth` / `Height` 18.75 (cod4-hud.md 6).
const PLAYER_ARROW: f32 = 18.75;
/// `compassFriendlyWidth` / `Height` 18.75 (0x6c6884) times `compassSize`
/// 1, `compassObjectiveWidth` / `Height` 20 (0x6df498); cod4-hud.md 10.
pub(super) const FRIENDLY_SIZE: f32 = 18.75;
pub(super) const OBJECTIVE_SIZE: f32 = 20.0;
/// A helicopter's icon is 40 square and a plane's 20, times `compassSize`
/// (cod4-hud.md 15.7).
const HELICOPTER_SIZE: f32 = 40.0;
const PLANE_SIZE: f32 = 20.0;
/// Objective states 1 (active) and 4 (current).
const OBJST_ACTIVE: u32 = 1;
const OBJST_CURRENT: u32 = 4;
/// An objective with this entity sits at its own origin.
pub(super) const ENTITYNUM_NONE: u32 = 1023;

/// An objective the compass and the full map draw.
pub(crate) struct ObjectiveMark {
    pub material: String,
    pub at: glam::Vec3,
    pub current: bool,
}

/// One icon on the compass, on the grid: its centre, size and turn.
#[derive(Clone, Debug, PartialEq)]
struct CompassIcon {
    pub(super) material: String,
    pub(super) centre: [f32; 2],
    pub(super) size: f32,
    /// Degrees, counterclockwise on screen.
    pub(super) turn: f32,
    /// A current objective's crosshair through it.
    pub(super) lines: bool,
    pub(super) alpha: f32,
}

/// A `size` quad centred on `centre`, turned `degrees` clockwise on
/// screen.
fn spun_quad(
    centre: [f32; 2],
    size: [f32; 2],
    degrees: f32,
    alpha: f32,
    texture: String,
) -> HudQuad {
    let (sin, cos) = degrees.to_radians().sin_cos();
    let corner = |u: f32, v: f32| {
        let (dx, dy) = (u * 0.5 * size[0], v * 0.5 * size[1]);
        [
            centre[0] + dx * cos - dy * sin,
            centre[1] + dx * sin + dy * cos,
        ]
    };
    HudQuad {
        verts: [
            corner(-1.0, -1.0),
            corner(1.0, -1.0),
            corner(1.0, 1.0),
            corner(-1.0, 1.0),
        ],
        uvs: HudQuad::UNIT_UVS,
        rgba: [1.0, 1.0, 1.0, alpha],
        texture,
    }
}

/// The texture coordinates at a screen-aligned square's corners (upper
/// left, upper right, lower right, lower left) for a window of the
/// texture centred on `centre`, `half` its half size, turned `degrees`.
fn window_uvs(centre: [f32; 2], half: [f32; 2], degrees: f32) -> [[f32; 2]; 4] {
    let (sin, cos) = degrees.to_radians().sin_cos();
    [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].map(|(a, b): (f32, f32)| {
        let (u, v) = (a * cos - b * sin, a * sin + b * cos);
        [centre[0] + u * half[0], centre[1] + v * half[1]]
    })
}

/// The minimap under the compass (cod4-hud.md 6): centred on the view's
/// place on the map, `compassMaxRange` across, turned by `northYaw -
/// viewYaw` so the view points up.
fn minimap_uvs(map: &MapAxes, eye: [f32; 2], yaw: f32) -> [[f32; 2]; 4] {
    let half = map.extent.map(|e| 0.5 * COMPASS_RANGE / e);
    window_uvs(map.fraction(eye), half, map.north_yaw - yaw)
}

/// The UAV's sweep over the compass (cod4-hud.md 15.7): `compass_radarline`
/// placed by the view's distance from the line, turned as the map is.
fn sweep_uvs(map: &MapAxes, progress: f32, eye: [f32; 2], yaw: f32) -> [[f32; 2]; 4] {
    let s = crate::cod4_radar::sweep_texture(map, progress, eye);
    window_uvs([s, 0.0], [SWEEP_HALF; 2], map.north_yaw - yaw)
}

/// A world point on the compass rect, off its centre (cod4-hud.md 10):
/// the offset from the eye at the rect's height over `compassMaxRange` an
/// inch, the view's yaw vector `up` pointing up the screen. A point past
/// an edge is pulled in along its line from the centre until it sits on
/// the rect (`compassClampIcons`).
fn world_to_compass(rect: [f32; 4], up: [f32; 2], eye: glam::Vec3, at: glam::Vec3) -> [f32; 2] {
    let scale = rect[3] / COMPASS_RANGE;
    let d = [(at.x - eye.x) * scale, (at.y - eye.y) * scale];
    let right = [up[1], -up[0]];
    let screen = [
        d[0] * right[0] + d[1] * right[1],
        -(d[0] * up[0] + d[1] * up[1]),
    ];
    let fit = (0..2)
        .map(|k| {
            let half = rect[2 + k] * 0.5;
            if screen[k].abs() > half {
                half / screen[k].abs()
            } else {
                1.0
            }
        })
        .fold(1.0, f32::min);
    screen.map(|v| v * fit)
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::super::tests::{close, replay};
    use crate::console::shell::Shell;

    /// The minimap's texture window: centred on the view's place on the
    /// map, `compassMaxRange` across, turned by `northYaw - viewYaw`; and
    /// the sweep's line over it (cod4-hud.md 6, 15.7).
    #[test]
    fn the_minimap_and_the_sweep_turn_with_the_view() {
        let cases = [
            (
                0.0,
                [-2000.0, 3000.0],
                [3000.0, -1000.0],
                [500.0, 200.0],
                30.0,
                [
                    0.27311707, 0.59150636, 0.8143829, 0.84150636, 1.1268829, 0.40849364,
                    0.58561707, 0.15849365,
                ],
                [
                    [
                        3.8602352, -0.4575317, 6.025298, -1.7075317, 7.275298, 0.4575317, 5.110235,
                        1.7075317,
                    ],
                    [
                        1.2995749, -0.4575317, 3.4646382, -1.7075317, 4.714638, 0.4575317,
                        2.5495749, 1.7075317,
                    ],
                    [
                        -2.541415, -0.4575317, -0.3763516, -1.7075317, 0.8736484, 0.4575317,
                        -1.291415, 1.7075317,
                    ],
                ],
            ),
            (
                90.0,
                [-1000.0, 1500.0],
                [2500.0, -1000.0],
                [-300.0, 100.0],
                200.0,
                [
                    -0.013454467,
                    1.2008562,
                    -0.25775456,
                    0.2611637,
                    0.41345447,
                    -0.08085638,
                    0.65775454,
                    0.8588362,
                ],
                [
                    [
                        2.7206764, 1.6021409, 1.865626, -0.7470906, 4.2148576, -1.6021409,
                        5.069908, 0.7470906,
                    ],
                    [
                        0.31001616,
                        1.6021409,
                        -0.54503417,
                        -0.7470906,
                        1.8041973,
                        -1.6021409,
                        2.6592476,
                        0.7470906,
                    ],
                    [
                        -3.305974, 1.6021409, -4.161024, -0.7470906, -1.8117925, -1.6021409,
                        -0.9567424, 0.7470906,
                    ],
                ],
            ),
            (
                0.0,
                [0.0, 0.0],
                [0.0, 0.0],
                [100.0, 100.0],
                0.0,
                [-1.35, -1.35, 1.15, -1.35, 1.15, 1.15, -1.35, 1.15],
                [
                    [
                        1.417767, -1.25, 3.917767, -1.25, 3.917767, 1.25, 1.417767, 1.25,
                    ],
                    [
                        -0.24289322,
                        -1.25,
                        2.2571068,
                        -1.25,
                        2.2571068,
                        1.25,
                        -0.24289322,
                        1.25,
                    ],
                    [
                        -2.7338834,
                        -1.25,
                        -0.23388326,
                        -1.25,
                        -0.23388326,
                        1.25,
                        -2.7338834,
                        1.25,
                    ],
                ],
            ),
            (
                -45.0,
                [-3000.0, 0.0],
                [0.0, -2000.0],
                [-2500.0, -100.0],
                -120.0,
                [
                    -0.8499998,
                    0.5530127,
                    -1.7650634,
                    -0.12999997,
                    1.6499997,
                    -0.3130127,
                    2.5650635,
                    0.36999997,
                ],
                [
                    [
                        3.3688078,
                        -1.5309311,
                        4.0158553,
                        0.88388336,
                        1.6010408,
                        1.5309311,
                        0.9539933,
                        -0.88388336,
                    ],
                    [
                        2.2202797,
                        -1.5309311,
                        2.8673272,
                        0.88388336,
                        0.45251286,
                        1.5309311,
                        -0.1945349,
                        -0.88388336,
                    ],
                    [
                        0.49748755,
                        -1.5309311,
                        1.1445352,
                        0.88388336,
                        -1.2702793,
                        1.5309311,
                        -1.9173269,
                        -0.88388336,
                    ],
                ],
            ),
        ];
        let flat = |q: [[f32; 2]; 4]| -> [f32; 8] { std::array::from_fn(|i| q[i / 2][i % 2]) };
        for (north, ul, lr, eye, yaw, map, sweeps) in cases {
            let m = MapAxes::new(north, ul, lr);
            close(flat(minimap_uvs(&m, eye, yaw)), map);
            for (p, want) in [0.0, 0.3, 0.75].into_iter().zip(sweeps) {
                close(flat(sweep_uvs(&m, p, eye, yaw)), want);
            }
        }
    }

    /// A point on the compass: off the eye at the rect's height over
    /// `compassMaxRange` an inch, the view's yaw up, and one past the
    /// edge pulled back onto it (cod4-hud.md 10).
    #[test]
    fn world_points_land_on_the_compass() {
        let cases = [
            (
                30.0f32,
                [0.0, 0.0, 0.0],
                [500.0, 200.0, 50.0],
                [3.1332335, -21.74692],
            ),
            (0.0, [100.0, 100.0, 0.0], [100.0, 1100.0, 0.0], [-40.8, 0.0]),
            (
                90.0,
                [0.0, 0.0, 0.0],
                [5000.0, 100.0, 0.0],
                [51.0, -1.0199977],
            ),
            (
                -150.0,
                [10.0, -20.0, 0.0],
                [-3000.0, 4000.0, 0.0],
                [51.0, -6.103284],
            ),
        ];
        for (yaw, eye, at, want) in cases {
            let (s, c) = yaw.to_radians().sin_cos();
            let got = world_to_compass(
                [6.0, 18.0, 102.0, 102.0],
                [c, s],
                glam::Vec3::from(eye),
                glam::Vec3::from(at),
            );
            close(got, want);
        }
    }

    /// The walker's compass pings its idle teammate (`mate`, allies)
    /// once both are in the world; no enemy shows without a UAV, and the
    /// war round carries no objective (cod4-hud.md 10).
    #[test]
    fn walk_capture_pings_the_teammate_on_the_compass() {
        let (mut frames, mut pinged, mut others) = (0, 0, 0);
        let mut tex = false;
        let (hud, _) = replay("play-1.7-walk.txt", |hud, s, now| {
            let (Some(Some(l)), Some(scene)) = (&hud.loaded, s.scene()) else {
                return;
            };
            frames += 1;
            let icons = hud.compass_icons(l, s, scene.eye, scene.angles.y);
            for i in &icons {
                if i.material == "compassping_friendly_mp" {
                    pinged += 1;
                    let [x, y] = i.centre;
                    assert!(
                        (6.0..=108.0).contains(&x) && (18.0..=120.0).contains(&y),
                        "{i:?}"
                    );
                } else {
                    others += 1;
                }
            }
            if !icons.is_empty() && !tex {
                // `compassping_friendly_mp` draws `common_mp`'s image by
                // reference (`,compassping_green_hollow_mp`).
                let q = hud.build(
                    s,
                    (1280.0, 960.0),
                    now,
                    Some((scene.eye, scene.angles.y)),
                    &Shell::new(),
                );
                tex |= q
                    .iter()
                    .any(|q| q.texture == "images/compassping_green_hollow_mp.iwi");
            }
        });
        if hud.loaded.is_none() {
            return;
        }
        println!("{frames} frames, {pinged} pings, {others} others");
        assert!(pinged > frames / 2, "{pinged} of {frames}");
        assert_eq!(others, 0);
        assert!(tex, "the ping's image draws");
    }

    /// Every committed 1.7 capture's playerstate carries
    /// `iCompassPlayerInfo` 0, the walk capture's while its teammate,
    /// whom `G_GetNonPVSPlayerInfo` scans without a UAV, stands far off:
    /// 1.7's `SV_inSnapshot` finds every playing client in the snapshot
    /// (cod4-hud.md 15.3), as vcod-server's snapshot is.
    #[test]
    fn retail_captures_send_no_compass_player_info() {
        let mut mates = 0;
        for name in [
            "play-1.7-walk.txt",
            "play-1.7-spec.txt",
            "play-1.7-fly.txt",
            "play-1.7-kill-shooter.txt",
            "play-1.7-kill-target.txt",
            "play-1.7-mantle.txt",
            "play-1.7-menus-together.txt",
            "play-1.7-prematch.txt",
            "play-1.7-score-target.txt",
        ] {
            let mut last = None;
            replay(name, |_, s, _| {
                let Some(snap) = s.client().snapshots.last() else {
                    return;
                };
                if last == Some(snap.server_time) {
                    return;
                }
                last = Some(snap.server_time);
                assert_eq!(snap.ps.get("iCompassPlayerInfo").unwrap_or(0), 0, "{name}");
                let me = snap.ps.get("clientNum").unwrap_or(0);
                let team = |n: u32| {
                    snap.clients
                        .iter()
                        .find(|c| c.0[0] == n)
                        .map_or(0, |c| client_u32(c, "team"))
                };
                mates += snap
                    .entities
                    .iter()
                    .filter(|e| e.etype() == crate::play::cod4::ET_PLAYER)
                    .filter(|e| {
                        let n = ent_u32(e, "clientNum");
                        n != me && matches!(team(me), 1 | 2) && team(n) == team(me)
                    })
                    .count();
            });
        }
        assert!(mates > 100, "{mates}");
    }
}
