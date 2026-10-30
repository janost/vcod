//! CoD2's crosshair names (`CG_DrawCrosshairNames`, Mac 0x1cddb4): a
//! trace 8192 units along the view finds a client's body, and his name
//! stays up 150 ms past the last frame that found him, fading over the
//! last 100, for a viewer on his team or a spectator (cod2-hud.md 19.6).

use glam::Vec3;
use vcod_cod2::net::Frame;
use vcod_cod2::net::delta::field_index;
use vcod_cod2::net::fields::CLIENT_STATE_FIELDS;
use vcod_common::collision::CollisionWorld;

/// The trace's reach (Mac 0x3265c8).
const RANGE: f32 = 8192.0;
/// `CG_TraceCapsule`'s mask: the world's solid and the bodies.
const MASK: u32 = 0x200_0001;
/// The packed box of a brush model, which the scan does not clip.
const SOLID_BMODEL: i32 = 0xff_ffff;
/// `eType` 3, which `CG_BuildSolidList` keeps as a trigger.
const ET_TRIGGER: i32 = 3;
const TEAM_SPECTATOR: u32 = 3;
/// `CG_FadeColor(time, 150, 100)`.
const SHOW_MS: i32 = 150;
const FADE_MS: i32 = 100;
/// `ps.stats[3]`'s "nobody" in six bits.
const NO_IDENTIFY: i32 = 63;

/// `CG_ClipMoveToEntities`' box for a packed `solid` (Mac 0x1de5b8): the
/// server's box shrunk a unit on every side.
fn unpack_solid(s: i32) -> (Vec3, Vec3) {
    let x = (s & 0xff) as f32;
    let zd = ((s >> 8) & 0xff) as f32;
    let zu = ((s >> 16) & 0xff) as f32 - 32.0;
    (
        Vec3::new(1.0 - x, 1.0 - x, 2.0 - zd),
        Vec3::new(x - 1.0, x - 1.0, zu - 1.0),
    )
}

/// The entry fraction of `start..end` into the box, 0 from inside it.
fn ray_box(start: Vec3, end: Vec3, lo: Vec3, hi: Vec3) -> Option<f32> {
    let d = end - start;
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for a in 0..3 {
        if d[a].abs() < 1e-6 {
            if start[a] < lo[a] || start[a] > hi[a] {
                return None;
            }
            continue;
        }
        let (mut p, mut q) = ((lo[a] - start[a]) / d[a], (hi[a] - start[a]) / d[a]);
        if p > q {
            std::mem::swap(&mut p, &mut q);
        }
        t0 = t0.max(p);
        t1 = t1.min(q);
        if t0 > t1 {
            return None;
        }
    }
    Some(t0)
}

/// The entity under the crosshair this frame, when it is a client's (a
/// number under 64): a point traced from the drawn eye along `forward`
/// through the world and the snapshot's solid entities' boxes, passing
/// the viewed client's own. Brush models are not clipped.
pub fn scan(
    frame: &Frame,
    eye: Vec3,
    forward: Vec3,
    world: Option<&CollisionWorld>,
) -> Option<u32> {
    let end = eye + forward * RANGE;
    let fraction = world.map_or(1.0, |w| w.point_trace(eye, end, MASK, false).fraction);
    if fraction == 0.0 {
        return None;
    }
    let own = frame.snap.ps.client_num;
    let (_, number) = frame
        .snap
        .entities
        .iter()
        .filter(|e| e.number as i32 != own && e.etype != ET_TRIGGER)
        .filter_map(|e| {
            let solid = e.raw.i32("solid");
            if solid == 0 || solid == SOLID_BMODEL {
                return None;
            }
            let (lo, hi) = unpack_solid(solid);
            let at = frame.entity_origin(e);
            let t = ray_box(eye, end, at + lo, at + hi)?;
            (t < fraction).then_some((t, e.number))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))?;
    (number < 64).then_some(number)
}

/// `cg + 0x2bdcc` and `+0x2bdd0`: the client the scan last found and
/// when.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CrosshairNames {
    client: u32,
    time: i32,
}

impl CrosshairNames {
    /// This frame's scan at cgame time `now`.
    pub fn update(&mut self, hit: Option<u32>, now: i32) {
        if let Some(client) = hit {
            *self = CrosshairNames { client, time: now };
        }
    }

    /// The name to draw and its colour, or `None`: the fade has run out,
    /// either side has no client info, the viewer is in no team, or the
    /// target is on another team and the viewer is not a spectator. White,
    /// or for the client in `ps.stats[3]` his `stats[4]` health as red
    /// through yellow to green; the alpha 0.6 of the fade's (Mac
    /// 0x326500).
    pub fn shown(&self, frame: &Frame, now: i32) -> Option<(String, [f32; 4])> {
        let since = now - self.time;
        if self.time == 0 || since >= SHOW_MS {
            return None;
        }
        let fade = ((SHOW_MS - since) as f32 / FADE_MS as f32).min(1.0);
        let team = field_index(&CLIENT_STATE_FIELDS, "team");
        let client = |n: u32| frame.snap.clients.iter().find(|c| c.number == n);
        let ps = &frame.snap.ps;
        let viewer = client(u32::try_from(ps.client_num).ok()?)?.f[team];
        if viewer == 0 {
            return None;
        }
        let target = client(self.client)?;
        if viewer != TEAM_SPECTATOR && viewer != target.f[team] {
            return None;
        }
        let name = vcod_common::net::strip_colors(&target.name());
        if name.is_empty() {
            return None;
        }
        let identify = ps.raw.stats[3];
        let [r, g, b] = if identify != NO_IDENTIFY && identify as u32 == self.client {
            health_rgb(ps.raw.stats[4])
        } else {
            [1.0; 3]
        };
        Some((name, [r, g, b, 0.6 * fade]))
    }
}

/// A health of 0..100 as red (0) through yellow (50) to green (100).
fn health_rgb(health: i32) -> [f32; 3] {
    let h = (health as f32 / 100.0).clamp(0.0, 1.0);
    if h <= 0.5 {
        [1.0, h * 2.0, 0.0]
    } else {
        [(1.0 - h) * 2.0, 1.0, 0.0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_runs_red_through_yellow_to_green() {
        assert_eq!(health_rgb(0), [1.0, 0.0, 0.0]);
        assert_eq!(health_rgb(25), [1.0, 0.5, 0.0]);
        assert_eq!(health_rgb(50), [1.0, 1.0, 0.0]);
        assert_eq!(health_rgb(75), [0.5, 1.0, 0.0]);
        assert_eq!(health_rgb(150), [0.0, 1.0, 0.0]);
    }

    #[test]
    fn a_standing_players_packed_box_unpacks_a_unit_inside() {
        // 0x66010f: maxs.x 15, mins.z 0, maxs.z 70 on the server.
        let (lo, hi) = unpack_solid(0x66010f);
        assert_eq!(lo, Vec3::new(-14.0, -14.0, 1.0));
        assert_eq!(hi, Vec3::new(14.0, 14.0, 69.0));
    }

    #[test]
    fn the_ray_enters_the_box_at_its_near_face() {
        let lo = Vec3::new(90.0, -10.0, -10.0);
        let hi = Vec3::new(110.0, 10.0, 10.0);
        let t = ray_box(Vec3::ZERO, Vec3::new(200.0, 0.0, 0.0), lo, hi).unwrap();
        assert!((t - 0.45).abs() < 1e-6);
        assert!(ray_box(Vec3::ZERO, Vec3::new(0.0, 200.0, 0.0), lo, hi).is_none());
    }

    fn snap(stats: (i32, i32), body_at: [f32; 3]) -> vcod_cod2::net::SnapshotView {
        use vcod_cod2::net::fields::ENTITY_STATE_FIELDS;
        use vcod_cod2::net::{ClientState2, EntityState2, PlayerState2, Snapshot2};
        let team = field_index(&CLIENT_STATE_FIELDS, "team");
        let client = |n: u32, name: &str, t: u32| {
            let mut c = ClientState2::null(n);
            c.f[team] = t;
            let mut bytes = name.as_bytes().to_vec();
            bytes.resize(32, 0);
            for k in 0..8 {
                let i = field_index(&CLIENT_STATE_FIELDS, &format!("name[{}]", k * 4));
                c.f[i] = u32::from_le_bytes(bytes[k * 4..k * 4 + 4].try_into().unwrap());
            }
            c
        };
        let mut ps = PlayerState2::default();
        ps.stats[3] = stats.0;
        ps.stats[4] = stats.1;
        let mut body = EntityState2::null(1);
        body.f[field_index(&ENTITY_STATE_FIELDS, "eType")] = 1;
        body.f[field_index(&ENTITY_STATE_FIELDS, "solid")] = 0x66010f;
        for (k, v) in body_at.iter().enumerate() {
            body.f[field_index(&ENTITY_STATE_FIELDS, &format!("pos.trBase[{k}]"))] = v.to_bits();
        }
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
                clients: vec![
                    client(0, "me", 1),
                    client(1, "^2Hans", 1),
                    client(2, "Tommy", 2),
                ],
            },
            0,
        )
    }

    #[test]
    fn a_teammate_found_shows_his_health_and_fades_out() {
        let snap = snap((1, 75), [200.0, 0.0, 0.0]);
        let cs = vcod_cod2::net::ConfigStrings::default();
        let f = Frame {
            snap: &snap,
            prev: None,
            time_ms: 1000,
            frac: 1.0,
            configstrings: &cs,
        };
        let hit = scan(&f, Vec3::new(0.0, 0.0, 40.0), Vec3::X, None);
        assert_eq!(hit, Some(1));
        assert_eq!(scan(&f, Vec3::new(0.0, 0.0, 40.0), Vec3::Y, None), None);
        let mut names = CrosshairNames::default();
        names.update(hit, 1000);
        let (name, rgba) = names.shown(&f, 1000).unwrap();
        assert_eq!(name, "Hans");
        assert_eq!(rgba, [0.5, 1.0, 0.0, 0.6]);
        // Lost: up for 50 ms more, then fading over the last 100.
        names.update(None, 1050);
        assert_eq!(names.shown(&f, 1050).unwrap().1[3], 0.6);
        let a = names.shown(&f, 1100).unwrap().1[3];
        assert!((a - 0.3).abs() < 1e-6, "{a}");
        assert_eq!(names.shown(&f, 1150), None);
        // Another team's body is never named to a player.
        let enemy = CrosshairNames {
            client: 2,
            time: 1000,
        };
        assert_eq!(enemy.shown(&f, 1000), None);
    }
}
