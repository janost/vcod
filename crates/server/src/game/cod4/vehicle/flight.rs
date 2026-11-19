//! `VEH_UpdateMoveToGoal` (`cod4_lnxded` 0x809e38a), the helicopter's
//! flight toward its goal with the turn and the tilt inlined, ported with
//! 1.7's float model: what the x87 keeps on its stack is an `f64` here,
//! what it stores is an `f32` (cod4-gsc.md 3.2, "The flight's float
//! model"). `crates/cod4/tests/fixtures/heli/think-gdb.txt` and
//! `yaw-gdb.txt` hold retail's vehicle record before and after each call,
//! and the test below replays every one.

use glam::Vec3;
use vcod_common::x87::F80;

use super::{DT, Heli4, INV_360, MAX_TILT_VEL, d, vectoangles4};

/// `AngleNormalize180` on the stack, its result kept: `(x - floor(x +
/// 0.5)) * 360` with `x` the angle times the float 1/360 (0x8209630).
fn norm_e(a: f64) -> f64 {
    let x = a * d(INV_360);
    (x - (x + 0.5).floor()) * 360.0
}

/// `Vec2Normalize` (0x81915d8): the length on the stack, each component
/// times its reciprocal stored.
fn normalize2(x: f32, y: f32) -> (f32, f32) {
    let len = (d(x) * d(x) + d(y) * d(y)).sqrt();
    let inv = if -len < 0.0 { 1.0 / len } else { 1.0 };
    ((d(x) * inv) as f32, (inv * d(y)) as f32)
}

/// `x` over the vehicle file's `accel`, `x` held to `0..=accel` first
/// (0x809f0ed..0x809f127, 0x809f330..0x809f34e).
fn share(x: f32, accel: f32) -> f64 {
    let c = if d(x) - d(accel) < 0.0 { x } else { accel };
    let c = if -d(x) < 0.0 { c } else { 0.0 };
    d(c) / d(accel)
}

/// One angle toward `target` the short way (the yaw at 0x809e8ff..
/// 0x809ef32, pitch and roll at 0x809f38a..0x809f4e9 and 0x809f50c..
/// 0x809f6ae): its rate climbs at `up` toward `top` until the stopping arc
/// less `overshoot` covers what is left, then falls at `down`; a rate
/// turning away climbs back. Once both the rate and the gap fit inside a
/// think it lands on `target`. The rate is stored; the angle takes the
/// stack's rate and is normalised before its store.
fn steer4(angle: &mut f32, rate: &mut f32, target: f64, top: f32, rates: (f64, f64), os: f32) {
    let (up, down) = rates;
    let left = norm_e(target - d(*angle));
    if left == 0.0 && *rate == 0.0 {
        return;
    }
    let r = d(*rate);
    let sp = d(rate.abs());
    let gap = d(left.abs() as f32);
    let arc = sp / down * (0.5 * sp) * (1.0 - d(os));
    let (toward, by) = if left * r < 0.0 || arc < gap {
        (d(top), up)
    } else {
        (0.0, down)
    };
    let tgt = if left >= 0.0 { toward } else { -toward };
    let by = by * d(DT);
    if by > sp && sp * d(DT) > gap {
        *angle = target as f32;
        *rate = 0.0;
        return;
    }
    let next = if tgt > r {
        let s = r + by;
        if s <= tgt { s } else { tgt }
    } else {
        let s = r - by;
        if tgt <= s { s } else { tgt }
    };
    *rate = next as f32;
    *angle = norm_e(d(*angle) + next * d(DT)) as f32;
}

impl Heli4 {
    /// One call of `VEH_UpdateMoveToGoal` toward `aim` (the goal, or the
    /// goal plus the wander while hovering), `look` the look-at entity's
    /// `r.currentOrigin`. Returns the notifies in their order: `goal_yaw`,
    /// `near_goal`, `goal`.
    pub(super) fn move_to_goal(&mut self, aim: Vec3, look: Option<Vec3>) -> Vec<&'static str> {
        let mut notes = Vec::new();
        let hovering = self.hovering;
        let o = self.origin;
        // x on the stack, y and z stored (0x809e3b6..0x809e3fa).
        let dx = d(aim.x) - d(o.x);
        let dy = (d(aim.y) - d(o.y)) as f32;
        let dz = (d(aim.z) - d(o.z)) as f32;
        let flat2 = (dx * dx + d(dy) * d(dy)) as f32;
        let dist = (d(dz) * d(dz) + d(flat2)).sqrt() as f32;
        if dist > 0.0 {
            let dir = self.fly4(dx, dy, dz, flat2, dist, hovering);
            self.turn4(dir, hovering, look, &mut notes);
            self.tilt4(hovering);
        }
        if hovering {
            return notes;
        }
        // 0x809e9e3..0x809ebdc.
        if self.near_goal_dist != 0.0 && dist < self.near_goal_dist {
            notes.push("near_goal");
        }
        let reached = if !self.stop_at_goal {
            d(DT) * d(self.speed) >= d(dist)
        } else if self.hover.radius == 0.0 {
            !((!self.stopping && dist != 0.0) || self.speed != 0.0)
        } else {
            dist <= self.hover.radius && self.speed < 2.0 * super::MPH
        };
        if reached {
            if self.stop_at_goal {
                self.hovering = true;
                if self.hover.radius == 0.0 {
                    self.vel = Vec3::ZERO;
                    self.smoothed_change = Vec3::ZERO;
                }
            }
            notes.push("goal");
        }
        notes
    }

    /// The speed, the velocity change, the move (0x809e464..0x809e74f).
    /// Returns the stored direction to the aim, which the yaw reads.
    fn fly4(
        &mut self,
        mut dx: f64,
        dy: f32,
        dz: f32,
        flat2: f32,
        dist: f32,
        hovering: bool,
    ) -> Vec3 {
        let v0 = self.vel;
        let speed = self.speed;
        let mut dt = DT;
        // A stop goal or the hover: whether the brake starts, and how much
        // of the frame is left once it does (0x809ea42..0x809eb10).
        if hovering || self.stop_at_goal {
            let p = self.pace;
            let next = if p.speed > speed {
                let s = d(speed) + d(p.accel) * d(DT);
                if s > d(p.speed) { d(p.speed) } else { s }
            } else {
                let s = d(speed) - d(p.accel) * d(DT);
                if d(p.speed) <= s { s } else { d(p.speed) }
            };
            let stop_dist = 0.5 * next * (next / d(p.decel));
            if stop_dist < next * -d(DT) + d(dist) || speed <= 0.0 {
                self.stopping = false;
            } else {
                self.stopping = true;
                if d(dist) > stop_dist {
                    let b = d(DT) - (d(dist) - stop_dist) / d(speed);
                    let m = if b - d(DT) < 0.0 { b as f32 } else { DT };
                    dt = if -b < 0.0 { m } else { 0.0 };
                }
            }
        }
        // The new speed and the change's limit (0x809e4c3..0x809e51d).
        let (target, rate, decel) = if hovering {
            let h = self.hover;
            (h.speed, h.rate, (0.5 * d(h.rate)) as f32)
        } else {
            (self.pace.speed, self.pace.accel, self.pace.decel)
        };
        let (rate, limit, new_speed) = if !self.stopping {
            let p = d(dt) * d(rate);
            let ns = if target <= speed {
                let s = d(speed) - p;
                if d(target) <= s { s } else { d(target) }
            } else {
                let s = d(speed) + p;
                if s > d(target) { d(target) } else { s }
            };
            (rate, p as f32, ns)
        } else {
            let p = d(decel) * d(dt);
            let ns = if 0.0 > speed {
                let s = d(speed) + p;
                if s <= 0.0 { s } else { 0.0 }
            } else {
                let s = d(speed) - p;
                if 0.0 > s { 0.0 } else { s }
            };
            (decel, p as f32, ns)
        };
        // The wanted velocity less the old (0x809e51d..0x809e56a).
        let inv = 1.0 / d(dist);
        let dir = Vec3::new(
            (dx * inv) as f32,
            (d(dy) * inv) as f32,
            (d(dz) * inv) as f32,
        );
        let (wx, wy) = (d(dir.x) * new_speed, d(dir.y) * new_speed);
        let mut cx = (wx - d(v0.x)) as f32;
        let mut cy = (wy - d(v0.y)) as f32;
        // z's change is kept at 64 bits: the velocity adds it back to
        // `v0.z`, and near rest the two all but cancel.
        let e = F80::from_f64;
        let mut cz =
            F80::from_f32(dz) * (e(1.0) / F80::from_f32(dist)) * e(new_speed) - F80::from_f32(v0.z);
        // Below the set speed a turn never brakes (0x809f701..0x809f7c2);
        // the call spills x and the change's z as floats.
        if !self.stopping
            && self.pace.speed >= self.speed
            && wx * d(v0.x) + wy * d(v0.y) > 0.0
            && 0.0 > d(v0.x) * d(cx) + d(cy) * d(v0.y)
        {
            let (nx, ny) = normalize2(v0.x, v0.y);
            let back = d(cx) * d(nx) + d(cy) * d(ny);
            cx = (d(cx) - d((d(nx) * back) as f32)) as f32;
            cy = (d(cy) - back * d(ny)) as f32;
            dx = d(dx as f32);
            cz = F80::from_f32(cz.to_f32());
        }
        // Held to the limit (0x809e594..0x809e5d9).
        let (fx, fy) = (F80::from_f32(cx), F80::from_f32(cy));
        let size = (fx * fx + fy * fy + cz * cz).sqrt();
        if size > F80::from_f32(limit) {
            let k = F80::from_f32(limit) / size;
            cx = (fx * k).to_f32();
            cy = (fy * k).to_f32();
            cz = cz * k;
        }
        if !hovering {
            (cx, cy) = self.ease_flat4((cx, cy), v0, dx, dy, flat2, rate);
            if let Some(c) = self.ease_rise4(cz.to_f64(), v0.z, dz) {
                cz = e(c);
            }
        }
        // The velocity, the speed, the smoothed change (0x809e5ef..0x809e6b1).
        let vel = Vec3::new(
            (d(v0.x) + d(cx)) as f32,
            (d(cy) + d(v0.y)) as f32,
            (F80::from_f32(v0.z) + cz).to_f32(),
        );
        self.vel = vel;
        self.speed =
            (d(vel.x) * d(vel.x) + d(vel.y) * d(vel.y) + d(vel.z) * d(vel.z)).sqrt() as f32;
        let sc = self.smoothed_change;
        let sx = d(sc.x) + (d(cx) - d(sc.x)) * 0.5;
        let sy = d(sc.y) + (d(cy) - d(sc.y)) * 0.5;
        let sz = d(sc.z) + (cz.to_f64() - d(sc.z)) * 0.5;
        self.smoothed_change = Vec3::new(sx as f32, sy as f32, sz as f32);
        let size = (sx * sx + sy * sy + sz * sz).sqrt();
        if size > d(limit) && !self.stop_at_goal {
            let k = d(limit) / size;
            let s = self.smoothed_change;
            self.smoothed_change =
                Vec3::new((sx * k) as f32, (d(s.y) * k) as f32, (k * d(s.z)) as f32);
        }
        // The move: the frame's head at the old velocity once the brake
        // started in it, the rest at the mean (0x809e6bd..0x809e74f,
        // 0x809ee2a..0x809ee6c; the head's x stays on the stack).
        let (mut ox, mut oy, mut oz) = (d(self.origin.x), self.origin.y, self.origin.z);
        if dt < DT {
            let h = d(DT) - d(dt);
            ox += d(v0.x) * h;
            oy = (d(v0.y) * h + d(oy)) as f32;
            oz = (h * d(v0.z) + d(oz)) as f32;
        }
        let t = d(dt);
        self.origin = Vec3::new(
            ((d(vel.x) + d(v0.x)) * 0.5 * t + ox) as f32,
            ((d(v0.y) + d(vel.y)) * 0.5 * t + d(oy)) as f32,
            ((d(vel.z) + d(v0.z)) * 0.5 * t + d(oz)) as f32,
        );
        dir
    }

    /// Outside the hover, the horizontal change (0x809ecc1..0x809eddc,
    /// 0x809fb07..0x809fb8f): a stop goal sheds the speed it needs to
    /// arrive at rest; else a turn whose radius `rate` cannot hold at this
    /// speed brakes by the turning ability (fully toward a stop).
    fn ease_flat4(
        &self,
        c: (f32, f32),
        v0: Vec3,
        dx: f64,
        dy: f32,
        flat2: f32,
        rate: f32,
    ) -> (f32, f32) {
        let (cx, cy) = c;
        let gl = d(flat2).sqrt();
        if 1.0 > gl {
            return c;
        }
        let sp = (d(v0.x) * d(v0.x) + d(v0.y) * d(v0.y)).sqrt();
        if self.stop_at_goal {
            let (nx, ny) = (d(cx) + d(v0.x), d(cy) + d(v0.y));
            let nsp = (nx * nx + ny * ny).sqrt();
            if nsp > 0.0 {
                let shed = sp * sp / (gl + gl) * d(DT);
                if shed > d((nsp - sp).abs() as f32) {
                    let k = (sp - shed) / nsp;
                    return (
                        (d(nx as f32) * k - d(v0.x)) as f32,
                        (k * d(ny as f32) - d(v0.y)) as f32,
                    );
                }
                return c;
            }
        }
        let off = (d(v0.y) * dx - d(dy) * d(v0.x)).abs() as f32;
        if d(off) <= gl {
            return c;
        }
        let dot = dx * d(v0.x) + d(dy) * d(v0.y);
        let inv = 1.0 / sp;
        let (side, ahead) = (d(off) * inv, dot * inv);
        let radius = (side * side + ahead * ahead) / (side + side);
        if radius <= 1.0 || sp * sp <= d(rate) * radius {
            return c;
        }
        let b = sp * sp / radius;
        let brake = if b > sp { sp } else { b };
        let ability = if self.stop_at_goal {
            -1.0
        } else {
            -d(self.turning_ability)
        };
        let k = ability * brake / sp * d(DT);
        ((d(v0.x) * k + d(cx)) as f32, (d(v0.y) * k + d(cy)) as f32)
    }

    /// Outside the hover, a climb or descent that would pass the goal's
    /// height brakes, at most three frames of the set rate a frame
    /// (0x809f97f..0x809fa5d). `None` leaves the change as it was.
    fn ease_rise4(&self, cz: f64, vz: f32, dz: f32) -> Option<f64> {
        if dz == 0.0
            || 0.001 > cz.abs() as f32
            || 0.001 > vz.abs()
            || cz * d(vz) >= 0.0
            || d(dz) * d(vz) <= 0.0
        {
            return None;
        }
        let a = -d(DT) * d(vz);
        let to_height = d(dz) / (0.5 * d(vz));
        if a / cz <= to_height {
            return None;
        }
        let needed = a / to_height;
        if needed * needed <= cz * cz {
            return None;
        }
        let most = d(self.pace.accel) * d(DT) * 3.0;
        let c = if needed - most < 0.0 { needed } else { most };
        Some(if -most - needed >= 0.0 { -most } else { c })
    }

    /// The yaw (0x809e751..0x809ef8a): toward the look-at entity's origin
    /// from the moved place (each difference stored, then `vectoyaw`,
    /// 0x809f19a..0x809f1f2); else the goal yaw once a stop or the hover
    /// is close enough to swing into it, else the target yaw, else
    /// (hovering) hold, else along `dir`; a fifth of the rates once the
    /// rate swung past the goal yaw, until the wanted yaw moves.
    fn turn4(
        &mut self,
        dir: Vec3,
        hovering: bool,
        look: Option<Vec3>,
        notes: &mut Vec<&'static str>,
    ) {
        let ay = self.angles.y;
        let o = self.origin;
        let look = look.map(|at| {
            let to = Vec3::new(
                (d(at.x) - d(o.x)) as f32,
                (d(at.y) - d(o.y)) as f32,
                (d(at.z) - d(o.z)) as f32,
            );
            vectoangles4(to).y
        });
        let due = self.goal_yaw.filter(|&gy| {
            if !(self.stopping || hovering) {
                return false;
            }
            let top = d(self.yaw.top);
            let stop_time = d(self.speed) / d(self.pace.decel);
            let swing = top / d(self.yaw.up) * 2.0;
            if stop_time <= swing {
                return true;
            }
            let swing_angle = top * 0.5 * swing;
            let left = d(norm_e(d(gy) - d(ay)).abs() as f32);
            left > swing_angle && swing + (left - swing_angle) / top >= stop_time
        });
        let wanted = look
            .or(due)
            .or(self.target_yaw)
            .unwrap_or_else(|| if hovering { ay } else { vectoangles4(dir).y });
        let off = self.goal_yaw.map(|gy| d(ay) - d(gy));
        let watch = off.is_some_and(|o| o.abs() as f32 > 0.001);
        let rate_before = self.angle_rates[1];
        if wanted != self.yaw.slow_for {
            self.yaw.slowed = false;
            self.yaw.slow_for = wanted;
        }
        let (mut up, mut down) = (d(self.yaw.up), d(self.yaw.down));
        if self.yaw.slowed {
            up *= d(0.2);
            down *= d(0.2);
        }
        let os = if self.goal_yaw.is_some() || self.target_yaw.is_some() {
            self.yaw.overshoot
        } else {
            0.0
        };
        steer4(
            &mut self.angles.y,
            &mut self.angle_rates[1],
            d(wanted),
            self.yaw.top,
            (up, down),
            os,
        );
        let Some(gy) = self.goal_yaw else {
            return;
        };
        if 0.0 > d(rate_before) * d(self.angle_rates[1]) {
            self.yaw.slowed = true;
        }
        if watch {
            let now = d(self.angles.y) - d(gy);
            let was = off.map_or(0.0, |o| d(o as f32));
            if 0.0 > was * now || 0.001 > now.abs() as f32 {
                notes.push("goal_yaw");
            }
        }
    }

    /// Pitch and roll (0x809ef90..0x809f6ae): toward `maxPitch` and
    /// `maxRoll` times the smoothed change plus the fake drag, along and
    /// across the new heading, times `f + (1 - f) 0.1` (`f` the change's
    /// share of the file's `accel`), eased while braking; at the rate
    /// `45 g + (1 - g)` and 0.4 of it back (`g` the set accel's share, or
    /// the hover's once a hovering body sat level).
    fn tilt4(&mut self, hovering: bool) {
        let sc = self.smoothed_change;
        let vel = self.vel;
        let ar = self.air_resistance;
        let accel = self.info.accel;
        let gs = (d(vel.x) * d(vel.x) + d(vel.y) * d(vel.y)).sqrt();
        let drag_share = ((if d(ar) - gs < 0.0 { d(ar) } else { gs }) / d(ar)) as f32;
        let (nx, ny) = normalize2(vel.x, vel.y);
        let drag = d(drag_share) * d(drag_share) * 5.0;
        let px = d(sc.x) + d((d(nx) * drag) as f32);
        let py = (drag * d(ny) + d(sc.y)) as f32;
        let sum2 = px * px + d(py) * d(py);
        let push_rate = (sum2.sqrt() / d(DT)) as f32;
        let len = (sum2 + d(sc.z) * d(sc.z)).sqrt();
        let inv = if -len < 0.0 { 1.0 / len } else { 1.0 };
        let npx = (d(px as f32) * inv) as f32;
        let npy = (inv * d(py)) as f32;
        let rad = (super::DEG2RAD * d(self.angles.y)) as f32;
        let (sy, cy) = (rad.sin(), rad.cos());
        let f = share(push_rate, accel);
        let mut ease = 1.0f32;
        if self.stopping && push_rate > 0.0 {
            let braking_for = gs / d(push_rate);
            let settle = (1.0 - (1.0 - f)) * 2.5 + 3.5 * (1.0 - f);
            if settle > braking_for {
                ease = (braking_for / settle) as f32;
            }
        }
        let scale = ((1.0 - f) * d(0.1) + f) as f32;
        let a = self.angles;
        let r = self.angle_rates;
        let level = a.x.abs() <= 5.0 && a.z.abs() <= 5.0 && r[0].abs() <= 3.0 && r[2].abs() <= 3.0;
        self.hover.settled = hovering && (self.hover.settled || level);
        let g = share(
            if self.hover.settled {
                self.hover.rate
            } else {
                self.pace.accel
            },
            accel,
        );
        let up = g * 45.0 + (1.0 - g);
        let rates = (d(up as f32), d((up * d(0.4)) as f32));
        let pitch = (d(npx) * d(cy) + d(npy) * d(sy)) * d(ease) * d(self.max_pitch) * d(scale);
        steer4(
            &mut self.angles.x,
            &mut self.angle_rates[0],
            pitch,
            MAX_TILT_VEL,
            rates,
            0.0,
        );
        let roll = (d(npx) * d(sy) - d(cy) * d(npy)) * d(ease) * d(self.max_roll) * d(scale);
        steer4(
            &mut self.angles.z,
            &mut self.angle_rates[2],
            roll,
            MAX_TILT_VEL,
            rates,
            0.0,
        );
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use vcod_gsc::EntId;

    /// The vehicle record's words a think reads or writes, by offset.
    type Rec = BTreeMap<u32, u32>;

    fn f(r: &Rec, o: u32) -> f32 {
        f32::from_bits(r[&o])
    }

    fn v(r: &Rec, o: u32) -> Vec3 {
        Vec3::new(f(r, o), f(r, o + 4), f(r, o + 8))
    }

    fn heli(r: &Rec) -> Heli4 {
        let mut h = Heli4::new(EntId(1, 0), 0, 0, v(r, 0xc0), v(r, 0xd8));
        assert_eq!((f(r, 0xf0), f(r, 0xf8)), (MAX_TILT_VEL, MAX_TILT_VEL));
        h.moving = true;
        h.vel = v(r, 0x138);
        h.angle_rates = v(r, 0x150).to_array();
        h.smoothed_change = v(r, 0x15c);
        h.max_pitch = f(r, 0x168);
        h.max_roll = f(r, 0x16c);
        h.hovering = r[&0x1c8] == 2;
        h.hover.radius = f(r, 0x224);
        h.hover.speed = f(r, 0x228);
        h.hover.rate = f(r, 0x22c);
        h.hover.settled = r[&0x23c] != 0;
        h.pace.speed = f(r, 0x24c);
        h.pace.accel = f(r, 0x250);
        h.pace.decel = f(r, 0x254);
        h.speed = f(r, 0x25c);
        h.air_resistance = f(r, 0x260);
        h.turning_ability = f(r, 0x264);
        h.target_yaw = (r[&0x26c] != 0).then(|| f(r, 0x29c));
        h.goal_yaw = (r[&0x270] != 0).then(|| f(r, 0x2ac));
        h.stop_at_goal = r[&0x274] != 0;
        h.stopping = r[&0x278] != 0;
        h.yaw.top = f(r, 0xf4);
        h.yaw.up = f(r, 0xfc);
        h.yaw.down = f(r, 0x100);
        h.yaw.slow_for = f(r, 0x2b0);
        h.yaw.overshoot = f(r, 0x2b4);
        h.yaw.slowed = r[&0x2b8] != 0;
        h.near_goal_dist = f(r, 0x2bc);
        h
    }

    /// What the think writes, in the record's words.
    fn written(h: &Heli4) -> Rec {
        let mut r = Rec::new();
        let mut put = |o: u32, x: Vec3| {
            for (k, c) in x.to_array().into_iter().enumerate() {
                r.insert(o + 4 * k as u32, c.to_bits());
            }
        };
        put(0xc0, h.origin);
        put(0xd8, h.angles);
        put(0x138, h.vel);
        put(0x150, Vec3::from(h.angle_rates));
        put(0x15c, h.smoothed_change);
        r.insert(0x1c8, if h.hovering { 2 } else { 1 });
        r.insert(0x23c, h.hover.settled as u32);
        r.insert(0x25c, h.speed.to_bits());
        r.insert(0x278, h.stopping as u32);
        r.insert(0x2b0, h.yaw.slow_for.to_bits());
        r.insert(0x2b8, h.yaw.slowed as u32);
        r
    }

    fn words(s: &str) -> Vec<(u32, u32)> {
        s.split_whitespace()
            .map(|kv| {
                let (k, v) = kv.split_once('=').unwrap();
                (
                    u32::from_str_radix(k, 16).unwrap(),
                    u32::from_str_radix(v, 16).unwrap(),
                )
            })
            .collect()
    }

    /// Replays one fixture: from retail's record before each think, the
    /// record after it word for word, and its notifies. Returns the runs,
    /// the thinks, and what parted.
    fn replay(text: &str) -> (usize, usize, Vec<String>) {
        let (mut rec, mut runs, mut thinks) = (Rec::new(), 0, 0);
        let mut off = Vec::new();
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            if line.starts_with("run ") {
                rec.clear();
                runs += 1;
                continue;
            }
            let parts: Vec<&str> = line.trim_start_matches("think ").split('|').collect();
            let floats = |s: &str| -> Vec<f32> {
                s.split_whitespace()
                    .map(|w| f32::from_bits(u32::from_str_radix(w, 16).unwrap()))
                    .collect()
            };
            let aim = floats(parts[0]);
            rec.extend(words(parts[1]));
            let look = parts.get(4).map(|p| floats(p)).filter(|v| !v.is_empty());
            // The look-at handle's entity number (`EntHandle`'s low half).
            assert_eq!(rec[&0x280] & 0xffff != 0, look.is_some(), "think {thinks}");
            let mut h = heli(&rec);
            let notes = h.move_to_goal(Vec3::from_slice(&aim), look.map(|v| Vec3::from_slice(&v)));
            rec.extend(words(parts[2]));
            let ours = written(&h);
            for (o, w) in &ours {
                if rec[o] != *w {
                    off.push(format!(
                        "run {runs} think {thinks} +{o:#x}: retail {:08x} ({}) ours {w:08x} ({})",
                        rec[o],
                        f32::from_bits(rec[o]),
                        f32::from_bits(*w)
                    ));
                }
            }
            let want: Vec<&str> = parts[3].split_whitespace().collect();
            if notes != want {
                off.push(format!(
                    "run {runs} think {thinks}: notes {want:?} ours {notes:?}"
                ));
            }
            thinks += 1;
        }
        (runs, thinks, off)
    }

    /// Every think of retail's runs under gdb at 0x809e38a and its two
    /// returns to the vehicle think: the idle and the flight captures'
    /// gsc (`think-gdb.txt`), and the yaw probe's goal yaws and look-at
    /// (`yaw-gdb.txt`).
    #[test]
    #[cfg_attr(
        not(all(target_os = "linux", target_env = "gnu")),
        ignore = "retail's words match glibc's sinf and cosf; other libms miss some by an ulp"
    )]
    fn every_think_matches_retail_word_for_word() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../cod4/tests/fixtures/heli/");
        let mut off = Vec::new();
        for (file, want) in [("think-gdb.txt", (2, 604)), ("yaw-gdb.txt", (1, 802))] {
            let text = std::fs::read_to_string(format!("{dir}{file}")).unwrap();
            let (runs, thinks, o) = replay(&text);
            assert_eq!((runs, thinks), want, "{file}");
            off.extend(o.into_iter().map(|l| format!("{file}: {l}")));
        }
        assert!(
            off.is_empty(),
            "{} off:\n{}",
            off.len(),
            off[..off.len().min(40)].join("\n")
        );
    }
}
