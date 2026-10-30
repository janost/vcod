//! The mantle: `Mantle_Check` (0x80de088), `Mantle_CheckLedge` (0x80dd65a),
//! `Mantle_Move` (0x80de260), `Mantle_CapView` (0x80de47a) and
//! `Mantle_IsWeaponInactive` (0x80de5d4) of `cod2_lnxded`, with the root
//! motion of the `mp_mantle_*` anims they step the body along
//! (docs/research/cod2-movement.md 9).
//!
//! `Mantle_Move` also picks the body's legs and torso anim (the mantle
//! movetypes of [`super::anim`]) when the step has an anim table.
//!
//! The anims are the engine's `PLAYER_MANTLE` set (`Mantle_CreateAnims`
//! 0x80ddaa4), loaded once per process off the paks ([`anims`]) and handed
//! to pmove through [`Rules::mantle_anims`](super::Rules); without them no
//! mantle starts.

use std::sync::OnceLock;
use vcod_common::asset_source::AssetSource;

use glam::Vec3;
use vcod_common::pk3::Pk3Fs;

use super::fpu::{self, d};
use super::{PlayerState, Pml, Pmove, button, ef, pm_type, pmf};
use crate::collision::{CONTENTS_MANTLE, SURF_MANTLE_ON, SURF_MANTLE_OVER};

/// `mantleState.flags` (ps 0x5c8).
pub mod flag {
    /// The ledge's surface is `MANTLE_OVER`: the over anim follows the up.
    pub const OVER: u32 = 0x1;
    /// The standing box does not fit at the ledge: `EV_STANCE_FORCE_CROUCH`
    /// every frame of the mantle.
    pub const CROUCH: u32 = 0x2;
    /// The standing box fits where the mantle ends: `EV_STANCE_FORCE_STAND`
    /// and `eFlags` 0x4000 cleared at the end.
    pub const STAND: u32 = 0x4;
    /// A mantle is available this frame (`Mantle_ClearHint` 0x80de5ba
    /// clears it every `PmoveSingle`).
    pub const HINT: u32 = 0x8;
}

/// `playerState_s.mantleState` (ps 0x5bc, STABS).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MantleState {
    /// `vectoyaw` of the wall's inward normal; the anims turn by it.
    pub yaw: f32,
    /// ms into the mantle.
    pub timer: i32,
    /// The row of [`TRANSITIONS`].
    pub trans_index: i32,
    pub flags: u32,
}

/// `s_mantleAnimNames` (`.data` 0x8184d60): the `XAnimCreateAnims` slots.
pub const ANIM_NAMES: [&str; 11] = [
    "mp_mantle_root",
    "mp_mantle_up_57",
    "mp_mantle_up_51",
    "mp_mantle_up_45",
    "mp_mantle_up_39",
    "mp_mantle_up_33",
    "mp_mantle_up_27",
    "mp_mantle_up_21",
    "mp_mantle_over_high",
    "mp_mantle_over_mid",
    "player_mantle_over_low",
];

/// `s_mantleTrans` (`.data` 0x8184d00): {up anim, over anim, the ledge
/// height the row is picked for}.
pub const TRANSITIONS: [(usize, usize, f32); 7] = [
    (1, 8, 57.0),
    (2, 8, 51.0),
    (3, 9, 45.0),
    (4, 9, 39.0),
    (5, 9, 33.0),
    (6, 10, 27.0),
    (7, 10, 21.0),
];

/// The over anim that leaves the weapon up (`Mantle_IsWeaponInactive`'s 10).
const OVER_LOW: usize = 10;

/// `Mantle_CheckLedge`'s ledge heights, tried in order (`Mantle_Check`'s
/// immediates 0x42700000, 0x42200000, 0x41a00000).
const LEDGE_HEIGHTS: [f32; 3] = [60.0, 40.0, 20.0];
/// How far into the wall the ledge box is pushed (0x41800000).
const LEDGE_REACH: f32 = 16.0;
/// rodata 0x81535a4: the down trace's floor over the start.
const LEDGE_FLOOR: f32 = 18.0;
/// rodata 0x81535a8: the flattest ledge normal.
const LEDGE_MIN_NORMAL_Z: f32 = f32::from_bits(0x3f33_3333);
/// The crouched box's top (0x42480000).
const CROUCH_TOP: f32 = 50.0;
/// The over anim's reach and drop (immediate 0x41f80000, rodata 0x815348c).
const OVER_REACH: f32 = 31.0;
const OVER_DROP: f32 = 18.0;
/// rodata 0x8153814: a wall normal flatter than this is vertical.
const MIN_NORMAL_LEN: f32 = f32::from_bits(0x38d1_b717);
/// rodata 0x8153818 (double): 180 / pi.
const RAD_TO_DEG: f64 = f64::from_bits(0x404c_a5dc_1a63_c1f8);
/// rodata 0x814d140 (double): pi / 180, `VectorAngleMultiply`'s.
const DEG_TO_RAD: f64 = f64::from_bits(0x3f91_df46_a252_9d39);
/// rodata 0x8153928: ms to seconds.
const MSEC_TO_SEC: f32 = f32::from_bits(0x3a83_126f);
/// `eFlags` 0x4000: mantling.
pub const EF_MANTLE: u32 = 0x4000;

/// One anim's root translation as the engine samples it.
#[derive(Clone, Debug, PartialEq)]
struct Clip {
    /// `XAnimGetLengthMsec`: frames over the frame rate, times 1000,
    /// truncated.
    length_ms: i32,
    /// The parts' `numframes`: the last key frame.
    numframes: u16,
    frames: Vec<u16>,
    keys: Vec<Vec3>,
}

impl Clip {
    /// `XAnimGetAbsDelta`'s translation at time fraction `t`
    /// (0x80c177c, 0x80bdc5a): the last key at 1, else the two keys
    /// around frame `t * numframes` (stored, truncated) lerped.
    fn abs_delta(&self, t: f32) -> Vec3 {
        let Some(&last) = self.keys.last() else {
            return Vec3::ZERO;
        };
        let n = self.numframes;
        if t == 1.0 || n == 0 || self.keys.len() == 1 {
            return last;
        }
        // 0x80bd0a6: the frame position stored, its whole part truncated.
        let pos = (d(t) * f64::from(n)) as f32;
        let whole = pos as i32;
        let count = self.keys.len() - 1;
        let (i, frac) = if count < usize::from(n) {
            // 0x80bd4c4 / 0x80bd876: the key pair around `whole`.
            let c = self
                .frames
                .partition_point(|&f| i32::from(f) <= whole)
                .saturating_sub(1)
                .min(count - 1);
            let (f0, f1) = (self.frames[c], self.frames[c + 1]);
            let frac = (d(pos) - f64::from(f0)) / f64::from(i32::from(f1) - i32::from(f0));
            (c, frac as f32)
        } else {
            (whole as usize, (d(pos) - f64::from(whole)) as f32)
        };
        if i + 1 > count {
            return last;
        }
        fpu::lerp(self.keys[i], self.keys[i + 1], frac)
    }
}

/// The `PLAYER_MANTLE` anims' root motion, by [`ANIM_NAMES`] slot.
#[derive(Clone, Debug, PartialEq)]
pub struct MantleAnims {
    clips: Vec<Clip>,
}

impl MantleAnims {
    /// Reads `xanim/<name>` for every slot past the root.
    pub fn load(fs: &dyn AssetSource) -> anyhow::Result<MantleAnims> {
        let mut clips = vec![Clip {
            length_ms: 0,
            numframes: 0,
            frames: Vec::new(),
            keys: Vec::new(),
        }];
        for name in &ANIM_NAMES[1..] {
            let path = format!("xanim/{name}");
            let data = fs
                .read(&path)
                .ok_or_else(|| anyhow::anyhow!("{path} not in the paks"))?;
            let motion = vcod_common::xanim::root_motion(name, &data)?;
            let anim = vcod_common::xanim::parse(name, &data)?;
            let (frames, keys) = anim
                .root
                .map(|r| r.trans_keys.into_iter().unzip())
                .unwrap_or_default();
            clips.push(Clip {
                length_ms: motion.length_ms,
                numframes: (anim.frame_count - 1) as u16,
                frames,
                keys,
            });
        }
        Ok(MantleAnims { clips })
    }

    /// `XAnimGetLengthMsec` of slot `anim`.
    pub fn length_ms(&self, anim: usize) -> i32 {
        self.clips[anim].length_ms
    }

    /// `XAnimGetAbsDelta`'s translation of slot `anim` at fraction `t`.
    pub fn abs_delta(&self, anim: usize, t: f32) -> Vec3 {
        self.clips[anim].abs_delta(t)
    }

    /// The up anim's length (`0x80dd120`) and the over anim's, 0 without
    /// [`flag::OVER`] (`0x80dd15a`).
    fn lengths(&self, st: &MantleState) -> (i32, i32) {
        let (up, over, _) = row(st);
        let over = if st.flags & flag::OVER != 0 {
            self.length_ms(over)
        } else {
            0
        };
        (self.length_ms(up), over)
    }

    /// `Mantle_GetAnimDelta` (0x80dd216): the root's offset `t` ms into the
    /// mantle, turned by the mantle's yaw.
    fn delta(&self, st: &MantleState, t: i32) -> Vec3 {
        let (up, over, _) = row(st);
        let (up_len, over_len) = self.lengths(st);
        let v = if up_len < t {
            let a = self.abs_delta(up, 1.0);
            let b = self.abs_delta(over, (t - up_len) as f32 / over_len as f32);
            add(b, a)
        } else {
            self.abs_delta(up, t as f32 / up_len as f32)
        };
        angle_multiply(v, st.yaw)
    }
}

/// `pmove_t`'s `mantleEndPos` and `mantleDuration` for a mantle under way
/// on `ps`: where the anims end it, and their whole length in ms.
/// INFERRED: retail stores the start's end and the two anims' lengths.
pub fn end_and_duration(anims: &MantleAnims, ps: &PlayerState) -> (Vec3, i32) {
    let st = &ps.mantle;
    let (up, over) = anims.lengths(st);
    let total = up + over;
    let left = sub(anims.delta(st, total), anims.delta(st, st.timer));
    (add(ps.origin, left), total)
}

static ANIMS: OnceLock<MantleAnims> = OnceLock::new();

/// The mantle anims off `fs`, read on the first call that finds them and
/// kept for the process (retail keeps one `s_mantleAnims`). `None` (logged)
/// where the paks lack them.
pub fn anims(fs: &Pk3Fs) -> Option<&'static MantleAnims> {
    if let Some(a) = ANIMS.get() {
        return Some(a);
    }
    match MantleAnims::load(fs) {
        Ok(a) => Some(ANIMS.get_or_init(|| a)),
        Err(e) => {
            log::warn!("no mantle: {e:#}");
            None
        }
    }
}

fn row(st: &MantleState) -> (usize, usize, f32) {
    TRANSITIONS[(st.trans_index.clamp(0, 6)) as usize]
}

fn add(a: Vec3, b: Vec3) -> Vec3 {
    Vec3::new(
        (d(a.x) + d(b.x)) as f32,
        (d(a.y) + d(b.y)) as f32,
        (d(a.z) + d(b.z)) as f32,
    )
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    Vec3::new(
        (d(a.x) - d(b.x)) as f32,
        (d(a.y) - d(b.y)) as f32,
        (d(a.z) - d(b.z)) as f32,
    )
}

/// `VectorAngleMultiply` (0x80a7670): x and y turned by `yaw` degrees, the
/// radians stored, libm `sin` and `cos` of them stored.
fn angle_multiply(v: Vec3, yaw: f32) -> Vec3 {
    let a = (DEG_TO_RAD * d(yaw)) as f32;
    let (s, c) = (d(a).sin() as f32, d(a).cos() as f32);
    Vec3::new(
        (d(v.x) * d(c) - d(v.y) * d(s)) as f32,
        (d(v.y) * d(c) + d(v.x) * d(s)) as f32,
        v.z,
    )
}

/// `Mantle_IsWeaponInactive` (0x80de5d4): mantling on a row whose over
/// anim is not `player_mantle_over_low`. `mantle_enable` is taken as on.
pub fn weapon_inactive(ps: &PlayerState) -> bool {
    ps.pm_flags & pmf::MANTLE != 0 && row(&ps.mantle).1 != OVER_LOW
}

fn enabled(pm: &Pmove) -> Option<&'static MantleAnims> {
    if pm.rules.mantle_enable {
        pm.rules.mantle_anims
    } else {
        None
    }
}

/// `MantleResults` (STABS, 56 bytes).
#[derive(Clone, Copy, Debug, Default)]
struct Results {
    dir: Vec3,
    start: Vec3,
    ledge: Vec3,
    end: Vec3,
    flags: u32,
}

/// `Mantle_Check` (0x80de088): refused dead, mantling, crouched or prone
/// (`eFlags` 0xc) or on the binoculars (weapon states 17..22); else a wall
/// flagged `MANTLE_ON` or `MANTLE_OVER` ahead, and a ledge on it at 60, 40
/// or 20 up.
pub(crate) fn check(pm: &mut Pmove, pml: &Pml) {
    let Some(anims) = enabled(pm) else {
        return;
    };
    let ps = &mut *pm.ps;
    ps.mantle.flags &= !flag::HINT;
    if ps.pm_type > pm_type::INTERMISSION
        || ps.pm_flags & pmf::MANTLE != 0
        || ps.e_flags & (ef::CROUCH | ef::PRONE) != 0
        || (17..=22).contains(&ps.weapon_state)
    {
        return;
    }
    let Some((surface, normal)) = find_surface(pm, pml.forward) else {
        return;
    };
    let mut res = Results {
        dir: normal,
        start: pm.ps.origin,
        flags: if surface & SURF_MANTLE_OVER != 0 {
            flag::OVER
        } else {
            0
        },
        ..Default::default()
    };
    for h in LEDGE_HEIGHTS {
        if check_ledge(pm, anims, &mut res, h) {
            break;
        }
    }
}

/// 0x80dddfe: a thin box (`mantle_check_radius`) swept flat along the view
/// from inside the body to `mantle_check_range` past it, under
/// `CONTENTS_MANTLE`. Returns the surface flags and the wall's inward
/// normal, flat.
fn find_surface(pm: &mut Pmove, forward: Vec3) -> Option<(u32, Vec3)> {
    let ps = &*pm.ps;
    let r = pm.rules.mantle_check_radius;
    let mins = Vec3::new(-r, -r, ps.mins.z);
    let maxs = Vec3::new(r, r, ps.maxs.z);
    let back = (d(ps.maxs.x) - d(r)) as f32;
    let range = pm.rules.mantle_check_range;
    let (dir, _) = fpu::normalize(Vec3::new(forward.x, forward.y, 0.0));
    let start = fpu::ma(ps.origin, -back, dir);
    let end = fpu::ma(ps.origin, (d(range) + d(back)) as f32, dir);
    let t = pm.raw_trace(start, end, mins, maxs, CONTENTS_MANTLE);
    if t.startsolid || t.allsolid || t.fraction == 1.0 {
        return None;
    }
    if t.surface_flags & (SURF_MANTLE_ON | SURF_MANTLE_OVER) == 0 {
        return None;
    }
    let (normal, len) = fpu::normalize(Vec3::new(-t.normal.x, -t.normal.y, 0.0));
    if len < MIN_NORMAL_LEN {
        return None;
    }
    let facing = f64::from(fpu::dot(dir, normal) as f32).acos();
    if f64::from(pm.rules.mantle_check_angle) < RAD_TO_DEG * facing {
        return None;
    }
    Some((t.surface_flags, normal))
}

/// `Mantle_CheckLedge` (0x80dd65a) at `height` over the body: a box pushed
/// 16 into the wall clear, a floor under it within 18 of the feet no
/// steeper than 0.7, and room to crouch there. Sets the hint; with the jump
/// key held, starts the mantle.
fn check_ledge(pm: &mut Pmove, anims: &MantleAnims, res: &mut Results, height: f32) -> bool {
    let r = pm.ps.maxs.x;
    let mins = Vec3::new(-r, -r, 0.0);
    let mut maxs = Vec3::new(r, r, (d(r) + d(r)) as f32);
    let mask = pm.tracemask;
    let mut start = res.start;
    start.z = (d(start.z) + d(height)) as f32;
    let end = fpu::ma(start, LEDGE_REACH, res.dir);
    let t = pm.raw_trace(start, end, mins, maxs, mask);
    if t.startsolid || t.fraction < 1.0 {
        return false;
    }
    let start = end;
    let mut end = end;
    end.z = (d(LEDGE_FLOOR) + d(res.start.z)) as f32;
    let t = pm.raw_trace(start, end, mins, maxs, mask);
    if t.startsolid || t.fraction == 1.0 {
        return false;
    }
    if t.normal.z < LEDGE_MIN_NORMAL_Z {
        return false;
    }
    res.ledge = end;
    res.ledge.z = ((d(end.z) - d(start.z)) * d(t.fraction) + d(start.z)) as f32;
    maxs.z = CROUCH_TOP;
    let t = pm.raw_trace(res.ledge, res.ledge, mins, maxs, mask);
    if t.startsolid {
        return false;
    }
    pm.ps.mantle.flags |= flag::HINT;
    res.flags |= flag::HINT;
    if pm.cmd.buttons & button::JUMP == 0 {
        return true;
    }
    check_over(pm, res);
    if pm.ps.e_flags & ef::CROUCH == 0 {
        let (mins, maxs) = (pm.ps.mins, pm.ps.maxs);
        if pm
            .raw_trace(res.ledge, res.ledge, mins, maxs, mask)
            .startsolid
        {
            res.flags |= flag::CROUCH;
        }
        if !pm.raw_trace(res.end, res.end, mins, maxs, mask).startsolid {
            res.flags |= flag::STAND;
        }
    }
    start_mantle(pm, anims, res);
    true
}

/// 0x80dd3dc: over a `MANTLE_OVER` wall the body ends 31 past the ledge and
/// up to 18 down, where a crouched box clears; anywhere else, or where it
/// does not clear, on the ledge.
fn check_over(pm: &mut Pmove, res: &mut Results) {
    res.end = res.ledge;
    if res.flags & flag::OVER == 0 {
        return;
    }
    let mins = pm.ps.mins;
    let mut maxs = pm.ps.maxs;
    maxs.z = CROUCH_TOP;
    let start = fpu::ma(res.ledge, OVER_REACH, res.dir);
    let mut end = start;
    end.z = (d(end.z) - d(OVER_DROP)) as f32;
    let t = pm.raw_trace(start, end, mins, maxs, pm.tracemask);
    if t.startsolid || t.fraction < 1.0 {
        res.flags &= !flag::OVER;
    } else {
        res.end = end;
        res.end.z = ((d(end.z) - d(start.z)) * d(t.fraction) + d(start.z)) as f32;
    }
}

/// 0x80dd35c: the row whose height is nearest the ledge's rise (stored),
/// the first on a tie.
fn nearest_row(start_z: f32, ledge_z: f32) -> i32 {
    let rise = ledge_z - start_z;
    let off = |k: usize| (TRANSITIONS[k].2 - rise).abs();
    let mut best = 0;
    let mut best_off = off(0);
    for k in 1..TRANSITIONS.len() {
        let o = off(k);
        if o < best_off {
            best = k;
            best_off = o;
        }
    }
    best as i32
}

/// 0x80dd552: the body set back from the mantle's end by the whole anim,
/// `pm_flags` 4 and `eFlags` 0x4000.
fn start_mantle(pm: &mut Pmove, anims: &MantleAnims, res: &Results) {
    let ps = &mut *pm.ps;
    ps.mantle = MantleState {
        yaw: fpu::vectoyaw(res.dir),
        timer: 0,
        trans_index: nearest_row(res.start.z, res.ledge.z),
        flags: res.flags,
    };
    let (up, over) = anims.lengths(&ps.mantle);
    let delta = anims.delta(&ps.mantle, up + over);
    ps.origin = sub(res.end, delta);
    ps.pm_flags |= pmf::MANTLE;
    ps.e_flags |= EF_MANTLE;
}

/// `Mantle_Move` (0x80de260): the timer steps by the frame, the body by the
/// anims' offset over it, the velocity is that offset over the frame; at
/// the end `pm_flags` 4 goes, and where the body can stand, `eFlags`
/// 0x4000 with `EV_STANCE_FORCE_STAND`.
pub(crate) fn mantle_move(pm: &mut Pmove, pml: &Pml) {
    let Some(anims) = enabled(pm) else {
        return;
    };
    let ps = &mut *pm.ps;
    ps.mantle.flags &= !flag::HINT;
    if ps.mantle.flags & flag::CROUCH != 0 {
        ps.add_event(super::ev::STANCE_FORCE_CROUCH, 0);
    }
    let (up, over) = anims.lengths(&ps.mantle);
    let total = up + over;
    let old = ps.mantle.timer;
    ps.mantle.timer = (old + pml.msec).min(total);
    let now = ps.mantle.timer;
    let d0 = anims.delta(&ps.mantle, old);
    let d1 = anims.delta(&ps.mantle, now);
    // 0x80dd1b2: the over anim once the timer is past the up anim.
    let (up_slot, over_slot, _) = row(&ps.mantle);
    let slot = if up < now { over_slot } else { up_slot };
    super::anim::mantle(pm, slot);
    let ps = &mut *pm.ps;
    let step = sub(d1, d0);
    ps.origin = add(step, ps.origin);
    let per_sec = (1.0 / (d(MSEC_TO_SEC) * f64::from((now - old) as f32))) as f32;
    ps.velocity = fpu::scale(step, per_sec);
    if now == total {
        ps.pm_flags &= !pmf::MANTLE;
        if ps.mantle.flags & flag::OVER != 0 {
            // `BG_AnimScriptEvent(ps, ANIM_ET_JUMP, 0, 1)`.
            super::anim::event(pm, super::anim::Event::Jump, false, true);
        }
        let ps = &mut *pm.ps;
        if ps.mantle.flags & flag::STAND != 0 {
            ps.add_event(super::ev::STANCE_FORCE_STAND, 0);
            ps.e_flags &= !EF_MANTLE;
        }
    }
}

/// `Mantle_CapView` (0x80de47a): a view more than `mantle_view_yawcap` off
/// the mantle's yaw is set back to the cap, the difference (wrapped into
/// the cap) pushed into `delta_angles[1]`.
pub(crate) fn cap_view(pm: &mut Pmove) {
    if !pm.rules.mantle_enable {
        return;
    }
    let cap = pm.rules.mantle_view_yawcap;
    let ps = &mut *pm.ps;
    let mut delta = fpu::angle_delta(ps.mantle.yaw, ps.view_angles[1]);
    if !(delta < -cap || cap < delta) {
        return;
    }
    while delta < -cap {
        delta = (d(delta) + d(cap)) as f32;
    }
    while cap < delta {
        delta = (d(delta) - d(cap)) as f32;
    }
    let edge = if 0.0 < delta { -cap } else { cap };
    ps.delta_angles[1] = ps.delta_angles[1].wrapping_add(fpu::shorts(delta));
    ps.view_angles[1] = fpu::normalize360_accurate((d(ps.mantle.yaw) + d(edge)) as f32);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(numframes: u16, frames: &[u16], keys: &[[f32; 3]]) -> Clip {
        Clip {
            length_ms: 0,
            numframes,
            frames: frames.to_vec(),
            keys: keys.iter().map(|&k| Vec3::from_array(k)).collect(),
        }
    }

    #[test]
    fn a_sparse_track_lerps_between_its_listed_frames() {
        let c = clip(
            10,
            &[0, 4, 10],
            &[[0.0; 3], [4.0, 0.0, 8.0], [10.0, 0.0, 8.0]],
        );
        assert_eq!(c.abs_delta(0.2), Vec3::new(2.0, 0.0, 4.0));
        assert_eq!(c.abs_delta(0.7), Vec3::new(7.0, 0.0, 8.0));
        assert_eq!(c.abs_delta(1.0), Vec3::new(10.0, 0.0, 8.0));
        let dense = clip(2, &[0, 1, 2], &[[0.0; 3], [1.0, 0.0, 0.0], [3.0, 0.0, 0.0]]);
        assert_eq!(dense.abs_delta(0.75), Vec3::new(2.0, 0.0, 0.0));
    }

    /// `Mantle_CreateAnims`' load check (0x80ddaa4): every up anim ends
    /// (16, 0, its row's height) along, every over anim (31, 0, -18), within
    /// a unit. Needs `COD2_DIR`.
    #[test]
    fn the_stock_anims_pass_the_engines_load_check() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let a = MantleAnims::load(&fs).unwrap();
        let near = |v: Vec3, want: Vec3| (v - want).abs().max_element() <= 1.0;
        for (up, over, h) in TRANSITIONS {
            let end = a.abs_delta(up, 1.0);
            assert!(
                near(end, Vec3::new(16.0, 0.0, h)),
                "{}: {end}",
                ANIM_NAMES[up]
            );
            let end = a.abs_delta(over, 1.0);
            assert!(
                near(end, Vec3::new(31.0, 0.0, -18.0)),
                "{}: {end}",
                ANIM_NAMES[over]
            );
        }
        // 25 frames at 30 a second.
        assert_eq!(a.length_ms(1), 833);
    }

    #[test]
    fn the_nearest_row_takes_the_first_on_a_tie() {
        assert_eq!(nearest_row(0.0, 57.0), 0);
        assert_eq!(nearest_row(0.0, 54.0), 0);
        assert_eq!(nearest_row(10.0, 40.0), 4);
        assert_eq!(nearest_row(10.0, 39.0), 5);
        assert_eq!(nearest_row(0.0, 0.0), 6);
        // The rise is stored: 76.125 - 28.125002 rounds to 48, a tie.
        assert_eq!(nearest_row(f32::from_bits(0x41e1_0001), 76.125), 1);
    }

    #[test]
    fn the_low_rows_keep_the_weapon_up() {
        let mut ps = PlayerState {
            pm_flags: pmf::MANTLE,
            ..Default::default()
        };
        ps.mantle.trans_index = 4;
        assert!(weapon_inactive(&ps));
        ps.mantle.trans_index = 5;
        assert!(!weapon_inactive(&ps));
        ps.pm_flags = 0;
        ps.mantle.trans_index = 0;
        assert!(!weapon_inactive(&ps));
    }
}
