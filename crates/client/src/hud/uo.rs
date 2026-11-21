//! United Offensive's additions to the native HUD, drawn on a protocol 22
//! connection: the sprint icon and fatigue fill on the stance slot, the
//! vehicle status icons and crosshair, vehicles on the compass, and the
//! hud.menu rects that moved (health, cursor hints). How `uo_cgame_mp_x86.dll`
//! draws each: docs/research/coduo-diff.md section 15.

use super::HudQuad;
use super::friends::{Mark, decode_friend_info};
use super::hudelem::Virtual;
use super::player::{COMPASS_CENTRE, COMPASS_RADIUS, HintLayout, compass_offset};

const WHITE: [f32; 4] = [1.0; 4];

/// The stance owner draw's rect (`hud.menu` "stance", and the vehicle
/// status menus at the same spot).
const STANCE_RECT: (f32, f32, f32, f32) = (100.0, 434.375, 40.0, 40.0);

/// What UO's HUD reads off the playerstate and the vehicle it sits in.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UoView {
    /// `pm_flags` 0x10000.
    pub sprinting: bool,
    /// `fatigueScale`, 1 rested.
    pub fatigue: f32,
    /// In a vehicle seat and not popped out of its hatch.
    pub seat: Option<Seat>,
    /// Friendly vehicles for the compass.
    pub vehicles: Vec<VehicleSighting>,
}

/// The seat the view sits in (`ps.vehPos`, `ps.vehType`) and the vehicle
/// entity `viewlocked_entNum` names.
#[derive(Clone, Debug, PartialEq)]
pub struct Seat {
    /// `ps.vehPos`: 1 the driver or main gun, 2 the gunner, 3 a passenger.
    pub pos: i32,
    /// `ps.vehType`: 1 a jeep, 2 a tank.
    pub kind: i32,
    /// The vehicle entity's `surfType`, the vehicle file's type, which the
    /// status icons are keyed on.
    pub vehicle_kind: i32,
    /// The vehicle's yaw, and its `tag_turret`'s when posed; world degrees.
    pub body_yaw: f32,
    pub turret_yaw: Option<f32>,
    /// `es.animMovetype`: bit n set while seat n is taken.
    pub seats_taken: i32,
}

impl Seat {
    /// The jeep's passenger seat keeps the weapon HUD and draws no vehicle
    /// crosshair (0x30019cf0, 0x30030c60).
    pub fn jeep_passenger(&self) -> bool {
        self.kind == 1 && self.pos == 3
    }
}

/// `AngleNormalize180(a - b)` (0x3004bd70).
fn normalize180(a: f32, b: f32) -> f32 {
    (a - b + 180.0).rem_euclid(360.0) - 180.0
}

/// The stance owner draw (0x3002efc0): the sprint icon over the stance's
/// while sprinting, then the fatigue fill, drained from the top as stamina
/// goes, then the stance flash in white. Not drawn from a vehicle seat.
pub fn stance(
    uo: &UoView,
    (prone, ducked): (bool, bool),
    flash: Option<f32>,
    v: &Virtual,
    out: &mut Vec<HudQuad>,
) {
    let (x, y, w, h) = STANCE_RECT;
    let pick = |kind: &str| {
        let stance = if uo.sprinting {
            "Sprint"
        } else if prone {
            "Prone"
        } else if ducked {
            "Crouch"
        } else {
            "Stand"
        };
        format!("hud{kind}{stance}")
    };
    out.push(v.quad(x, y, w, h, WHITE, &pick("Stance")));
    let f = uo.fatigue.min(1.0);
    if f > 0.0 {
        let d = h * (1.0 - f);
        let uvs = [[0.0, 1.0 - f], [1.0, 1.0 - f], [1.0, 1.0], [0.0, 1.0]];
        out.push(v.quad_uv(x, y + d, w, h - d, uvs, WHITE, &pick("Fatigue")));
    }
    if let Some(alpha) = flash {
        // UO registers `cg_hudStanceFlash_r/g/b` and never reads them.
        out.push(v.quad(x, y, w, h, [1.0, 1.0, 1.0, alpha], "hudStanceFlash"));
    }
}

/// The status icons `hud.menu` puts at the stance's spot, for a vehicle
/// type (`textfont`): `(background, owner draw, seat)`; the seat is the
/// `textstyle` a position icon tests.
#[derive(Clone, Copy)]
enum Status {
    Body,
    Barrel,
    Position(i32),
}

const TANK_STATUS: [(&str, Status); 4] = [
    ("gfx/hud/hud@tankbody.tga", Status::Body),
    ("gfx/hud/hud@tankbarrel.tga", Status::Barrel),
    ("gfx/hud/hud@tank_maingun.tga", Status::Position(1)),
    ("gfx/hud/hud@tank_machinegun.tga", Status::Position(2)),
];

const JEEP_STATUS: [(&str, Status); 4] = [
    ("gfx/hud/hud@jeepbody.tga", Status::Body),
    ("gfx/hud/hud@jeep_driver_1.tga", Status::Position(1)),
    ("gfx/hud/hud@jeep_gunner_1.tga", Status::Position(2)),
    ("gfx/hud/hud@jeep_passenger_1.tga", Status::Position(3)),
];

/// `CG_TANK_BODY_DIR` (0x30031d50), `CG_TANK_BARREL_DIR` (0x30031e20) and
/// `CG_TANK_POSITION_STATUS` (0x30031f70): the vehicle's outline turned by
/// its yaw off the view's, its turret about the body's centre, and an icon
/// per taken seat. A free seat draws nothing.
pub fn vehicle_status(seat: &Seat, view_yaw: f32, v: &Virtual, out: &mut Vec<HudQuad>) {
    let items: &[(&str, Status)] = match seat.vehicle_kind {
        2 => &TANK_STATUS,
        1 => &JEEP_STATUS,
        _ => return,
    };
    let (x, y, w, h) = STANCE_RECT;
    let centre = (x + w / 2.0, y + h / 2.0);
    let (hw, hh) = (w / 2.0, h / 2.0);
    let box_corners = [[-hw, -hh], [hw, -hh], [hw, hh], [-hw, hh]];
    let body = normalize180(view_yaw, seat.body_yaw);
    for &(shader, status) in items {
        match status {
            Status::Body => out.push(v.rotated(centre, box_corners, body, WHITE, shader)),
            Status::Barrel => {
                let Some(turret) = seat.turret_yaw else {
                    continue;
                };
                // Pivoted a quarter up the art, mirrored left to right.
                let corners = [
                    [-hw, h / 4.0],
                    [-hw, -0.75 * h],
                    [hw, -0.75 * h],
                    [hw, h / 4.0],
                ];
                let mut q = v.rotated(
                    centre,
                    corners,
                    normalize180(view_yaw, turret),
                    WHITE,
                    shader,
                );
                q.uvs = [[1.0, 1.0], [1.0, 0.0], [0.0, 0.0], [0.0, 1.0]];
                out.push(q);
            }
            Status::Position(n) => {
                if seat.seats_taken & 1 << n != 0 {
                    out.push(v.rotated(centre, box_corners, body, WHITE, shader));
                }
            }
        }
    }
}

/// The seat's crosshair (0x30019cf0): two 60% black bars, wide for the
/// main gun, narrow for the gunner. None for a jeep's driver or passenger.
pub fn vehicle_crosshair(seat: &Seat, v: &Virtual, out: &mut Vec<HudQuad>) {
    let bars: [(f32, f32, f32, f32); 2] = match seat.pos {
        1 if seat.kind != 1 => [(300.0, 240.0, 40.0, 2.0), (319.0, 242.0, 2.0, 16.0)],
        2 => [(310.0, 240.0, 20.0, 2.0), (319.0, 242.0, 2.0, 8.0)],
        _ => return,
    };
    for (x, y, w, h) in bars {
        out.push(v.quad(x, y, w, h, [0.0, 0.0, 0.0, 0.6], "white"));
    }
}

/// UO's health block (`hud.menu` "Health" at 488,436): the back, the bar
/// and the cross moved from 1.1's.
pub struct HealthRects {
    pub back: (f32, f32, f32, f32, &'static str),
    pub bar: (f32, f32, f32, f32),
    pub cross: (f32, f32, f32, f32),
}

pub const HEALTH_V1: HealthRects = HealthRects {
    back: (501.0, 460.0, 130.0, 12.0, "gfx/hud/hud@health_back.tga"),
    bar: (502.0, 461.0, 128.0, 10.0),
    cross: (488.0, 460.0, 12.0, 12.0),
};

pub const HEALTH_V22: HealthRects = HealthRects {
    back: (501.0, 436.0, 128.0, 32.0, "gfx/hud/hud@health.tga"),
    bar: (502.0, 460.0, 126.0, 6.0),
    cross: (488.0, 457.0, 12.0, 12.0),
};

/// UO's cursor hint icons (0x3044b6f8): 1.1's with `hintLMGMount` at 7,
/// the bipod's mount point, and everything after it one higher; 128
/// weapons' icons from 11 and their ammo's from 139.
pub const HINTS: HintLayout = HintLayout {
    named: &[
        "hintActivate",
        "hintNoActivate",
        "hintDoor",
        "hintNoDoor",
        "hintMg42",
        "hintLMGMount",
        "hintHealth",
        "hintLadder",
        "hintFriendly",
    ],
    weapons: 128,
    health: 8,
};

/// A friendly vehicle seen this frame (`CG_PLAYER_COMPASS_TANKS`,
/// 0x30016c00): its slot in the game's vehicle pool, where it is, its yaw
/// and its type.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VehicleSighting {
    pub slot: u32,
    pub at: Mark,
    pub yaw: f32,
    /// `None` keeps the slot's last type: the packed `iCompassTankInfo`
    /// carries none.
    pub kind: Option<i32>,
}

/// `ps.iCompassTankInfo` (ps+0x604), packed as `iCompassFriendInfo` is.
pub fn decode_tank_info(info: i32, origin: [f32; 3]) -> Option<VehicleSighting> {
    let s = decode_friend_info(info, origin, false)?;
    Some(VehicleSighting {
        slot: s.client,
        at: s.at,
        yaw: s.yaw,
        kind: None,
    })
}

#[derive(Clone, Copy, Default)]
struct TankSlot {
    seen: i32,
    at: Option<Mark>,
    yaw: f32,
    kind: i32,
}

/// The 64 vehicle slots, kept across frames; a vehicle lingers 800 ms.
pub struct CompassVehicles {
    slots: [TankSlot; 64],
}

impl Default for CompassVehicles {
    fn default() -> Self {
        CompassVehicles {
            slots: [TankSlot::default(); 64],
        }
    }
}

const LINGER_MS: i32 = 800;
/// Icon size at `cg_hudCompassSize` 1 (0x3007bf00).
const TANK_ICON: f32 = 16.0;

impl CompassVehicles {
    pub fn feed(&mut self, now: i32, sightings: impl IntoIterator<Item = VehicleSighting>) {
        for s in sightings {
            let Some(slot) = self.slots.get_mut(s.slot as usize) else {
                continue;
            };
            slot.seen = now;
            slot.at = Some(s.at);
            slot.yaw = s.yaw;
            if let Some(kind) = s.kind {
                slot.kind = kind;
            }
        }
    }

    /// Green icons by type (jeep, flak 88, else a tank), turned by the
    /// viewer's yaw against the vehicle's.
    pub fn build(
        &mut self,
        now: i32,
        (view_yaw, eye): (f32, [f32; 3]),
        v: &Virtual,
        out: &mut Vec<HudQuad>,
    ) {
        for slot in &mut self.slots {
            if now < slot.seen {
                slot.seen = 0;
            }
            let Some(at) = slot
                .at
                .filter(|_| slot.seen >= now - LINGER_MS && slot.seen > 0)
            else {
                continue;
            };
            let (dx, dy) = match at {
                Mark::At([x, y]) => compass_offset(view_yaw, eye, [x, y, 0.0]),
                Mark::Toward([x, y]) => {
                    let (ox, oy) = compass_offset(view_yaw, [0.0; 3], [x, y, 0.0]);
                    let len = (ox * ox + oy * oy).sqrt().max(f32::EPSILON);
                    (ox / len * COMPASS_RADIUS, oy / len * COMPASS_RADIUS)
                }
            };
            let (cx, cy) = (COMPASS_CENTRE.0 + dx, COMPASS_CENTRE.1 + dy);
            let half = TANK_ICON / 2.0;
            let corners = [[-half, -half], [half, -half], [half, half], [-half, half]];
            let shader = match slot.kind {
                1 => "gfx/hud/hud@objective_jeep.dds",
                5 => "gfx/hud/hud@objective_flak_88.dds",
                _ => "gfx/hud/hud@objective_tank.tga",
            };
            out.push(v.rotated(
                (cx, cy),
                corners,
                normalize180(view_yaw, slot.yaw),
                [0.0, 1.0, 0.0, 1.0],
                shader,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::weapon::WeaponDef;

    fn v() -> Virtual {
        Virtual::new((640.0, 480.0))
    }

    #[test]
    fn sprinting_swaps_the_icon_and_fatigue_fills_from_the_bottom() {
        let mut out = Vec::new();
        let uo = UoView {
            sprinting: true,
            fatigue: 0.25,
            ..UoView::default()
        };
        stance(&uo, (false, true), Some(0.5), &v(), &mut out);
        let names: Vec<_> = out.iter().map(|q| q.texture.as_str()).collect();
        assert_eq!(
            names,
            ["hudStanceSprint", "hudFatigueSprint", "hudStanceFlash"]
        );
        // A quarter left: the bottom 10 of the 40, the art's bottom quarter.
        let fill = &out[1];
        assert_eq!(fill.verts[0], [100.0, 434.375 + 30.0]);
        assert_eq!(fill.verts[2], [140.0, 474.375]);
        assert_eq!(fill.uvs[0], [0.0, 0.75]);
        assert_eq!(out[2].rgba, [1.0, 1.0, 1.0, 0.5]);
        // Spent: no fill; crouched, the crouch icon.
        out.clear();
        let uo = UoView {
            sprinting: false,
            fatigue: 0.0,
            ..UoView::default()
        };
        stance(&uo, (false, true), None, &v(), &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].texture, "hudStanceCrouch");
    }

    fn tank(pos: i32, taken: i32) -> Seat {
        Seat {
            pos,
            kind: 2,
            vehicle_kind: 2,
            body_yaw: 90.0,
            turret_yaw: Some(0.0),
            seats_taken: taken,
        }
    }

    #[test]
    fn a_tank_draws_its_body_turret_and_taken_seats() {
        let mut out = Vec::new();
        vehicle_status(&tank(1, 0b10), 90.0, &v(), &mut out);
        let names: Vec<_> = out.iter().map(|q| q.texture.as_str()).collect();
        assert_eq!(
            names,
            [
                "gfx/hud/hud@tankbody.tga",
                "gfx/hud/hud@tankbarrel.tga",
                "gfx/hud/hud@tank_maingun.tga"
            ]
        );
        // Facing along the body: upright about the stance slot's centre.
        assert_eq!(out[0].verts[0], [100.0, 434.375]);
        // A turret 90 degrees right of the view turns clockwise on screen by
        // the view's yaw less its own: its top points right.
        let barrel = &out[1];
        let top = barrel.verts[1];
        assert!(
            (top[0] - 150.0).abs() < 1e-3 && (top[1] - 434.375).abs() < 1e-3,
            "{top:?}"
        );
    }

    #[test]
    fn the_seat_crosshair_follows_the_position() {
        let bars = |seat: &Seat| {
            let mut out = Vec::new();
            vehicle_crosshair(seat, &v(), &mut out);
            out.iter().map(|q| q.verts[0]).collect::<Vec<_>>()
        };
        assert_eq!(bars(&tank(1, 0)), [[300.0, 240.0], [319.0, 242.0]]);
        assert_eq!(bars(&tank(2, 0)), [[310.0, 240.0], [319.0, 242.0]]);
        let jeep_driver = Seat {
            kind: 1,
            ..tank(1, 0)
        };
        assert!(bars(&jeep_driver).is_empty());
        assert!(
            Seat {
                kind: 1,
                ..tank(3, 0)
            }
            .jeep_passenger()
        );
    }

    #[test]
    fn uo_hints_put_the_lmg_mount_at_seven() {
        let weapons = vec![
            None,
            Some(WeaponDef {
                hud_icon: Some("w_icon".into()),
                ammo_icon: Some("a_icon".into()),
                ..WeaponDef::default()
            }),
        ];
        assert_eq!(HINTS.material(7, &weapons).as_deref(), Some("hintLMGMount"));
        assert_eq!(
            HINTS.material(10, &weapons).as_deref(),
            Some("hintFriendly")
        );
        assert_eq!(HINTS.material(11, &weapons).as_deref(), Some("w_icon"));
        assert_eq!(HINTS.material(139, &weapons).as_deref(), Some("a_icon"));
        assert_eq!(HINTS.material(267, &weapons), None);
        // Unset slots draw nothing (0x300303fe), blank keys `hintActivate`.
        assert_eq!(HINTS.material(12, &weapons), None);
        assert_eq!(HINTS.material(140, &weapons), None);
        let bare = vec![None, Some(WeaponDef::default())];
        assert_eq!(HINTS.material(11, &bare).as_deref(), Some("hintActivate"));
        assert_eq!(HINTS.material(139, &bare).as_deref(), Some("hintActivate"));
    }

    #[test]
    fn vehicles_linger_and_keep_their_type_through_a_packed_sighting() {
        let mut c = CompassVehicles::default();
        let seen = |kind| VehicleSighting {
            slot: 3,
            at: Mark::At([0.0, 100.0]),
            yaw: 0.0,
            kind,
        };
        c.feed(1000, [seen(Some(1))]);
        c.feed(1100, [seen(None)]);
        let mut out = Vec::new();
        c.build(1900, (0.0, [0.0; 3]), &v(), &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].texture, "gfx/hud/hud@objective_jeep.dds");
        assert_eq!(out[0].rgba, [0.0, 1.0, 0.0, 1.0]);
        out.clear();
        c.build(1901, (0.0, [0.0; 3]), &v(), &mut out);
        assert!(out.is_empty());
    }
}
