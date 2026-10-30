//! CoD2's sprites in the world, drawn over the scene before the 2D HUD:
//! the script's waypoint hudelems (`setWaypoint`, type 13:
//! `CG_Draw3dHudElems`, Mac 0x1e1ade) and the icons over players' heads
//! (`CG_DrawPlayerSprites`, Mac 0x1cef3e). Their materials test no depth,
//! so each is a camera-facing quad projected onto the screen
//! (cod2-hud.md 19).

use std::collections::HashMap;

use glam::{Mat4, Vec3, Vec4};
use vcod_cod2::hudelem::HudElem;
use vcod_cod2::net::Frame;
use vcod_cod2::net::delta::field_index;
use vcod_cod2::net::fields::CLIENT_STATE_FIELDS;

use crate::hud::HudQuad;

/// `ET_PLAYER`.
const ET_PLAYER: i32 = 1;
/// `he_type_t` 13, what `HECmd_SetWaypoint` (lnxded 0x81044c0) writes.
const HE_TYPE_WAYPOINT: i32 = 13;
/// The head icon block: an entity's `iHeadIcon` n is configstring
/// `0x1e + n` (`CG_PlayerSprites`, Mac 0x1e662c).
const CS_HEAD_ICONS: usize = 0x1e;
const TEAM_SPECTATOR: i32 = 3;
/// `eFlags` 0x80: the client's connection is interrupted.
const EF_DISCONNECTED: i32 = 0x80;
/// `eFlags` 0x200000: the client is typing.
const EF_TALK: i32 = 0x200000;
/// Over `j_head`, or over the origin for a body with no head
/// (Mac 0x326658 21, 0x326654 82).
const ABOVE_HEAD: f32 = 21.0;
const ABOVE_ORIGIN: f32 = 82.0;
/// Every icon's radius is its size dvar plus 10 (Mac 0x32626c); the
/// second sprite stacks 16 plus `cg_scriptIconSize` over a head icon
/// (0x326260), the talk balloon 5 lower (0x32628c). The size dvars
/// (`cg_scriptIconSize`, `cg_voiceIconSize`, `cg_connectionIconSize`)
/// default to 0.
const ICON_RADIUS: f32 = 10.0;
const STACK: f32 = 16.0;
const TALK_DROP: f32 = 5.0;
/// `cg_headIconMinScreenRadius`'s default: a head icon is at least this
/// much of the screen's height in radius.
const MIN_SCREEN_RADIUS: f32 = 0.02;
/// A constant-size sprite's radius is its size times this, a fraction of
/// the screen's height (Mac 0x326648).
pub(crate) const CONSTANT_SCALE: f32 = 0.0043;
/// `cg_youInKillCamSize`'s default (`CG_RegisterDvars`).
const YOU_IN_KILLCAM_SIZE: f32 = 6.0;

/// The view the scene is drawn with.
pub struct WorldView {
    pub view_proj: Mat4,
    /// The camera's right and up, which a sprite faces along.
    pub right: Vec3,
    pub up: Vec3,
    /// Real pixels.
    pub size: (f32, f32),
}

/// How a sprite's radius reads (`R_AddCmdDrawSprite`'s flags,
/// `RB_TessEntity` case 4, Mac 0xfe7xx).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Size {
    /// World units, at least `min` of the screen's height on screen.
    World { radius: f32, min: f32 },
    /// A fraction of the screen's height whatever the distance, the
    /// sprite raised by its own world radius (flags 0x3000).
    Constant(f32),
}

impl WorldView {
    fn clip(&self, p: Vec3) -> Vec4 {
        self.view_proj * p.extend(1.0)
    }

    /// The quad `sprite` draws, `None` behind the eye.
    pub(crate) fn sprite(&self, at: Vec3, size: Size, rgba: [f32; 4], m: &str) -> Option<HudQuad> {
        let c = self.clip(at);
        if c.w <= 0.0 {
            return None;
        }
        // NDC height of one world unit along the view's up at this depth.
        let k = ((self.clip(at + self.up).y / c.w) - c.y / c.w).abs();
        if k <= 0.0 || !k.is_finite() {
            return None;
        }
        let (r, at) = match size {
            Size::World { radius, min } => {
                let frac = radius * k * 0.5;
                (if frac < min { min / (k * 0.5) } else { radius }, at)
            }
            Size::Constant(frac) => {
                let r = 2.0 * frac / k;
                (r, at + Vec3::new(0.0, 0.0, r))
            }
        };
        let (w, h) = self.size;
        let screen = |p: Vec3| {
            let c = self.clip(p);
            [(c.x / c.w * 0.5 + 0.5) * w, (0.5 - c.y / c.w * 0.5) * h]
        };
        let (rx, uy) = (self.right * r, self.up * r);
        Some(HudQuad {
            verts: [
                screen(at - rx + uy),
                screen(at + rx + uy),
                screen(at + rx - uy),
                screen(at - rx - uy),
            ],
            uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            rgba,
            texture: m.to_string(),
        })
    }
}

/// The waypoints and the head icons for `frame` at cgame time `now`;
/// `heads` is the drawn bodies' `j_head` by entity.
pub fn build(
    frame: &Frame,
    heads: &HashMap<u32, Vec3>,
    view: &WorldView,
    now: i32,
) -> Vec<HudQuad> {
    let mut out = Vec::new();
    waypoints(frame, view, now, &mut out);
    head_icons(frame, heads, view, &mut out);
    out
}

/// `CG_Draw3dHudElems`: the archived then the current elements, sorted as
/// the 2D ones are; each waypoint with an alpha and a material is a white
/// sprite at `(x, y, z)` its height across, scaled from `fromHeight` over
/// `scaleTime`; a `value` above 0 (`setWaypoint(true)`) keeps it one
/// size on screen.
fn waypoints(frame: &Frame, view: &WorldView, now: i32, out: &mut Vec<HudQuad>) {
    let ps = &frame.snap.ps.raw;
    let mut elems: Vec<HudElem> = ps
        .hud_archival
        .iter()
        .chain(ps.hud_current.iter())
        .map(HudElem::from_wire)
        .filter(|e| e.kind == HE_TYPE_WAYPOINT && e.color[3] != 0)
        .collect();
    elems.sort_by(|a, b| a.sort.total_cmp(&b.sort));
    for e in &elems {
        let Some(m) = frame.configstrings.material(e.material) else {
            continue;
        };
        let mut height = e.height as f32;
        let since = now - e.scale_start_time;
        if e.scale_time > 0 && since < e.scale_time {
            let t = (since as f32 / e.scale_time as f32).clamp(0.0, 1.0);
            height = (height - e.from_height as f32) * t + e.from_height as f32;
        }
        if height == 0.0 {
            continue;
        }
        let size = if e.value > 0.0 {
            Size::Constant(height * CONSTANT_SCALE)
        } else {
            Size::World {
                radius: height,
                min: 0.0,
            }
        };
        let a = e.color[3] as f32 / 255.0;
        let at = Vec3::new(e.x, e.y, e.z);
        out.extend(view.sprite(at, size, [1.0, 1.0, 1.0, a], m));
    }
}

/// `CG_PlayerSprites` for every player entity: the script's head icon,
/// shown to everyone, a spectator or the icon's team, then over it the
/// killcam's mark on the local client's body, else the disconnected icon,
/// else for a teammate or a spectator the talk balloon. The voice chat
/// icon is left out: nothing in the cgame sets its timer (cod2-hud.md
/// 19.2).
fn head_icons(frame: &Frame, heads: &HashMap<u32, Vec3>, view: &WorldView, out: &mut Vec<HudQuad>) {
    let ps = &frame.snap.ps;
    let team = field_index(&CLIENT_STATE_FIELDS, "team");
    let team_of = |client: i32| {
        frame
            .snap
            .clients
            .iter()
            .find(|c| c.number as i32 == client)
            .map(|c| c.f[team] as i32)
    };
    let Some(viewer) = team_of(ps.client_num) else {
        return;
    };
    let killcam = ps.raw.i32("deltaTime") != 0;
    for e in frame.snap.entities.iter().filter(|e| e.etype == ET_PLAYER) {
        // The viewed body, seen from inside: `CG_PlayerFloatSprite` skips
        // it in the first person (`ps.pm_flags` 0xc00000 while following).
        if e.client_num == ps.client_num {
            continue;
        }
        let Some(his) = team_of(e.client_num) else {
            continue;
        };
        let place = |offset: f32| match heads.get(&e.number) {
            Some(h) => *h + Vec3::new(0.0, 0.0, offset.trunc() + ABOVE_HEAD),
            None => frame.entity_origin(e) + Vec3::new(0.0, 0.0, offset.trunc() + ABOVE_ORIGIN),
        };
        let world = Size::World {
            radius: ICON_RADIUS,
            min: MIN_SCREEN_RADIUS,
        };
        let icon = e.raw.i32("iHeadIcon");
        let icon_team = e.raw.i32("iHeadIconTeam");
        let mut offset = 0.0;
        if icon != 0 && (icon_team == 0 || viewer == TEAM_SPECTATOR || viewer == icon_team) {
            let m = frame.configstrings.get(CS_HEAD_ICONS + icon as usize);
            if !m.is_empty() {
                out.extend(view.sprite(place(0.0), world, [1.0; 4], m));
                offset = STACK;
            }
        }
        // The local client's own body in the killcam (`ps.deltaTime` set,
        // cod2-hud.md 19.3), whatever his team, always constant size.
        if killcam && e.number as i32 == ps.own_client_num {
            let size = Size::Constant((YOU_IN_KILLCAM_SIZE + ICON_RADIUS) * CONSTANT_SCALE);
            out.extend(view.sprite(place(offset), size, [1.0; 4], "headiconyouinkillcam"));
        } else if e.eflags & EF_DISCONNECTED != 0 {
            out.extend(view.sprite(place(offset), world, [1.0; 4], "headicondisconnected"));
        } else if (his == viewer || viewer == TEAM_SPECTATOR) && e.eflags & EF_TALK != 0 {
            out.extend(view.sprite(
                place(offset - TALK_DROP),
                world,
                [1.0; 4],
                "headicontalkballoon",
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> WorldView {
        let (w, h) = (640.0, 480.0);
        WorldView {
            view_proj: crate::camera::view_proj_from(Vec3::ZERO, 0.0, 0.0, 0.0, 80.0, w / h),
            right: Vec3::new(0.0, -1.0, 0.0),
            up: Vec3::Z,
            size: (w, h),
        }
    }

    fn height(q: &HudQuad) -> f32 {
        q.verts[3][1] - q.verts[0][1]
    }

    #[test]
    fn a_constant_sprite_keeps_its_size_and_sits_on_its_point() {
        let v = view();
        let near = v
            .sprite(
                Vec3::new(200.0, 0.0, 0.0),
                Size::Constant(0.0301),
                [1.0; 4],
                "m",
            )
            .unwrap();
        let far = v
            .sprite(
                Vec3::new(2000.0, 0.0, 0.0),
                Size::Constant(0.0301),
                [1.0; 4],
                "m",
            )
            .unwrap();
        // 2 x 0.0301 of 480 rows.
        assert!((height(&near) - 28.9).abs() < 0.1, "{}", height(&near));
        assert!((height(&far) - height(&near)).abs() < 0.01);
        // Raised by its radius: the bottom edge on the point's row.
        assert!((near.verts[3][1] - 240.0).abs() < 0.01, "{:?}", near.verts);
    }

    #[test]
    fn a_far_head_icon_holds_its_minimum_screen_radius() {
        let v = view();
        let world = Size::World {
            radius: ICON_RADIUS,
            min: MIN_SCREEN_RADIUS,
        };
        let far = v
            .sprite(Vec3::new(5000.0, 0.0, 0.0), world, [1.0; 4], "m")
            .unwrap();
        assert!((height(&far) - 2.0 * 0.02 * 480.0).abs() < 0.01);
        let near = v
            .sprite(Vec3::new(100.0, 0.0, 0.0), world, [1.0; 4], "m")
            .unwrap();
        assert!(height(&near) > 19.2);
        assert!(
            v.sprite(Vec3::new(-100.0, 0.0, 0.0), world, [1.0; 4], "m")
                .is_none()
        );
    }

    /// In the killcam the local client's own body carries the "you" mark,
    /// constant size, over whatever head icon.
    #[test]
    fn the_killcam_marks_the_local_clients_body() {
        use vcod_cod2::net::fields::{ENTITY_STATE_FIELDS, PLAYER_STATE_FIELDS};
        use vcod_cod2::net::{ClientState2, EntityState2, PlayerState2, Snapshot2};
        let team = field_index(&CLIENT_STATE_FIELDS, "team");
        let client = |n: u32, t: u32| {
            let mut c = ClientState2::null(n);
            c.f[team] = t;
            c
        };
        let mut ps = PlayerState2::default();
        // The killer's archived view, the local client 0 his victim.
        ps.f[field_index(&PLAYER_STATE_FIELDS, "clientNum")] = 1;
        let mut body = EntityState2::null(0);
        body.f[field_index(&ENTITY_STATE_FIELDS, "eType")] = 1;
        body.f[field_index(&ENTITY_STATE_FIELDS, "pos.trBase[0]")] = 300f32.to_bits();
        let snapshot = |delta_time: u32| {
            let mut ps = ps.clone();
            ps.f[field_index(&PLAYER_STATE_FIELDS, "deltaTime")] = delta_time;
            vcod_cod2::net::SnapshotView::new(
                &Snapshot2 {
                    message_num: 1,
                    server_time: 1000,
                    delta_num: -1,
                    snap_flags: 0,
                    valid: true,
                    ps_lc: 0,
                    ps,
                    entities: vec![body],
                    clients: vec![client(0, 1), client(1, 2)],
                },
                0,
            )
        };
        let cs = vcod_cod2::net::ConfigStrings::default();
        let v = view();
        let quads = |snap: &vcod_cod2::net::SnapshotView| {
            let f = Frame {
                snap,
                prev: None,
                time_ms: 1000,
                frac: 1.0,
                configstrings: &cs,
            };
            build(&f, &HashMap::new(), &v, 1000)
        };
        let live = snapshot(0);
        assert!(quads(&live).is_empty());
        let killcam = snapshot(-3000i32 as u32);
        let q = quads(&killcam);
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].texture, "headiconyouinkillcam");
        // (6 + 10) x 0.0043 of the height a side of the centre.
        assert!((height(&q[0]) - 2.0 * 16.0 * 0.0043 * 480.0).abs() < 0.1);
    }
}
