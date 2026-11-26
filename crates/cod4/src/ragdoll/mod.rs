//! CoD4 1.7's client ragdoll: the defs `ragdoll.cfg` builds (`iw_00.iwd`,
//! exec'd at start up), the `ragdoll_*` dvars' 1.7 defaults, and the
//! corpses' bodies, ODE capsules and joints in world 2 stepped against
//! the clip map as iw3mp.exe steps them ([`pool`]; docs/research/
//! cod4-player-anims.md 10, 10.7).

use glam::Vec3;

pub mod iw;
pub mod pool;

pub use pool::{Clip, DObj, Ragdolls, Record, SlotId, State};

/// `ragdollDefs[2]`.
pub const RAGDOLL_DEFS: usize = 2;
/// 1.7's caps on a def's bones, joints, limit axes a joint, self pairs and
/// base lerp bones: a line past one is dropped (`cmp eax,0xe` at 0x5bab01
/// and kin, cod4-player-anims.md 10).
const MAX_BONES: usize = 14;
const MAX_JOINTS: usize = 28;
const MAX_LIMITS: usize = 3;
const MAX_SELF_PAIRS: usize = 33;
const MAX_BASE_LERP: usize = 9;
/// The def a corpse binds (`Ragdoll_CreateRagdollForDObj` binds 1 whatever
/// it is asked).
pub const MP_DEF: usize = 1;
/// `PHYS_WORLD_CLIPMASK`, the mask of `Ragdoll_BoneTrace` (iw3mp.exe
/// 0x5be2e0) and of the bodies' world collision.
pub const CLIP_MASK: u32 = 0x280_6c91;

/// One `ragdoll_limit` axis (def joint +0xc, +0x30, +0x3c, +0x48): the
/// axis in the child body's frame, a leading `-` negating it; the stops in
/// radians, clamped to -pi..pi; the friction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limit {
    pub axis: [f32; 3],
    pub lo: f32,
    pub hi: f32,
    pub friction: f32,
}

/// `ragdoll_bone <def> <p0> <p1> <radius> <cog lerp> <mass> <friction>
/// <parent> <mirror> <geom>`.
#[derive(Clone, Debug, PartialEq)]
pub struct BoneDef {
    pub p0: String,
    pub p1: String,
    pub radius: f32,
    pub cog_lerp: f32,
    pub mass: f32,
    pub friction: f32,
    pub parent: Option<usize>,
    pub mirror: bool,
}

/// `ragdoll_joint`'s type (def joint +4, the names at 0x71e708).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JointKind {
    /// No ODE joint.
    None,
    Hinge,
    /// A ball and an Euler angular motor.
    Swivel,
}

/// `ragdoll_joint <def> <bone> <type>` (0x54 bytes from def +0x558) and
/// the `ragdoll_limit`s on it in the cfg's order, at most 3.
#[derive(Clone, Debug, PartialEq)]
pub struct JointDef {
    pub bone: usize,
    pub kind: JointKind,
    pub limits: Vec<Limit>,
}

/// `ragdoll_baselerp_bone <def> <bone> <msec> [<ragdoll bone>]` (0x20
/// bytes from def +0x434, 0x5bb130): the msec clamped to 100..6000, the
/// ragdoll bone it eases under 0 when left out.
#[derive(Clone, Debug, PartialEq)]
pub struct BaseLerp {
    pub bone: String,
    pub msec: i32,
    pub ragdoll_bone: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RagdollDef {
    pub bones: Vec<BoneDef>,
    pub joints: Vec<JointDef>,
    pub self_pairs: Vec<(usize, usize)>,
    pub base_lerp: Vec<BaseLerp>,
}

/// `ragdoll.cfg`'s commands into the two defs. A malformed line is logged
/// and skipped, as each command's own checks refuse it
/// (`Ragdoll: Unknown bone geom type %s` and kin, iw3mp.exe strings).
pub fn parse_cfg(text: &str) -> [RagdollDef; RAGDOLL_DEFS] {
    let mut defs: [RagdollDef; RAGDOLL_DEFS] = Default::default();
    for (n, line) in text.lines().enumerate() {
        let line = line.split("//").next().unwrap_or("");
        let t: Vec<&str> = line.split_whitespace().collect();
        let Some((&cmd, args)) = t.split_first() else {
            continue;
        };
        if let Err(e) = apply(&mut defs, cmd, args) {
            log::warn!("ragdoll.cfg:{}: {cmd}: {e}", n + 1);
        }
    }
    defs
}

fn apply(defs: &mut [RagdollDef; RAGDOLL_DEFS], cmd: &str, a: &[&str]) -> Result<(), String> {
    let arg = |i: usize| a.get(i).copied().ok_or(format!("missing arg {i}"));
    let float = |i: usize| -> Result<f32, String> {
        arg(i)?
            .parse()
            .map_err(|_| format!("arg {i} is not a number"))
    };
    let int = |i: usize| -> Result<i32, String> {
        arg(i)?
            .parse()
            .map_err(|_| format!("arg {i} is not an integer"))
    };
    let cmd = cmd.to_ascii_lowercase();
    if !cmd.starts_with("ragdoll_") {
        return Ok(());
    }
    let d = usize::try_from(int(0)?)
        .ok()
        .filter(|&d| d < RAGDOLL_DEFS)
        .ok_or("no such def")?;
    let def = &mut defs[d];
    let full = |what: &str, n: usize, max: usize| {
        if n >= max {
            Err(format!("more than {max} {what}"))
        } else {
            Ok(())
        }
    };
    match cmd.as_str() {
        "ragdoll_clear" => *def = RagdollDef::default(),
        "ragdoll_bone" => {
            full("bones", def.bones.len(), MAX_BONES)?;
            let parent = int(7)?;
            let parent = match usize::try_from(parent) {
                Ok(p) if p < def.bones.len() => Some(p),
                Ok(_) => return Err("child bones must follow their parents".into()),
                Err(_) => None,
            };
            if !arg(9)?.eq_ignore_ascii_case("capsule") && !arg(9)?.eq_ignore_ascii_case("box") {
                return Err(format!("unknown bone geom type {}", arg(9)?));
            }
            def.bones.push(BoneDef {
                p0: arg(1)?.to_string(),
                p1: arg(2)?.to_string(),
                radius: float(3)?,
                cog_lerp: float(4)?,
                mass: float(5)?,
                friction: float(6)?,
                parent,
                mirror: int(8)? != 0,
            });
        }
        "ragdoll_selfpair" => {
            full("self pairs", def.self_pairs.len(), MAX_SELF_PAIRS)?;
            let (x, y) = (int(1)?, int(2)?);
            let ok = |v: i32| usize::try_from(v).ok().filter(|&v| v < def.bones.len());
            match (ok(x), ok(y)) {
                (Some(x), Some(y)) => def.self_pairs.push((x, y)),
                _ => return Err("bad self collision pair bone".into()),
            }
        }
        "ragdoll_joint" => {
            full("joints", def.joints.len(), MAX_JOINTS)?;
            // 0x5badb0 refuses bone 0 and one past the count; vcod also a
            // bone with no parent, whose joint 1.7's build would read
            // through a null parent (0x5bcb4f).
            let bone = usize::try_from(int(1)?)
                .ok()
                .filter(|&b| b < def.bones.len() && def.bones[b].parent.is_some())
                .ok_or("joint references a bone with no parent")?;
            let kind = match arg(2)?.to_ascii_lowercase().as_str() {
                "none" => JointKind::None,
                "hinge" => JointKind::Hinge,
                "swivel" => JointKind::Swivel,
                k => return Err(format!("unknown joint type {k}")),
            };
            def.joints.push(JointDef {
                bone,
                kind,
                limits: Vec::new(),
            });
        }
        "ragdoll_limit" => {
            let j = usize::try_from(int(1)?)
                .ok()
                .filter(|&j| j < def.joints.len())
                .ok_or("limit on a nonexistent joint")?;
            let word = arg(2)?;
            let (neg, name) = match word.strip_prefix('-') {
                Some(n) => (true, n),
                None => (false, word),
            };
            // The table at 0x71e6b0; `-` negates each part (0x5ba9e5).
            let mut axis = match name.to_ascii_lowercase().as_str() {
                "x" => [1.0, 0.0, 0.0],
                "y" => [0.0, 1.0, 0.0],
                "z" => [0.0, 0.0, 1.0],
                "n" => [0.0, 0.0, 0.0],
                k => return Err(format!("unknown axis {k}")),
            };
            if neg {
                axis = axis.map(|v: f32| -v);
            }
            if def.joints[j].limits.len() >= MAX_LIMITS {
                return Err(format!("more than {MAX_LIMITS} limit axes for joint {j}"));
            }
            // Degrees times the double 0x70b2d0, clamped by 0x401070.
            let pi = std::f32::consts::PI;
            let rad = |v: f32| ((v as f64 * 0.017_453_292_384_743_69) as f32).clamp(-pi, pi);
            def.joints[j].limits.push(Limit {
                axis,
                friction: float(3)?,
                lo: rad(float(4)?),
                hi: rad(float(5)?),
            });
        }
        "ragdoll_baselerp_bone" => {
            full("base lerp bones", def.base_lerp.len(), MAX_BASE_LERP)?;
            // `atoi`: a word that is no number reads 0.
            let atoi = |i: usize| a.get(i).and_then(|w| w.parse::<i32>().ok()).unwrap_or(0);
            def.base_lerp.push(BaseLerp {
                bone: arg(1)?.to_string(),
                msec: atoi(2).clamp(100, 6000),
                ragdoll_bone: usize::try_from(atoi(3)).unwrap_or(0),
            })
        }
        // `ragdoll_pin_bone` is a single-player def's; MP's cfg has none.
        _ => {}
    }
    Ok(())
}

/// The `ragdoll_*` dvars the ragdoll reads, at 1.7's registered defaults
/// (iw3mp.exe 0x5ba330); the `phys_*` ones are `ode::world::PhysDvars`
/// and `ode::collide::Dvars`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dvars {
    /// `ragdoll_enable` (1, archived): off, a corpse plays its death anim
    /// to its end and stays there, and nothing steps (0x5becb0).
    pub enable: bool,
    /// `ragdoll_max_life` (4500 ms): a ragdoll simulating longer holds its
    /// pose (0x5be890).
    pub max_life_ms: i32,
    /// `ragdoll_max_simulating` (16): no ragdoll starts or wakes past it.
    pub max_simulating: usize,
    /// `ragdoll_jointlerp_time` (3000 ms): the joints' friction eases over
    /// it (`Ragdoll_UpdateFriction`, 0x5be780).
    pub jointlerp_ms: i32,
    /// `ragdoll_rotvel_scale` (1): the start's angular velocity times it.
    pub rotvel_scale: f32,
    /// `ragdoll_explode_force` (18000) and `ragdoll_explode_upbias` (0.8):
    /// a blast's push, split over the bodies, and its lift.
    pub explode_force: f32,
    pub explode_upbias: f32,
}

impl Default for Dvars {
    fn default() -> Self {
        Dvars {
            enable: true,
            max_life_ms: 4500,
            max_simulating: 16,
            jointlerp_ms: 3000,
            rotvel_scale: 1.0,
            explode_force: 18000.0,
            explode_upbias: 0.8,
        }
    }
}

/// What a sphere sweep met: the fraction reached, the contact normal, and
/// whether it started in solid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sweep {
    pub fraction: f32,
    pub normal: Vec3,
    pub startsolid: bool,
}

/// A sweep of a sphere of the given radius from the first point to the
/// second through the world (a zero radius is a point).
pub type Trace<'a> = dyn FnMut(Vec3, Vec3, f32) -> Sweep + 'a;

/// One blast's push on the ragdolls: an `EV_*_EXPLODE` or a script
/// `physicsExplosion*` (cod4-player-anims.md 10.1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Explosion {
    pub origin: Vec3,
    pub inner: f32,
    pub outer: f32,
    /// Measures the body's distance in the horizontal plane and pushes
    /// level (`physicsExplosionCylinder`, `physicsJolt`).
    pub cylinder: bool,
    /// `physicsJolt`'s direction; zero pushes away from the origin.
    pub impulse: Vec3,
    pub scale: f32,
}

/// How far a blast reaches, as [`Ragdolls::explode`] measures it.
struct Reach {
    outer_sq: f32,
    inner_sq: f32,
    /// `1 / (outer^2 - inner^2)`, 0 for an empty band.
    inv_band: f32,
    level: bool,
}

impl Reach {
    /// `None` for a blast with no outer radius.
    fn of(e: &Explosion) -> Option<Reach> {
        if e.outer <= 0.0 {
            return None;
        }
        let (outer_sq, inner_sq) = (e.outer * e.outer, e.inner * e.inner);
        Some(Reach {
            outer_sq,
            inner_sq,
            inv_band: if inner_sq < outer_sq {
                1.0 / (outer_sq - inner_sq)
            } else {
                0.0
            },
            level: e.cylinder,
        })
    }

    /// A cylinder's offsets are measured and pushed level.
    fn level(&self, mut v: Vec3) -> Vec3 {
        if self.level {
            v.z = 0.0;
        }
        v
    }

    /// The share of the blast a bone `d_sq` squared units off takes: all
    /// of it inside the inner radius, falling to 0 at the outer, `None`
    /// at or past it.
    fn falloff(&self, d_sq: f32) -> Option<f32> {
        if d_sq >= self.outer_sq {
            None
        } else if self.inner_sq < d_sq {
            Some((self.outer_sq - d_sq) * self.inv_band)
        } else {
            Some(1.0)
        }
    }
}

#[cfg(test)]
mod tests;
