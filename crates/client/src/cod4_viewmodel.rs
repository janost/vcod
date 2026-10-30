//! The CoD4 view weapon, renderer-free: the hands and gun the drawn
//! playerstate holds, their clips picked off `weapAnim` as the 1.7 cgame's
//! `WeaponRunXModelAnims` picks them, the sight layer off
//! `fWeaponPosFrac`, and the gun's place off the view (lean, the move
//! offsets, idle drift, bob and sway) as `CG_AddViewWeapon` computes it.
//! docs/research/cod4-client-view.md 4.
//!
//! [`ViewDef`] is what a zone gives a weapon (built at load, the zones
//! dropped after); [`Cod4View`] holds the rigs and the state the frames
//! step. The damage kick, the gun recoil and the landing dip come in on
//! [`ViewPs`] (`play::cod4_kick`). Left out: the perks' anim rates, camo
//! variants (every gun draws `gunModel[0]`, its `hideTags` surfaces left
//! out) and the rocket and goggles models.

use glam::{Quat, Vec3};
use std::collections::HashMap;
use std::sync::Arc;
use vcod_common::model::Model;
use vcod_common::{skeleton, xanim::XAnim};

use crate::cod2_viewmodel::ViewFrame;
use crate::cod4_fly::WeaponModels;
use crate::play::cod4_players::WeaponTags;
use crate::renderer::VmDraw;
use crate::viewmodel::ViewModels;

/// `pm_type`s: a spectator and the intermission draw no view weapon
/// (`CG_AddViewWeapon`), from `PM_DEAD` on its anims reset
/// (`CG_UpdateViewWeaponAnim`).
const PM_SPECTATOR: u32 = 4;
const PM_INTERMISSION: u32 = 5;
const PM_DEAD: u32 = 7;
/// `eFlags`: on a turret (no view weapon), crouched, prone.
const EF_TURRET: u32 = 0x300;
const EF_CROUCH: u32 = 4;
const EF_PRONE: u32 = 8;
/// `pm_flags`: the sight is up; sprinting.
pub(crate) const PMF_SIGHT: u32 = 0x10;
const PMF_SPRINT: u32 = 0x8000;
/// `weapFlags`: the off hand's weapon is out (`BG_GetViewmodelWeaponIndex`).
const WF_OFFHAND: u32 = 2;
/// `weaponstate`s the cgame tests: reloading, the night vision on and off.
const WEAPON_RELOADING: u32 = 7;
const WEAPON_NV_WEAR: u32 = 25;
const WEAPON_NV_REMOVE: u32 = 26;
/// `weapAnim`'s restart toggle.
const ANIM_TOGGLEBIT: u32 = 0x200;
/// The view tree's nodes: the main clips 1..=30, the sight's two.
const NODES: usize = 33;
const IDLE: usize = 1;
const EMPTY_IDLE: usize = 2;
const ADS_UP: usize = 31;
const ADS_DOWN: usize = 32;

/// `cg_fov`, `cg_fovScale` and `cg_fovMin`'s defaults (iw3mp.exe
/// 0x43ad70, 0x43adca, 0x43ae2c); `cg_gun_x/y/z` default 0 (0x43beb7..).
pub const CG_FOV: f32 = 65.0;
const CG_FOV_SCALE: f32 = 1.0;
const CG_FOV_MIN: f32 = 10.0;

/// `bg_bobAmplitude*`' defaults (iw3mp.exe 0x40a0bd-0x40a24c), horizontal
/// then vertical.
const BOB_STANDING: [f32; 2] = [0.007, 0.007];
const BOB_DUCKED: [f32; 2] = [0.0075, 0.0075];
const BOB_PRONE: [f32; 2] = [0.02, 0.005];
const BOB_SPRINTING: [f32; 2] = [0.02, 0.014];

/// The drawn playerstate's fields the view weapon reads: the newer
/// snapshot's, `fWeaponPosFrac`, the velocity and `leanf` lerped as
/// `CG_InterpolatePlayerState` lerps them, the view angles the scene's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewPs {
    pub client_num: u32,
    pub pm_type: u32,
    pub pm_flags: u32,
    pub eflags: u32,
    pub weap_flags: u32,
    pub weapon: u32,
    pub off_hand: u32,
    pub weap_anim: u32,
    pub weaponstate: u32,
    pub weapon_time: i32,
    pub pos_frac: f32,
    pub bob_cycle: u32,
    pub xyspeed: f32,
    pub speed: f32,
    pub view_height_target: i32,
    pub view_height_current: f32,
    pub leanf: f32,
    pub viewmodel_index: u32,
    /// Pitch, yaw, roll.
    pub angles: Vec3,
    pub ammoclip: [i32; 128],
    /// What the crosshair reads (`cod4_crosshair`): `aimSpreadScale`
    /// (0..255), `perks`, `grenadeTimeLeft`, `spreadOverride` and its
    /// state.
    pub aim_spread_scale: f32,
    pub perks: u32,
    pub grenade_time_left: i32,
    pub spread_override: i32,
    pub spread_override_state: i32,
    /// The feet (`ps.origin`) and `otherFlags` (2 following, 4 our own
    /// view): what the grenade indicator measures from (cod4-hud.md 21).
    pub origin: Vec3,
    pub other_flags: u32,
    /// The gun spring's pitch and yaw, the landing (`landChange`, ms
    /// since), and the damage kick with its ms since the hit
    /// (`play::cod4_kick`, cod4-client-view.md 14).
    pub gun_kick: [f32; 2],
    pub land: (f32, i32),
    pub damage: Option<(crate::play::cod4_kick::DamageKick, i32)>,
    /// `viewlocked` and `viewlocked_entNum`: the turret a gunner is
    /// locked to (1023 none).
    pub viewlocked: u32,
    pub viewlocked_ent: u32,
    /// The gun's forward in the view's frame (forward, left, up) while a
    /// sight's lerp turns it off the view, the frame's view weapon's
    /// (`CalculateWeaponPosition_SaveOffsetAngles`); `None`
    /// when `cg.gunPitch` and `gunYaw` are the view's.
    pub gun_dir: Option<[f32; 3]>,
}

impl Default for ViewPs {
    fn default() -> Self {
        ViewPs {
            client_num: 0,
            pm_type: PM_SPECTATOR,
            pm_flags: 0,
            eflags: 0,
            weap_flags: 0,
            weapon: 0,
            off_hand: 0,
            weap_anim: 0,
            weaponstate: 0,
            weapon_time: 0,
            pos_frac: 0.0,
            bob_cycle: 0,
            xyspeed: 0.0,
            speed: 190.0,
            view_height_target: 60,
            view_height_current: 60.0,
            leanf: 0.0,
            viewmodel_index: 0,
            angles: Vec3::ZERO,
            ammoclip: [0; 128],
            aim_spread_scale: 0.0,
            perks: 0,
            grenade_time_left: 0,
            spread_override: 0,
            spread_override_state: 0,
            origin: Vec3::ZERO,
            other_flags: 0,
            gun_kick: [0.0; 2],
            land: (0.0, i32::MAX),
            damage: None,
            viewlocked: 0,
            viewlocked_ent: 1023,
            gun_dir: None,
        }
    }
}

impl ViewPs {
    /// `BG_GetViewmodelWeaponIndex`: the off hand's weapon under
    /// `weapFlags` 2, else `ps.weapon`.
    pub fn view_weapon(&self) -> u32 {
        if self.weap_flags & WF_OFFHAND != 0 {
            self.off_hand
        } else {
            self.weapon
        }
    }

    /// `CG_AddViewWeapon`'s test: a body's own view, not a spectator's,
    /// the intermission's or a turret gunner's.
    pub fn draws_gun(&self) -> bool {
        !matches!(self.pm_type, PM_SPECTATOR | PM_INTERMISSION) && self.eflags & EF_TURRET == 0
    }
}

/// `WeaponDef`'s fields the view weapon reads, at their 1.7 offsets
/// (cod4-client-view.md 4.1).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewNumbers {
    pub aim_down_sight: bool,
    pub overlay_reticle: bool,
    /// `adsOverlayShader` (+1072) set: with `overlay_reticle`, what makes
    /// `CG_GetWeapReticleZoom` look at the sight at all.
    pub overlay_material: bool,
    pub ads_zoom_fov: f32,
    pub ads_zoom_in_frac: f32,
    pub ads_zoom_out_frac: f32,
    pub ads_bob_factor: f32,
    pub ads_aim_pitch: f32,
    pub position_reload_trans_time: i32,
    /// Stand, ducked, prone: the move offset, the move turn, the stance
    /// offset (ducked and prone only), each move's and turn's min speed.
    pub move_ofs: [[f32; 3]; 3],
    pub move_rot: [[f32; 3]; 3],
    pub stance_ofs: [[f32; 3]; 3],
    pub move_min_speed: [f32; 3],
    pub rot_min_speed: [f32; 3],
    /// `posMoveRate`, `posProneMoveRate`, `posRotRate`,
    /// `posProneRotRate`.
    pub move_rate: [f32; 2],
    pub rot_rate: [f32; 2],
    /// Ads then hip idle amount and speed; the crouch and prone factors.
    pub idle_amount: [f32; 2],
    pub idle_speed: [f32; 2],
    pub idle_factor: [f32; 2],
    /// Hip then ads: max angle, lerp speed, pitch, yaw, horizontal and
    /// vertical scale.
    pub sway: [[f32; 6]; 2],
    /// `nightVisionWearTime`.
    pub nv_wear_time: i32,
    /// `adsDofStart`, `adsDofEnd`: the view model's own depth of field
    /// up the sight (`crate::cod4_dof`).
    pub ads_dof: [f32; 2],
    /// Each node's time in ms its clip is stretched over
    /// (`g_animRateOffsets`), `None` for a node that plays at its own rate.
    pub anim_ms: Vec<Option<i32>>,
}

/// `g_animRateOffsets` (iw3mp.exe 0x727580): each node's time field in
/// `WeaponDef`, -1 for none.
const ANIM_RATE_OFFSETS: [i32; NODES] = [
    -1, -1, -1, -1, 888, -1, -1, 896, 900, 904, 912, 920, 928, 936, 956, 932, 944, 940, 952, 948,
    960, 964, 968, 972, 976, -1, 980, 992, -1, -1, -1, -1, -1,
];

impl ViewNumbers {
    /// Read off a `WeaponDef`'s 0x878 bytes as the zone loaded them.
    pub fn from_raw(raw: &[u8]) -> ViewNumbers {
        let i = |o: usize| {
            raw.get(o..o + 4)
                .map_or(0, |b| i32::from_le_bytes(b.try_into().unwrap()))
        };
        let f = |o: usize| f32::from_bits(i(o) as u32);
        let v = |o: usize| [f(o), f(o + 4), f(o + 8)];
        ViewNumbers {
            aim_down_sight: i(1260) != 0,
            overlay_reticle: i(1080) != 0,
            overlay_material: i(1072) != 0,
            ads_zoom_fov: f(1060),
            ads_zoom_in_frac: f(1064),
            ads_zoom_out_frac: f(1068),
            ads_bob_factor: f(1096),
            ads_aim_pitch: f(1740),
            position_reload_trans_time: i(1940),
            move_ofs: [v(564), v(600), v(636)],
            move_rot: [v(576), v(612), v(648)],
            stance_ofs: [[0.0; 3], v(588), v(624)],
            move_min_speed: [f(668), f(672), f(676)],
            rot_min_speed: [f(688), f(692), f(696)],
            move_rate: [f(660), f(664)],
            rot_rate: [f(680), f(684)],
            idle_amount: [f(1164), f(1168)],
            idle_speed: [f(1172), f(1176)],
            idle_factor: [f(1180), f(1184)],
            sway: [
                [f(1196), f(1200), f(1204), f(1208), f(1212), f(1216)],
                [f(1224), f(1228), f(1232), f(1236), f(1240), f(1244)],
            ],
            nv_wear_time: i(980),
            ads_dof: [f(2160), f(2164)],
            anim_ms: ANIM_RATE_OFFSETS
                .iter()
                .map(|&o| (o >= 0).then(|| i(o as usize)))
                .collect(),
        }
    }
}

/// What a zone gives one weapon's view: the gun and hands models (in the
/// session's models by name), the 33 clips by node and the numbers.
pub struct ViewDef {
    /// The gun's key in the session's models: `gunModel[0]`'s name, with
    /// `#` and the `hideTags` appended when it hides parts.
    pub gun: String,
    /// `gunModel[0]`'s name, and the tags whose surfaces it hides.
    pub gun_model: String,
    pub hide_tags: Vec<String>,
    pub hands: Option<String>,
    /// `knifeModel`, grafted at `tag_knife_attach` while a melee plays.
    pub knife: Option<String>,
    /// Node `n`'s clip; an empty view anim plays idle's
    /// (`CG_CreateWeaponViewModelXAnim`). `None` where no zone holds it.
    pub anims: Vec<Option<Arc<XAnim>>>,
    pub clip_name: String,
    pub numbers: ViewNumbers,
    /// `notetrackSoundMap` (`WeaponDef` +232 keys, +264 values, 16 script
    /// strings each): a view clip's notetrack, lower case, and the alias
    /// it plays (the notetrack sound step, `iw3mp.exe` 0x454a70).
    pub note_sounds: Vec<(String, String)>,
}

/// One weapon's hands and gun grafted at `tag_weapon` (and the knife at
/// `tag_knife_attach` during a melee), each node's clip bound to the
/// skeleton and its rate.
struct Rig {
    models: ViewModels,
    skeleton: skeleton::Skeleton,
    clips: Vec<Option<(Arc<XAnim>, skeleton::AnimBinding)>>,
    rates: Vec<f32>,
    def: Arc<ViewDef>,
}

impl Rig {
    fn build(def: Arc<ViewDef>, hands: &Model, gun: &Model, knife: Option<&Model>) -> Rig {
        let mut parts = vec![(hands, None), (gun, Some("tag_weapon"))];
        parts.extend(knife.map(|k| (k, Some("tag_knife_attach"))));
        let skeleton = skeleton::Skeleton::build_grafted(&parts);
        let clips: Vec<_> = def
            .anims
            .iter()
            .map(|c| c.clone().map(|c| (c.clone(), skeleton.bind(&c))))
            .collect();
        // The anim rate: the clip's length over its time, 0 for a
        // time of 0, 1 for a node with no time.
        let rates = (0..NODES)
            .map(|n| {
                match (
                    def.numbers.anim_ms.get(n).copied().flatten(),
                    clips.get(n).and_then(|c| c.as_ref()),
                ) {
                    (Some(0), _) => 0.0,
                    (Some(ms), Some((c, _))) => c.duration() * 1000.0 / ms as f32,
                    _ => 1.0,
                }
            })
            .collect();
        Rig {
            models: parts
                .iter()
                .map(|(m, _)| (*m).clone())
                .collect::<Vec<_>>()
                .into(),
            skeleton,
            clips,
            rates,
            def,
        }
    }

    fn clip(&self, n: usize) -> Option<&(Arc<XAnim>, skeleton::AnimBinding)> {
        self.clips.get(n)?.as_ref()
    }

    /// `XAnimHasFinished`: a looping clip never does.
    fn finished(&self, n: usize, t: f32) -> bool {
        self.clip(n)
            .is_none_or(|(c, _)| !c.looping && t >= c.duration())
    }
}

/// `WeaponRunXModelAnims`' switch:
/// the node a `weapAnim` (toggle bit off) starts; `None` for 0 and 1,
/// which wait for the running clip and then idle.
pub fn anim_node(weap_anim: u32) -> Option<usize> {
    Some(match weap_anim & !ANIM_TOGGLEBIT {
        0 | 1 => return None,
        2 => 3,
        3 => 5,
        4 => 6,
        5 => 28,
        6 => 29,
        7 => 30,
        8 => 7,
        9 => 8,
        10 => 15,
        11 => 13,
        12 => 14,
        13 => 9,
        14 => 10,
        15 => 11,
        16 => 12,
        17 => 17,
        18 => 16,
        19 => 19,
        20 => 18,
        21 => 21,
        22 => 20,
        23 => 22,
        24 => 23,
        25 => 24,
        26 => 4,
        27 => 25,
        28 => 26,
        29 => 27,
        _ => IDLE,
    })
}

/// The main node and its clock as `WeaponRunXModelAnims` moves them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewAnim {
    pub node: usize,
    /// Seconds into the node's clip (rate applied).
    pub t: f32,
    /// The last anim: the `weapAnim` last acted on, `None` (-1) while a 0
    /// waits for the running clip.
    prev: Option<u32>,
    /// How many times a node started, for the replay gate.
    pub starts: usize,
}

impl Default for ViewAnim {
    fn default() -> Self {
        ViewAnim {
            node: IDLE,
            t: 0.0,
            prev: None,
            starts: 0,
        }
    }
}

impl ViewAnim {
    fn start(&mut self, node: usize, weap_anim: u32) {
        *self = ViewAnim {
            node,
            t: 0.0,
            prev: Some(weap_anim),
            starts: self.starts + 1,
        };
    }

    /// Steps `dt` seconds under `weap_anim`; a changed value starts its
    /// node, a 0 or 1 waits for the running clip (a 1 cuts it) and then
    /// plays idle, or empty idle with the clip empty. Returns the node's
    /// time before the step (0 for a node it started).
    fn step(&mut self, rig: &Rig, weap_anim: u32, clip_empty: bool, dt: f32) -> f32 {
        if self.prev != Some(weap_anim) {
            match anim_node(weap_anim) {
                Some(n) => self.start(n, weap_anim),
                None => {
                    let running =
                        (1..ADS_UP).contains(&self.node) && !rig.finished(self.node, self.t);
                    if running && weap_anim & 1 == 0 {
                        self.prev = None;
                    } else {
                        self.start(if clip_empty { EMPTY_IDLE } else { IDLE }, weap_anim);
                    }
                }
            }
        }
        let from = self.t;
        self.t += dt * rig.rates.get(self.node).copied().unwrap_or(1.0);
        from
    }
}

/// The notetracks of `clip` whose time falls in `[from, to)` seconds, in
/// order, a looping clip's in every cycle; `end` is the clip's own
/// (the notetrack step, `iw3mp.exe` 0x454ae0, skips it).
fn notes_crossed(clip: &XAnim, from: f32, to: f32) -> Vec<&str> {
    let period = clip.duration();
    if to <= from || clip.framerate <= 0.0 {
        return Vec::new();
    }
    let cycles = match clip.looping && period > 0.0 {
        true => (from / period).floor() as i32..=(to / period).floor() as i32,
        false => 0..=0,
    };
    let mut out = Vec::new();
    for c in cycles {
        for (name, frame) in &clip.notes {
            let at = c as f32 * period + f32::from(*frame) / clip.framerate;
            if (from..to).contains(&at) && !name.eq_ignore_ascii_case("end") {
                out.push(name.as_str());
            }
        }
    }
    out
}

/// The sight layer's weights, ADS up and ADS down, as `PlayADSAnim` sets
/// their goals (1 for the wanted one, 0 for the other) over 0.1 s each
/// frame. Registering starts ADS down at 1 (`CG_RegisterWeapon`).
#[derive(Clone, Copy, Debug, PartialEq)]
struct AdsBlend([f32; 2]);

impl Default for AdsBlend {
    fn default() -> Self {
        AdsBlend([0.0, 1.0])
    }
}

impl AdsBlend {
    fn step(&mut self, up: bool, dt: f32) {
        let goals = if up { [1.0, 0.0] } else { [0.0, 1.0] };
        for (w, g) in self.0.iter_mut().zip(goals) {
            let step = dt / 0.1;
            *w = if *w < g {
                (*w + step).min(g)
            } else {
                (*w - step).max(g)
            };
        }
    }
}

/// The gun's place off the view as the frames carry it: the eased move
/// offset and turn, the idle drift's scale and clock, the sway
/// (cod4-client-view.md 4.3).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct GunState {
    /// Forward, left, up.
    move_offset: [f32; 3],
    /// Pitch, yaw, roll.
    move_turn: [f32; 3],
    idle_scale: f32,
    /// `weapIdleTime`: ms scaled by the idle speed, truncated each frame.
    idle_clock: i32,
    sway: Sway,
    /// The sight-up flag: the sight is going up.
    to_ads: bool,
}

/// The gun's lag behind the view's turn: the view it last saw, the
/// left and up offset and the pitch and yaw it trails by.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Sway {
    view: [f32; 3],
    lag: [f32; 2],
    tilt: [f32; 2],
}

/// The fastest the move offset and the move turn ease, per ms: 0.1 a
/// second for the offset, a hair under it for the turn.
const OFFSET_MIN_PER_MS: f32 = 0.1 * 0.001;
const TURN_MIN_PER_MS: f32 = 0.0001;

/// `a` to `b` by `f`.
fn lerp(a: f32, b: f32, f: f32) -> f32 {
    (b - a) * f + a
}

/// `a - b` folded into one turn about 0.
fn angle_diff(a: f32, b: f32) -> f32 {
    let d = a - b;
    d - 360.0 * (d / 360.0).round()
}

/// `cur` moved `step` toward `goal`, never past it.
fn approach(cur: f32, goal: f32, step: f32) -> f32 {
    if goal < cur {
        (cur - step).max(goal)
    } else {
        (cur + step).min(goal)
    }
}

/// `DiffTrack`: `cur` closes `rate` of the gap a second, landing on
/// `goal` within 0.001 or when the step would overshoot.
fn track(goal: f32, cur: f32, rate: f32, dt: f32) -> f32 {
    let gap = goal - cur;
    let step = rate * gap * dt;
    if gap.abs() > 0.001 && step.abs() <= gap.abs() {
        cur + step
    } else {
        goal
    }
}

/// `DiffTrackAngle`: [`track`] the short way round, wrapped to -180..180.
fn track_angle(goal: f32, cur: f32, rate: f32, dt: f32) -> f32 {
    let mut near = goal;
    while near - cur > 180.0 {
        near -= 360.0;
    }
    while near - cur < -180.0 {
        near += 360.0;
    }
    let turns = track(near, cur, rate, dt) / 360.0;
    (turns - (turns + 0.5).floor()) * 360.0
}

/// How much of the move offset and turn shows: all on the hip, fading
/// out over the sight's first half.
fn hip_share(frac: f32) -> f32 {
    if frac == 0.0 {
        1.0
    } else if frac < 0.5 {
        1.0 - 2.0 * frac
    } else {
        0.0
    }
}

/// `leanf` eased toward its ends.
fn lean_curve(leanf: f32) -> f32 {
    (2.0 - leanf.abs()) * leanf
}

fn sum3(terms: &[[f32; 3]]) -> [f32; 3] {
    terms
        .iter()
        .fold([0.0; 3], |acc, t| std::array::from_fn(|k| acc[k] + t[k]))
}

/// The per-stance slot of [`ViewNumbers`]' arrays, off `eFlags`.
fn stance_slot(ps: &ViewPs) -> usize {
    match ps.eflags {
        f if f & EF_PRONE != 0 => 2,
        f if f & EF_CROUCH != 0 => 1,
        _ => 0,
    }
}

impl ViewPs {
    fn night_vision_moving(&self) -> bool {
        matches!(self.weaponstate, WEAPON_NV_WEAR | WEAPON_NV_REMOVE)
    }

    /// How far past `min_speed` toward full speed the body runs, 0..1;
    /// `None` below it, reloading, or putting the night vision on or off.
    fn run_share(&self, min_speed: f32) -> Option<f32> {
        let busy = self.weaponstate == WEAPON_RELOADING || self.night_vision_moving();
        (!busy && self.xyspeed > min_speed)
            .then(|| ((self.xyspeed - min_speed) / (self.speed - min_speed)).clamp(0.0, 1.0))
    }
}

/// The bob's (vertical, horizontal) at `phase` and `speed`: the stance's
/// `bg_bobAmplitude*` times the speed, at most 10 each
/// (`BG_GetVerticalBobFactor`, `BG_GetHorizontalBobFactor`).
fn bob_wave(ps: &ViewPs, phase: f32, speed: f32) -> (f32, f32) {
    let amp = if ps.view_height_target == 11 {
        BOB_PRONE
    } else if ps.view_height_target == 40 {
        BOB_DUCKED
    } else if ps.pm_flags & PMF_SPRINT != 0 {
        BOB_SPRINTING
    } else {
        BOB_STANDING
    };
    let [h, v] = amp.map(|a| (speed * a).min(10.0));
    let lift = (phase * 4.0 + std::f32::consts::FRAC_PI_2).sin() * 0.2 + (phase * 2.0).sin();
    (lift * 0.75 * v, phase.sin() * h)
}

/// The bob as (pitch, yaw, roll), cod4-client-view.md 4.3
/// (`BG_CalculateWeaponPosition_BobOffset`, iw3mp.exe 0x41b0d0): the
/// roll trails the cycle and runs at 1.5 times the speed, and only its
/// dips show.
fn bob_turn(ps: &ViewPs, n: &ViewNumbers) -> [f32; 3] {
    use std::f32::consts::{FRAC_PI_4, PI, TAU};
    let phase = ps.bob_cycle as f32 / 255.0 * PI * 2.0 + TAU + FRAC_PI_4;
    let speed = ps.xyspeed * 0.16;
    let (lift, swing) = bob_wave(ps, phase, speed);
    let (_, roll) = bob_wave(ps, phase - 0.471_238_94, speed * 1.5);
    let frac = ps.pos_frac;
    let mut out = [-lift, -swing, roll.min(0.0)];
    if frac != 0.0 {
        let sight = 1.0 - (1.0 - n.ads_bob_factor) * frac;
        out = out.map(|x| x * sight);
    }
    if n.overlay_reticle {
        out = out.map(|x| x * (1.0 - frac));
    }
    out
}

impl Sway {
    /// `BG_CalculateWeaponPosition_Sway`: the view's turn since the last
    /// frame, per 60th of a second and capped, tracked into the lag and
    /// the tilt. Frozen under a scope's overlay and on a 0 ms frame.
    fn step(&mut self, ps: &ViewPs, n: &ViewNumbers, frame_ms: i32) {
        let frac = ps.pos_frac;
        let scoped = n.aim_down_sight && n.overlay_reticle && frac > 0.0;
        if frame_ms == 0 || scoped {
            return;
        }
        let [hip, ads] = n.sway;
        let [cap, rate, pitch_k, yaw_k, left_k, up_k] = if n.aim_down_sight {
            std::array::from_fn(|k| lerp(hip[k], ads[k], frac))
        } else {
            hip
        };
        let dt = frame_ms as f32 * 0.001;
        let view = ps.angles.to_array();
        let [pitch, yaw] =
            [0, 1].map(|k| (angle_diff(view[k], self.view[k]) / (dt * 60.0)).clamp(-cap, cap));
        self.lag = [
            track(yaw * left_k, self.lag[0], rate, dt),
            track(pitch * up_k, self.lag[1], rate, dt),
        ];
        self.tilt = [
            track_angle(pitch * pitch_k, self.tilt[0], rate, dt),
            track_angle(yaw * yaw_k, self.tilt[1], rate, dt),
        ];
        self.view = view;
    }
}

impl GunState {
    /// One frame of the gun's place off the eye: the offset (forward,
    /// left, up) and the angles (pitch, yaw, roll), cod4-client-view.md
    /// 4.3 (the weapon position step, `BG_CalculateWeaponAngles`).
    fn place(&mut self, ps: &ViewPs, n: &ViewNumbers, frame_ms: i32) -> (Vec3, [f32; 3]) {
        self.sway.step(ps, n, frame_ms);
        let slot = stance_slot(ps);
        let prone_now = usize::from(ps.view_height_current == 11.0);

        let mut offset_goal = ps
            .run_share(n.move_min_speed[slot])
            .map_or([0.0; 3], |s| n.move_ofs[slot].map(|v| v * s));
        let low = match ps.view_height_target {
            40 => Some(1),
            11 => Some(2),
            _ => None,
        };
        if let Some(low) = low.filter(|_| !ps.night_vision_moving()) {
            offset_goal = std::array::from_fn(|k| offset_goal[k] + n.stance_ofs[low][k]);
        }
        let frac = ps.pos_frac;
        let turn_goal = ps
            .run_share(n.rot_min_speed[slot])
            .map_or([0.0; 3], |s| n.move_rot[slot].map(|v| v * s * (1.0 - frac)));
        ease3(
            &mut self.move_offset,
            offset_goal,
            n.move_rate[prone_now],
            OFFSET_MIN_PER_MS,
            frame_ms,
        );
        ease3(
            &mut self.move_turn,
            turn_goal,
            n.rot_rate[prone_now],
            TURN_MIN_PER_MS,
            frame_ms,
        );
        let share = hip_share(frac);

        let offset = sum3(&[
            lean_offset(ps),
            self.move_offset.map(|v| v * share),
            [0.0, -self.sway.lag[0], self.sway.lag[1]],
            landing_dip(ps),
        ]);

        let ads = n.aim_down_sight;
        let lean_roll = match ps.leanf {
            0.0 => [0.0; 3],
            f => [0.0, 0.0, -2.0 * lean_curve(f)],
        };
        let aim_pitch = [if ads { frac * n.ads_aim_pitch } else { 0.0 }, 0.0, 0.0];
        let idle = self.idle_drift(ps, n, slot, frame_ms);
        let hit = ps
            .damage
            .map_or([0.0; 3], |(d, t)| d.gun(t, frac, n.overlay_reticle));
        let spring = match ads {
            true => [ps.gun_kick[0], ps.gun_kick[1], 0.0],
            false => [0.0; 3],
        };
        let mut angles = sum3(&[
            lean_roll,
            aim_pitch,
            self.move_turn.map(|v| v * share),
            idle,
            bob_turn(ps, n),
            hit,
            spring,
        ]);
        for (a, tilt) in angles.iter_mut().zip(self.sway.tilt) {
            *a = angle_diff(*a, tilt);
        }
        (Vec3::from(offset), angles)
    }

    /// `BG_CalculateWeaponPosition_IdleAngles`: the slow drift as (pitch,
    /// yaw, roll), its size the hip's or lerped to the sight's, its scale
    /// easing to the stance's factor at 0.5 a second (held under a
    /// scope's overlay), its clock running at the idle speed.
    fn idle_drift(&mut self, ps: &ViewPs, n: &ViewNumbers, slot: usize, frame_ms: i32) -> [f32; 3] {
        let frac = ps.pos_frac;
        let [ads_amount, hip_amount] = n.idle_amount;
        let [ads_speed, hip_speed] = n.idle_speed;
        let (mut size, speed) = match (n.aim_down_sight, hip_amount) {
            (true, _) => (
                lerp(hip_amount, ads_amount, frac),
                lerp(hip_speed, ads_speed, frac),
            ),
            (false, 0.0) => (80.0, 1.0),
            (false, _) => (hip_amount, hip_speed),
        };
        let goal = [1.0, n.idle_factor[0], n.idle_factor[1]][slot];
        if !n.overlay_reticle || frac == 0.0 {
            self.idle_scale = approach(self.idle_scale, goal, frame_ms as f32 * 0.0005);
        }
        size *= self.idle_scale;
        if n.overlay_reticle {
            size *= 1.0 - frac;
        }
        self.idle_clock += (frame_ms as f32 * speed) as i32;
        let clock = self.idle_clock as f32;
        [0.001, 0.0007, 0.0005].map(|w| (clock * w).sin() * size * 0.01)
    }
}

/// Each of `cur` eased toward `goal`: `rate` of the gap a second, at
/// least `min_per_ms` a ms, never past it.
fn ease3(cur: &mut [f32; 3], goal: [f32; 3], rate: f32, min_per_ms: f32, frame_ms: i32) {
    let ms = frame_ms as f32;
    for (c, g) in cur.iter_mut().zip(goal) {
        let by_gap = rate * (g - *c) * ms * 0.001;
        *c = approach(*c, g, by_gap.abs().max(ms * min_per_ms));
    }
}

/// The lean's shift: along the rolled view's right, gone as the sight
/// comes up.
fn lean_offset(ps: &ViewPs) -> [f32; 3] {
    if ps.leanf == 0.0 || ps.pos_frac >= 1.0 {
        return [0.0; 3];
    }
    let lean = lean_curve(ps.leanf);
    let (s, c) = (-(lean + lean)).to_radians().sin_cos();
    let dist = (1.0 - ps.pos_frac) * lean * 1.6;
    [0.0 * dist, -c * dist, -s * dist]
}

/// The landing's dip on the world's up (iw3mp.exe 0x455500, cod4-client-view.md
/// 14), turned into the view's frame.
fn landing_dip(ps: &ViewPs) -> [f32; 3] {
    let dip = crate::play::cod4_kick::gun_dip(ps.land);
    let (s, c) = ps.angles.x.to_radians().sin_cos();
    [-s * dip, 0.0 * dip, c * dip]
}

/// `CG_GetViewFov`: `cg_fov`, the
/// sight's zoom eased in over the last `adsZoomInFrac` of the raise
/// (out over `adsZoomOutFrac` of the drop), 90 at the intermission, 55
/// on a turret, times `cg_fovScale`, at least `cg_fovMin`.
pub fn view_fov(ps: &ViewPs, n: Option<&ViewNumbers>, to_ads: bool) -> f32 {
    let mut fov = CG_FOV;
    if ps.pm_type == PM_INTERMISSION {
        fov = 90.0;
    } else if let Some(n) = n.filter(|n| n.aim_down_sight) {
        let f = ps.pos_frac;
        if f == 1.0 {
            fov = n.ads_zoom_fov;
        } else if f != 0.0 {
            let span = if to_ads {
                n.ads_zoom_in_frac
            } else {
                n.ads_zoom_out_frac
            };
            let mut z = f - (1.0 - span);
            if z > 0.0 {
                z /= span;
                fov -= (fov - n.ads_zoom_fov) * z;
            }
        }
    }
    if ps.eflags & EF_TURRET != 0 {
        fov = 55.0;
    }
    (fov * CG_FOV_SCALE).max(CG_FOV_MIN)
}

/// A rig's key: the weapon, the hands and whether the knife is out.
type RigKey = (String, String, bool);

/// One frame's view weapon: the frame for the renderer, the fov, and the
/// gun's `tag_flash` and `tag_brass` in view space (X right, Y up, -Z
/// forward).
pub struct Cod4ViewFrame {
    pub frame: Option<ViewFrame>,
    pub fov: f32,
    pub tags: WeaponTags,
    /// The aliases the main clip's notetracks mapped this frame, played
    /// as client sounds.
    pub sounds: Vec<String>,
    /// [`ViewPs::gun_dir`] for the crosshair.
    pub gun_dir: Option<[f32; 3]>,
}

/// What a session's view weapon keeps across frames.
#[derive(Default)]
pub struct Cod4View {
    rigs: HashMap<RigKey, Option<Arc<Rig>>>,
    current: Option<(RigKey, skeleton::PoseBuffer, ViewAnim, AdsBlend)>,
    gun: GunState,
    last_ms: Option<i32>,
    /// The weapon list the clip slots were built for, and the slots.
    clip_slots: (Vec<String>, Vec<usize>),
    /// A new rig's models that came while the scope hid the gun, handed
    /// over with the next drawn frame.
    pending_models: Option<ViewModels>,
}

impl Cod4View {
    /// The drawn main node and its clock, for the replay gate.
    #[cfg(test)]
    pub fn anim(&self) -> Option<(&str, ViewAnim)> {
        self.current.as_ref().map(|(k, _, a, _)| (k.0.as_str(), *a))
    }

    /// Drops the drawn rig (a free spectator, a new map).
    pub fn clear(&mut self) {
        self.current = None;
    }

    fn rig(
        &mut self,
        key: &RigKey,
        def: &Arc<ViewDef>,
        models: &HashMap<String, Model>,
    ) -> Option<Arc<Rig>> {
        self.rigs
            .entry(key.clone())
            .or_insert_with(|| {
                let knife = match def.knife.as_ref().filter(|_| key.2) {
                    Some(k) => match models.get(k) {
                        Some(m) => Some(m),
                        None => {
                            log::warn!("cod4 viewmodel {}: knife {k} not loaded", key.0);
                            return None;
                        }
                    },
                    None => None,
                };
                let (Some(h), Some(g)) = (models.get(&key.1), models.get(&def.gun)) else {
                    log::warn!(
                        "cod4 viewmodel {}: {} or {} not loaded",
                        key.0,
                        key.1,
                        def.gun
                    );
                    return None;
                };
                if def.anims.get(IDLE).is_none_or(Option::is_none) {
                    log::warn!("cod4 viewmodel {}: no idle anim", key.0);
                    return None;
                }
                Some(Arc::new(Rig::build(def.clone(), h, g, knife)))
            })
            .clone()
    }

    /// Steps and poses the view weapon `ps` holds at render time
    /// `time_ms`; `list` is the gamestate's weapon list, `hands_cs` names
    /// `CS_MODELS + viewmodelIndex`. The frame is `None` when no gun
    /// draws (a spectator, a turret, no weapon or no rig).
    pub fn frame(
        &mut self,
        ps: &ViewPs,
        list: &[String],
        hands_cs: Option<&str>,
        weapons: &HashMap<String, WeaponModels>,
        models: &HashMap<String, Model>,
        time_ms: i32,
    ) -> Cod4ViewFrame {
        let frame_ms = self
            .last_ms
            .map_or(0, |l| time_ms.wrapping_sub(l).clamp(0, 1000));
        self.last_ms = Some(time_ms);
        let name = crate::play::cod4::weapon_name(list, ps.view_weapon());
        let def = name.and_then(|w| weapons.get(w)?.view.clone());
        let mut out = Cod4ViewFrame {
            frame: None,
            fov: view_fov(ps, def.as_ref().map(|d| &d.numbers), self.gun.to_ads),
            tags: WeaponTags::default(),
            sounds: Vec::new(),
            gun_dir: None,
        };
        if !ps.draws_gun() || ps.pm_type >= PM_DEAD {
            self.current = None;
            return out;
        }
        let (Some(name), Some(def)) = (name, def) else {
            self.current = None;
            return out;
        };
        // `CG_UpdateHandViewmodels`: the playerstate's hands over the file's.
        let hands = match hands_cs.filter(|h| models.contains_key(*h)) {
            Some(h) => h.to_string(),
            None => match &def.hands {
                Some(h) => h.clone(),
                None => return out,
            },
        };
        // The knife test: the melee's two anims.
        let knife = def.knife.is_some() && matches!(ps.weap_anim & !ANIM_TOGGLEBIT, 8 | 9);
        let key = (name.to_string(), hands, knife);
        let Some(rig) = self.rig(&key, &def, models) else {
            self.current = None;
            return out;
        };
        let mut new_models = None;
        if self.current.as_ref().is_none_or(|(k, ..)| *k != key) {
            let mut p = skeleton::PoseBuffer::new(&rig.skeleton);
            let (c, b) = rig.clip(IDLE).expect("a rig holds idle");
            p.apply(c, b, 0.0);
            // The knife coming or going keeps the running clip and sight.
            let (anim, ads) = match self.current.take() {
                Some((k, _, anim, ads)) if (&k.0, &k.1) == (&key.0, &key.1) => (anim, ads),
                _ => (ViewAnim::default(), AdsBlend::default()),
            };
            self.current = Some((key, p, anim, ads));
            new_models = Some(rig.models.clone());
        }
        if self.clip_slots.0 != list {
            let defs = list
                .iter()
                .map(|w| {
                    let clip = weapons
                        .get(w)
                        .and_then(|m| m.view.as_ref())
                        .map_or(String::new(), |d| d.clip_name.clone());
                    let hud = vcod_cod4::hud::WeaponHud {
                        clip_name: clip,
                        ..Default::default()
                    };
                    (w.clone(), hud)
                })
                .collect();
            let slots = vcod_cod4::hud::ammo_slots(list, &defs);
            self.clip_slots = (list.to_vec(), slots.iter().map(|s| s.1).collect());
        }
        let clip_empty = (ps.view_weapon() as usize)
            .checked_sub(1)
            .and_then(|i| self.clip_slots.1.get(i))
            .is_some_and(|&s| ps.ammoclip.get(s) == Some(&0));
        let n = &rig.def.numbers;
        let dt = frame_ms as f32 / 1000.0;
        let (_, pose, anim, ads) = self.current.as_mut().expect("set above");
        let from = anim.step(&rig, ps.weap_anim, clip_empty, dt);
        if let Some((c, _)) = rig.clip(anim.node) {
            for note in notes_crossed(c, from, anim.t) {
                out.sounds.extend(
                    (rig.def.note_sounds.iter())
                        .filter(|(k, _)| k.eq_ignore_ascii_case(note))
                        .map(|(_, v)| v.clone()),
                );
            }
        }
        // `WeaponRunXModelAnims`' sight: up while `pm_flags` 8 and the off
        // hand is not out, unless a reload's time is still past
        // `adsReloadTransTime`.
        let frac = ps.pos_frac.clamp(0.0, 1.0);
        let layer = if n.aim_down_sight {
            let reloading = ps.weaponstate == WEAPON_RELOADING
                && ps.weapon_time - n.position_reload_trans_time > 0;
            let up = !reloading && ps.pm_flags & PMF_SIGHT != 0 && ps.weap_flags & WF_OFFHAND == 0;
            ads.step(up, dt);
            Some((frac, ads.0))
        } else {
            rig.clip(ADS_DOWN).map(|_| (0.0, [0.0, 1.0]))
        };
        if let Some((frac, w)) = layer {
            let layers: Vec<skeleton::ClipAt> = [(ADS_UP, frac), (ADS_DOWN, 1.0 - frac)]
                .into_iter()
                .zip(w)
                .filter(|&(_, w)| w > 0.0)
                .filter_map(|((node, t), w)| {
                    let (c, b) = rig.clip(node)?;
                    Some((&**c, b, c.frame_pos(t * c.duration(), false), w))
                })
                .collect();
            match layers.as_slice() {
                [] => {}
                [(c, b, t, _)] => pose.apply(c, b, *t),
                _ => pose.blend(&layers),
            }
        }
        let (c, b) = rig
            .clip(anim.node)
            .or_else(|| rig.clip(IDLE))
            .expect("a rig holds idle");
        pose.apply(c, b, c.frame_pos(anim.t, c.looping));
        // `CG_AddViewWeapon` (`iw3mp.exe` 0x457010): no gun while the
        // sight's overlay is in (`CG_GetWeapReticleZoom`, 0x430120, called
        // at 0x45707b before the lerp's direction updates); the pose, the
        // tags and the notetracks still step.
        let hidden = reticle_zoom(ps.pos_frac, n, self.gun.to_ads).is_some();
        // `CalculateWeaponPostion_PositionToADS`.
        if frac == 1.0 {
            self.gun.to_ads = false;
        } else if frac == 0.0 {
            self.gun.to_ads = true;
        }
        let (offset, angles) = self.gun.place(ps, n, frame_ms);
        // `CalculateWeaponPosition_SaveOffsetAngles`: the gun
        // axis's angles become `cg.gunPitch` and `gunYaw` only while a
        // non-overlay sight is up or coming up.
        if n.aim_down_sight && ps.pos_frac != 0.0 && !n.overlay_reticle {
            out.gun_dir = Some(vcod_common::pmove::aim::angles_to_axis_msvc(angles)[0]);
        }
        let transform =
            crate::play::view::gun_transform(crate::play::view::view_from_ful(offset), angles);
        let tag = |name: &str| {
            let b = rig.skeleton.bone_index(name)?;
            let (p, q) = pose.bone_world(&rig.skeleton, b);
            let (_, turn, _) = transform.to_scale_rotation_translation();
            Some((transform.transform_point3(p), turn * q))
        };
        out.tags = WeaponTags {
            flash: tag("tag_flash"),
            brass: tag("tag_brass"),
        };
        if hidden {
            if new_models.is_some() {
                self.pending_models = new_models;
            }
            return out;
        }
        let pending = self.pending_models.take();
        let bone_sets = (0..rig.models.len())
            .map(|m| pose.skin_matrices(&rig.skeleton, m))
            .collect();
        out.frame = Some(ViewFrame {
            models: new_models.or(pending),
            draw: VmDraw {
                transform,
                light_origin: Vec3::ZERO,
                fov_x: 0.0,
                bone_sets,
            },
        });
        out
    }
}

/// `CG_GetWeapReticleZoom` (`iw3mp.exe` 0x430120): how far a weapon with
/// a sight overlay has zoomed in, off the sight's lerp past
/// `1 - adsZoomInFrac` (`adsZoomOutFrac` going down); `None` at 0.01
/// or less, or without an overlay.
pub fn reticle_zoom(pos_frac: f32, n: &ViewNumbers, to_ads: bool) -> Option<f32> {
    if (!n.overlay_material && !n.overlay_reticle) || pos_frac == 0.0 {
        return None;
    }
    let frac = if to_ads {
        n.ads_zoom_in_frac
    } else {
        n.ads_zoom_out_frac
    };
    let mut zoom = pos_frac - (1.0 - frac);
    if zoom > 0.0 {
        zoom /= frac;
    }
    (zoom > 0.01).then_some(zoom)
}

/// A view-space tag (X right, Y up, -Z forward) in the world, for an eye
/// at `eye` looking along `basis` (`camera::basis`: forward, right, up).
pub fn tag_to_world(
    (pos, q): (Vec3, Quat),
    eye: Vec3,
    (forward, right, up): (Vec3, Vec3, Vec3),
) -> (Vec3, Quat) {
    let world = glam::Mat3::from_cols(right, up, -forward);
    (eye + world * pos, Quat::from_mat3(&world) * q)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sight's fov eases in over the raise's last `adsZoomInFrac`,
    /// out over the drop's last `adsZoomOutFrac`, and floors at
    /// `cg_fovMin`.
    #[test]
    fn the_sight_zooms_over_its_last_fraction() {
        let n = ViewNumbers {
            aim_down_sight: true,
            ads_zoom_fov: 50.0,
            ads_zoom_in_frac: 0.5,
            ads_zoom_out_frac: 0.25,
            ..Default::default()
        };
        let ps = |f: f32| ViewPs {
            pm_type: 0,
            pos_frac: f,
            ..Default::default()
        };
        assert_eq!(view_fov(&ps(0.0), Some(&n), true), CG_FOV);
        assert_eq!(view_fov(&ps(0.4), Some(&n), true), CG_FOV);
        assert!((view_fov(&ps(0.75), Some(&n), true) - 57.5).abs() < 1e-4);
        assert_eq!(view_fov(&ps(0.75), Some(&n), false), CG_FOV);
        assert_eq!(view_fov(&ps(1.0), Some(&n), false), 50.0);
        let turret = ViewPs {
            eflags: EF_TURRET,
            ..ps(0.0)
        };
        assert_eq!(view_fov(&turret, Some(&n), true), 55.0);
    }

    /// The table off `WeaponRunXModelAnims`: a shot's fire, the last
    /// round's, the ADS fire, a reload, the raise; the toggle bit
    /// ignored, 0 and 1 waiting on the running clip.
    #[test]
    fn weap_anim_names_its_node() {
        assert_eq!(anim_node(0), None);
        assert_eq!(anim_node(1 | ANIM_TOGGLEBIT), None);
        assert_eq!(anim_node(2), Some(3));
        assert_eq!(anim_node(2 | ANIM_TOGGLEBIT), Some(3));
        assert_eq!(anim_node(3), Some(5));
        assert_eq!(anim_node(5), Some(28));
        assert_eq!(anim_node(13), Some(9));
        assert_eq!(anim_node(11), Some(13));
        assert_eq!(anim_node(99), Some(IDLE));
    }

    /// The plain MP5's gun hides the ACOG and reflex sight its model
    /// carries (its `hideTags`), and a melee's two anims graft the knife
    /// at `tag_knife_attach` and play the knife clips, the gun coming back
    /// once `weapAnim` does. Vacuous without `COD4_DIR`.
    #[test]
    fn notetracks_fire_once_as_the_clip_crosses_them() {
        let clip = |looping| XAnim {
            name: "c".into(),
            frame_count: 31,
            framerate: 30.0,
            looping,
            tracks: Vec::new(),
            root: None,
            notes: vec![("a".into(), 0), ("b".into(), 15), ("end".into(), 30)],
        };
        let once = clip(false);
        assert_eq!(notes_crossed(&once, 0.0, 0.1), ["a"]);
        assert!(notes_crossed(&once, 0.1, 0.5).is_empty());
        assert_eq!(notes_crossed(&once, 0.5, 2.0), ["b"], "the end is left out");
        let looped = clip(true);
        assert_eq!(
            notes_crossed(&looped, 0.9, 1.6),
            ["a", "b"],
            "the next cycle's"
        );
    }

    #[test]
    fn an_m16_reload_sounds_its_clip_out_and_in_off_its_notetracks() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let list: Vec<String> = vec!["m16_mp".into()];
        let ents = crate::cod4_fly::EntityAssets {
            models: Vec::new(),
            weapons: list.clone(),
        };
        let (models, weapons, _) =
            crate::cod4_fly::load_bodies(install.root(), "mp_crash", &ents).unwrap();
        let mut view = Cod4View::default();
        let mut ps = ViewPs {
            pm_type: 0,
            weapon: 1,
            ammoclip: [30; 128],
            ..Default::default()
        };
        view.frame(&ps, &list, None, &weapons, &models, 0);
        // `weapAnim` 13, the reload (node 9, 2 s of `reloadTime`).
        ps.weap_anim = 13 | ANIM_TOGGLEBIT;
        let mut heard = Vec::new();
        for ms in (16..2200).step_by(16) {
            for a in view.frame(&ps, &list, None, &weapons, &models, ms).sounds {
                heard.push((a, ms - 16));
            }
        }
        let at = |a: &str| heard.iter().find(|h| h.0 == a).map(|h| h.1);
        // Frames 12 and 37 of the 60-frame clip at its 2 s rate.
        assert!(
            at("weap_m16_clipout_plr").is_some_and(|t| (380..=420).contains(&t)),
            "{heard:?}"
        );
        assert!(
            at("weap_m16_clipin_plr").is_some_and(|t| (1230..=1270).contains(&t)),
            "{heard:?}"
        );
        assert_eq!(heard.len(), 2, "{heard:?}");
    }

    /// The sniper rifles' guns hide once their overlay zooms in past
    /// 0.01 (`CG_AddViewWeapon`, cod4-client-view.md 4.6), up the sight
    /// and down it, their models handed over with the next drawn frame;
    /// the M16's sight never hides it. Vacuous without `COD4_DIR`.
    #[test]
    fn a_scoped_rifle_hides_its_gun_under_the_overlay() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let snipers = ["m40a3_mp", "remington700_mp", "dragunov_mp", "barrett_mp"];
        let list: Vec<String> = snipers
            .iter()
            .chain(&["m16_mp"])
            .map(|s| s.to_string())
            .collect();
        let ents = crate::cod4_fly::EntityAssets {
            models: Vec::new(),
            weapons: list.clone(),
        };
        let (models, weapons, _) =
            crate::cod4_fly::load_bodies(install.root(), "mp_crash", &ents).unwrap();
        for (k, w) in list.iter().enumerate() {
            let n = weapons[w].view.as_ref().unwrap().numbers.clone();
            let scoped = k < snipers.len();
            assert_eq!(n.overlay_reticle && n.overlay_material, scoped, "{w}");
            let mut view = Cod4View::default();
            let mut ps = ViewPs {
                pm_type: 0,
                weapon: k as u32 + 1,
                ammoclip: [30; 128],
                ..Default::default()
            };
            // up the sight from the hip in 400 steps, then down it
            const N: i32 = 400;
            let at = |step: i32| match step <= N {
                true => (step as f32 / N as f32, true),
                false => ((2 * N - step) as f32 / N as f32, false),
            };
            let mut drawn = Vec::new();
            let mut models_at = Vec::new();
            for step in 0..=2 * N {
                let (frac, up) = at(step);
                ps.pos_frac = frac;
                ps.pm_flags = if up { PMF_SIGHT } else { 0 };
                let f = view.frame(&ps, &list, None, &weapons, &models, step * 16);
                if let Some(f) = &f.frame {
                    drawn.push(step);
                    if f.models.is_some() {
                        models_at.push(step);
                    }
                }
            }
            if !scoped {
                assert_eq!(drawn.len(), 2 * N as usize + 1, "{w}");
                continue;
            }
            assert_eq!(models_at, [0], "{w}");
            assert!(drawn.len() < 2 * N as usize - 20, "{w}: {}", drawn.len());
            for step in 0..=2 * N {
                let (frac, up) = at(step);
                let f = if up {
                    n.ads_zoom_in_frac
                } else {
                    n.ads_zoom_out_frac
                };
                // the zoom passes 0.01 at a lerp of `1 - f + 0.01 f`
                let edge = 1.0 - f + 0.01 * f;
                if (frac - edge).abs() < 1e-4 {
                    continue;
                }
                assert_eq!(
                    drawn.contains(&step),
                    frac < edge,
                    "{w}: step {step}, {n:?}"
                );
            }
        }
    }

    #[test]
    fn hidden_tags_and_the_melee_knife() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let list: Vec<String> = ["mp5_mp", "mp5_acog_mp"].map(String::from).to_vec();
        let ents = crate::cod4_fly::EntityAssets {
            models: Vec::new(),
            weapons: list.clone(),
        };
        let (models, weapons, _) =
            crate::cod4_fly::load_bodies(install.root(), "mp_crash", &ents).unwrap();
        let def = |w: &str| weapons[w].view.clone().unwrap();
        let mats = |w: &str| models[&def(w).gun].lods[0].materials.join(" ");
        assert!(def("mp5_mp").gun.starts_with("viewmodel_mp5_mp#"));
        assert!(!mats("mp5_mp").contains("acog") && !mats("mp5_mp").contains("red_dot"));
        assert!(mats("mp5_acog_mp").contains("acog_lens"));
        let mut view = Cod4View::default();
        let mut ps = ViewPs {
            pm_type: 0,
            weapon: 1,
            ammoclip: [30; 128],
            ..Default::default()
        };
        let drawn = |view: &mut Cod4View, ps: &ViewPs, ms: i32| {
            let f = view.frame(ps, &list, None, &weapons, &models, ms);
            f.frame.unwrap().models.map(|m| m.len())
        };
        assert_eq!(drawn(&mut view, &ps, 0), Some(2));
        ps.weap_anim = 8 | ANIM_TOGGLEBIT;
        assert_eq!(drawn(&mut view, &ps, 50), Some(3), "the knife comes out");
        assert_eq!(view.anim().unwrap().1.node, 7);
        assert_eq!(drawn(&mut view, &ps, 100), None, "the same rig");
        ps.weap_anim = 0;
        assert_eq!(drawn(&mut view, &ps, 150), Some(2), "the knife goes");
        // The swing's clip keeps running until it ends.
        assert_eq!(view.anim().unwrap().1.node, 7);
        assert_eq!(drawn(&mut view, &ps, 1000), None);
        assert_eq!(drawn(&mut view, &ps, 1016), None);
        assert_eq!(view.anim().unwrap().1.node, IDLE);
    }

    /// A tag straight ahead of the eye in view space lands ahead along
    /// the view in the world.
    #[test]
    fn view_tags_land_along_the_view() {
        let basis = crate::camera::basis(0.7, -0.3);
        let eye = Vec3::new(100.0, -40.0, 60.0);
        let (p, q) = tag_to_world((Vec3::new(0.0, 0.0, -20.0), Quat::IDENTITY), eye, basis);
        assert!((p - (eye + basis.0 * 20.0)).length() < 1e-4, "{p}");
        // The view's -Z is the world's forward.
        assert!((q * Vec3::NEG_Z).dot(basis.0) > 0.9999);
    }

    /// The gun's offset and angles over a scripted run: walking, turning,
    /// the sight up and down, a sprint, the crouch and prone offsets, a
    /// lean, a landing, a hit, the gun spring, a reload and the night
    /// vision, on a hip rifle, a sighted rifle and a scoped one. Pinned
    /// bit for bit off the build that first drew the view weapon.
    #[test]
    fn the_gun_place_holds_over_a_scripted_run() {
        let base = ViewNumbers {
            ads_bob_factor: 0.3,
            ads_aim_pitch: 1.5,
            move_ofs: [[-1.0, 0.5, -0.8], [-0.6, 0.4, -0.5], [-2.0, 0.0, 1.0]],
            move_rot: [[2.0, -1.5, 0.75], [1.0, -1.0, 0.5], [4.0, 3.0, -2.0]],
            stance_ofs: [[0.0; 3], [-0.5, 0.3, -1.2], [-1.5, 0.6, -2.5]],
            move_min_speed: [20.0, 10.0, 0.0],
            rot_min_speed: [30.0, 15.0, 5.0],
            move_rate: [6.0, 3.5],
            rot_rate: [8.0, 2.5],
            idle_amount: [10.0, 60.0],
            idle_speed: [0.5, 1.2],
            idle_factor: [0.6, 0.3],
            sway: [
                [8.0, 6.0, 0.4, 0.6, 0.2, 0.3],
                [4.0, 9.0, 0.1, 0.15, 0.05, 0.08],
            ],
            ..Default::default()
        };
        let guns = [
            ViewNumbers {
                idle_amount: [0.0, 0.0],
                ..base.clone()
            },
            ViewNumbers {
                aim_down_sight: true,
                ..base.clone()
            },
            ViewNumbers {
                aim_down_sight: true,
                overlay_reticle: true,
                ..base
            },
        ];
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        let mut eat = |x: f32| {
            for b in x.to_bits().to_le_bytes() {
                hash = (hash ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
            }
        };
        let mut dump = String::new();
        for n in &guns {
            let mut g = GunState::default();
            for i in 0..400i32 {
                let t = i as f32;
                let ms = [16, 17, 17, 0, 33, 50][(i % 6) as usize];
                let frac = match i {
                    60..80 => (i - 60) as f32 / 20.0,
                    80..120 => 1.0,
                    120..130 => 1.0 - (i - 120) as f32 / 10.0,
                    300..310 => 0.37,
                    _ => 0.0,
                };
                let mut ps = ViewPs {
                    pm_type: 0,
                    pos_frac: frac,
                    bob_cycle: (i as u32 * 7) & 0xff,
                    xyspeed: (t * 0.9).min(190.0) * if (200..220).contains(&i) { 0.0 } else { 1.0 },
                    speed: 190.0,
                    angles: Vec3::new((t * 0.05).sin() * 30.0, t * 1.7 - 180.0, 0.0),
                    ..Default::default()
                };
                if (140..170).contains(&i) {
                    ps.pm_flags |= PMF_SPRINT;
                }
                match i {
                    170..200 => {
                        (ps.eflags, ps.view_height_target, ps.view_height_current) =
                            (EF_CROUCH, 40, 40.0)
                    }
                    200..240 => {
                        (ps.eflags, ps.view_height_target, ps.view_height_current) =
                            (EF_PRONE, 11, 11.0 + (240 - i).min(20) as f32)
                    }
                    _ => {}
                }
                if (240..280).contains(&i) {
                    ps.leanf = ((i - 240) as f32 / 40.0 * 6.0).sin() * 0.9;
                }
                if (100..115).contains(&i) {
                    ps.leanf = -0.5;
                }
                if (280..300).contains(&i) {
                    ps.land = (-12.0, (i - 280) * 25);
                }
                if (310..330).contains(&i) {
                    let d = crate::play::cod4_kick::DamageKick {
                        pitch: 4.0,
                        roll: -3.0,
                        time: 1,
                    };
                    ps.damage = Some((d, (i - 310) * 30));
                }
                if (85..100).contains(&i) {
                    ps.gun_kick = [0.8 - (i - 85) as f32 * 0.05, 0.2];
                }
                if (330..350).contains(&i) {
                    ps.weaponstate = WEAPON_RELOADING;
                }
                if (350..370).contains(&i) {
                    ps.weaponstate = WEAPON_NV_WEAR + (i as u32 & 1);
                }
                let (o, a) = g.place(&ps, n, ms);
                dump += &format!("{i} {o:?} {a:?}\n");
                o.to_array().into_iter().chain(a).for_each(&mut eat);
            }
        }
        if let Some(path) = std::env::var_os("GUN_PIN_DUMP") {
            std::fs::write(path, dump).unwrap();
        }
        assert_eq!(hash, 0x8401_29b8_e657_61f7, "{hash:#x}");
    }
}
