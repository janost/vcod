//! CoD4 players and corpses posed, renderer-free: each body's rig (its
//! client's body, head and held weapon, grafted) built once, its legs and
//! torso stepped and posed every frame by `vcod_cod2::player`'s body over
//! the shared pose code under CoD4's controllers (`Controllers::cod4`,
//! the lean off the server's dvars), the clips and models coming out of the zones
//! through a [`BodySource`]. `cod4_client` draws what this returns; the
//! replay gate counts it. docs/research/cod4-player-anims.md.

use glam::{Quat, Vec3};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use vcod_cod2::player::{
    Character, PlayerAnimSet, PlayerBody, PlayerRig, PosedPlayer, RigParts, posed,
};
use vcod_cod4::anim::BodySource;
use vcod_cod4::ragdoll::{
    CLIP_MASK, Dvars, Explosion, Ragdoll, RagdollDef, State as RagState, Sweep,
};
use vcod_common::animclock::Times;
use vcod_common::playerpose::{BG_SWING_SPEED, LeanDvars};

use crate::cod4_fly::WeaponModels;
use crate::play::cod4::BodyView;

/// The bone a held weapon hangs off: `cg_weaponrightbone`'s default, as
/// `CG_AttachWeapon` attaches it.
const WEAPON_TAG: &str = "tag_weapon_right";

/// `eFlags` bit of a corpse whose death anim still plays
/// (`BG_RunLerpFrameRate`'s new-anim flag), the same bit on CoD2 and CoD4.
const FRESH_CORPSE: i32 = vcod_cod2::player::eflags::FRESH_CORPSE;

/// A posed body's held weapon tags in the world: `tag_flash`, where its
/// muzzle flash plays, and `tag_brass`, where its shells eject
/// (`CG_FireWeapon`, `CG_EjectWeaponBrass`, cod4-effects.md 10).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WeaponTags {
    pub flash: Option<(Vec3, Quat)>,
    pub brass: Option<(Vec3, Quat)>,
}

/// The gun a gunner rides, as the placement reads it: its frame, its
/// model's `tag_aim` and `tag_weapon` at the bind, and its weapon's
/// `animHorRotateInc`.
#[derive(Clone, Debug)]
pub struct GunInfo {
    pub frame: vcod_cod2::player::GunFrame,
    pub tags: (Vec3, Vec3),
    pub rotate_inc: f32,
}

struct Body {
    character: Character,
    body: PlayerBody,
    /// The render time it was last drawn; its clips hold still while away.
    seen_ms: i32,
    /// A corpse's client and its `eFlags` teleport bit when it was last
    /// reset, which a body queue slot's reuse flips.
    corpse: Option<(i32, i32)>,
    /// A corpse's ragdoll once its entity took a ragdoll type.
    ragdoll: Rag,
    /// A corpse's bones in the world as last drawn, and when: the
    /// ragdoll's starting velocities.
    last_world: Option<(Vec<(Vec3, Quat)>, i32)>,
    /// The rig last drawn: its held weapon's world model, the rig (`None`
    /// when it did not build) and its `tag_flash` and `tag_brass` bones.
    rig: Option<HeldRig>,
    /// The ragdoll's skins as last drawn and what they were drawn from:
    /// its version, the rig's address and the entity's transform. A
    /// ragdoll steps once in several frames at a high frame rate and an
    /// asleep one never, so most frames draw these again
    /// (cod4-client-perf.md 38).
    rag_skins: Option<(RagSkinsKey, Skins)>,
}

type RagSkinsKey = (u64, usize, [u32; 16]);
/// Each rig model's name and skin matrices.
type Skins = Vec<(String, Vec<glam::Mat4>)>;

type HeldRig = (Option<String>, Option<(Rc<PlayerRig>, [Option<usize>; 2])>);

/// A corpse's ragdoll: not started, running or asleep, or refused (the
/// tunnel test, a def bone missing, `ragdoll_max_simulating` reached at
/// its start), whose corpse then keeps its death anim.
#[derive(Default)]
enum Rag {
    #[default]
    None,
    On(Box<Ragdoll>),
    Refused,
}

/// What a corpse's ragdoll needs: the def `ragdoll.cfg` built, the
/// level's clip map and the dvars.
#[derive(Clone)]
pub struct RagdollWorld {
    pub def: Arc<RagdollDef>,
    pub clip: Arc<vcod_cod4::collision::ClipMap>,
    pub dvars: Dvars,
}

/// `eFlags`' teleport bit, which `G_SpawnPlayerClone` flips on each reuse
/// of a body queue slot.
const TELEPORT: i32 = 0x2;

/// A fresh corpse's models: its client's, less any attachment on
/// `J_Spine4` (cod4-player-anims.md 9; INFERRED, not yet confirmed on 1.7).
fn corpse_character(c: &Character) -> Character {
    let mut c = c.clone();
    c.attachments.retain(|(_, tag)| {
        !tag.as_deref()
            .is_some_and(|t| t.eq_ignore_ascii_case("j_spine4"))
    });
    c
}

/// One session's bodies: the anim set, the rigs by character (weapon
/// included) and one [`Body`] per entity.
pub struct Cod4Players {
    /// `None` until first needed; `Some(None)` once the load failed.
    anims: Option<Option<PlayerAnimSet>>,
    rigs: HashMap<Character, Option<Rc<PlayerRig>>>,
    /// The rigs' meshes by part, shared across every weapon and head a
    /// body is seen with (cod4-client-perf.md 23).
    parts: RigParts,
    bodies: HashMap<u32, Body>,
    pub swing_speed: f32,
    /// The server's lean dvars, which CoD4's controllers read.
    pub lean: LeanDvars,
    /// The last frame's render time, for the next frame's step.
    last_ms: Option<i32>,
    /// The last [`Self::posed`]'s weapon tags by entity number.
    pub tags: HashMap<u32, WeaponTags>,
    /// Set by the caller; without it corpses only play their death anims.
    pub ragdoll: Option<RagdollWorld>,
    /// The blasts' jitter draws.
    seed: u32,
}

impl Default for Cod4Players {
    fn default() -> Self {
        Cod4Players {
            anims: None,
            rigs: HashMap::new(),
            parts: RigParts::new(),
            bodies: HashMap::new(),
            swing_speed: BG_SWING_SPEED,
            lean: LeanDvars::default(),
            last_ms: None,
            tags: HashMap::new(),
            ragdoll: None,
            seed: 1,
        }
    }
}

impl Cod4Players {
    /// The anim set loaded off `src`: how many wire indices the tree has
    /// (0 when it failed to load).
    #[cfg(test)]
    pub fn anim_count(&mut self, src: &BodySource) -> usize {
        self.set(src).map_or(0, |s| s.anims.index.len())
    }

    /// The anim wire value `wire` plays, `None` past the tree.
    #[cfg(test)]
    pub fn anim_name(&mut self, src: &BodySource, wire: i32) -> Option<String> {
        Some(self.set(src)?.anims.name(wire)?.to_string())
    }

    /// Each legs anim wire value's movetype bits
    /// ([`vcod_cod4::pmove::anim::movetype_mask`]), the toggle bit off;
    /// `None` without the anim set.
    pub fn legs_movetypes(&mut self, src: &BodySource) -> Option<std::sync::Arc<[u64]>> {
        let a = &self.set(src)?.anims;
        Some(
            (0..a.index.len())
                .map(|w| {
                    a.name(w as i32)
                        .map_or(0, |n| vcod_cod4::pmove::anim::movetype_mask(&a.script, n))
                })
                .collect(),
        )
    }

    /// The wire value of each legs anim a `fireweapon` clause's `both` or
    /// `legs` line puts on (the grenade throws'); `None` without the anim
    /// set.
    #[cfg(test)]
    pub fn fire_event_legs(&mut self, src: &BodySource) -> Option<Vec<u32>> {
        let a = &self.set(src)?.anims;
        let block = a.script.event("fireweapon")?;
        Some(
            block
                .clauses
                .iter()
                .flat_map(|c| &c.legs)
                .filter_map(|r| Some(a.wire_of(&r.name)? as u32))
                .collect(),
        )
    }

    /// `Ragdoll_ExplosionEvent` (cod4-player-anims.md 10.1): `e` pushes
    /// every corpse whose ragdoll runs or sleeps; how many it reached.
    pub fn explode(&mut self, e: &Explosion) -> usize {
        let seed = &mut self.seed;
        // `flrand(-1, 1)` over the MSVC `rand()` 1.7 draws from.
        let mut rand = || {
            *seed = seed.wrapping_mul(0x343fd).wrapping_add(0x269ec3);
            ((*seed >> 16) & 0x7fff) as f32 / 32767.0 * 2.0 - 1.0
        };
        self.bodies
            .values_mut()
            .filter_map(|b| match &mut b.ragdoll {
                Rag::On(r) => Some(r),
                _ => None,
            })
            .map(|r| r.explode(e, &mut rand))
            .filter(|&hit| hit)
            .count()
    }

    /// Body `n`'s ragdoll, once one runs.
    #[cfg(test)]
    pub fn ragdoll_of(&self, n: u32) -> Option<&Ragdoll> {
        match &self.bodies.get(&n)?.ragdoll {
            Rag::On(r) => Some(r),
            _ => None,
        }
    }

    /// Whether body `n`'s ragdoll was refused.
    #[cfg(test)]
    pub fn ragdoll_refused(&self, n: u32) -> bool {
        self.bodies
            .get(&n)
            .is_some_and(|b| matches!(b.ragdoll, Rag::Refused))
    }

    /// The models body `n` is posed with.
    #[cfg(test)]
    pub fn character(&self, n: u32) -> Option<&Character> {
        self.bodies.get(&n).map(|b| &b.character)
    }

    fn set(&mut self, src: &BodySource) -> Option<&mut PlayerAnimSet> {
        self.anims
            .get_or_insert_with(|| {
                PlayerAnimSet::load(src)
                    .inspect_err(|e| log::warn!("cod4 player anims: {e:#}, players stay unposed"))
                    .ok()
            })
            .as_mut()
    }

    /// Steps and poses `bodies` (entity number, its view) at
    /// `render_time`; `snapshot_ms` is the newest snapshot's time, the
    /// clock a body's anim rate reads. Each posed body comes back with its
    /// index in `bodies`; one whose rig does not build is left out. Bodies
    /// not in `bodies` are forgotten. A gunner's gun in `guns` (parallel
    /// to `bodies`) puts his body on it.
    #[allow(clippy::too_many_arguments)]
    pub fn posed(
        &mut self,
        src: &BodySource,
        weapons: &HashMap<String, WeaponModels>,
        bodies: &[(u32, &BodyView)],
        guns: &[Option<GunInfo>],
        render_time: i32,
        snapshot_ms: i32,
    ) -> Vec<(usize, Rc<PlayerRig>, PosedPlayer)> {
        let frametime_ms = self
            .last_ms
            .map_or(0, |t| render_time.wrapping_sub(t).clamp(0, 1000));
        self.last_ms = Some(render_time);
        self.tags.clear();
        let swing_speed = self.swing_speed;
        if self.set(src).is_none() {
            return Vec::new();
        }
        // `CG_ResetEntity`'s corpse arm (cod4-player-anims.md 9): a corpse
        // new to the snapshot, or whose slot was reused, copies its
        // client's models then and keeps them; a fresh one also takes over
        // the client's live body, its death anim playing on from there.
        for (n, v) in bodies.iter().filter(|(_, v)| v.corpse) {
            let client = v.frame.client_num;
            let teleport = v.frame.eflags & TELEPORT;
            let old = self.bodies.get(n);
            if old.is_some_and(|b| b.corpse.is_some_and(|(_, t)| t == teleport)) {
                continue;
            }
            let character = corpse_character(&v.character);
            let body = if v.frame.eflags & FRESH_CORPSE != 0 {
                let live = u32::try_from(client).ok().and_then(|c| self.bodies.get(&c));
                live.map(|b| (character, b.body.clone(), b.seen_ms))
            } else {
                old.map(|b| {
                    let same = b.corpse.is_some_and(|(c, _)| c == client);
                    let character = if same { b.character.clone() } else { character };
                    (character, b.body.clone(), b.seen_ms)
                })
            };
            let (character, body, seen_ms) = body.unwrap_or_else(|| {
                (
                    corpse_character(&v.character),
                    PlayerBody::default(),
                    render_time,
                )
            });
            self.bodies.insert(
                *n,
                Body {
                    character,
                    body,
                    seen_ms,
                    corpse: Some((client, teleport)),
                    ragdoll: Rag::None,
                    last_world: None,
                    rig: None,
                    rag_skins: None,
                },
            );
        }
        self.bodies
            .retain(|n, _| bodies.iter().any(|(b, _)| b == n));
        let mut running = self
            .bodies
            .values()
            .filter(|b| matches!(&b.ragdoll, Rag::On(r) if r.state == RagState::Running))
            .count();
        let mut out = Vec::new();
        for (i, (n, v)) in bodies.iter().enumerate() {
            let body = self.bodies.entry(*n).or_insert_with(|| Body {
                character: v.character.clone(),
                body: PlayerBody::default(),
                seen_ms: render_time,
                corpse: None,
                ragdoll: Rag::None,
                last_world: None,
                rig: None,
                rag_skins: None,
            });
            // A corpse keeps the models it was copied with; a live body
            // follows its client state.
            if body.corpse.is_none() && body.character != v.character {
                body.character = v.character.clone();
                body.body = PlayerBody::default();
                body.rig = None;
            }
            let held = v
                .weapon
                .as_ref()
                .and_then(|w| weapons.get(w)?.world.as_ref());
            // The rig of the models and the weapon held last frame, else
            // the one the models armed with this weapon key.
            let rig = match &body.rig {
                Some((w, rig)) if w.as_ref() == held => rig.clone(),
                _ => {
                    let mut armed = body.character.clone();
                    if let Some(model) = held {
                        armed
                            .attachments
                            .insert(0, (model.clone(), Some(WEAPON_TAG.to_string())));
                    }
                    let rig = self
                        .rigs
                        .entry(armed.clone())
                        .or_insert_with(|| {
                            PlayerRig::assemble_with(src, &armed, &mut self.parts)
                                .inspect_err(|e| log::warn!("cod4 player {}: {e:#}", armed.body))
                                .ok()
                                .map(Rc::new)
                        })
                        .clone()
                        .map(|rig| {
                            let bone = |name: &str| {
                                rig.skeleton
                                    .bones()
                                    .iter()
                                    .position(|b| b.name.eq_ignore_ascii_case(name))
                            };
                            let tags = [bone("tag_flash"), bone("tag_brass")];
                            (rig, tags)
                        });
                    body.rig = Some((held.cloned(), rig.clone()));
                    rig
                }
            };
            let Some((rig, [flash, brass])) = rig else {
                continue;
            };
            let body = self.bodies.get_mut(n).expect("inserted above");
            let unseen = render_time
                .wrapping_sub(frametime_ms)
                .wrapping_sub(body.seen_ms);
            if unseen > 0 {
                body.body.pause(unseen);
            }
            body.seen_ms = render_time;
            body.body.cod4_lean = Some(self.lean);
            let Some(set) = self.anims.as_mut().and_then(|s| s.as_mut()) else {
                return out;
            };
            let at = Times {
                clock_ms: render_time,
                snapshot_ms,
            };
            body.body
                .advance(&set.anims, &v.frame, at, frametime_ms, swing_speed);
            // `CG_PlayerTurretPositionAndBlend`: a gunner stands where his
            // gun's placement puts him, turned with it, the group's leaves
            // weighted as the server's; the height stays the server's
            // trace (cod4-gsc.md 9.2).
            let mut frame = v.frame;
            let placed = guns.get(i).and_then(Option::as_ref).and_then(|g| {
                vcod_cod2::player::gunner_placement(
                    vcod_common::turretpose::GunnerRule::Iw3,
                    set,
                    src,
                    &v.frame,
                    &g.frame,
                    g.tags,
                    g.rotate_inc,
                )
            });
            if let Some(g) = placed.as_ref().filter(|g| g.origin.is_finite()) {
                frame.origin = Vec3::new(g.origin.x, g.origin.y, frame.origin.z);
                frame.angles = [0.0, g.yaw(), 0.0];
            }
            let turret = placed.as_ref().map(|g| (g.node, g.rows.as_slice()));
            let rag = self
                .ragdoll
                .as_ref()
                .filter(|w| w.dvars.enable && v.ragdoll);
            // A corpse whose ragdoll runs or sleeps draws the ragdoll's
            // bones alone: the death anim's pose under it is never drawn
            // (cod4-client-perf.md 31), and a corpse fires no tag.
            if v.corpse && rag.is_some() && matches!(body.ragdoll, Rag::On(_)) {
                let mut drawn = PosedPlayer {
                    transform: frame.transform(),
                    models: Vec::new(),
                };
                let r = ragdolled(
                    body,
                    rag,
                    &rig,
                    &drawn,
                    &[],
                    &mut running,
                    frametime_ms,
                    render_time,
                );
                drawn.models = r.expect("a ragdoll on with its world draws");
                out.push((i, rig.clone(), drawn));
                continue;
            }
            let pose = body.body.pose(&rig, set, src, &frame, render_time, turret);
            let mut drawn = posed(&rig, &pose, &frame);
            if v.corpse {
                let worlds = pose.bone_worlds(&rig.skeleton);
                let body = self.bodies.get_mut(n).expect("inserted above");
                if let Some(r) = ragdolled(
                    body,
                    rag,
                    &rig,
                    &drawn,
                    &worlds,
                    &mut running,
                    frametime_ms,
                    render_time,
                ) {
                    drawn.models = r;
                }
            }
            let (_, turn, at) = drawn.transform.to_scale_rotation_translation();
            let tag = |b: Option<usize>| {
                let (p, q) = pose.bone_world(&rig.skeleton, b?);
                Some((turn * p + at, turn * q))
            };
            self.tags.insert(
                *n,
                WeaponTags {
                    flash: tag(flash),
                    brass: tag(brass),
                },
            );
            out.push((i, rig.clone(), drawn));
        }
        out
    }
}

/// A corpse's skins under its ragdoll, its world bones kept for the next
/// frame's start: the ragdoll starts on the first frame its entity reads
/// a ragdoll type with `ragdoll_enable` on (fewer than
/// `ragdoll_max_simulating` running), steps by the frame time and poses
/// every bone; a corpse whose entity goes back to another type drops it.
/// `None` draws the death anim's pose.
#[allow(clippy::too_many_arguments)]
fn ragdolled(
    body: &mut Body,
    rag: Option<&RagdollWorld>,
    rig: &PlayerRig,
    drawn: &PosedPlayer,
    model_worlds: &[(Vec3, Quat)],
    running: &mut usize,
    frametime_ms: i32,
    render_time: i32,
) -> Option<Skins> {
    let (_, turn, at) = drawn.transform.to_scale_rotation_translation();
    let world: Vec<(Vec3, Quat)> = model_worlds
        .iter()
        .map(|&(p, q)| (turn * p + at, turn * q))
        .collect();
    let last = body.last_world.replace((world, render_time));
    let Some(w) = rag else {
        body.ragdoll = Rag::None;
        return None;
    };
    let clip = w.clip.clone();
    let mut trace = move |a: Vec3, b: Vec3, r: f32| {
        let e = Vec3::splat(r);
        let t = clip.trace(a, b, -e, e, CLIP_MASK);
        Sweep {
            fraction: t.fraction,
            normal: t.normal,
            startsolid: t.startsolid,
        }
    };
    if matches!(body.ragdoll, Rag::None) {
        let now = &body.last_world.as_ref().expect("just set").0;
        let before = last
            .as_ref()
            .map(|(b, t)| (b.as_slice(), render_time.wrapping_sub(*t) as f32));
        body.rag_skins = None;
        body.ragdoll = match (*running < w.dvars.max_simulating)
            .then(|| Ragdoll::new(&w.def, &rig.skeleton, now, before, w.dvars, &mut trace))
            .flatten()
        {
            Some(r) => {
                *running += 1;
                Rag::On(Box::new(r))
            }
            None => Rag::Refused,
        };
    }
    let Rag::On(r) = &mut body.ragdoll else {
        return None;
    };
    r.step(frametime_ms as f32, &mut trace);
    let key = (
        r.version(),
        std::ptr::from_ref(rig) as usize,
        drawn.transform.to_cols_array().map(f32::to_bits),
    );
    if let Some((k, skins)) = &body.rag_skins
        && *k == key
    {
        return Some(skins.clone());
    }
    let inv = turn.inverse();
    let local: Vec<(Vec3, Quat)> = r
        .pose()
        .into_iter()
        .map(|(p, q)| (inv * (p - at), inv * q))
        .collect();
    let skins: Vec<_> = rig
        .models
        .iter()
        .enumerate()
        .map(|(i, (name, _))| (name.clone(), rig.skeleton.skin_matrices(&local, i)))
        .collect();
    body.rag_skins = Some((key, skins.clone()));
    Some(skins)
}

/// The ragdoll push a fired event carries (`CG_EntityEvent`'s
/// `Ragdoll_ExplosionEvent` calls, iw3mp.exe 0x436b3e..0x437359,
/// cod4-player-anims.md 10.1): a grenade's explode reaches its weapon's
/// `explosionRadius` falling off from the middle, a rocket's or a
/// custom one's the whole radius at full force; the script
/// `physicsExplosionSphere` and `..Cylinder` carry the outer radius as
/// the parm and the inner and the magnitude in `lerp.u`, `physicsJolt`
/// the inner and the impulse there.
pub fn explosion_of(
    f: &crate::play::cod4::Fired,
    weapons: &[String],
    defs: &HashMap<String, crate::cod4_audio::WeaponAv>,
) -> Option<Explosion> {
    let word = |k: usize| {
        f.state.as_ref().map_or(0.0, |e| {
            crate::play::cod4::ent_f32(e, &format!("lerp.u.anonymous.data[{k}]"))
        })
    };
    let radius = || {
        crate::play::cod4::weapon_name(weapons, f.weapon)
            .and_then(|w| defs.get(w))
            .map(|w| w.explosion_radius)
            .filter(|r| *r > 0.0)
    };
    let mut e = Explosion {
        origin: f.origin,
        inner: 0.0,
        outer: 0.0,
        cylinder: false,
        impulse: Vec3::ZERO,
        scale: 1.0,
    };
    match f.name {
        "EV_GRENADE_EXPLODE" => e.outer = radius()?,
        "EV_ROCKET_EXPLODE"
        | "EV_ROCKET_EXPLODE_NOMARKS"
        | "EV_CUSTOM_EXPLODE"
        | "EV_CUSTOM_EXPLODE_NOMARKS" => {
            e.outer = radius()?;
            e.inner = e.outer;
        }
        "EV_PHYS_EXPLOSION_SPHERE" | "EV_PHYS_EXPLOSION_CYLINDER" => {
            e.outer = f.parm as i32 as f32;
            e.inner = word(0);
            e.scale = word(1);
            e.cylinder = f.name == "EV_PHYS_EXPLOSION_CYLINDER";
        }
        "EV_PHYS_EXPLOSION_JOLT" => {
            e.outer = f.parm as i32 as f32;
            e.inner = word(0);
            e.impulse = Vec3::new(word(1), word(2), word(3));
            e.cylinder = true;
        }
        _ => return None,
    }
    (e.outer > 0.0).then_some(e)
}
