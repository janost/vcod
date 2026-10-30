//! CoD2 client-side prediction: `CG_PredictPlayerState` (Mac 0x1ddafa).
//! Every frame the newest snapshot's playerstate is rebuilt for
//! `vcod_cod2::pmove` ([`PlayerState::from_wire`]) and every sent cmd past
//! its `commandTime` is stepped on top through `player_step`, the server's
//! own move, over the map's collision world. What a new snapshot corrects
//! eases out over `cg_errordecay` (100 ms, Mac 0x1ddafa reads it twice,
//! `CG_CalcViewValues` Mac 0x1d3b28 adds `f * predictedError` to the eye):
//! a correction no longer than 0.1 (Mac 0x326388, VERIFIED, the float) is
//! dropped, a longer one decays what is left of the last and adds itself.
//!
//! The arithmetic is the Linux server's stored floats (`Arith::Stored`
//! and `vcod_cod2::pmove`'s `fpu`), not the Windows client's MSVC keep
//! (cod2-movement.md 2): vcod predicts what the server will compute, so a
//! replay against either server can be exact. INFERRED: retail's own
//! client, keeping its intermediates, corrects in the last bits.
//!
//! `oldVelocity` is not on the wire. Retail's predictor starts every replay
//! from the snapshot's copy, which the parser never writes (INFERRED, the
//! open question of cod2-movement.md 14); vcod carries the value its own
//! last replay reached at the snapshot's `commandTime`, as the A/B gate's
//! resync does, which is what the server holds.
//!
//! The mantle is predicted when [`Rules::mantle_anims`] holds the anims.
//! With the level's weapon table the weapon half runs beside the move
//! (`vcod_cod2::pmove::weapon::armed_step`, as `PmoveSingle` runs
//! `PM_Weapon` on the client too): `fWeaponPosFrac`, `weaponstate`,
//! `weapAnim` and the timers advance every frame instead of every
//! snapshot, so the sight's rise, its zoom and the view weapon's clips
//! run at the frame rate. Other players and solid entities clip the
//! replay as `CG_ClipMoveToEntities` clips them ([`solid_bodies`]); brush
//! models and item pickups are not predicted.
//! Events: the move's own (footsteps, jumps, landings, stance refusals,
//! the step view) and the weapon's fire off the replay once each, keyed
//! on the cmd that raised them; the snapshot's copies of them are skipped
//! ([`predicted_event`]) and everything else plays off the snapshot.

use std::collections::VecDeque;

use glam::Vec3;
use vcod_cod2::net::events::{ET_ITEM, ET_PLAYER};
use vcod_cod2::net::flags::EF_TELEPORT_BIT;
use vcod_cod2::net::view::Frame;
use vcod_cod2::net::{PlayerState2, UserCmd2};
use vcod_cod2::pmove::weapon::{self, WeaponState, Weapons};
use vcod_cod2::pmove::{self, PlayerState, Rules, ev, pm_type};
use vcod_common::collision::CollisionWorld;
use vcod_common::movetrace::{Body, CONTENTS_BODY, MoveWorld};

/// `cg_errordecay`'s default (`Dvar_RegisterFloat("cg_errordecay", 100.0`).
const ERROR_DECAY_MS: f64 = 100.0;
/// `CG_PredictPlayerState`'s miss threshold (Mac 0x326388).
const MISS_EPSILON: f32 = 0.1;
/// `pm_flags` that make the cgame interpolate instead
/// (`CG_PredictPlayerState`'s `snap->ps.pm_flags` byte 2 & 0x40): a
/// spectator following someone.
const PMF_FOLLOW: u32 = 0x40_0000;
/// How many sent cmds are kept: retail's `CMD_BACKUP` 128
/// (`CL_GetCurrentCmdNumber() - 0x7f`).
pub const CMD_BACKUP: usize = 128;

/// Whether `event` is one vcod's pmove raises, so the replay plays it
/// and the snapshot's copy is skipped: the move's, and `PM_Weapon`'s
/// (the shots, reloads, switches, offhands and binoculars) now that the
/// weapon is predicted, so a shot's sound, flash and kick play with its
/// predicted anim. Left to the snapshot: `EV_RESET_ADS` (149, which
/// retail's stance code also raises and vcod's does not, cod2-movement.md
/// 17.3) and the grenade suicide (197, the server's kill).
pub fn predicted_event(event: i32) -> bool {
    use vcod_cod2::pmove::weapon::ev as wev;
    (ev::FOOTSTEP_RUN..=ev::STEP_VIEW).contains(&event)
        || matches!(event, wev::NOAMMO | wev::EMPTY_OFFHAND)
        || (wev::RELOAD..=wev::FIRE_WEAPON).contains(&event)
        || (wev::FIRE_WEAPON_LASTSHOT..=wev::BINOCULAR_DROP).contains(&event)
}

/// What a frame's prediction gives the view.
#[derive(Clone, Debug)]
pub struct Predicted2 {
    /// The replayed state at the newest sent cmd.
    pub ps: PlayerState,
    /// Its weapon half: the snapshot's, stepped with `ps` when the
    /// weapon table was handed in.
    pub ws: WeaponState,
    /// What the camera draws from: the state stepped on to the frame's
    /// own time (the cmd being built) when there is one, else `ps`.
    pub drawn: PlayerState,
    pub drawn_ws: WeaponState,
    /// `drawn.origin` less what is left of the correction.
    pub origin: Vec3,
    /// The move's events not fired before: (event, parm).
    pub events: Vec<(i32, i32)>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PredictStats {
    pub frames: u64,
    /// Frames drawn unpredicted: the cmd history no longer reached the
    /// snapshot's `commandTime`.
    pub misses: u64,
    /// Snapshots whose replay moved last frame's prediction at all, and
    /// past [`MISS_EPSILON`].
    pub corrected: u64,
    pub missed: u64,
    /// The longest correction since [`Cod2Predictor::take_max`].
    pub max_correction: f32,
    pub cmds_run: u64,
}

#[derive(Default)]
pub struct Cod2Predictor {
    pub rules: Rules,
    pub stats: PredictStats,
    /// Last frame's newest `commandTime` and origin, uncorrected.
    last: Option<(i32, Vec3)>,
    /// The snapshot last predicted from: its `commandTime` and teleport bit.
    from: Option<(i32, bool)>,
    error: Vec3,
    /// Local ms the error eases from (`cg.predictedErrorTime`).
    error_ms: f64,
    /// The last frame's local ms (`cg.oldTime`): a miss eases from there,
    /// so the frame that finds it already draws part of it.
    last_ms: Option<f64>,
    /// `oldVelocity` after each replayed cmd, oldest first.
    old_velocity: VecDeque<(i32, [f32; 2])>,
    /// The move events fired, by the cmd time that raised them.
    fired: VecDeque<(i32, i32, i32)>,
}

impl Cod2Predictor {
    /// Predicts off `snap` (the newest snapshot's playerstate) with `cmds`,
    /// every cmd sent and kept, oldest first, and `pending`, the cmd being
    /// built this frame (drawn, never kept). `defs` is the level's weapon
    /// table (`CS_WEAPONS`), which runs the weapon half beside the move;
    /// without it only the move runs, at `move_scale`, the held weapon's
    /// `moveSpeedScale` (0 for none). `None` for a state the cgame
    /// interpolates (followed, intermission) or a miss. `bodies` are the
    /// frame's [`solid_bodies`], clipped beside `world`.
    #[allow(clippy::too_many_arguments)]
    pub fn predict(
        &mut self,
        snap: &PlayerState2,
        cmds: &VecDeque<UserCmd2>,
        pending: Option<&UserCmd2>,
        world: &CollisionWorld,
        bodies: &[Body],
        defs: Option<&dyn Weapons>,
        move_scale: f32,
        now_ms: f64,
    ) -> Option<Predicted2> {
        let old_ms = self.last_ms.replace(now_ms);
        let mut ps = PlayerState::from_wire(snap);
        let mut ws = WeaponState::from_wire(snap);
        let pm_flags = ps.pm_flags;
        if pm_flags & PMF_FOLLOW != 0 || ps.pm_type == pm_type::INTERMISSION {
            self.reset();
            return None;
        }
        self.stats.frames += 1;
        let teleport = ps.e_flags & EF_TELEPORT_BIT as u32 != 0;
        if self.from.is_some_and(|(_, t)| t != teleport) {
            self.reset();
        }
        let ct = ps.command_time;
        self.from = Some((ct, teleport));
        let Some(oldest) = cmds.front() else {
            self.stats.misses += 1;
            return None;
        };
        if oldest.server_time > ct && cmds.back().is_some_and(|c| c.server_time > ct) {
            // The history starts after the snapshot: a gap only when a cmd
            // past it was ever dropped, which a full ring tells.
            if cmds.len() >= CMD_BACKUP {
                self.stats.misses += 1;
                self.reset();
                return None;
            }
        }
        if let Some(&(_, v)) = self.old_velocity.iter().find(|(t, _)| *t == ct) {
            ps.old_velocity = v;
        }
        let mut old = cmds
            .iter()
            .rev()
            .find(|c| c.server_time == ct)
            .copied()
            .unwrap_or_else(|| UserCmd2 {
                server_time: ct,
                ..vcod_cod2::net::usercmd::default_usercmd(snap)
            });
        // Last frame's prediction against this one's at the same cmd
        // (`CG_PredictPlayerState`'s `commandTime == oldPlayerState`'s).
        let last = self.last;
        let mut at_last = last.filter(|&(t, _)| t == ct).map(|_| ps.origin);
        let mut events = Vec::new();
        self.old_velocity.retain(|(t, _)| *t >= ct);
        self.fired.retain(|(t, _, _)| *t > ct);
        // `CG_ClipMoveToEntities` passes over `ps.clientNum`.
        let pass = ps.client_num;
        let world_of = || MoveWorld::new(world, bodies, pass);
        let rules = self.rules;
        let step =
            |ps: &mut PlayerState, ws: &mut WeaponState, cmd: &UserCmd2, old: &mut UserCmd2| {
                match defs {
                    Some(d) => weapon::armed_step(ps, ws, cmd, old, world_of(), rules, d),
                    None => {
                        pmove::player_step(ps, cmd, old, world_of(), rules, move_scale);
                    }
                }
            };
        for cmd in cmds.iter().filter(|c| c.server_time > ct) {
            let seq = ps.event_sequence;
            step(&mut ps, &mut ws, cmd, &mut old);
            self.stats.cmds_run += 1;
            let t = cmd.server_time;
            for i in seq.max(ps.event_sequence - 4)..ps.event_sequence {
                let k = (i & 3) as usize;
                let (e, p) = (ps.events[k], ps.event_parms[k]);
                if predicted_event(e) && !self.fired.contains(&(t, e, p)) {
                    self.fired.push_back((t, e, p));
                    events.push((e, p));
                }
            }
            match self.old_velocity.iter_mut().find(|(at, _)| *at == t) {
                Some(slot) => slot.1 = ps.old_velocity,
                None => self.old_velocity.push_back((t, ps.old_velocity)),
            }
            if last.is_some_and(|(lt, _)| lt == t) {
                at_last = Some(ps.origin);
            }
        }
        if let (Some((_, before)), Some(after)) = (last, at_last) {
            let delta = after - before;
            let len = delta.length();
            self.stats.max_correction = self.stats.max_correction.max(len);
            if delta != Vec3::ZERO {
                self.stats.corrected += 1;
            }
            if len > MISS_EPSILON {
                self.stats.missed += 1;
                self.error = self.error * self.decay(now_ms) + delta;
                // `cg.predictedErrorTime = cg.oldTime` (Mac 0x1ddafa).
                self.error_ms = old_ms.unwrap_or(now_ms);
            }
        }
        self.last = Some((ps.command_time, ps.origin));
        let mut drawn = ps.clone();
        let mut drawn_ws = ws.clone();
        if let Some(p) = pending.filter(|p| p.server_time > ps.command_time) {
            let mut o = old;
            step(&mut drawn, &mut drawn_ws, p, &mut o);
        }
        let origin = drawn.origin - self.error * self.decay(now_ms);
        Some(Predicted2 {
            ps,
            ws,
            drawn,
            drawn_ws,
            origin,
            events,
        })
    }

    /// The longest correction since the last call.
    pub fn take_max(&mut self) -> f32 {
        std::mem::take(&mut self.stats.max_correction)
    }

    /// The correction still drawn at `now_ms`.
    #[cfg(test)]
    fn drawn_error(&self, now_ms: f64) -> f32 {
        (self.error * self.decay(now_ms)).length()
    }

    /// Forgets the last frame and any correction in flight.
    pub fn reset(&mut self) {
        self.last = None;
        self.from = None;
        self.error = Vec3::ZERO;
        self.old_velocity.clear();
    }

    /// `(cg_errordecay - (cg.time - predictedErrorTime)) / cg_errordecay`,
    /// 0 outside (0, 1) as `CG_CalcViewValues` drops it.
    fn decay(&self, now_ms: f64) -> f32 {
        let f = (ERROR_DECAY_MS - (now_ms - self.error_ms)) / ERROR_DECAY_MS;
        if f > 0.0 && f < 1.0 { f as f32 } else { 0.0 }
    }
}

/// `es.solid` of a brush model (`SV_LinkEntity` 0x809c63a).
const SOLID_BMODEL: i32 = 0xff_ffff;

/// The entities of `frame.snap` (`cg.nextSnap`) the prediction clips, at
/// where the frame draws them: `CG_BuildSolidList` (Mac 0x1dea06) lists
/// every one with a `solid` but an item, and `CG_ClipMoveToEntities`
/// (Mac 0x1de5b8) passes over `own` and clips the rest as
/// [`body_of_solid`]'s capsule (cod2-movement.md 20). Brush models are
/// left out: the client's world never moves its submodels.
pub fn solid_bodies(frame: &Frame, own: i32) -> Vec<Body> {
    frame
        .snap
        .entities
        .iter()
        .filter(|e| e.number as i32 != own && e.etype != ET_ITEM)
        .filter_map(|e| {
            let solid = e.raw.i32("solid");
            if solid == 0 || solid == SOLID_BMODEL {
                return None;
            }
            let contents = if e.etype == ET_PLAYER {
                CONTENTS_BODY
            } else {
                1
            };
            Some(body_of_solid(
                e.number,
                frame.entity_origin(e),
                solid,
                contents,
            ))
        })
        .collect()
}

/// `CG_ClipMoveToEntities`' temp box of a packed `solid`: a unit inside the
/// box the server packed on every side, `1 - x`..`x - 1` across, `2 - zd`
/// up to `zu - 33` (Mac 0x1de5b8, the float 1.0 at 0x326188).
pub fn body_of_solid(entity: u32, origin: Vec3, solid: i32, contents: u32) -> Body {
    let x = (solid & 0xff) as f32;
    let zd = ((solid >> 8) & 0xff) - 1;
    let zu = ((solid >> 16) & 0xff) - 32;
    Body {
        entity,
        origin,
        mins: Vec3::new(1.0 - x, 1.0 - x, 1.0 - zd as f32),
        maxs: Vec3::new(x - 1.0, x - 1.0, zu as f32 - 1.0),
        contents,
    }
}

/// `bg_bobAmplitudeStanding`, `Ducked`, `Prone` and `bg_bobMax` at their
/// `BG_RegisterDvars` defaults (cod2-movement.md 13).
const BOB_AMPLITUDE: [f32; 3] = [0.007, 0.0075, 0.03];
const BOB_MAX: f32 = 8.0;

/// A gunner's eye: the gun's `tag_player` in the world, the barrel turned
/// about `tag_aim` to where the view looks off the gun's `angles`, as the
/// cgame's controller turns the drawn gun (`CG_mg42_DoControllers`).
pub fn gunner_eye(aim: Vec3, player: Vec3, origin: Vec3, angles: Vec3, view: [f32; 3]) -> Vec3 {
    use vcod_common::pmove::aim::angle_subtract;
    use vcod_common::turretpose::angles_quat;
    let barrel = angles_quat([
        angle_subtract(view[0], angles.x),
        angle_subtract(view[1], angles.y),
        0.0,
    ]);
    origin + angles_quat(angles.to_array()) * (aim + barrel * (player - aim))
}

/// The first-person eye `CG_CalcViewValues` (Mac 0x1d3b28) draws for
/// `ps` with its body at `origin`: the view height, the vertical bob
/// (`BG_GetVerticalBobFactor` Mac 0x7a0c8), the horizontal bob along the
/// view's right (`BG_GetHorizontalBobFactor` Mac 0x7a1b2), the lean
/// (`AddLeanToPosition` Mac 0x2923c with 16 and 20), and the floor at 8
/// over the origin. `land` is the landing's dip on the height, which goes
/// in after the bob (`play::cod2_view::Land::eye`). VERIFIED, the functions and their constants (Mac
/// rodata 0x32618c 255, 0x340c08 pi, 0x340c78 2 pi, 0x3261c0 4, 0x340c80
/// pi / 2, 0x3262dc 0.2, 0x326234 0.75, 0x3261e4 2, 0x3262f8 8).
/// The damage kick, `BG_CalculateViewAngles`' sway and the camera shake
/// are the view's angles (`play::cod2_view`).
pub fn first_person_eye(ps: &PlayerState, origin: Vec3, land: f32) -> Vec3 {
    let mut eye = origin + Vec3::Z * ps.view_height_current;
    // `BG_GetBobCycle`: the low byte as a phase past a whole turn.
    let cycle = (f64::from(f32::from(ps.bob_cycle as u8) / 255.0) * std::f64::consts::PI * 2.0
        + std::f64::consts::TAU) as f32;
    // `BG_GetSpeed` off a ladder: the flat speed.
    let speed = ps.velocity.truncate().length();
    let amp = match ps.view_height_target {
        pmove::VIEW_PRONE => BOB_AMPLITUDE[2],
        pmove::VIEW_CROUCH => BOB_AMPLITUDE[1],
        _ => BOB_AMPLITUDE[0],
    };
    let bob = (speed * amp).min(BOB_MAX);
    let vertical = bob
        * ((cycle + cycle).sin()
            + ((f64::from(cycle) * 4.0 + std::f64::consts::FRAC_PI_2).sin() as f32) * 0.2)
        * 0.75;
    eye.z += vertical;
    let [_, right, _] = vcod_common::x87::angle_vectors(Vec3::from_array(ps.view_angles));
    eye += right * (bob * cycle.sin());
    eye.z += land;
    // `AddLeanToPosition`: eased by `(2 - |leanf|) * leanf`, 20 units along
    // the right of the yaw rolled 16 times the ease.
    let lean = ps.leanf;
    if lean != 0.0 {
        let f = (2.0 - lean.abs()) * lean;
        let [_, right, _] =
            vcod_common::x87::angle_vectors(Vec3::new(0.0, ps.view_angles[1], 16.0 * f));
        eye += right * (f * 20.0);
    }
    eye.z = eye.z.max(origin.z + 8.0);
    eye
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_cod2::net::delta::field_index;
    use vcod_cod2::net::fields::PLAYER_STATE_FIELDS;
    use vcod_cod2::pmove::weapon::WeaponParams;
    use vcod_common::patch::Arith;

    /// The gunner's eye swings with the view about `tag_aim`, on a gun
    /// turned by its own angles.
    #[test]
    fn a_gunners_eye_rides_tag_player_about_tag_aim() {
        let (aim, player) = (Vec3::new(0.0, 0.0, 10.0), Vec3::new(-20.0, 0.0, 10.0));
        let gun = Vec3::new(100.0, 0.0, 0.0);
        let close = |a: Vec3, b: Vec3| a.distance(b) < 1e-3;
        let ahead = gunner_eye(aim, player, gun, Vec3::ZERO, [0.0, 0.0, 0.0]);
        assert!(close(ahead, Vec3::new(80.0, 0.0, 10.0)), "{ahead}");
        let left = gunner_eye(aim, player, gun, Vec3::ZERO, [0.0, 90.0, 0.0]);
        assert!(close(left, Vec3::new(100.0, -20.0, 10.0)), "{left}");
        // The gun turned 90: the same view is straight down its barrel.
        let turned = gunner_eye(
            aim,
            player,
            gun,
            Vec3::new(0.0, 90.0, 0.0),
            [0.0, 90.0, 0.0],
        );
        assert!(close(turned, Vec3::new(100.0, -20.0, 10.0)), "{turned}");
    }

    #[test]
    fn the_move_events_are_the_predicted_ones() {
        assert!(predicted_event(ev::FOOTSTEP_RUN));
        assert!(predicted_event(ev::FOOTSTEP_RUN + 5));
        assert!(predicted_event(ev::JUMP));
        assert!(predicted_event(ev::LANDING_PAIN + 3));
        assert!(predicted_event(ev::STEP_VIEW));
        assert!(predicted_event(ev::FOLIAGE_SOUND));
        assert!(!predicted_event(0));
        assert!(!predicted_event(149), "EV_RESET_ADS plays off the snapshot");
        assert!(predicted_event(158), "EV_FIRE_WEAPON");
        assert!(predicted_event(160) && predicted_event(150) && predicted_event(172));
        assert!(!predicted_event(147) && !predicted_event(159) && !predicted_event(197));
    }

    #[test]
    fn the_eye_bobs_with_speed_and_leans_aside() {
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0, 0);
        assert_eq!(
            first_person_eye(&ps, Vec3::ZERO, 0.0),
            Vec3::new(0.0, 0.0, 60.0)
        );
        // A full run bobs, at most `bg_bobMax`.
        ps.velocity = Vec3::new(190.0, 0.0, 0.0);
        ps.bob_cycle = 64;
        let e = first_person_eye(&ps, Vec3::ZERO, 0.0);
        assert!(e.distance(Vec3::new(0.0, 0.0, 60.0)) > 0.5, "{e}");
        assert!(e.distance(Vec3::new(0.0, 0.0, 60.0)) < 8.0 * 1.2 + 8.0);
        // A full lean right puts the eye 20 units to the right, at yaw 0 -y.
        ps.velocity = Vec3::ZERO;
        ps.leanf = 1.0;
        let e = first_person_eye(&ps, Vec3::ZERO, 0.0);
        assert!(e.y < -15.0, "{e}");
    }

    #[test]
    fn a_correction_eases_out_over_the_decay() {
        let mut p = Cod2Predictor {
            error: Vec3::new(10.0, 0.0, 0.0),
            error_ms: 1000.0,
            ..Default::default()
        };
        assert_eq!(p.drawn_error(1000.0), 0.0, "f = 1 is outside (0, 1)");
        assert!((p.drawn_error(1050.0) - 5.0).abs() < 1e-4);
        assert_eq!(p.drawn_error(1100.0), 0.0);
        p.reset();
        assert_eq!(p.drawn_error(1050.0), 0.0);
    }

    /// One retail capture of `crates/cod2/tests/fixtures/moves/` or
    /// `combat/`: the map, the level's weapons (the `# weapons` line), every
    /// cmd the probe sent and every snapshot's playerstate, and the weapon
    /// held (the header's `probe_weapon`, else the kar98k).
    struct Capture {
        map: String,
        weapons: Option<Vec<String>>,
        weapon: String,
        cmds: Vec<UserCmd2>,
        snaps: Vec<PlayerState2>,
    }

    fn parse(text: &str) -> Capture {
        let (mut map, mut cmds, mut snaps) = (String::new(), Vec::new(), Vec::new());
        let mut weapons = None;
        let mut weapon = "kar98k_mp".to_string();
        let mut ps = PlayerState2::default();
        for line in text.lines() {
            if let Some((_, rest)) = line
                .strip_prefix('#')
                .and_then(|l| l.split_once("probe_weapon "))
            {
                weapon = rest
                    .split(|c: char| c == ',' || c == ')' || c.is_whitespace())
                    .next()
                    .unwrap()
                    .to_string();
            }
            let mut w = line.split_whitespace();
            match w.next() {
                Some("#") => match w.next() {
                    Some("map") => map = w.next().unwrap().to_string(),
                    Some("weapons") => weapons = Some(w.map(str::to_string).collect()),
                    _ => {}
                },
                Some("c") => {
                    let n: Vec<i64> = w
                        .enumerate()
                        .map(|(k, t)| {
                            if k == 1 {
                                i64::from_str_radix(t, 16).unwrap()
                            } else {
                                t.parse().unwrap()
                            }
                        })
                        .collect();
                    cmds.push(UserCmd2 {
                        server_time: n[0] as i32,
                        buttons: n[1] as i32,
                        weapon: n[2] as u8,
                        off_hand_index: n[3] as u8,
                        angles: [n[4] as i32, n[5] as i32, n[6] as i32],
                        forward_move: n[7] as i8,
                        right_move: n[8] as i8,
                    });
                }
                Some("s") => {
                    let _ = (w.next(), w.next());
                    for kv in w {
                        let (k, v) = kv.split_once('=').unwrap();
                        let v = u32::from_str_radix(v, 16).unwrap();
                        // `S[i]`, `A[i]` and `C[i]`: the stats, reserve and
                        // clip arrays (`e` and `e-` entity lines are skipped).
                        let slot = |k: &str| k[2..k.len() - 1].parse::<usize>().unwrap();
                        match k.as_bytes()[0] {
                            b'S' if k.starts_with("S[") => ps.stats[slot(k)] = v as i32,
                            b'A' if k.starts_with("A[") => ps.ammo[slot(k)] = v as i32,
                            b'C' if k.starts_with("C[") => ps.ammoclip[slot(k)] = v as i32,
                            _ => ps.f[field_index(&PLAYER_STATE_FIELDS, k)] = v,
                        }
                    }
                    snaps.push(ps.clone());
                }
                _ => {}
            }
        }
        Capture {
            map,
            weapons,
            weapon,
            cmds,
            snaps,
        }
    }

    /// The capture's weapon table as `Cod2Session` builds it off
    /// `CS_WEAPONS`; `probe_moves.gsc`'s captures log none, and hand out
    /// their one weapon as weapon 3 with ammo and clip index 2 (moves_ab).
    fn table(fs: &vcod_common::pk3::Pk3Fs, cap: &Capture) -> Vec<Option<WeaponParams>> {
        match &cap.weapons {
            Some(n) => weapon::table(fs, &n.iter().map(String::as_str).collect::<Vec<_>>()),
            None => {
                let def = vcod_cod2::weapon::load(fs, &cap.weapon).unwrap().def;
                vec![
                    None,
                    None,
                    None,
                    Some(WeaponParams::from_def(&def, 2, 2, 0)),
                ]
            }
        }
    }

    fn world_of(fs: &vcod_common::pk3::Pk3Fs, map: &str) -> CollisionWorld {
        let data = fs.read(&format!("maps/mp/{map}.d3dbsp")).unwrap();
        let b = vcod_cod2::d3dbsp::D3dBsp::parse(&data).unwrap();
        let w = vcod_cod2::world::from_bsp(&b);
        let cw = vcod_cod2::map::collision_world(&b, &w, &[]).unwrap();
        cw.set_arith(Arith::Stored);
        cw
    }

    fn fixtures(sub: &str) -> String {
        format!(
            "{}/../cod2/tests/fixtures/{sub}",
            env!("CARGO_MANIFEST_DIR")
        )
    }

    /// The playerstate words the prediction must land on, the weapon's
    /// beside the move's; a 16-bit timer as its low half.
    const PREDICTED: &[&str] = &[
        "origin[0]",
        "origin[1]",
        "origin[2]",
        "pm_flags",
        "weapon",
        "weaponstate",
        "weaponTime",
        "weaponDelay",
        "weapAnim",
        "fWeaponPosFrac",
        "holdBreathScale",
        "holdBreathTimer",
    ];

    fn wire_word(ps: &PlayerState2, n: &str) -> u32 {
        let i = field_index(&PLAYER_STATE_FIELDS, n);
        match PLAYER_STATE_FIELDS[i].bits {
            -16 | 16 => ps.f[i] & 0xffff,
            _ => ps.f[i],
        }
    }

    /// Each retail capture (c6-moveab's movement ones and the combat
    /// captures' rifle, smg, bar, pistol, switch and breath, on
    /// `cod2_lnxded`) played through the predictor as the client would have
    /// run it: at each snapshot, the cmds sent up to the next snapshot's
    /// `commandTime` on top of it, the weapon half beside the move. The
    /// predicted origin, sight, weapon state and timers at that
    /// `commandTime` must be the next snapshot's, bit for bit, in every
    /// row, the mantles included (cod2-movement.md 9). The same with
    /// the replay started from each snapshot's `oldVelocity` read as 0, as
    /// retail's client would, is printed, not asserted. Needs `COD2_DIR`.
    #[test]
    fn retail_captures_predict_bit_for_bit() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let dir = fixtures("moves");
        let mut names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| {
                let p = e.ok()?.path();
                (p.extension()? == "txt").then(|| p.file_stem()?.to_str().map(String::from))?
            })
            .map(|n| format!("{dir}/{n}"))
            .collect();
        names.sort();
        assert!(names.len() >= 10, "captures: {names:?}");
        // The frag's own blast knocks him back server-side (moves_ab's GAPS).
        for n in ["rifle", "smg", "bar", "pistol", "switch", "breath"] {
            names.push(format!("{}/{n}", fixtures("combat")));
        }
        let mut worlds = std::collections::BTreeMap::new();
        let mut bad = Vec::new();
        let rules = Rules {
            mantle_anims: vcod_cod2::pmove::mantle::anims(&fs),
            ..Rules::default()
        };
        let mut sighted = 0;
        for path in &names {
            let name = path.rsplit('/').next().unwrap();
            let cap = parse(&std::fs::read_to_string(format!("{path}.txt")).unwrap());
            let defs = table(&fs, &cap);
            let world = worlds
                .entry(cap.map.clone())
                .or_insert_with(|| world_of(&fs, &cap.map));
            let ct = |ps: &PlayerState2| ps.i32("commandTime");
            // From the first snapshot whose cmd is logged with one before it.
            let start = cap
                .snaps
                .iter()
                .position(|s| cap.cmds.iter().skip(1).any(|c| c.server_time == ct(s)))
                .unwrap();
            let (mut rows, mut wrong, mut wrong_zero) = (0, 0, 0);
            let mut p = Cod2Predictor {
                rules,
                ..Default::default()
            };
            let mut ring = VecDeque::new();
            for k in start..cap.snaps.len() - 1 {
                let (snap, next) = (&cap.snaps[k], &cap.snaps[k + 1]);
                ring.clear();
                ring.extend(
                    cap.cmds
                        .iter()
                        .filter(|c| c.server_time <= ct(next))
                        .copied(),
                );
                let ms = f64::from(k as i32 * 50);
                let pred = p
                    .predict(snap, &ring, None, world, &[], Some(&defs), 0.0, ms)
                    .unwrap();
                rows += 1;
                assert_eq!(pred.ps.command_time, ct(next));
                let mut got = pred.ps.to_wire(snap);
                pred.ws.write_wire(&mut got);
                sighted += usize::from(pred.ws.pos_frac > 0.0 && pred.ws.pos_frac < 1.0);
                let diff: Vec<_> = PREDICTED
                    .iter()
                    .filter(|n| wire_word(&got, n) != wire_word(next, n))
                    .map(|n| {
                        format!(
                            "{n} {:x} retail {:x}",
                            wire_word(&got, n),
                            wire_word(next, n)
                        )
                    })
                    .collect();
                if !diff.is_empty() {
                    wrong += 1;
                    if wrong == 1 {
                        bad.push(format!("{name} row {k}: {}", diff.join(", ")));
                    }
                }
                // Retail's own start: `oldVelocity` 0 every replay.
                let mut q = Cod2Predictor {
                    rules,
                    ..Default::default()
                };
                let pz = q
                    .predict(snap, &ring, None, world, &[], Some(&defs), 0.0, 0.0)
                    .unwrap();
                let origin = |ps: &PlayerState2| {
                    [0, 1, 2]
                        .map(|i| ps.f[field_index(&PLAYER_STATE_FIELDS, &format!("origin[{i}]"))])
                };
                wrong_zero +=
                    usize::from(pz.ps.origin.to_array().map(f32::to_bits) != origin(next));
            }
            eprintln!(
                "{name:<13} rows {rows:>4} wrong {wrong:>3} (oldVelocity 0 origin: {wrong_zero}) \
                 corrections {}",
                p.stats.missed
            );
        }
        assert!(
            sighted > 10,
            "the captures raise a sight: {sighted} rows mid-rise"
        );
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }

    /// Between two snapshots the sight rises with every frame's cmd: the
    /// rifle capture's kar98k, from a snapshot partway up, predicted after
    /// each cmd the client sent before the next snapshot (and once more on
    /// a cmd being built at a frame's own time), its `fWeaponPosFrac`
    /// climbing each time while the snapshot's own stays put. Retail's
    /// `CG_PredictPlayerState` (Mac 0x1ddafa) runs `PmoveSingle` with
    /// `PM_Weapon` every frame the same way. Needs `COD2_DIR`.
    #[test]
    fn the_sight_rises_every_frame_between_snapshots() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let cap =
            parse(&std::fs::read_to_string(format!("{}/rifle.txt", fixtures("combat"))).unwrap());
        let defs = table(&fs, &cap);
        let world = world_of(&fs, &cap.map);
        let pos = |ps: &PlayerState2| ps.f32("fWeaponPosFrac");
        let ct = |ps: &PlayerState2| ps.i32("commandTime");
        let k = (0..cap.snaps.len() - 1)
            .find(|&k| {
                let (a, b) = (pos(&cap.snaps[k]), pos(&cap.snaps[k + 1]));
                a > 0.0 && b < 1.0 && b > a
            })
            .expect("a snapshot mid-rise");
        let (snap, next) = (&cap.snaps[k], &cap.snaps[k + 1]);
        let between: Vec<_> = cap
            .cmds
            .iter()
            .filter(|c| c.server_time > ct(snap) && c.server_time <= ct(next))
            .copied()
            .collect();
        assert!(between.len() >= 3, "{} cmds", between.len());
        let mut p = Cod2Predictor::default();
        let mut last = pos(snap);
        let mut ring: VecDeque<_> = cap
            .cmds
            .iter()
            .filter(|c| c.server_time <= ct(snap))
            .copied()
            .collect();
        for cmd in &between {
            // A frame halfway to this cmd draws a cmd being built then.
            let mut built = *cmd;
            built.server_time = (ring.back().unwrap().server_time + cmd.server_time) / 2;
            let half = p
                .predict(
                    snap,
                    &ring,
                    Some(&built),
                    &world,
                    &[],
                    Some(&defs),
                    0.0,
                    0.0,
                )
                .unwrap();
            assert!(
                half.drawn_ws.pos_frac > last,
                "{} after {last}",
                half.drawn_ws.pos_frac
            );
            ring.push_back(*cmd);
            let pred = p
                .predict(snap, &ring, None, &world, &[], Some(&defs), 0.0, 0.0)
                .unwrap();
            assert!(
                pred.ws.pos_frac > half.drawn_ws.pos_frac,
                "{} after {last}",
                pred.ws.pos_frac
            );
            last = pred.ws.pos_frac;
        }
        assert_eq!(
            last.to_bits(),
            pos(next).to_bits(),
            "lands on the next snapshot's"
        );
        // Without the table the sight holds the snapshot's.
        let held = p
            .predict(snap, &ring, None, &world, &[], None, 0.0, 0.0)
            .unwrap();
        assert_eq!(held.ws.pos_frac, pos(snap));
    }

    /// A standing player's live packing (0x66010f: x 15, `1 - mins.z` 1,
    /// `32 + maxs.z` 102, cod2-protocol.md 15.6) clips a unit inside the
    /// server's (-15,-15,0)..(15,15,70).
    #[test]
    fn a_packed_body_clips_a_unit_inside_its_box() {
        let b = body_of_solid(1, Vec3::ZERO, 0x66010f, CONTENTS_BODY);
        assert_eq!(b.mins, Vec3::new(-14.0, -14.0, 1.0));
        assert_eq!(b.maxs, Vec3::new(14.0, 14.0, 69.0));
    }

    /// `CG_BuildSolidList` keeps what has a `solid` but items and brush
    /// models, `CG_ClipMoveToEntities` passes over the own client, and a
    /// player clips as a body, anything else as contents 1.
    #[test]
    fn the_solid_list_keeps_bodies_and_solids_but_self_items_and_brushes() {
        use vcod_cod2::net::EntityState2;
        use vcod_cod2::net::fields::ENTITY_STATE_FIELDS;
        use vcod_cod2::net::view::{ConfigStrings, EntityView, PlayerView, SnapshotView};
        let ent = |n: u32, etype: u32, solid: u32, x: f32| {
            let mut es = EntityState2::null(n);
            let mut set = |name: &str, v: u32| es.f[field_index(&ENTITY_STATE_FIELDS, name)] = v;
            set("eType", etype);
            set("solid", solid);
            set("pos.trBase[0]", x.to_bits());
            EntityView::new(&es)
        };
        let snap = SnapshotView {
            message_num: 0,
            server_time: 1000,
            snap_flags: 0,
            ps: PlayerView::new(&PlayerState2::default(), 0),
            entities: vec![
                ent(0, 1, 0x66010f, 0.0),
                ent(1, 1, 0x66010f, 100.0),
                ent(2, 3, 0x66010f, 200.0),
                ent(3, 6, 0xff_ffff, 300.0),
                ent(4, 0, 0x30080a, 400.0),
                ent(5, 1, 0, 500.0),
            ],
            clients: Vec::new(),
        };
        let cs = ConfigStrings::default();
        let frame = Frame {
            snap: &snap,
            prev: None,
            time_ms: 1000,
            frac: 0.0,
            configstrings: &cs,
        };
        let got: Vec<_> = solid_bodies(&frame, 0)
            .iter()
            .map(|b| (b.entity, b.origin.x, b.contents))
            .collect();
        assert_eq!(got, [(1, 100.0, CONTENTS_BODY), (4, 400.0, 1)]);
    }

    /// The walk capture replayed with a standing body half a run ahead of
    /// it: the replay stops where the two capsules meet, 15 + 14 apart in
    /// the plane, where the bare world's runs on through. Needs `COD2_DIR`.
    #[test]
    fn a_body_ahead_stops_the_replay() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let cap =
            parse(&std::fs::read_to_string(format!("{}/walk.txt", fixtures("moves"))).unwrap());
        let world = world_of(&fs, &cap.map);
        let defs = table(&fs, &cap);
        let ct = |ps: &PlayerState2| ps.i32("commandTime");
        let snap = cap
            .snaps
            .iter()
            .find(|s| cap.cmds.iter().skip(1).any(|c| c.server_time == ct(s)))
            .unwrap();
        let start = PlayerState::from_wire(snap).origin;
        let ring: VecDeque<_> = cap
            .cmds
            .iter()
            .filter(|c| c.server_time <= ct(snap) + 1500)
            .copied()
            .collect();
        let run = |bodies: &[Body]| {
            Cod2Predictor::default()
                .predict(snap, &ring, None, &world, bodies, Some(&defs), 0.0, 0.0)
                .unwrap()
                .ps
                .origin
        };
        let free = run(&[]);
        let ahead = start + (free - start).with_z(0.0) * 0.5;
        let body = body_of_solid(1, ahead, 0x66010f, CONTENTS_BODY);
        let stopped = run(&[body]);
        let gap = (ahead - stopped).truncate().length();
        assert!(
            free.distance(start) > 50.0,
            "the walk runs {free} from {start}"
        );
        assert!(
            (28.8..29.2).contains(&gap),
            "stopped {gap} short of the body"
        );
        // The own client's number is passed over.
        let own = body_of_solid(0, ahead, 0x66010f, CONTENTS_BODY);
        assert_eq!(run(&[own]), free);
    }
}
