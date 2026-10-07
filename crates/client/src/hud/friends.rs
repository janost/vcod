//! Teammates on the compass (`CG_PLAYER_COMPASS_FRIENDS`): the ones in the
//! snapshot, and the one out of it the server packs into
//! `iCompassFriendInfo`. docs/research/cod11-hud-protocol.md, section 9,
//! "Compass friendlies".

use super::HudQuad;
use super::hudelem::Virtual;
use super::player::{COMPASS_CENTRE, COMPASS_RADIUS, compass_offset};

pub use vcod_common::net::flags::EF_FRIEND_PING as PS_EF_FRIEND_PING;
/// An `ET_PLAYER` entity's `eFlags` bit `pingPlayer` sets.
pub use vcod_common::net::flags::EF_PING;

/// A teammate seen this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sighting {
    pub client: u32,
    pub at: Mark,
    /// World degrees.
    pub yaw: f32,
    pub pinged: bool,
}

/// Where a teammate is: a world point, or, past the packed offset's range, a
/// unit direction from the viewer drawn on the compass rim.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mark {
    At([f32; 2]),
    Toward([f32; 2]),
}

/// `iCompassFriendInfo` read against the playerstate's origin. 0 is nobody.
pub fn decode_friend_info(info: i32, origin: [f32; 3], pinged: bool) -> Option<Sighting> {
    if info == 0 {
        return None;
    }
    let info = info as u32;
    let offset = |shift: u32| ((info >> shift) & 0x1ff) as f32 * 4.0 - 1020.0;
    let (dx, dy) = (offset(6), offset(15));
    let clamped = |d: f32| d == 1024.0 || d == -1020.0;
    let at = if clamped(dx) || clamped(dy) {
        let len = (dx * dx + dy * dy).sqrt();
        Mark::Toward([dx / len, dy / len])
    } else {
        Mark::At([origin[0] + dx, origin[1] + dy])
    };
    Some(Sighting {
        client: info & 0x3f,
        at,
        yaw: f32::from((info >> 24) as u8 as i8) * 1.40625,
        pinged,
    })
}

#[derive(Clone, Copy, Default)]
struct Slot {
    /// When last seen; 0 is never.
    seen: i32,
    at: Option<Mark>,
    yaw: f32,
    flash_until: i32,
}

/// A teammate stays on the compass this long after it was last seen.
const LINGER_MS: i32 = 800;
/// A ping flashes the icon this long, in 500 ms periods.
const PING_MS: i32 = 3000;
/// Icon size, virtual units, at `cg_hudCompassSize` 1.
const ICON: f32 = 10.0;

/// One slot per client, kept across frames.
pub struct CompassFriends {
    slots: [Slot; 64],
}

impl Default for CompassFriends {
    fn default() -> Self {
        CompassFriends {
            slots: [Slot::default(); 64],
        }
    }
}

impl CompassFriends {
    /// Stamps each sighting at `now`. A ping arms the 3 s flash unless one is
    /// already running.
    pub fn feed(&mut self, now: i32, sightings: impl IntoIterator<Item = Sighting>) {
        for s in sightings {
            let Some(slot) = self.slots.get_mut(s.client as usize) else {
                continue;
            };
            slot.seen = now;
            slot.at = Some(s.at);
            slot.yaw = s.yaw;
            if s.pinged && slot.flash_until <= now {
                slot.flash_until = now + PING_MS;
            }
        }
    }

    /// `(client, chat)` for every teammate seen in the last 800 ms but
    /// `own`; `chat` picks the chat icon over the arrow, on the second half
    /// of each 500 ms flash period.
    fn live(&mut self, now: i32, own: i32) -> Vec<(usize, bool)> {
        let mut live = Vec::new();
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if now < slot.seen {
                slot.seen = 0;
            }
            if slot.seen < now - LINGER_MS || i as i32 == own || slot.at.is_none() {
                continue;
            }
            let chat = now < slot.flash_until && (slot.flash_until - now) % 500 >= 250;
            live.push((i, chat));
        }
        live
    }

    /// The icons, turned by the viewer's yaw against each teammate's.
    pub fn build(
        &mut self,
        now: i32,
        own: i32,
        (view_yaw, eye): (f32, [f32; 3]),
        v: &Virtual,
        out: &mut Vec<HudQuad>,
    ) {
        for (i, chat) in self.live(now, own) {
            let slot = &self.slots[i];
            let (dx, dy) = match slot.at.expect("live slots have a mark") {
                Mark::At([x, y]) => compass_offset(view_yaw, eye, [x, y, 0.0]),
                Mark::Toward([x, y]) => {
                    let (ox, oy) = compass_offset(view_yaw, [0.0; 3], [x, y, 0.0]);
                    let len = (ox * ox + oy * oy).sqrt().max(f32::EPSILON);
                    (ox / len * COMPASS_RADIUS, oy / len * COMPASS_RADIUS)
                }
            };
            let (cx, cy) = (COMPASS_CENTRE.0 + dx, COMPASS_CENTRE.1 + dy);
            let half = ICON / 2.0;
            if chat {
                out.push(v.quad(
                    cx - half,
                    cy - half,
                    ICON,
                    ICON,
                    [1.0; 4],
                    "gfx/hud/hud@objective_friendly_chat.tga",
                ));
            } else {
                let corners = [[-half, -half], [half, -half], [half, half], [-half, half]];
                out.push(v.rotated(
                    (cx, cy),
                    corners,
                    view_yaw - slot.yaw,
                    [1.0; 4],
                    "gfx/hud/hud@objective_friendly.tga",
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(client: u32, at: Mark, pinged: bool) -> Sighting {
        Sighting {
            client,
            at,
            yaw: 0.0,
            pinged,
        }
    }

    #[test]
    fn a_teammate_lingers_800_ms_and_never_shows_for_ourselves() {
        let mut c = CompassFriends::default();
        c.feed(10_000, [seen(3, Mark::At([0.0; 2]), false)]);
        c.feed(10_000, [seen(5, Mark::At([0.0; 2]), false)]);
        let live = |c: &mut CompassFriends, now| -> Vec<usize> {
            c.live(now, 5).into_iter().map(|(i, _)| i).collect()
        };
        assert_eq!(live(&mut c, 10_000), [3]);
        assert_eq!(live(&mut c, 10_800), [3]);
        assert!(live(&mut c, 10_801).is_empty());
        // A clock that ran backward (a new map) forgets the sighting.
        c.feed(10_000, [seen(3, Mark::At([0.0; 2]), false)]);
        assert!(live(&mut c, 5_000).is_empty());
    }

    #[test]
    fn a_ping_flashes_the_chat_icon_for_three_seconds() {
        let mut c = CompassFriends::default();
        c.feed(1_000, [seen(2, Mark::At([0.0; 2]), true)]);
        let chat = |c: &mut CompassFriends, now: i32| {
            c.feed(now, [seen(2, Mark::At([0.0; 2]), true)]);
            c.live(now, -1)[0].1
        };
        // 3000 ms left: 0 into the period, the arrow; 250 left of a period
        // and above it, the chat icon.
        assert!(!chat(&mut c, 1_000));
        assert!(chat(&mut c, 1_100), "2900 left, 400 into the period");
        assert!(!chat(&mut c, 1_300), "2700 left, 200 into the period");
        // Held ping: the flash re-arms only once the last one ran out.
        assert!(!chat(&mut c, 3_990));
        assert!(!chat(&mut c, 4_000), "re-armed: 3000 left");
        assert!(chat(&mut c, 4_100));
    }

    #[test]
    fn friend_info_unpacks_the_offset_and_the_rim_direction() {
        // Slot 7, 256 units east and 128 north, yaw byte 64 (90 degrees).
        let pack = |dx: i32, dy: i32| {
            7 | ((dx / 4 + 255) as u32) << 6 | ((dy / 4 + 255) as u32) << 15 | 64 << 24
        };
        let s = decode_friend_info(pack(256, 128) as i32, [100.0, 200.0, 0.0], true).unwrap();
        assert_eq!(s.client, 7);
        assert_eq!(s.at, Mark::At([356.0, 328.0]));
        assert_eq!(s.yaw, 90.0);
        assert!(s.pinged);
        // Clamped to the edge of the range: only the direction survives.
        let s = decode_friend_info(pack(1024, 0) as i32, [100.0, 200.0, 0.0], false).unwrap();
        let Mark::Toward([x, y]) = s.at else {
            panic!("{:?}", s.at);
        };
        assert!((x - 1.0).abs() < 1e-3 && y.abs() < 1e-2, "{x} {y}");
        assert_eq!(decode_friend_info(0, [0.0; 3], false), None);
        // A negative yaw byte.
        let s = decode_friend_info(
            (pack(0, 0) & 0xff_ffff) as i32 | (0xc0u32 << 24) as i32,
            [0.0; 3],
            false,
        )
        .unwrap();
        assert_eq!(s.yaw, -90.0);
    }

    #[test]
    fn a_teammate_ahead_sits_above_the_centre_and_a_far_one_on_the_rim() {
        let mut c = CompassFriends::default();
        c.feed(
            0,
            [
                seen(1, Mark::At([512.0, 0.0]), false),
                seen(2, Mark::Toward([0.0, 1.0]), false),
            ],
        );
        let mut out = Vec::new();
        c.build(
            0,
            -1,
            (0.0, [0.0; 3]),
            &Virtual::new((640.0, 480.0)),
            &mut out,
        );
        let centre = |q: &HudQuad| {
            let x = q.verts.iter().map(|v| v[0]).sum::<f32>() / 4.0;
            let y = q.verts.iter().map(|v| v[1]).sum::<f32>() / 4.0;
            (x, y)
        };
        let (x, y) = centre(&out[0]);
        assert!(
            (x - 55.0).abs() < 1e-3 && (y - (425.0 - 21.875)).abs() < 1e-3,
            "{x} {y}"
        );
        // Due left (+y with the viewer facing +x), at the full radius.
        let (x, y) = centre(&out[1]);
        assert!(
            (x - (55.0 - 43.75)).abs() < 1e-3 && (y - 425.0).abs() < 1e-3,
            "{x} {y}"
        );
        assert!(
            out.iter()
                .all(|q| q.texture == "gfx/hud/hud@objective_friendly.tga")
        );
    }
}
