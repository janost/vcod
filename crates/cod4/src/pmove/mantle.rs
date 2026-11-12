//! 1.7's mantle: `Mantle_Check` (0x8051f7a), `Mantle_CheckLedge`
//! (0x8051768), `Mantle_Move` (0x80514f2), `Mantle_GetAnimDelta`
//! (0x80513a6) and `Mantle_CapView` (0x8050dc6) of `cod4_lnxded`, with the
//! root motion of the `mp_mantle_*` clips they step the body along
//! (docs/research/cod4-pmove.md 17). CoD2's (`vcod_cod2::pmove::mantle`)
//! with a fixed 15-unit box, the trace's `walkable` for the ledge, `eFlags`
//! 0x8000 and `mantleState.flags` 0x10.
//!
//! The clips are the engine's `PLAYER_MANTLE` set (`Mantle_CreateAnims`
//! 0x8050fa4) out of `common_mp`, installed once per process
//! ([`install`]); [`Rules::mantle_anims`](super::Rules) carries them, and
//! without them no mantle starts.

use std::sync::OnceLock;

use glam::Vec3;

use super::fpu::{self, d};
use super::{PlayerState, Pml, Pmove, button, ef, ev, pm_type, pmf};
use crate::assets::Assets;
use crate::assets::model::XAnim;

/// `MANTLE_ON` and `MANTLE_OVER` surface flags, and the contents the
/// surface probe traces (`CONTENTS_MANTLE`).
pub const SURF_MANTLE_ON: u32 = 0x200_0000;
pub const SURF_MANTLE_OVER: u32 = 0x400_0000;
pub const CONTENTS_MANTLE: u32 = 0x100_0000;
/// `eFlags` 0x8000: mantling.
pub const EF_MANTLE: u32 = 0x8000;

/// `mantleState.flags` (ps 0x5ec).
pub mod flag {
    /// The wall is `MANTLE_OVER`: the over clip follows the up clip.
    pub const OVER: u32 = 0x1;
    /// The standing box does not fit on the ledge: `EV_STANCE_FORCE_CROUCH`
    /// every frame of the mantle.
    pub const CROUCH: u32 = 0x2;
    /// The standing box fits where the mantle ends.
    pub const STAND: u32 = 0x4;
    /// A mantle is available this frame.
    pub const HINT: u32 = 0x8;
    /// The mantle ended this frame (`Mantle_Check` clears it).
    pub const ENDED: u32 = 0x10;
}

/// `playerState_s.mantleState` (ps 0x5e0).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MantleState {
    /// `vectoyaw` of the wall's inward normal; the clips turn by it.
    pub yaw: f32,
    /// ms into the mantle.
    pub timer: i32,
    /// The row of [`TRANSITIONS`].
    pub trans_index: i32,
    pub flags: u32,
}

/// `s_mantleAnimNames` (the `XAnimCreateAnims` slots, strings 0x820a4ef..).
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

/// `s_mantleTrans` (rodata 0x820aa20): {up clip, over clip, the rise the
/// row is picked for}.
pub const TRANSITIONS: [(usize, usize, f32); 7] = [
    (1, 8, 57.0),
    (2, 8, 51.0),
    (3, 9, 45.0),
    (4, 9, 39.0),
    (5, 9, 33.0),
    (6, 10, 27.0),
    (7, 10, 21.0),
];

/// The over clip that leaves the weapon up (`Mantle_IsWeaponInactive`'s 10).
const OVER_LOW: usize = 10;
/// `Mantle_CheckLedge`'s heights, tried in order (immediates 0x42700000,
/// 0x42200000, 0x41a00000).
const LEDGE_HEIGHTS: [f32; 3] = [60.0, 40.0, 20.0];
/// The player's radius (rodata 0x820e030, `playerMaxs[0]`).
const RADIUS: f32 = 15.0;
/// The surface probe's height (rodata 0x820e038, `playerMaxs[2]`).
const PROBE_TOP: f32 = 70.0;
/// How far into the wall the ledge box goes (rodata 0x820967c).
const LEDGE_REACH: f32 = 16.0;
/// The down trace's floor over the start, and the over clip's drop
/// (rodata 0x820a314).
const LEDGE_FLOOR: f32 = 18.0;
/// The crouched box's top (immediate 0x42480000).
const CROUCH_TOP: f32 = 50.0;
/// The over clip's reach (rodata 0x820a9fc).
const OVER_REACH: f32 = 31.0;
/// rodata 0x820aa04: a wall normal flatter than this is vertical.
const MIN_NORMAL_LEN: f32 = f32::from_bits(0x38d1_b717);
/// rodata 0x820aa08 (double): 180 / pi.
const RAD_TO_DEG: f64 = f64::from_bits(0x404c_a5dc_1a63_c1f8);
/// rodata 0x820a288 (double): `VectorAngleMultiply`'s degrees to radians.
const DEG_TO_RAD: f64 = f64::from_bits(0x3f91_df46_a252_9d39);
/// rodata 0x8209640: ms to seconds.
const MSEC_TO_SEC: f32 = f32::from_bits(0x3a83_126f);

/// One clip's root translation track as the zone stores it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Clip {
    /// `XAnimGetLengthMsec` (0x81b726c): `numframes / framerate * 1000`
    /// kept, stored, truncated.
    length_ms: i32,
    numframes: u16,
    /// Key frames, and each key's quantized offset; one key is `frame0`.
    frames: Vec<u16>,
    quant: Vec<[u16; 3]>,
    mins: [f32; 3],
    size: [f32; 3],
    frame0: Vec3,
}

impl Clip {
    pub(crate) fn of(x: &XAnim) -> Clip {
        let length_ms = ((f64::from(x.num_frames) / d(x.framerate) * 1000.0) as f32).trunc() as i32;
        let mut c = Clip {
            length_ms,
            numframes: x.num_frames,
            ..Default::default()
        };
        if let Some(r) = &x.root {
            if r.trans_quant.is_empty() {
                c.frame0 = r.trans.first().map_or(Vec3::ZERO, |t| Vec3::from(t.1));
            } else {
                c.frames = r.trans.iter().map(|t| t.0).collect();
                c.quant = r.trans_quant.clone();
                c.mins = r.trans_mins;
                c.size = r.trans_size;
            }
        }
        c
    }

    /// `size * q + mins` per axis, the product kept.
    fn dequant(&self, q: [f64; 3]) -> Vec3 {
        Vec3::from_array(std::array::from_fn(|i| {
            (d(self.size[i]) * q[i] + d(self.mins[i])) as f32
        }))
    }

    /// `XAnim_CalcDeltaForTime`'s translation (0x81b0930) at fraction `t`:
    /// the last key at 1 (or without frames); else the keys around frame
    /// `(int)(float)(t * numframes)`, the quantized offsets lerped by the
    /// kept `(t * numframes - k0) / (k1 - k0)` and stored, then scaled.
    pub(crate) fn abs_delta(&self, t: f32) -> Vec3 {
        if self.quant.is_empty() {
            return self.frame0;
        }
        let q = |k: usize| self.quant[k].map(f64::from);
        let last = self.quant.len() - 1;
        if t == 1.0 || self.numframes == 0 {
            return self.dequant(q(last));
        }
        let frame_frac = d(t) * f64::from(self.numframes);
        let frame = (frame_frac as f32) as i32;
        let k = self
            .frames
            .partition_point(|&f| i32::from(f) <= frame)
            .saturating_sub(1)
            .min(last.saturating_sub(1));
        let (f0, f1) = (f64::from(self.frames[k]), f64::from(self.frames[k + 1]));
        let frac = (frame_frac - f0) / (f1 - f0);
        let (a, b) = (q(k), q(k + 1));
        let lerp: [f64; 3] = std::array::from_fn(|i| d(((b[i] - a[i]) * frac + a[i]) as f32));
        self.dequant(lerp)
    }
}

/// The `PLAYER_MANTLE` clips' root motion, by [`ANIM_NAMES`] slot.
#[derive(Clone, Debug, PartialEq)]
pub struct MantleAnims {
    clips: Vec<Clip>,
}

impl MantleAnims {
    /// The clips off the zones, first zone first; `None` names the
    /// missing clip.
    pub fn from_zones(zones: &[&Assets]) -> Result<MantleAnims, String> {
        let mut clips = vec![Clip::default()];
        for name in &ANIM_NAMES[1..] {
            let x = zones
                .iter()
                .flat_map(|a| a.xanims.iter())
                .find(|x| x.name == *name)
                .ok_or_else(|| format!("xanim {name} in no zone"))?;
            clips.push(Clip::of(x));
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

    /// `Mantle_GetUpLength` and `Mantle_GetOverLength` (0 without
    /// [`flag::OVER`]).
    fn lengths(&self, st: &MantleState) -> (i32, i32) {
        let (up, over, _) = row(st);
        let over = if st.flags & flag::OVER != 0 {
            self.length_ms(over)
        } else {
            0
        };
        (self.length_ms(up), over)
    }

    /// `Mantle_GetAnimDelta` (0x80513a6): the root's offset `t` ms in, the
    /// fraction `(float)(t / len)`; past the up clip its whole offset plus
    /// the over clip's, the sum stored; turned by the mantle's yaw.
    fn delta(&self, st: &MantleState, t: i32) -> Vec3 {
        let (up, over, _) = row(st);
        let (up_len, over_len) = self.lengths(st);
        let v = if up_len < t {
            let a = self.abs_delta(up, 1.0);
            let f = (f64::from(t - up_len) / f64::from(over_len)) as f32;
            let b = self.abs_delta(over, f);
            add(b, a)
        } else {
            let f = (f64::from(t) / f64::from(up_len)) as f32;
            self.abs_delta(up, f)
        };
        angle_multiply(v, st.yaw)
    }
}

static ANIMS: OnceLock<MantleAnims> = OnceLock::new();

/// Reads the mantle clips off `zones` the first time they hold them and
/// keeps them for the process, as retail keeps one `s_mantleAnims`.
pub fn install(zones: &[&Assets]) -> Option<&'static MantleAnims> {
    if let Some(a) = ANIMS.get() {
        return Some(a);
    }
    match MantleAnims::from_zones(zones) {
        Ok(a) => Some(ANIMS.get_or_init(|| a)),
        Err(e) => {
            log::warn!("cod4: no mantle: {e}");
            None
        }
    }
}

/// The clips [`install`] kept, if any zone held them yet.
pub fn installed() -> Option<&'static MantleAnims> {
    ANIMS.get()
}

/// `pmove_t`'s `mantleEndPos` and `mantleDuration` for a mantle under way:
/// where the clips end it and their whole length in ms.
pub fn end_and_duration(anims: &MantleAnims, ps: &PlayerState) -> (Vec3, i32) {
    let st = &ps.mantle;
    let (up, over) = anims.lengths(st);
    let total = up + over;
    let left = sub(anims.delta(st, total), anims.delta(st, st.timer));
    (add(ps.origin, left), total)
}

fn row(st: &MantleState) -> (usize, usize, f32) {
    TRANSITIONS[st.trans_index.clamp(0, 6) as usize]
}

fn add(a: Vec3, b: Vec3) -> Vec3 {
    Vec3::from_array(std::array::from_fn(|i| (d(a[i]) + d(b[i])) as f32))
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    Vec3::from_array(std::array::from_fn(|i| (d(a[i]) - d(b[i])) as f32))
}

/// `VectorAngleMultiply` (0x819558e): the radians stored, `sinf` stored,
/// `cosf` kept as returned, each new axis summed on the stack.
fn angle_multiply(v: Vec3, yaw: f32) -> Vec3 {
    let a = (DEG_TO_RAD * d(yaw)) as f32;
    let (s, c) = (d(a.sin()), d(a.cos()));
    Vec3::new(
        (d(v.x) * c - d(v.y) * s) as f32,
        (c * d(v.y) + s * d(v.x)) as f32,
        v.z,
    )
}

/// `Mantle_IsWeaponInactive` (0x8050d90): mantling on a row whose over clip
/// is not `player_mantle_over_low`.
pub fn weapon_inactive(ps: &PlayerState, rules: &super::Rules) -> bool {
    rules.mantle_enable && ps.pm_flags & pmf::MANTLE != 0 && row(&ps.mantle).1 != OVER_LOW
}

fn enabled(pm: &Pmove) -> Option<&'static MantleAnims> {
    if pm.rules.mantle_enable {
        pm.rules.mantle_anims
    } else {
        None
    }
}

/// `MantleResults` (0x38 bytes).
#[derive(Clone, Copy, Debug, Default)]
struct Results {
    dir: Vec3,
    start: Vec3,
    ledge: Vec3,
    end: Vec3,
    flags: u32,
}

/// `Mantle_Check` (0x8051f7a): clears [`flag::ENDED`] and the hint; refused
/// dead, mantling, crouched or prone (`eFlags` 0xc); else a `MANTLE_ON` or
/// `MANTLE_OVER` wall ahead and a ledge on it 60, 40 or 20 up.
pub(crate) fn check(pm: &mut Pmove, pml: &Pml) {
    pm.ps.mantle.flags &= !flag::ENDED;
    let Some(anims) = enabled(pm) else {
        return;
    };
    let ps = &mut *pm.ps;
    ps.mantle.flags &= !flag::HINT;
    if ps.pm_type >= pm_type::DEAD
        || ps.pm_flags & pmf::MANTLE != 0
        || ps.e_flags & (ef::CROUCH | ef::PRONE) != 0
    {
        return;
    }
    let Some((surface, dir)) = find_surface(pm, pml.forward) else {
        return;
    };
    let mut res = Results {
        dir,
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

/// The surface probe (inlined in `Mantle_Check`): a thin box
/// (`mantle_check_radius`, 0..70 tall) swept flat along the view from
/// inside the body to `mantle_check_range` past it, under
/// `CONTENTS_MANTLE`. Returns the surface flags and the wall's inward
/// normal, flat.
fn find_surface(pm: &mut Pmove, forward: Vec3) -> Option<(u32, Vec3)> {
    let r = pm.rules.mantle_check_radius;
    let mins = Vec3::new(-r, -r, 0.0);
    let maxs = Vec3::new(r, r, PROBE_TOP);
    let inner = d(RADIUS) - d(r);
    let reach = d(pm.rules.mantle_check_range) + inner;
    let len = (d(forward.y) * d(forward.y) + d(forward.x) * d(forward.x)).sqrt();
    let inv = if len > 0.0 { 1.0 / len } else { 1.0 };
    let (fx, fy) = (d(forward.x) * inv, d(forward.y) * inv);
    let o = pm.ps.origin;
    let start = Vec3::new(
        (fx * -inner + d(o.x)) as f32,
        (fy * -inner + d(o.y)) as f32,
        o.z,
    );
    let end = Vec3::new(
        (reach * fx + d(o.x)) as f32,
        (reach * fy + d(o.y)) as f32,
        o.z,
    );
    let t = pm.raw_trace(start, end, mins, maxs, CONTENTS_MANTLE);
    if t.startsolid || t.allsolid || t.fraction == 1.0 {
        return None;
    }
    if t.surface_flags & (SURF_MANTLE_ON | SURF_MANTLE_OVER) == 0 {
        return None;
    }
    let (nx, ny) = (-d(t.normal.x), -d(t.normal.y));
    let nlen = (ny * ny + nx * nx).sqrt();
    let ninv = if nlen > 0.0 { 1.0 / nlen } else { 1.0 };
    if nlen < d(MIN_NORMAL_LEN) {
        return None;
    }
    let dot = fy * ny * ninv + fx * nx * ninv;
    if d(pm.rules.mantle_check_angle) < dot.acos() * RAD_TO_DEG {
        return None;
    }
    Some((
        t.surface_flags,
        Vec3::new((nx * ninv) as f32, (ny * ninv) as f32, 0.0),
    ))
}

/// The crouched-height box all the ledge traces sweep: 15 wide, 0..30.
const LEDGE_MINS: Vec3 = Vec3::new(-RADIUS, -RADIUS, 0.0);

/// `Mantle_CheckLedge` (0x8051768) at `height` over the start: the box
/// pushed 16 into the wall clear, a walkable floor under it within 18 of
/// the feet, and room to crouch there. Sets the hint; with the jump key
/// held, starts the mantle.
fn check_ledge(pm: &mut Pmove, anims: &MantleAnims, res: &mut Results, height: f32) -> bool {
    let mut maxs = Vec3::new(RADIUS, RADIUS, RADIUS + RADIUS);
    let mask = pm.tracemask;
    let mut start = res.start;
    start.z = (d(height) + d(start.z)) as f32;
    let end = fpu::ma(start, LEDGE_REACH, res.dir);
    let t = pm.raw_trace(start, end, LEDGE_MINS, maxs, mask);
    if t.startsolid || t.fraction < 1.0 {
        return false;
    }
    let start = end;
    let mut end = end;
    end.z = (d(res.start.z) + d(LEDGE_FLOOR)) as f32;
    let t = pm.raw_trace(start, end, LEDGE_MINS, maxs, mask);
    if t.startsolid || t.fraction == 1.0 || !t.walkable {
        return false;
    }
    res.ledge = end;
    res.ledge.z = ((d(end.z) - d(start.z)) * d(t.fraction) + d(start.z)) as f32;
    maxs.z = CROUCH_TOP;
    if pm
        .raw_trace(res.ledge, res.ledge, LEDGE_MINS, maxs, mask)
        .startsolid
    {
        return false;
    }
    pm.ps.mantle.flags |= flag::HINT;
    res.flags |= flag::HINT;
    if pm.cmd.buttons & button::JUMP == 0 {
        return true;
    }
    end_pos(pm, res);
    if pm.ps.e_flags & ef::CROUCH == 0 {
        let (mins, maxs) = (super::PLAYER_MINS, super::PLAYER_MAXS);
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

/// `Mantle_CalcEndPos` (inlined): over a `MANTLE_OVER` wall the body ends
/// 31 past the ledge and up to 18 down where a crouched box clears both
/// sweeps; anywhere else, or where it does not clear, on the ledge.
fn end_pos(pm: &mut Pmove, res: &mut Results) {
    res.end = res.ledge;
    if res.flags & flag::OVER == 0 {
        return;
    }
    let maxs = Vec3::new(RADIUS, RADIUS, CROUCH_TOP);
    let mask = pm.tracemask;
    let start = res.ledge;
    let end = fpu::ma(start, OVER_REACH, res.dir);
    let t = pm.raw_trace(start, end, LEDGE_MINS, maxs, mask);
    if !t.startsolid && t.fraction >= 1.0 {
        let start = end;
        let mut end = end;
        end.z = (d(end.z) - d(LEDGE_FLOOR)) as f32;
        let t = pm.raw_trace(start, end, LEDGE_MINS, maxs, mask);
        if !t.startsolid && t.fraction >= 1.0 {
            res.end = end;
            res.end.z = ((d(end.z) - d(start.z)) * d(t.fraction) + d(start.z)) as f32;
            return;
        }
    }
    res.flags &= !flag::OVER;
}

/// `Mantle_FindTransition` (inlined): the row whose height is nearest the
/// rise (kept), each distance stored, the first on a tie.
fn nearest_row(start_z: f32, ledge_z: f32) -> i32 {
    let rise = d(ledge_z) - d(start_z);
    let off = |k: usize| ((d(TRANSITIONS[k].2) - rise) as f32).abs();
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

/// `Mantle_Start` (inlined): the body set back from the end by the clips'
/// whole offset, `pm_flags` 4, `eFlags` 0x8000, and `pmove_t`'s
/// `mantleStarted`, end and duration.
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
    pm.mantle_started = Some((res.end, up + over));
}

/// `Mantle_Move` (0x80514f2): the timer steps by the frame, the body by the
/// clips' offset over it (stored, the sum taking the kept difference), the
/// velocity is the stored offset over the frame; the legs play the clip's
/// movetype, the over clip's once the timer is past the up clip. At the end
/// `pm_flags` 4 goes, over a wall the jump event plays, and where the body
/// can stand `EV_STANCE_FORCE_STAND` and `eFlags` 0x8000 with it, and
/// [`flag::ENDED`] is set (docs/research/cod4-pmove.md 17.5).
pub(crate) fn mantle_move(pm: &mut Pmove, pml: &Pml) {
    let Some(anims) = enabled(pm) else {
        return;
    };
    let ps = &mut *pm.ps;
    ps.mantle.flags &= !flag::HINT;
    if ps.mantle.flags & flag::CROUCH != 0 {
        ps.add_event(ev::STANCE_FORCE_CROUCH, 0);
    }
    let (up, over) = anims.lengths(&ps.mantle);
    let total = up + over;
    let old = ps.mantle.timer;
    ps.mantle.timer = (old + pml.msec).min(total);
    let now = ps.mantle.timer;
    let d0 = anims.delta(&ps.mantle, old);
    let d1 = anims.delta(&ps.mantle, now);
    let (up_clip, over_clip, _) = row(&ps.mantle);
    let clip = if up < now { over_clip } else { up_clip };
    super::anim::mantle(pm, super::anim::movetype::MANTLE_ROOT + clip as u8);
    let ps = &mut *pm.ps;
    let kept: [f64; 3] = std::array::from_fn(|i| d(d1[i]) - d(d0[i]));
    let step = Vec3::from_array(kept.map(|k| k as f32));
    ps.origin = Vec3::from_array(std::array::from_fn(|i| (kept[i] + d(ps.origin[i])) as f32));
    let per_sec = 1.0 / (d(MSEC_TO_SEC) * f64::from(now - old));
    ps.velocity = Vec3::from_array(std::array::from_fn(|i| (d(step[i]) * per_sec) as f32));
    if now == total {
        ps.pm_flags &= !pmf::MANTLE;
        pm.mantle_started = None;
        if ps.mantle.flags & flag::OVER != 0 {
            super::anim::event(pm, super::anim::Event::Jump, false, true);
        }
        let ps = &mut *pm.ps;
        if ps.mantle.flags & flag::STAND != 0 {
            ps.add_event(ev::STANCE_FORCE_STAND, 0);
            ps.e_flags &= !EF_MANTLE;
        }
        ps.mantle.flags |= flag::ENDED;
    }
}

/// `Mantle_CapView` (0x8050dc6): a view more than `mantle_view_yawcap` off
/// the mantle's yaw is set back to the cap; the difference, wrapped into the
/// cap and kept, goes into `delta_angles[1]`.
pub(crate) fn cap_view(pm: &mut Pmove) {
    if !pm.rules.mantle_enable {
        return;
    }
    let cap = d(pm.rules.mantle_view_yawcap);
    let ps = &mut *pm.ps;
    let mut delta = angle_delta(ps.mantle.yaw, ps.view_angles[1]);
    if !(delta < -cap || cap < delta) {
        return;
    }
    while delta < -cap {
        delta += cap;
    }
    while cap < delta {
        delta -= cap;
    }
    let edge = if delta <= 0.0 { cap } else { -cap };
    ps.delta_angles[1] = (delta + d(ps.delta_angles[1])) as f32;
    ps.view_angles[1] = angle_normalize360((edge + d(ps.mantle.yaw)) as f32) as f32;
}

/// rodata 0x8209630: the float 1 / 360 the out-of-line angle helpers take.
const INV_360: f32 = f32::from_bits(0x3b36_0b61);

/// `AngleDelta` (0x8190590), out of line: `a - b` in turns, kept, less
/// `floor` of it plus a half, times 360, returned on the stack.
fn angle_delta(a: f32, b: f32) -> f64 {
    let s = (d(a) - d(b)) * d(INV_360);
    let fl = (s + 0.5).floor() as f32;
    (s - d(fl)) * 360.0
}

/// `AngleNormalize360` (0x8190540), out of line: the turns kept, their
/// fraction off `floor` times 360, less 360 once more at a whole turn,
/// returned on the stack.
fn angle_normalize360(a: f32) -> f64 {
    let s = d(a) * d(INV_360);
    let fl = s.floor() as f32;
    let r = (s - d(fl)) * 360.0;
    if r - 360.0 < 0.0 { r } else { r - 360.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(numframes: u16, frames: &[u16], quant: &[[u16; 3]]) -> Clip {
        Clip {
            length_ms: 0,
            numframes,
            frames: frames.to_vec(),
            quant: quant.to_vec(),
            mins: [0.0; 3],
            size: [1.0; 3],
            frame0: Vec3::ZERO,
        }
    }

    #[test]
    fn a_sparse_track_lerps_between_its_listed_frames() {
        let c = clip(10, &[0, 4, 10], &[[0; 3], [4, 0, 8], [10, 0, 8]]);
        assert_eq!(c.abs_delta(0.2), Vec3::new(2.0, 0.0, 4.0));
        assert_eq!(c.abs_delta(0.7), Vec3::new(7.0, 0.0, 8.0));
        assert_eq!(c.abs_delta(1.0), Vec3::new(10.0, 0.0, 8.0));
    }

    #[test]
    fn the_nearest_row_takes_the_first_on_a_tie() {
        assert_eq!(nearest_row(0.0, 57.0), 0);
        assert_eq!(nearest_row(0.0, 54.0), 0);
        assert_eq!(nearest_row(10.0, 40.0), 4);
        assert_eq!(nearest_row(10.0, 39.0), 5);
        assert_eq!(nearest_row(0.0, 0.0), 6);
    }

    /// `Mantle_CreateAnims`' load check (0x8050fa4): every up clip ends
    /// (16, 0, its row's rise) along, every over clip (31, 0, -18), within a
    /// unit. Needs `COD4_DIR`.
    #[test]
    fn the_stock_clips_pass_the_engines_load_check() {
        let Some(install) = crate::testing::cod4_install() else {
            return;
        };
        let z = crate::zone::Zone::open_path(&install.zone("english", "common_mp")).unwrap();
        let a = crate::walk::walk(z, Default::default()).unwrap().assets;
        let m = MantleAnims::from_zones(&[&a]).unwrap();
        let near = |v: Vec3, want: Vec3| (v - want).abs().max_element() <= 1.0;
        for (up, over, h) in TRANSITIONS {
            let end = m.abs_delta(up, 1.0);
            assert!(
                near(end, Vec3::new(16.0, 0.0, h)),
                "{}: {end}",
                ANIM_NAMES[up]
            );
            let end = m.abs_delta(over, 1.0);
            assert!(
                near(end, Vec3::new(31.0, 0.0, -18.0)),
                "{}: {end}",
                ANIM_NAMES[over]
            );
            println!(
                "{} {} ms, {} {} ms",
                ANIM_NAMES[up],
                m.length_ms(up),
                ANIM_NAMES[over],
                m.length_ms(over)
            );
        }
    }

    #[test]
    fn the_low_rows_keep_the_weapon_up() {
        let rules = crate::pmove::Rules::default();
        let mut ps = PlayerState {
            pm_flags: pmf::MANTLE,
            ..Default::default()
        };
        ps.mantle.trans_index = 4;
        assert!(weapon_inactive(&ps, &rules));
        ps.mantle.trans_index = 5;
        assert!(!weapon_inactive(&ps, &rules));
        ps.pm_flags = 0;
        ps.mantle.trans_index = 0;
        assert!(!weapon_inactive(&ps, &rules));
    }
}
