//! A CoD2 player's drawn body: its models ([`character`]), the skeleton they
//! graft into, and the pose its `legsAnim`/`torsoAnim` and angles give it,
//! through vcod-common's CoD1 pose code (`animtree`, `animclock`,
//! `playerpose`, `skeleton`) under CoD2's rules.
//! docs/research/cod2-player-anims.md.

pub mod character;

use crate::net::EntityState2;
use anyhow::Result;
use glam::{Mat4, Quat, Vec3};
use std::collections::HashMap;
use std::rc::Rc;
use vcod_common::animclock::{ChannelClock, Seen, Times, update_body};
use vcod_common::animtree::PlayerAnims;
use vcod_common::asset_source::AssetSource;
use vcod_common::model::{self, Model};
use vcod_common::net::flags::{
    EF_CROUCH, EF_DEAD, EF_FIRING, EF_MOUNTED_DUCK, EF_MOUNTED_PRONE, EF_MOUNTED_STAND, EF_PRONE,
};
use vcod_common::playerpose::{
    self, BodyAngles, BodyInput, BodySlope, Controllers, EF_ADS, LeanDvars, Mantle, PoseInputs,
    Updater,
};
use vcod_common::skeleton::{PoseBuffer, Skeleton};
use vcod_common::xanim::{self, XAnim};

pub use character::Character;

/// CoD2's `eFlags` bits the body reads (cod2-player-anims.md 4).
pub mod eflags {
    pub const CROUCH: i32 = 0x4;
    pub const PRONE: i32 = 0x8;
    pub const FIRING: i32 = 0x40;
    /// A gunner's stance: 0x100 prone, 0x200 crouched, both standing.
    pub const MOUNTED: i32 = 0x300;
    pub const MANTLE: i32 = 0x4000;
    pub const DEAD: i32 = 0x20000;
    pub const ADS: i32 = 0x40000;
    /// A corpse whose death anim still plays rather than jumping to its end
    /// (`BG_RunLerpFrameRate`'s `eFlags >> 19 & 1`).
    pub const FRESH_CORPSE: i32 = 0x80000;
}

/// 1.1's fresh-corpse bit, which `animclock` reads (`EF_FRESH_CORPSE`).
const EF_FRESH_CORPSE_V1: i32 = 0x800;

/// CoD2's `eFlags` in 1.1's numbering, the one the shared pose code reads,
/// and the mantle, which 1.1 has no bit for.
pub fn body_flags(e: i32) -> (i32, Mantle) {
    let mut out = 0;
    for (cod2, v1) in [
        (eflags::DEAD, EF_DEAD),
        (eflags::CROUCH, EF_CROUCH),
        (eflags::PRONE, EF_PRONE),
        (eflags::FIRING, EF_FIRING),
        (eflags::ADS, EF_ADS),
        (eflags::FRESH_CORPSE, EF_FRESH_CORPSE_V1),
    ] {
        if e & cod2 != 0 {
            out |= v1;
        }
    }
    out |= match e & eflags::MOUNTED {
        0x100 => EF_MOUNTED_PRONE,
        0x200 => EF_MOUNTED_DUCK,
        0x300 => EF_MOUNTED_STAND,
        _ => 0,
    };
    let mantle = Mantle {
        on: e & eflags::MANTLE != 0,
        alone: e == eflags::MANTLE,
    };
    (out, mantle)
}

/// What a body's pose reads off its entity, already interpolated by the
/// caller: the integration's own input struct, filled from an
/// [`EntityState2`] by [`BodyFrame::from_entity`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BodyFrame {
    pub origin: Vec3,
    /// `apos.trBase`: the view, engine degrees.
    pub angles: [f32; 3],
    /// Wire values, restart toggle (512) included.
    pub legs_anim: i32,
    pub torso_anim: i32,
    /// CoD2's numbering ([`eflags`]).
    pub eflags: i32,
    /// `angles2[1]`, `ps.movementDir` (`BG_PlayerStateToEntityState`).
    pub movement_dir: f32,
    pub lean: f32,
    pub torso_height: f32,
    pub torso_pitch: f32,
    pub waist_pitch: f32,
    /// `clientNum`: a body's clock phase.
    pub client_num: i32,
}

impl BodyFrame {
    pub fn from_entity(e: &EntityState2) -> BodyFrame {
        BodyFrame {
            origin: Vec3::from(e.origin()),
            angles: [
                e.f32("apos.trBase[0]"),
                e.f32("apos.trBase[1]"),
                e.f32("apos.trBase[2]"),
            ],
            legs_anim: e.i32("legsAnim"),
            torso_anim: e.i32("torsoAnim"),
            eflags: e.i32("eFlags"),
            movement_dir: e.f32("angles2[1]"),
            lean: e.f32("leanf"),
            torso_height: e.f32("fTorsoHeight"),
            torso_pitch: e.f32("fTorsoPitch"),
            waist_pitch: e.f32("fWaistPitch"),
            client_num: e.i32("clientNum"),
        }
    }

    /// Where the body samples the light grid: the origin raised by
    /// `fTorsoHeight` and 32 standing, 20 crouched, 12 prone, the lighting
    /// origin `CG_Player` (Mac 0x1e6834) and `CG_Corpse` (0x1e78ea) set
    /// (cod2-materials.md 9.2).
    pub fn light_origin(&self) -> Vec3 {
        let stance = if self.eflags & eflags::PRONE != 0 {
            12.0
        } else if self.eflags & eflags::CROUCH != 0 {
            20.0
        } else {
            32.0
        };
        self.origin + Vec3::Z * (self.torso_height + stance)
    }

    /// The model's frame: the origin and the view's yaw alone; the legs'
    /// turn off it rides `tag_origin`'s control (cod2-player-anims.md 6).
    pub fn transform(&self) -> Mat4 {
        Mat4::from_rotation_translation(
            Quat::from_rotation_z(self.angles[1].to_radians()),
            self.origin,
        )
    }
}

/// The player animtree and script (`animtrees/multiplayer.atr`,
/// `mp/playeranim.script`) and the clips loaded so far.
pub struct PlayerAnimSet {
    pub anims: PlayerAnims,
    clips: HashMap<String, Option<Rc<XAnim>>>,
}

/// The gun a gunner's body rides, as [`gunner_placement`] reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct GunFrame {
    /// The gun's model (no `xmodel/`) and CS 7 weapon.
    pub model: String,
    pub weapon: String,
    pub origin: Vec3,
    /// Engine degrees.
    pub angles: [f32; 3],
    /// The barrel's `angles2`.
    pub barrel: [f32; 3],
}

/// `CG_Player`'s turret arm (Mac 0x1e6834): a body with `eFlags & 0x300`
/// on a gun is put where its anim group, blended by the barrel's yaw over
/// the gun's `animHorRotateInc` and by the body's height over the rows,
/// hangs it off the gun's `tag_weapon`, the server's placement
/// (`turretpose::place_gunner`). VERIFIED, the calls; the client's trace
/// down after it is left out (the snapshot's origin already carries the
/// server's). `None` for a gun whose model, tags or weapon do not load.
/// `rule` is the game's walk of the group (CoD4 shares the rest).
pub fn gunner_placement(
    rule: vcod_common::turretpose::GunnerRule,
    set: &mut PlayerAnimSet,
    fs: &dyn AssetSource,
    f: &BodyFrame,
    gun: &GunFrame,
    tags: (Vec3, Vec3),
    rotate_inc: f32,
) -> Option<vcod_common::turretpose::GunnerPlacement> {
    if f.eflags & eflags::MOUNTED == 0 || rotate_inc <= 0.0 {
        return None;
    }
    let PlayerAnimSet { anims, clips } = set;
    vcod_common::turretpose::place_gunner_by(
        rule,
        anims,
        |n| load_clip(clips, fs, n),
        f.legs_anim,
        vcod_common::turretpose::tag_weapon_local(tags.0, tags.1, gun.barrel),
        (gun.origin, vcod_common::turretpose::angles_quat(gun.angles)),
        f.origin,
        rotate_inc,
    )
}

impl PlayerAnimSet {
    /// The wire's anim array is 1.1's walk over CoD2's tree and script
    /// (cod2-player-anims.md 3).
    pub fn load(fs: &dyn AssetSource) -> Result<PlayerAnimSet> {
        let mut anims = PlayerAnims::load(fs)?;
        // `moveSpeed` is a float on CoD2 (lnxded 0x80d72e6; cod2-combat.md 9.3).
        anims.float_move_speed = true;
        Ok(PlayerAnimSet {
            anims,
            clips: HashMap::new(),
        })
    }

    /// `xanim/<name>`, loaded once; `None` (warned once) where the paks
    /// carry no such clip.
    pub fn clip(&mut self, fs: &dyn AssetSource, name: &str) -> Option<Rc<XAnim>> {
        load_clip(&mut self.clips, fs, name)
    }
}

/// A player anim clip, cached by name.
fn load_clip(
    cache: &mut HashMap<String, Option<Rc<XAnim>>>,
    fs: &dyn AssetSource,
    name: &str,
) -> Option<Rc<XAnim>> {
    xanim::load_cached(cache, fs, name, "cod2 player anim")
}

/// The parts [`PlayerRig::assemble_with`] has loaded, by model name and
/// whether its hit boxes were dropped (`ignoreCollision`).
pub type RigParts = HashMap<(String, bool), Rc<Model>>;

/// A character's models and the skeleton they graft into: the body, then
/// each attachment on its tag or, untagged, its root bone's namesake
/// (a head's `j_spine4`, a helmet's `j_head`).
pub struct PlayerRig {
    /// Parallel to the skeleton's models: index 0 the body. An attachment
    /// that failed to load is left out.
    pub models: Vec<(String, Rc<Model>)>,
    pub skeleton: Skeleton,
    /// The skeleton's clip bindings, kept from pose to pose.
    pub binds: vcod_common::skeleton::BindCache,
}

impl PlayerRig {
    /// Fails only on the body; a missing attachment is warned and skipped,
    /// one attached with `ignoreCollision` keeps no hit box.
    /// Needs [`crate::model::register`] to have run.
    pub fn assemble(fs: &dyn AssetSource, c: &Character) -> Result<PlayerRig> {
        Self::assemble_with(fs, c, &mut RigParts::new())
    }

    /// [`Self::assemble`] taking each part from `parts` when an earlier
    /// rig loaded it, so the rigs of one body under many weapons share
    /// its meshes instead of each baking its own.
    pub fn assemble_with(
        fs: &dyn AssetSource,
        c: &Character,
        parts: &mut RigParts,
    ) -> Result<PlayerRig> {
        let mut part = |name: &str, no_hits: bool| -> Result<Rc<Model>> {
            if let Some(m) = parts.get(&(name.to_string(), no_hits)) {
                return Ok(m.clone());
            }
            let mut m = model::load(fs, name)?;
            if no_hits {
                for b in &mut m.bones {
                    b.hit_mins = glam::Vec3::ZERO;
                    b.hit_maxs = glam::Vec3::ZERO;
                }
            }
            let m = Rc::new(m);
            parts.insert((name.to_string(), no_hits), m.clone());
            Ok(m)
        };
        let mut models = vec![(c.body.clone(), part(&c.body, false)?)];
        let mut tags = vec![None];
        for (i, (name, tag)) in c.attachments.iter().enumerate() {
            match part(name, c.ignore_collision >> i & 1 != 0) {
                Ok(m) => {
                    models.push((name.clone(), m));
                    tags.push(tag.clone());
                }
                Err(e) => log::warn!("cod2 player part '{name}': {e:#}, drawing without it"),
            }
        }
        let refs: Vec<(&Model, Option<&str>)> = models
            .iter()
            .zip(&tags)
            .map(|((_, m), t)| (m.as_ref(), t.as_deref()))
            .collect();
        let skeleton = Skeleton::build_grafted(&refs);
        Ok(PlayerRig {
            models,
            skeleton,
            binds: Default::default(),
        })
    }
}

/// One body's state across frames: the cgame's two lerp frames, the
/// swung angles and the eased controls (`clientInfo_t`).
#[derive(Clone, Default)]
pub struct PlayerBody {
    legs: ChannelClock,
    torso: ChannelClock,
    /// The stance-change window (`ci+0x4ac`, 400 ms; Mac 0x183492).
    window_ms: i32,
    angles: BodyAngles,
    controls: Controllers,
    /// A CoD4 body's lean dvars: its controls are CoD4's
    /// ([`Controllers::cod4`]); `None` poses CoD2's.
    pub cod4_lean: Option<LeanDvars>,
}

impl PlayerBody {
    /// Steps the body one frame to `at.clock_ms`, `frametime_ms` after the
    /// last: the two channels, the angle swings, then the controls eased
    /// toward their targets, in `BG_PlayerAnimation`'s order (Mac 0x183ef2;
    /// the controls are `BG_Player_DoControllers`, 0x182b82, run off it).
    pub fn advance(
        &mut self,
        anims: &PlayerAnims,
        f: &BodyFrame,
        at: Times,
        frametime_ms: i32,
        swing_speed: f32,
    ) {
        self.animate(anims, f, at, frametime_ms, swing_speed);
        self.ease_controls(anims, f, frametime_ms);
    }

    /// [`Self::advance`] without the controls: what the server's
    /// `ClientEndFrame` runs each frame (lnxded 0x80dbdbc, the swings, the
    /// conditions and the two lerp frames). Its controls ease only when a
    /// trace builds the skeleton ([`Self::ease_controls`]).
    pub fn animate(
        &mut self,
        anims: &PlayerAnims,
        f: &BodyFrame,
        at: Times,
        frametime_ms: i32,
        swing_speed: f32,
    ) {
        let (flags, _) = body_flags(f.eflags);
        let seen = Seen {
            client_num: f.client_num,
            tr_base: f.origin,
            eflags: flags,
        };
        update_body(
            (&mut self.legs, &mut self.torso),
            anims,
            (f.legs_anim, f.torso_anim),
            &seen,
            at,
            Some(&mut self.window_ms),
        );
        let input = Self::input(anims, f);
        let (_, mantle) = body_flags(f.eflags);
        let updater = match self.cod4_lean {
            Some(_) => Updater::Cod4(mantle),
            None => Updater::Cod2(mantle),
        };
        self.angles
            .step_by(updater, &input, frametime_ms, swing_speed);
        self.angles.update_conditions(&input);
    }

    /// `BG_Player_DoControllers` (lnxded 0x80dbc70): the controls eased
    /// `frametime_ms` toward the targets the swung angles and `f` give.
    pub fn ease_controls(&mut self, anims: &PlayerAnims, f: &BodyFrame, frametime_ms: i32) {
        let input = Self::input(anims, f);
        let slope = BodySlope {
            lean: f.lean,
            torso_height: f.torso_height,
            torso_pitch: f.torso_pitch,
            waist_pitch: f.waist_pitch,
        };
        let target = match &self.cod4_lean {
            Some(lean) => Controllers::cod4(&self.angles, &input, &slope, lean),
            None => Controllers::cod2(&self.angles, &input, &slope),
        };
        self.controls.ease_toward(&target, frametime_ms);
    }

    fn input(anims: &PlayerAnims, f: &BodyFrame) -> BodyInput {
        BodyInput {
            view: f.angles,
            movement_dir: f.movement_dir,
            eflags: body_flags(f.eflags).0,
            legs: anims.record(f.legs_anim),
            seat_weapon: Default::default(),
        }
    }

    /// Holds the clips still over `ms` the body was out of the snapshot.
    pub fn pause(&mut self, ms: i32) {
        self.legs.pause(ms);
        self.torso.pause(ms);
    }

    pub fn controls(&self) -> &Controllers {
        &self.controls
    }

    /// The pose at `now_ms`: the legs and torso clips blended as the tree
    /// walk blends `main`'s two nodes, the controls over them.
    ///
    /// `turret` is a gunner's placement blend (`CG_Player`'s turret arm,
    /// Mac 0x1e6834): its turret group node and weighted rows, which pose
    /// the legs in place of the group's own clip. A drawn pose: it keeps
    /// no engine records, so no stored-arithmetic trace reads it.
    pub fn pose(
        &self,
        rig: &PlayerRig,
        set: &mut PlayerAnimSet,
        fs: &dyn AssetSource,
        f: &BodyFrame,
        now_ms: i32,
        turret: Option<(usize, &[vcod_common::turretpose::TurretRow])>,
    ) -> PoseBuffer {
        self.pose_with(rig, set, fs, f, now_ms, false, turret, true)
    }

    /// [`Self::pose`], each clip sampled at its node time when `node_time`
    /// (the server's tree, stepped a frame at a time: `G_RunFrame`'s
    /// lnxded 0x80920d0 over every entity's DObj).
    #[allow(clippy::too_many_arguments)]
    pub fn pose_in(
        &self,
        rig: &PlayerRig,
        set: &mut PlayerAnimSet,
        fs: &dyn AssetSource,
        f: &BodyFrame,
        now_ms: i32,
        node_time: bool,
        turret: Option<(usize, &[vcod_common::turretpose::TurretRow])>,
    ) -> PoseBuffer {
        self.pose_with(rig, set, fs, f, now_ms, node_time, turret, false)
    }

    /// [`Self::pose_in`]; a `drawn` pose skips the engine records
    /// (`playerpose::pose_player_drawn`), which only a trace reads.
    #[allow(clippy::too_many_arguments)]
    fn pose_with(
        &self,
        rig: &PlayerRig,
        set: &mut PlayerAnimSet,
        fs: &dyn AssetSource,
        f: &BodyFrame,
        now_ms: i32,
        node_time: bool,
        turret: Option<(usize, &[vcod_common::turretpose::TurretRow])>,
        drawn: bool,
    ) -> PoseBuffer {
        let PlayerAnimSet { anims, clips } = set;
        let inputs = PoseInputs {
            anims,
            legs: f.legs_anim,
            torso: f.torso_anim,
            legs_layers: self.legs.layers(now_ms),
            torso_layers: self.torso.layers(now_ms),
            nodes: self.torso.node_weights(now_ms),
            group_pitch: f.torso_pitch,
            turret_leaves: turret,
            turret_left: None,
            seat_leaves: Vec::new(),
            // `BG_Player_DoControllers` (Mac 0x182b82) zeroes every
            // control of a gunner (`eFlags & 0x300`).
            controllers: turret.is_none().then_some(self.controls),
            leaf_times: Vec::new(),
            node_time,
            binds: Some(&rig.binds),
        };
        let mut load = |name: &str| load_clip(clips, fs, name);
        match drawn {
            true => playerpose::pose_player_drawn(&rig.skeleton, &inputs, &mut load),
            false => playerpose::pose_player(&rig.skeleton, &inputs, &mut load),
        }
    }
}

/// One drawn player: each model of its rig with its skin matrices
/// (`PoseBuffer::skin_matrices`), under one model-to-world transform. What
/// the renderer's dynamic model path takes, model by model.
pub struct PosedPlayer {
    pub transform: Mat4,
    /// Model name (no `xmodel/`) and its bones' skin matrices.
    pub models: Vec<(String, Vec<Mat4>)>,
}

/// The skin matrices of every model of `rig` under `pose`.
pub fn posed(rig: &PlayerRig, pose: &PoseBuffer, f: &BodyFrame) -> PosedPlayer {
    PosedPlayer {
        transform: f.transform(),
        models: {
            let worlds = pose.bone_worlds(&rig.skeleton);
            rig.models
                .iter()
                .enumerate()
                .map(|(i, (name, _))| (name.clone(), rig.skeleton.skin_matrices(&worlds, i)))
                .collect()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cod2_flags_map_onto_the_shared_numbering() {
        let (f, m) = body_flags(eflags::CROUCH | eflags::FIRING | eflags::ADS);
        assert_eq!(f, EF_CROUCH | EF_FIRING | EF_ADS);
        assert_eq!(m, Mantle::default());
        let (f, _) = body_flags(eflags::DEAD | eflags::FRESH_CORPSE | eflags::PRONE);
        assert_eq!(f, EF_DEAD | EF_FRESH_CORPSE_V1 | EF_PRONE);
        assert_eq!(body_flags(0x100).0, EF_MOUNTED_PRONE);
        assert_eq!(body_flags(0x200).0, EF_MOUNTED_DUCK);
        assert_eq!(body_flags(0x300).0, EF_MOUNTED_STAND);
        assert_eq!(
            body_flags(eflags::MANTLE).1,
            Mantle {
                on: true,
                alone: true
            }
        );
        assert_eq!(
            body_flags(eflags::MANTLE | eflags::CROUCH).1,
            Mantle {
                on: true,
                alone: false
            }
        );
    }
}
