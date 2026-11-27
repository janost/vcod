//! What CoD2's and CoD4's mantle share: the state, the flag bits, the anim
//! and transition tables, the scalars both `Mantle_*` ports read, and the
//! root-motion arithmetic over a game's clips. Each game keeps its clip
//! storage, ledge and surface traces, `Mantle_Move` and `Mantle_CapView`
//! (`vcod_cod2::pmove::mantle`, `vcod_cod4::pmove::mantle`; the differences
//! are cod4-pmove.md 17.1).

use glam::Vec3;

/// `mantleState.flags`; CoD4 adds its own `ENDED` (0x10).
pub mod flag {
    /// The ledge's surface is `MANTLE_OVER`: the over anim follows the up.
    pub const OVER: u32 = 0x1;
    /// The standing box does not fit at the ledge: `EV_STANCE_FORCE_CROUCH`
    /// every frame of the mantle.
    pub const CROUCH: u32 = 0x2;
    /// The standing box fits where the mantle ends: `EV_STANCE_FORCE_STAND`
    /// and the mantle `eFlags` bit cleared at the end.
    pub const STAND: u32 = 0x4;
    /// A mantle is available this frame (cleared every `PmoveSingle`).
    pub const HINT: u32 = 0x8;
}

/// `playerState_s.mantleState`.
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

/// `s_mantleAnimNames`: the `XAnimCreateAnims` slots.
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

/// `s_mantleTrans`: {up anim, over anim, the ledge height the row is picked
/// for}.
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
pub const OVER_LOW: usize = 10;
/// `Mantle_CheckLedge`'s heights, tried in order (immediates 0x42700000,
/// 0x42200000, 0x41a00000).
pub const LEDGE_HEIGHTS: [f32; 3] = [60.0, 40.0, 20.0];
/// How far into the wall the ledge box is pushed (0x41800000).
pub const LEDGE_REACH: f32 = 16.0;
/// The down trace's floor over the start.
pub const LEDGE_FLOOR: f32 = 18.0;
/// The crouched box's top (0x42480000).
pub const CROUCH_TOP: f32 = 50.0;
/// The over anim's reach (immediate 0x41f80000).
pub const OVER_REACH: f32 = 31.0;
/// A wall normal flatter than this is vertical.
pub const MIN_NORMAL_LEN: f32 = f32::from_bits(0x38d1_b717);
/// 180 / pi, a double.
pub const RAD_TO_DEG: f64 = f64::from_bits(0x404c_a5dc_1a63_c1f8);
/// pi / 180, a double: `VectorAngleMultiply`'s.
pub const DEG_TO_RAD: f64 = f64::from_bits(0x3f91_df46_a252_9d39);

/// One game's anim clip.
pub trait Clip {
    /// `XAnimGetLengthMsec`.
    fn length_ms(&self) -> i32;
    /// `XAnimGetAbsDelta`'s translation at time fraction `t`.
    fn abs_delta(&self, t: f32) -> Vec3;
    /// `VectorAngleMultiply`: `v` turned by `yaw` degrees, as the game's
    /// binary rounds it.
    fn turn(v: Vec3, yaw: f32) -> Vec3;
}

/// The `PLAYER_MANTLE` anims' root motion, by [`ANIM_NAMES`] slot.
#[derive(Clone, Debug, PartialEq)]
pub struct MantleAnims<C> {
    pub clips: Vec<C>,
}

/// The row of [`TRANSITIONS`] `st` stands on.
pub fn row(st: &MantleState) -> (usize, usize, f32) {
    TRANSITIONS[st.trans_index.clamp(0, 6) as usize]
}

/// `a + b`, each component summed in `f64` and stored.
pub fn add(a: Vec3, b: Vec3) -> Vec3 {
    Vec3::from_array(std::array::from_fn(|i| {
        (f64::from(a[i]) + f64::from(b[i])) as f32
    }))
}

/// `a - b`, each component differenced in `f64` and stored.
pub fn sub(a: Vec3, b: Vec3) -> Vec3 {
    Vec3::from_array(std::array::from_fn(|i| {
        (f64::from(a[i]) - f64::from(b[i])) as f32
    }))
}

impl<C: Clip> MantleAnims<C> {
    /// `XAnimGetLengthMsec` of slot `anim`.
    pub fn length_ms(&self, anim: usize) -> i32 {
        self.clips[anim].length_ms()
    }

    /// `XAnimGetAbsDelta`'s translation of slot `anim` at fraction `t`.
    pub fn abs_delta(&self, anim: usize, t: f32) -> Vec3 {
        self.clips[anim].abs_delta(t)
    }

    /// The up anim's length and the over anim's, 0 without [`flag::OVER`].
    pub fn lengths(&self, st: &MantleState) -> (i32, i32) {
        let (up, over, _) = row(st);
        let over = if st.flags & flag::OVER != 0 {
            self.length_ms(over)
        } else {
            0
        };
        (self.length_ms(up), over)
    }

    /// `Mantle_GetAnimDelta`: the root's offset `t` ms into the mantle, the
    /// fraction `(float)(t / len)`; past the up anim its whole offset plus
    /// the over anim's, the sum stored; turned by the mantle's yaw.
    pub fn delta(&self, st: &MantleState, t: i32) -> Vec3 {
        let (up, over, _) = row(st);
        let (up_len, over_len) = self.lengths(st);
        let v = if up_len < t {
            let a = self.abs_delta(up, 1.0);
            let f = (f64::from(t - up_len) / f64::from(over_len)) as f32;
            add(self.abs_delta(over, f), a)
        } else {
            self.abs_delta(up, (f64::from(t) / f64::from(up_len)) as f32)
        };
        C::turn(v, st.yaw)
    }

    /// Where the anims end a mantle under way from `origin`, and their whole
    /// length in ms. INFERRED: retail stores the start's end and the two
    /// anims' lengths.
    pub fn end_and_duration(&self, st: &MantleState, origin: Vec3) -> (Vec3, i32) {
        let (up, over) = self.lengths(st);
        let total = up + over;
        let left = sub(self.delta(st, total), self.delta(st, st.timer));
        (add(origin, left), total)
    }
}
