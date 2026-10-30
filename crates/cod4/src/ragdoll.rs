//! CoD4 1.7's client ragdoll: the defs `ragdoll.cfg` builds (`iw_00.iwd`,
//! exec'd at start up), the `ragdoll_*` and `phys_*` dvars' 1.7 defaults,
//! and a corpse's bones simulated once its entity takes a ragdoll
//! trajectory (docs/research/cod4-player-anims.md 10).
//!
//! Retail runs each def bone as an ODE capsule body, its joints as hinges
//! and angular motors. This is a position-based stand-in over the same
//! data: each bone's two joints are points carrying half its mass and its
//! radius, the bone lengths and the attachments are distance links, each
//! joint's limits clamp the child's swing in its parent's frame, the self
//! collision pairs push capsules apart, and every point sweeps the world's
//! clip map as a sphere. What retail and this share is the def, the
//! dvars, gravity, the clip mask, the tunnel test, the sleep rule and
//! the step (17 ms, at most two a frame); the integrator, the iteration
//! counts and the damping are vcod's.

use glam::{Mat3, Quat, Vec3};
use vcod_common::skeleton::Skeleton;

/// `ragdollDefs[2]`.
pub const RAGDOLL_DEFS: usize = 2;
/// The def a corpse binds (`Ragdoll_CreateRagdollForDObj` binds 1 whatever
/// it is asked).
pub const MP_DEF: usize = 1;
/// `PHYS_WORLD_CLIPMASK`, the mask of `Ragdoll_BoneTrace` (iw3mp.exe
/// 0x5be2e0) and of the bodies' world collision.
pub const CLIP_MASK: u32 = 0x280_6c91;
/// The ragdoll world's step, ms: 1.7's `g_phys_msecStep[2]`, 17 at
/// most 32 awake ODE bodies (iw3mp.exe 0x71e7a0; cod4-client-perf.md 36).
/// vcod's bodies are not ODE's, so it stays at the floor.
const STEP_MS: f32 = 17.0;
/// `Phys_RunToTime`'s steps a call (iw3mp.exe 0x5a6dfb).
const MAX_RUN_STEPS: usize = 2;
/// The longest step vcod takes: past it a hitch loses time, as the old
/// step cap did. vcod's; 1.7's has none.
const MAX_STEP_MS: f32 = 100.0;
/// Constraint rounds per step, each ending in a world collision pass, and
/// the constraint iterations in a round. vcod's.
const ROUNDS: usize = 4;
const ITERATIONS: usize = 4;

/// One `ragdoll_limit` axis: friction, low and high angle (degrees).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limit {
    pub friction: f32,
    pub min: f32,
    pub max: f32,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JointKind {
    Hinge,
    Swivel,
}

/// `ragdoll_joint <def> <bone> <type>` and the `ragdoll_limit`s on it, by
/// axis x, y, z.
#[derive(Clone, Debug, PartialEq)]
pub struct JointDef {
    pub bone: usize,
    pub kind: JointKind,
    pub limits: [Option<Limit>; 3],
}

/// `ragdoll_baselerp_bone <def> <bone> <msec> [<ragdoll bone>]`.
#[derive(Clone, Debug, PartialEq)]
pub struct BaseLerp {
    pub bone: String,
    pub msec: i32,
    pub body: Option<usize>,
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
    match cmd.as_str() {
        "ragdoll_clear" => *def = RagdollDef::default(),
        "ragdoll_bone" => {
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
            let (x, y) = (int(1)?, int(2)?);
            let ok = |v: i32| usize::try_from(v).ok().filter(|&v| v < def.bones.len());
            match (ok(x), ok(y)) {
                (Some(x), Some(y)) => def.self_pairs.push((x, y)),
                _ => return Err("bad self collision pair bone".into()),
            }
        }
        "ragdoll_joint" => {
            let bone = usize::try_from(int(1)?)
                .ok()
                .filter(|&b| b < def.bones.len() && def.bones[b].parent.is_some())
                .ok_or("joint references a bone with no parent")?;
            let kind = match arg(2)?.to_ascii_lowercase().as_str() {
                "hinge" => JointKind::Hinge,
                "swivel" => JointKind::Swivel,
                k => return Err(format!("unknown joint type {k}")),
            };
            def.joints.push(JointDef {
                bone,
                kind,
                limits: [None; 3],
            });
        }
        "ragdoll_limit" => {
            let j = usize::try_from(int(1)?)
                .ok()
                .filter(|&j| j < def.joints.len())
                .ok_or("limit on a nonexistent joint")?;
            let axis = match arg(2)?
                .trim_start_matches('-')
                .to_ascii_lowercase()
                .as_str()
            {
                "x" => 0,
                "y" => 1,
                "z" => 2,
                k => return Err(format!("unknown axis {k}")),
            };
            def.joints[j].limits[axis] = Some(Limit {
                friction: float(3)?,
                min: float(4)?,
                max: float(5)?,
            });
        }
        "ragdoll_baselerp_bone" => def.base_lerp.push(BaseLerp {
            bone: arg(1)?.to_string(),
            msec: int(2)?,
            body: a.get(3).and_then(|s| s.parse().ok()),
        }),
        // `ragdoll_pin_bone` is a single-player def's; MP's cfg has none.
        _ => {}
    }
    Ok(())
}

/// The dvars the ragdoll reads, at 1.7's registered defaults (iw3mp.exe
/// 0x5ba330 for `ragdoll_*`, 0x5a3120.. for `phys_*`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dvars {
    /// `ragdoll_enable` (1, archived): off, a corpse plays its death anim
    /// to its end and stays there.
    pub enable: bool,
    /// `ragdoll_max_life` (4500 ms): a body still moving then freezes.
    pub max_life_ms: i32,
    /// `ragdoll_max_simulating` (16): no new ragdoll starts past it.
    pub max_simulating: usize,
    /// `ragdoll_baselerp_time` (1000 ms).
    pub baselerp_ms: i32,
    /// `ragdoll_jointlerp_time` (3000 ms): the joints' friction falls from
    /// full to a tenth over it (`Ragdoll_UpdateFriction`).
    pub jointlerp_ms: i32,
    /// `ragdoll_self_collision_scale` (1.2).
    pub self_collision_scale: f32,
    /// `phys_gravity` (-800).
    pub gravity: f32,
    /// `phys_autoDisableLinear` (20) and `phys_autoDisableTime` (0.9 s): a
    /// body slower than this that long goes to sleep.
    pub sleep_speed: f32,
    pub sleep_ms: f32,
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
            baselerp_ms: 1000,
            jointlerp_ms: 3000,
            self_collision_scale: 1.2,
            gravity: -800.0,
            sleep_speed: 20.0,
            sleep_ms: 900.0,
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

/// How far a blast reaches, as [`Ragdoll::explode`] measures it.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Running,
    /// Asleep or past `ragdoll_max_life`: the pose holds (`BS_IDLE`).
    Idle,
}

#[derive(Clone, Debug)]
struct Point {
    pos: Vec3,
    prev: Vec3,
    /// Inverse mass.
    w: f32,
    radius: f32,
    /// The radius it sweeps with now: shrunk where the death anim left
    /// it inside the world, grown back once it is clear.
    swept: f32,
    friction: f32,
    /// The surface it met this step, and how far the collisions pushed
    /// it back out along its normal in all.
    contact: Option<(Vec3, f32)>,
}

#[derive(Clone, Debug)]
struct Body {
    p0: usize,
    p1: usize,
    radius: f32,
    /// The two points across it (its mirrored children's roots), whose
    /// line fixes its twist. Without them a bone under a parent keeps
    /// the twist it started with against the parent: its frame is the
    /// parent's turned by the shortest turn of its axis from where it
    /// started in the parent's frame, so it does not depend on the path
    /// the axis took. A root without them is carried by the shortest
    /// turn of its axis from step to step.
    side: Option<(usize, usize)>,
    /// The side basis to the bone's own frame.
    offset: Quat,
    /// Its axis at the last frame update, which a root's shortest turn
    /// carries from.
    axis: Vec3,
    /// Its def parent, its axis at the start in the parent's frame and
    /// its frame then in the parent's.
    parent: Option<usize>,
    rel_axis: Vec3,
    rel_rot: Quat,
    /// The p0 bone's world rotation now and at the start.
    rot: Quat,
    start_rot: Quat,
    start_p0: Vec3,
    /// `ragdoll_bone`'s mass, which a blast's push is divided by.
    mass: f32,
}

/// A joint's swing limit: the child's bind direction and its y and z axes
/// in the parent's frame, each axis's range in radians.
#[derive(Clone, Debug)]
struct Swing {
    parent: usize,
    child: usize,
    x: Vec3,
    y: Vec3,
    z: Vec3,
    y_lim: (f32, f32),
    z_lim: (f32, f32),
}

#[derive(Clone, Copy, Debug)]
struct Link {
    a: usize,
    b: usize,
    rest: f32,
}

#[derive(Clone, Copy, Debug)]
enum Owner {
    /// The p0 bone of this body, or (for the bones above the root body)
    /// carried by it.
    Body(usize),
    /// Rigid under its parent, its local rotation easing to its bind over
    /// the given ms when a `ragdoll_baselerp_bone` names it.
    Child(Option<i32>),
}

/// One corpse's ragdoll.
#[derive(Clone, Debug)]
pub struct Ragdoll {
    points: Vec<Point>,
    links: Vec<Link>,
    bodies: Vec<Body>,
    swings: Vec<Swing>,
    /// The self collision pairs and the closest each may come: their radii
    /// scaled by `ragdoll_self_collision_scale`, or less where the start
    /// pose had them closer.
    pairs: Vec<(usize, usize, f32)>,
    /// Per def bone: whether its segment crossed the world in the pose it
    /// started from.
    started_through: Vec<bool>,
    owner: Vec<Owner>,
    parents: Vec<i32>,
    /// Every skeleton bone's world pose at the start, and each child bone's
    /// start and bind local rotations.
    start: Vec<(Vec3, Quat)>,
    start_local: Vec<(Vec3, Quat)>,
    bind_local: Vec<Quat>,
    dvars: Dvars,
    pub state: State,
    msec: f32,
    still_ms: f32,
    acc_ms: f32,
    /// The last step's length, ms: a point's `pos - prev` moved over it.
    last_ms: f32,
    /// Bumped by whatever moves what [`Self::pose`] reads: a step, an
    /// explosion's restart of the joint lerps.
    version: u64,
}

fn bind_worlds(skel: &Skeleton) -> Vec<(Vec3, Quat)> {
    let mut out: Vec<(Vec3, Quat)> = Vec::with_capacity(skel.bone_count());
    for b in skel.bones() {
        out.push(if b.parent >= 0 {
            let (pp, pr) = out[b.parent as usize];
            (pp + pr * b.local_pos, pr * b.local_rot)
        } else {
            (b.local_pos, b.local_rot)
        });
    }
    out
}

/// The frame whose x is `x` and whose y is `side` made square to it.
fn basis(x: Vec3, side: Vec3) -> Option<Quat> {
    let y = side - x * side.dot(x);
    let y = y.try_normalize()?;
    Some(Quat::from_mat3(&Mat3::from_cols(x, y, x.cross(y))).normalize())
}

/// The swing taking `x` to `d` (both unit) as a rotation vector, and its
/// parts along `y` and `z`.
fn swing_parts(x: Vec3, y: Vec3, z: Vec3, d: Vec3) -> (f32, f32) {
    let axis = x.cross(d);
    let s = axis.length();
    if s < 1e-6 {
        return (0.0, 0.0);
    }
    let r = axis / s * s.atan2(x.dot(d));
    (r.dot(y), r.dot(z))
}

/// The closest points of segments `a0 a1` and `b0 b1`, as the parameter
/// along each.
fn closest_params(a0: Vec3, a1: Vec3, b0: Vec3, b1: Vec3) -> (f32, f32) {
    let (d1, d2, r) = (a1 - a0, b1 - b0, a0 - b0);
    let (a, e, f) = (d1.dot(d1), d2.dot(d2), d2.dot(r));
    if a < 1e-8 && e < 1e-8 {
        return (0.0, 0.0);
    }
    if a < 1e-8 {
        return (0.0, (f / e).clamp(0.0, 1.0));
    }
    let c = d1.dot(r);
    if e < 1e-8 {
        return ((-c / a).clamp(0.0, 1.0), 0.0);
    }
    let b = d1.dot(d2);
    let den = a * e - b * b;
    let mut s = if den > 1e-8 {
        ((b * f - c * e) / den).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let mut t = (b * s + f) / e;
    if t < 0.0 {
        t = 0.0;
        s = (-c / a).clamp(0.0, 1.0);
    } else if t > 1.0 {
        t = 1.0;
        s = ((b - c) / a).clamp(0.0, 1.0);
    }
    (s, t)
}

impl Ragdoll {
    /// A ragdoll of `def` over `skel` posed in world as `now`; `before`
    /// is the same bones `ms` earlier, whose difference starts the points
    /// moving (`Ragdoll_EstimateInitialVelocities`). `None` when a def bone
    /// is missing from the skeleton or the tunnel test fails: a bone, or
    /// the way from a bone's middle to a child's root, blocked forward and
    /// back (`Ragdoll_TunnelTest`; its `Ragdoll_BoneTrace`,
    /// iw3mp.exe 0x5be2e0, traces forward and, when that hits, back).
    pub fn new(
        def: &RagdollDef,
        skel: &Skeleton,
        now: &[(Vec3, Quat)],
        before: Option<(&[(Vec3, Quat)], f32)>,
        dvars: Dvars,
        trace: &mut Trace,
    ) -> Option<Ragdoll> {
        let n = skel.bone_count();
        if def.bones.is_empty() || now.len() != n {
            return None;
        }
        let bind = bind_worlds(skel);
        let mut point_of: Vec<Option<usize>> = vec![None; n];
        let mut points: Vec<Point> = Vec::new();
        let mut point = |bone: usize, radius: f32, mass: f32, friction: f32| {
            let i = *point_of[bone].get_or_insert_with(|| {
                points.push(Point {
                    pos: now[bone].0,
                    prev: now[bone].0,
                    w: 0.0,
                    radius: 0.0,
                    swept: 0.0,
                    friction: 0.0,
                    contact: None,
                });
                points.len() - 1
            });
            let p = &mut points[i];
            p.radius = p.radius.max(radius);
            p.friction = p.friction.max(friction);
            p.w += mass;
            i
        };
        let mut bodies = Vec::with_capacity(def.bones.len());
        for b in &def.bones {
            let i0 = skel.bone_index(&b.p0)?;
            let i1 = skel.bone_index(&b.p1)?;
            let m = b.mass.max(0.01) * 0.5;
            let p0 = point(i0, b.radius, m, b.friction);
            let p1 = point(i1, b.radius, m, b.friction);
            bodies.push(Body {
                p0,
                p1,
                radius: b.radius,
                side: None,
                offset: Quat::IDENTITY,
                axis: (now[i1].0 - now[i0].0).normalize_or_zero(),
                rot: now[i0].1,
                start_rot: now[i0].1,
                start_p0: now[i0].0,
                mass: b.mass.max(0.01),
                parent: b.parent,
                rel_axis: Vec3::ZERO,
                rel_rot: Quat::IDENTITY,
            });
        }
        for p in &mut points {
            p.w = 1.0 / p.w;
            p.swept = p.radius;
        }
        // Bone lengths, and each child whose root is not its parent's end
        // held to both of the parent's points; mirrored siblings so held
        // to each other, and they fix the parent's twist.
        let mut links = Vec::new();
        let mut link = |a: usize, b: usize, pts: &[Point]| {
            if a != b {
                links.push(Link {
                    a,
                    b,
                    rest: (pts[a].pos - pts[b].pos).length(),
                });
            }
        };
        for (i, b) in def.bones.iter().enumerate() {
            link(bodies[i].p0, bodies[i].p1, &points);
            if let Some(p) = b.parent
                && bodies[i].p0 != bodies[p].p1
            {
                link(bodies[i].p0, bodies[p].p0, &points);
                link(bodies[i].p0, bodies[p].p1, &points);
            }
        }
        for (i, a) in def.bones.iter().enumerate() {
            for (j, b) in def.bones.iter().enumerate().skip(i + 1) {
                let detached = |k: usize| {
                    def.bones[k]
                        .parent
                        .is_some_and(|p| bodies[k].p0 != bodies[p].p1)
                };
                if a.parent.is_some()
                    && a.parent == b.parent
                    && a.mirror != b.mirror
                    && detached(i)
                    && detached(j)
                {
                    link(bodies[i].p0, bodies[j].p0, &points);
                    let p = a.parent.unwrap_or(0);
                    if bodies[p].side.is_none() {
                        let (l, r) = if a.mirror { (j, i) } else { (i, j) };
                        bodies[p].side = Some((bodies[l].p0, bodies[r].p0));
                    }
                }
            }
        }
        for i in 0..bodies.len() {
            if let Some(p) = bodies[i].parent {
                let inv = bodies[p].rot.inverse();
                bodies[i].rel_axis = inv * bodies[i].axis;
                bodies[i].rel_rot = (inv * bodies[i].rot).normalize();
            }
        }
        for b in &mut bodies {
            if let Some((l, r)) = b.side {
                let x = (points[b.p1].pos - points[b.p0].pos).normalize_or_zero();
                match basis(x, points[l].pos - points[r].pos) {
                    Some(f) => b.offset = f.inverse() * b.rot,
                    None => b.side = None,
                }
            }
        }
        // The joints' swing limits, read in the parent bone's bind frame.
        let mut swings = Vec::new();
        for j in &def.joints {
            let Some(parent) = def.bones[j.bone].parent else {
                continue;
            };
            let c = &def.bones[j.bone];
            let (c0, c1) = (skel.bone_index(&c.p0)?, skel.bone_index(&c.p1)?);
            let pb = skel.bone_index(&def.bones[parent].p0)?;
            let inv = bind[pb].1.inverse();
            let Some(x) = (inv * (bind[c1].0 - bind[c0].0)).try_normalize() else {
                continue;
            };
            let y = inv * (bind[c0].1 * Vec3::Y);
            let Some(y) = (y - x * y.dot(x)).try_normalize() else {
                continue;
            };
            let z = x.cross(y);
            let range = |l: Option<Limit>| {
                l.map_or((-std::f32::consts::PI, std::f32::consts::PI), |l| {
                    (l.min.to_radians(), l.max.to_radians())
                })
            };
            let (mut y_lim, mut z_lim) = match j.kind {
                JointKind::Swivel => (range(j.limits[1]), range(j.limits[2])),
                // A hinge turns about its one limited axis (z on every
                // stock hinge) and holds the other still.
                JointKind::Hinge if j.limits[1].is_some() && j.limits[2].is_none() => {
                    (range(j.limits[1]), (0.0, 0.0))
                }
                JointKind::Hinge => ((0.0, 0.0), range(j.limits[2])),
            };
            // The cfg's angle signs follow ODE's joint axes, which this
            // frame need not share: an axis whose start reads outside its
            // range and inside the mirrored one is flipped, and a range is
            // widened to take the start (the death anim's pose).
            let rp = now[pb].1.inverse();
            let d = (rp * (now[c1].0 - now[c0].0)).normalize_or_zero();
            let (sy, sz) = swing_parts(x, y, z, d);
            let fit = |lim: &mut (f32, f32), v: f32| {
                let tol = 10f32.to_radians();
                let inside = |l: (f32, f32), v: f32| v >= l.0 - tol && v <= l.1 + tol;
                if !inside(*lim, v) && inside((-lim.1, -lim.0), v) {
                    *lim = (-lim.1, -lim.0);
                }
                *lim = (lim.0.min(v), lim.1.max(v));
            };
            fit(&mut y_lim, sy);
            fit(&mut z_lim, sz);
            swings.push(Swing {
                parent,
                child: j.bone,
                x,
                y,
                z,
                y_lim,
                z_lim,
            });
        }
        // Which body carries each skeleton bone.
        let root_bone = skel.bone_index(&def.bones[0].p0)?;
        let mut above_root = vec![false; n];
        let mut k = root_bone as i32;
        while k >= 0 {
            above_root[k as usize] = true;
            k = skel.bones()[k as usize].parent;
        }
        let lerp = |name: &str| {
            def.base_lerp
                .iter()
                .find(|l| l.bone.eq_ignore_ascii_case(name))
                .map(|l| l.msec)
        };
        let owner: Vec<Owner> = (0..n)
            .map(|b| {
                if let Some(i) = bodies
                    .iter()
                    .enumerate()
                    .find(|(_, body)| point_of[b] == Some(body.p0))
                    .map(|(i, _)| i)
                    .filter(|&i| skel.bone_index(&def.bones[i].p0) == Some(b))
                {
                    Owner::Body(i)
                } else if above_root[b] || skel.bones()[b].parent < 0 {
                    Owner::Body(0)
                } else {
                    Owner::Child(lerp(&skel.bones()[b].name))
                }
            })
            .collect();
        let parents: Vec<i32> = skel.bones().iter().map(|b| b.parent).collect();
        let start_local = (0..n)
            .map(|b| match parents[b] {
                p if p >= 0 => {
                    let (pp, pr) = now[p as usize];
                    let inv = pr.inverse();
                    (inv * (now[b].0 - pp), (inv * now[b].1).normalize())
                }
                _ => now[b],
            })
            .collect();
        let bind_local = skel.bones().iter().map(|b| b.local_rot).collect();
        // The tunnel test: each bone along itself, and a bone with more
        // than one child from its middle to each child's root, each
        // through `Ragdoll_BoneTrace`.
        let mut bone_trace = |a: Vec3, b: Vec3| {
            let f = trace(a, b, 0.0);
            if !f.startsolid && f.fraction >= 1.0 {
                return true;
            }
            let r = trace(b, a, 0.0);
            !r.startsolid && (f.startsolid || r.fraction >= 1.0)
        };
        for (i, b) in bodies.iter().enumerate() {
            let (a, e) = (points[b.p0].pos, points[b.p1].pos);
            let children: Vec<usize> = (0..def.bones.len())
                .filter(|&c| def.bones[c].parent == Some(i))
                .collect();
            if children.len() > 1 {
                let mid = (a + e) * 0.5;
                if children
                    .iter()
                    .any(|&c| !bone_trace(mid, points[bodies[c].p0].pos))
                {
                    return None;
                }
            }
            if !bone_trace(a, e) {
                return None;
            }
        }
        let pairs = def
            .self_pairs
            .iter()
            .map(|&(i, j)| {
                let (bi, bj) = (&bodies[i], &bodies[j]);
                let [a0, a1, b0, b1] = [bi.p0, bi.p1, bj.p0, bj.p1].map(|k| points[k].pos);
                let (s, t) = closest_params(a0, a1, b0, b1);
                let start = (a0.lerp(a1, s) - b0.lerp(b1, t)).length();
                let min = (bi.radius + bj.radius) * dvars.self_collision_scale;
                (i, j, min.min(start))
            })
            .collect();
        let started_through = bodies
            .iter()
            .map(|b| {
                let t = trace(points[b.p0].pos, points[b.p1].pos, 0.0);
                t.startsolid || t.fraction < 1.0
            })
            .collect();
        // The anim's last move starts each point, scaled to one step.
        if let Some((before, ms)) = before.filter(|(b, ms)| b.len() == n && *ms > 0.0) {
            let scale = STEP_MS / ms;
            for (bone, i) in point_of.iter().enumerate() {
                if let Some(i) = *i {
                    let v = (now[bone].0 - before[bone].0) * scale;
                    points[i].prev = points[i].pos - v.clamp_length_max(400.0 * STEP_MS / 1000.0);
                }
            }
        }
        Some(Ragdoll {
            points,
            links,
            bodies,
            swings,
            pairs,
            started_through,
            owner,
            parents,
            start: now.to_vec(),
            start_local,
            bind_local,
            dvars,
            state: State::Running,
            msec: 0.0,
            still_ms: 0.0,
            acc_ms: 0.0,
            last_ms: STEP_MS,
            version: 0,
        })
    }

    /// A blast on this body, `Ragdoll_ExplosionEvent` (iw3mp.exe 0x5bdc10,
    /// cod4-player-anims.md 10.1): with its root inside the outer radius
    /// the body wakes, and every bone whose jittered centre is inside takes
    /// its share of `ragdoll_explode_force` as a change of momentum.
    /// Whether the blast reached the body.
    pub fn explode(&mut self, e: &Explosion, rand: &mut dyn FnMut() -> f32) -> bool {
        let Some(reach) = Reach::of(e) else {
            return false;
        };
        let root = self.points[self.bodies[0].p0].pos;
        if reach.level(root - e.origin).length_squared() >= reach.outer_sq {
            return false;
        }
        self.wake();
        let step_s = self.last_ms / 1000.0;
        let share = 1.0 / self.bodies.len() as f32;
        for b in &self.bodies {
            // the jitter's three draws come before the bone's range test
            let jitter = Vec3::new(rand(), rand(), rand());
            let centre = (self.points[b.p0].pos + self.points[b.p1].pos) * 0.5 + jitter;
            let off = centre - e.origin;
            let Some(falloff) = reach.falloff(off.length_squared()) else {
                continue;
            };
            let mut heading = if e.impulse == Vec3::ZERO {
                reach.level(off).normalize_or_zero()
            } else {
                e.impulse
            };
            heading.z += self.dvars.explode_upbias;
            let force = e.scale * falloff * self.dvars.explode_force * share;
            let kick = heading.normalize_or_zero() * force / b.mass * step_s;
            self.points[b.p0].prev -= kick;
            self.points[b.p1].prev -= kick;
        }
        true
    }

    /// Back to running from rest, its clocks restarted.
    fn wake(&mut self) {
        self.state = State::Running;
        self.msec = 0.0;
        self.still_ms = 0.0;
        self.version += 1;
    }

    /// Runs `ms` of the ragdoll through `trace`'s world. As 1.7's
    /// `Phys_RunToTime` (iw3mp.exe 0x5a6dfb..0x5a6e6b) at most two steps
    /// a call, the first `max(STEP_MS, owed / 2)` long, the second the
    /// rest; a frame owing under a step runs none and keeps the time.
    pub fn step(&mut self, ms: f32, trace: &mut Trace) {
        if self.state == State::Idle {
            return;
        }
        self.acc_ms += ms.max(0.0);
        for left in (1..=MAX_RUN_STEPS).rev() {
            if self.acc_ms < STEP_MS || self.state != State::Running {
                break;
            }
            let dt = (self.acc_ms / left as f32).max(STEP_MS);
            self.acc_ms -= dt;
            self.substep(dt.min(MAX_STEP_MS), trace);
        }
    }

    fn substep(&mut self, ms: f32, trace: &mut Trace) {
        let dt = ms / 1000.0;
        let d = &self.dvars;
        // `Ragdoll_UpdateFriction`'s lerp: full joint friction at the
        // start, a tenth past `ragdoll_jointlerp_time`; here a damping,
        // per 1/60 s.
        let t = (1.0 - self.msec / d.jointlerp_ms.max(1) as f32).max(0.0);
        let damp = 0.01 + 0.06 * (t * t * 0.9 + 0.1);
        let keep = (1.0 - damp).powf(ms * 0.06) * (ms / self.last_ms);
        self.last_ms = ms;
        let g = Vec3::new(0.0, 0.0, d.gravity) * dt * dt;
        let starts: Vec<Vec3> = self.points.iter().map(|p| p.pos).collect();
        for p in &mut self.points {
            p.contact = None;
            let v = (p.pos - p.prev) * keep;
            p.prev = p.pos;
            p.pos += v + g;
        }
        let mut safe = starts.clone();
        for _ in 0..ROUNDS {
            for _ in 0..ITERATIONS {
                self.solve_links();
                self.update_frames();
                self.solve_swings();
                self.solve_pairs();
            }
            self.collide(&mut safe, trace);
        }
        self.keep_bones_clear(&starts, trace);
        self.hold_contacts(&starts, trace);
        self.update_frames();
        self.msec += ms;
        self.version += 1;
        // Asleep after `phys_autoDisableTime` with every point slower
        // than `phys_autoDisableLinear`. ODE also holds each body's turn
        // under `phys_autoDisableAngular` (1 rad/s; `Phys_RunFrame` writes
        // the squares and the time into the world each frame, iw3mp.exe
        // 0x5a6505..0x5a6533); this solver's points settle with a few
        // hundredths of a unit of noise a step, a degree on the pelvis,
        // which at 17 ms steps reads over it and would keep every body
        // awake to `ragdoll_max_life` (cod4-player-anims.md 10.2).
        let still = self
            .points
            .iter()
            .all(|p| (p.pos - p.prev).length() / dt < self.dvars.sleep_speed);
        if still {
            self.still_ms += ms;
        } else {
            self.still_ms = 0.0;
        }
        if self.still_ms >= self.dvars.sleep_ms || self.msec > self.dvars.max_life_ms as f32 {
            self.state = State::Idle;
        }
    }

    fn solve_links(&mut self) {
        for l in &self.links {
            let (a, b) = (&self.points[l.a], &self.points[l.b]);
            let delta = b.pos - a.pos;
            let len = delta.length();
            let w = a.w + b.w;
            if len < 1e-6 || w <= 0.0 {
                continue;
            }
            let fix = delta * ((len - l.rest) / (len * w));
            let (wa, wb) = (a.w, b.w);
            self.points[l.a].pos += fix * wa;
            self.points[l.b].pos -= fix * wb;
        }
    }

    /// Each bone's frame from its points, parents before children (the
    /// def lists a parent first).
    fn update_frames(&mut self) {
        for i in 0..self.bodies.len() {
            let b = &self.bodies[i];
            let x = (self.points[b.p1].pos - self.points[b.p0].pos).normalize_or_zero();
            if x == Vec3::ZERO {
                continue;
            }
            let side = b
                .side
                .and_then(|(l, r)| basis(x, self.points[l].pos - self.points[r].pos));
            let parent = b.parent.map(|p| self.bodies[p].rot);
            let rot = match (side, parent) {
                (Some(f), _) => (f * b.offset).normalize(),
                (None, Some(pr)) if b.rel_axis != Vec3::ZERO => {
                    let a = pr.inverse() * x;
                    (pr * Quat::from_rotation_arc(b.rel_axis, a) * b.rel_rot).normalize()
                }
                (None, _) if b.axis != Vec3::ZERO => {
                    (Quat::from_rotation_arc(b.axis, x) * b.rot).normalize()
                }
                (None, _) => b.rot,
            };
            let b = &mut self.bodies[i];
            b.rot = rot;
            b.axis = x;
        }
    }

    fn solve_swings(&mut self) {
        for s in &self.swings {
            let pr = self.bodies[s.parent].rot;
            let c = &self.bodies[s.child];
            let (a, b) = (self.points[c.p0].pos, self.points[c.p1].pos);
            let Some(d) = (pr.inverse() * (b - a)).try_normalize() else {
                continue;
            };
            let (ry, rz) = swing_parts(s.x, s.y, s.z, d);
            let (cy, cz) = (
                ry.clamp(s.y_lim.0, s.y_lim.1),
                rz.clamp(s.z_lim.0, s.z_lim.1),
            );
            if (cy - ry).abs() < 1e-4 && (cz - rz).abs() < 1e-4 {
                continue;
            }
            let dir = pr * (Quat::from_scaled_axis(s.y * cy + s.z * cz) * s.x);
            let target = a + dir * (b - a).length();
            self.points[c.p1].pos += (target - b) * 0.5;
        }
    }

    fn solve_pairs(&mut self) {
        for &(i, j, min) in &self.pairs {
            let (bi, bj) = (&self.bodies[i], &self.bodies[j]);
            let ids = [bi.p0, bi.p1, bj.p0, bj.p1];
            if ids[..2].iter().any(|p| ids[2..].contains(p)) {
                continue;
            }
            let [a0, a1, b0, b1] = ids.map(|k| self.points[k].pos);
            let (s, t) = closest_params(a0, a1, b0, b1);
            let delta = b0.lerp(b1, t) - a0.lerp(a1, s);
            let dist = delta.length();
            if dist >= min || dist < 1e-5 {
                continue;
            }
            let ws = [1.0 - s, s, 1.0 - t, t];
            let w: [f32; 4] = std::array::from_fn(|k| self.points[ids[k]].w * ws[k]);
            let total = w.iter().sum::<f32>();
            if total <= 0.0 {
                continue;
            }
            let push = delta / dist * ((min - dist) * 0.5 / total);
            for k in 0..4 {
                let sign = if k < 2 { -1.0 } else { 1.0 };
                self.points[ids[k]].pos += push * (sign * w[k]);
            }
        }
    }

    /// Each point swept from where it last stood clear to where the
    /// constraints put it; a hit stops it on the surface and takes the
    /// velocity into it and some along it.
    fn collide(&mut self, safe: &mut [Vec3], trace: &mut Trace) {
        for (p, from) in self.points.iter_mut().zip(safe.iter_mut()) {
            if p.swept < p.radius {
                let r = (p.swept + 0.5).min(p.radius);
                if !trace(*from, *from, r).startsolid {
                    p.swept = r;
                }
            }
            if p.pos == *from {
                continue;
            }
            let mut tr = trace(*from, p.pos, p.swept);
            while tr.startsolid && p.swept > 0.0 {
                p.swept = if p.swept > 0.5 { p.swept * 0.5 } else { 0.0 };
                tr = trace(*from, p.pos, p.swept);
            }
            if tr.startsolid {
                // Inside the world even as a point (a foot the death anim
                // sank into the ground): lifted to the first clear spot
                // up to 16 units over it, else let through this step.
                if let Some(up) = (1..=16)
                    .map(|k| *from + Vec3::Z * k as f32)
                    .find(|&q| !trace(q, q, 0.0).startsolid)
                {
                    p.pos = up;
                    p.prev = up;
                }
                *from = p.pos;
                continue;
            }
            if tr.fraction < 1.0 {
                let hit = from.lerp(p.pos, tr.fraction) + tr.normal * 0.05;
                let depth = (hit - p.pos).dot(tr.normal).max(0.0);
                let total = p.contact.map_or(0.0, |c| c.1);
                p.contact = Some((tr.normal, total + depth));
                p.pos = hit;
                // Coulomb friction, the bone's friction as the coefficient:
                // the speed into the surface goes, and the speed along it
                // loses that times what went, all of it below that.
                let v = p.pos - p.prev;
                let vn = v.dot(tr.normal);
                let into = (-vn).max(0.0);
                let along = v - tr.normal * vn;
                let slip = along.length();
                let along = if slip <= p.friction * into {
                    Vec3::ZERO
                } else {
                    along * (1.0 - p.friction * into / slip)
                };
                p.prev = p.pos - (along + tr.normal * vn.max(0.0));
            }
            *from = p.pos;
        }
    }

    /// The points sweep as spheres, so a bone could close over a thin
    /// edge or a ridge between its ends: a bone clear at the step's start
    /// and crossing the world at its end is lifted off the contact along
    /// its normal, its ends shared by where along it the contact is, a
    /// unit at a time; one still crossing after four goes back to where it
    /// started. vcod's; retail's capsules collide whole.
    fn keep_bones_clear(&mut self, starts: &[Vec3], trace: &mut Trace) {
        for i in 0..self.bodies.len() {
            let (a, b) = (self.bodies[i].p0, self.bodies[i].p1);
            let crossing = |pa: Vec3, pb: Vec3, trace: &mut Trace| {
                let t = trace(pa, pb, 0.0);
                (t.startsolid || t.fraction < 1.0).then_some(t)
            };
            if crossing(starts[a], starts[b], trace).is_some() {
                continue;
            }
            let mut tries = 0;
            while let Some(t) = crossing(self.points[a].pos, self.points[b].pos, trace) {
                tries += 1;
                if tries > 4 || t.startsolid || t.normal == Vec3::ZERO {
                    for k in [a, b] {
                        self.points[k].pos = starts[k];
                        self.points[k].prev = starts[k];
                    }
                    break;
                }
                for (k, share) in [(a, 1.0 - t.fraction), (b, t.fraction)] {
                    let p = &mut self.points[k];
                    let to = p.pos + t.normal * share;
                    let m = trace(p.pos, to, 0.0);
                    if !m.startsolid && m.fraction >= 1.0 {
                        p.prev += to - p.pos;
                        p.pos = to;
                    }
                }
            }
        }
    }

    /// Static friction: a point that met a surface this step and slid
    /// along it less than its bone's friction times how far the surface
    /// pushed it back out goes back to where it stood along the surface,
    /// its speed along it gone, so a body at rest stops creeping under its
    /// constraints. vcod's, for ODE's contact friction.
    fn hold_contacts(&mut self, starts: &[Vec3], trace: &mut Trace) {
        for (p, s) in self.points.iter_mut().zip(starts) {
            let Some((n, depth)) = p.contact else {
                continue;
            };
            let moved = p.pos - *s;
            let slip = moved - n * moved.dot(n);
            if slip == Vec3::ZERO || slip.length() > p.friction * depth {
                continue;
            }
            let to = p.pos - slip;
            let t = trace(p.pos, to, p.swept);
            if t.startsolid || t.fraction < 1.0 {
                continue;
            }
            let v = p.pos - p.prev;
            p.pos = to;
            p.prev = to - n * v.dot(n);
        }
    }

    /// Every skeleton bone's world pose now.
    pub fn pose(&self) -> Vec<(Vec3, Quat)> {
        let n = self.owner.len();
        let mut out: Vec<(Vec3, Quat)> = Vec::with_capacity(n);
        for bone in 0..n {
            let world = match self.owner[bone] {
                Owner::Body(i) => {
                    let b = &self.bodies[i];
                    let turn = (b.rot * b.start_rot.inverse()).normalize();
                    let at = self.points[b.p0].pos;
                    let (p, q) = self.start[bone];
                    (at + turn * (p - b.start_p0), (turn * q).normalize())
                }
                Owner::Child(lerp) => {
                    let (pp, pr) = out[self.parents[bone] as usize];
                    let (lp, mut lr) = self.start_local[bone];
                    if let Some(ms) = lerp {
                        let f = (self.msec / ms.max(1) as f32).min(1.0);
                        lr = lr.slerp(self.bind_local[bone], f);
                    }
                    (pp + pr * lp, (pr * lr).normalize())
                }
            };
            out.push(world);
        }
        out
    }

    /// Each point's position and radius, for a check against the world.
    pub fn spheres(&self) -> impl Iterator<Item = (Vec3, f32)> + '_ {
        self.points.iter().map(|p| (p.pos, p.radius))
    }

    /// Each point's position and the radius it sweeps with now (below its
    /// own where it started inside the world).
    pub fn swept_spheres(&self) -> impl Iterator<Item = (Vec3, f32)> + '_ {
        self.points.iter().map(|p| (p.pos, p.swept))
    }

    /// Each def bone's two ends, and whether it crossed the world in the
    /// pose the ragdoll started from.
    pub fn segments(&self) -> impl Iterator<Item = (Vec3, Vec3, bool)> + '_ {
        self.bodies
            .iter()
            .zip(&self.started_through)
            .map(|(b, &t)| (self.points[b.p0].pos, self.points[b.p1].pos, t))
    }

    /// Each def bone's two ends and its rotation (its p0 bone's world
    /// rotation), in the def's order.
    pub fn frames(&self) -> impl Iterator<Item = (Vec3, Vec3, Quat)> + '_ {
        self.bodies
            .iter()
            .map(|b| (self.points[b.p0].pos, self.points[b.p1].pos, b.rot))
    }

    /// The ms simulated.
    pub fn msec(&self) -> f32 {
        self.msec
    }

    /// Changes whenever [`Self::pose`] may: equal versions of one ragdoll
    /// pose the same.
    pub fn version(&self) -> u64 {
        self.version
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::model::{Bone, Model};

    const CFG: &str = "
ragdoll_clear 1
ragdoll_bone 1 j_mainroot j_neck   5.0 0.5 20.0 0.7 -1 0 capsule // torso
ragdoll_bone 1 j_neck     j_head   3.5 0.5  5.0 0.6  0 0 capsule
ragdoll_bone 1 j_shoulder_le j_elbow_le 2.5 0.5 8.0 0.5 0 0 capsule
ragdoll_bone 1 j_shoulder_ri j_elbow_ri 2.5 0.5 8.0 0.5 0 1 capsule
ragdoll_bone 1 j_a j_b 1 1 1 1 9 0 capsule
ragdoll_selfpair 1 2 3
ragdoll_joint 1 1 swivel
ragdoll_joint 1 2 hinge
ragdoll_joint 1 0 hinge
ragdoll_limit 1 0 x 60.0 -50.0 50.0
ragdoll_limit 1 1 -z 60.0 -120.0 2.0
ragdoll_baselerp_bone 1 j_head 2000
ragdoll_baselerp_bone 1 j_hiptwist_le 1000 8
";

    #[test]
    fn the_cfg_builds_its_def_and_refuses_bad_lines() {
        let defs = parse_cfg(CFG);
        assert!(defs[0].bones.is_empty());
        let d = &defs[1];
        // `j_a`'s parent 9 does not exist yet: refused.
        assert_eq!(d.bones.len(), 4);
        assert_eq!(d.bones[0].parent, None);
        assert_eq!(d.bones[3].parent, Some(0));
        assert!(d.bones[3].mirror);
        assert_eq!(d.self_pairs, [(2, 3)]);
        // A joint on the root bone has no parent: refused.
        assert_eq!(d.joints.len(), 2);
        assert_eq!(d.joints[0].kind, JointKind::Swivel);
        assert_eq!(d.joints[0].limits[0].unwrap().max, 50.0);
        assert_eq!(d.joints[1].limits[2].unwrap().min, -120.0);
        assert_eq!(d.base_lerp[1].body, Some(8));
        assert_eq!(d.base_lerp[0].msec, 2000);
    }

    /// The stock `ragdoll.cfg`: MP's def 1, and def 0 beside it.
    #[test]
    fn the_stock_cfg_builds_the_mp_def() {
        let Some(install) = crate::testing::cod4_install() else {
            return;
        };
        let fs =
            vcod_common::pk3::Pk3Fs::open_iwd_layers(&install.root().join("main"), &[]).unwrap();
        let text = fs.read("ragdoll.cfg").expect("iw_00.iwd's ragdoll.cfg");
        let defs = parse_cfg(&String::from_utf8_lossy(&text));
        let d = &defs[MP_DEF];
        let counts = (
            d.bones.len(),
            d.joints.len(),
            d.self_pairs.len(),
            d.base_lerp.len(),
        );
        assert_eq!(counts, (12, 11, 33, 9));
        assert_eq!(
            (d.bones[0].p0.as_str(), d.bones[0].mass),
            ("j_mainroot", 20.0)
        );
        assert_eq!(d.bones[11].parent, Some(10));
        let knee = d.joints.iter().find(|j| j.bone == 9).unwrap();
        assert_eq!(knee.kind, JointKind::Hinge);
        assert_eq!(knee.limits[2].map(|l| (l.min, l.max)), Some((-120.0, 3.5)));
        assert_eq!(defs[0].bones.len(), 10);
    }

    /// A bone at `pos` (world, unturned) under `parent`.
    fn bone(name: &str, parent: i32, pos: Vec3, parent_pos: Vec3) -> Bone {
        Bone {
            name: name.into(),
            parent,
            pos,
            rot: Quat::IDENTITY,
            local_pos: pos - parent_pos,
            local_rot: Quat::IDENTITY,
            hit_mins: Vec3::ZERO,
            hit_maxs: Vec3::ZERO,
            hit_location: 0,
        }
    }

    fn figure() -> Skeleton {
        let p = |x, y, z| Vec3::new(x, y, z);
        let root = p(0.0, 0.0, 40.0);
        let neck = p(0.0, 0.0, 60.0);
        let bones = vec![
            bone("tag_origin", -1, Vec3::ZERO, Vec3::ZERO),
            bone("j_mainroot", 0, root, Vec3::ZERO),
            bone("j_neck", 1, neck, root),
            bone("j_head", 2, p(0.0, 0.0, 68.0), neck),
            bone("j_shoulder_le", 2, p(0.0, 8.0, 58.0), neck),
            bone("j_elbow_le", 4, p(0.0, 20.0, 58.0), p(0.0, 8.0, 58.0)),
            bone("j_shoulder_ri", 2, p(0.0, -8.0, 58.0), neck),
            bone("j_elbow_ri", 6, p(0.0, -20.0, 58.0), p(0.0, -8.0, 58.0)),
        ];
        Skeleton::build(&[&Model {
            name: "figure".into(),
            bones,
            lods: Vec::new(),
            collision: Vec::new(),
        }])
    }

    /// The world as the floor z 0 alone.
    fn floor(a: Vec3, b: Vec3, r: f32) -> Sweep {
        let up = Vec3::Z;
        if a.z < r - 0.01 {
            return Sweep {
                fraction: 0.0,
                normal: up,
                startsolid: true,
            };
        }
        if b.z >= r || (a.z - b.z).abs() < 1e-6 {
            return Sweep {
                fraction: 1.0,
                normal: Vec3::ZERO,
                startsolid: false,
            };
        }
        Sweep {
            fraction: ((a.z - r) / (a.z - b.z)).clamp(0.0, 1.0),
            normal: up,
            startsolid: false,
        }
    }

    #[test]
    fn a_body_dropped_on_the_floor_comes_to_rest_on_it() {
        let skel = figure();
        let defs = parse_cfg(CFG);
        let pose = bind_worlds(&skel);
        let mut trace = floor;
        let mut r = Ragdoll::new(&defs[1], &skel, &pose, None, Dvars::default(), &mut trace)
            .expect("the figure's bones are all there");
        let mut ms = 0.0;
        while r.state == State::Running && ms < 6000.0 {
            r.step(50.0, &mut trace);
            ms += 50.0;
        }
        assert_eq!(r.state, State::Idle, "still moving at {ms} ms");
        assert!(ms < 4600.0, "{ms} ms: only the max life stopped it");
        for (p, rad) in r.spheres() {
            assert!(p.z >= rad - 0.1, "{p} sinks under the floor");
        }
        // It lies on the floor: its lowest point touching it, the root
        // down from the 40 it stood at.
        let w = r.pose();
        let lowest = r
            .spheres()
            .map(|(p, rad)| p.z - rad)
            .fold(f32::MAX, f32::min);
        assert!(lowest < 0.2, "lowest point {lowest} over the floor");
        let neck = skel.bone_index("j_neck").unwrap();
        let head = skel.bone_index("j_head").unwrap();
        // The posed bones kept their lengths.
        assert!(((w[head].0 - w[neck].0).length() - 8.0).abs() < 0.5);
        let root = skel.bone_index("j_mainroot").unwrap();
        assert!(((w[neck].0 - w[root].0).length() - 20.0).abs() < 0.5);
        assert!(w[root].0.z < 10.0, "the root stands at {}", w[root].0.z);
    }

    /// A body asleep on the floor: a grenade's blast (inner 0, outer
    /// 256) 48 units off its root wakes it and throws it away from the
    /// blast and up; one past the outer radius leaves it asleep.
    #[test]
    fn a_blast_wakes_a_sleeping_body_and_throws_it_away() {
        let skel = figure();
        let defs = parse_cfg(CFG);
        let pose = bind_worlds(&skel);
        let mut trace = floor;
        let mut r = Ragdoll::new(&defs[1], &skel, &pose, None, Dvars::default(), &mut trace)
            .expect("the figure's bones are all there");
        while r.state == State::Running {
            r.step(50.0, &mut trace);
        }
        let root = skel.bone_index("j_mainroot").unwrap();
        let rest = r.pose()[root].0;
        let mut no_jitter = || 0.0;
        let far = Explosion {
            origin: rest - Vec3::X * 300.0,
            inner: 0.0,
            outer: 256.0,
            cylinder: false,
            impulse: Vec3::ZERO,
            scale: 1.0,
        };
        assert!(!r.explode(&far, &mut no_jitter));
        assert_eq!(r.state, State::Idle);
        let near = Explosion {
            origin: rest - Vec3::X * 48.0,
            ..far
        };
        assert!(r.explode(&near, &mut no_jitter));
        assert_eq!(r.state, State::Running);
        let (mut top, mut ms) = (rest.z, 0.0);
        while r.state == State::Running && ms < 6000.0 {
            r.step(50.0, &mut trace);
            top = top.max(r.pose()[root].0.z);
            ms += 50.0;
        }
        let moved = r.pose()[root].0 - rest;
        assert!(moved.x > 4.0, "the root moved {moved}");
        assert!(top > rest.z + 1.0, "the root never left the floor ({top})");
        assert_eq!(r.state, State::Idle, "still moving at {ms} ms");
    }

    /// The neck's end swung out 30 degrees, round the cone and back
    /// leaves the neck's frame where it started: carried by the turn of
    /// its axis from its start in the torso's frame, it gains no twist
    /// from the path (a turn carried from step to step gains the cone's
    /// solid angle, 48 degrees).
    #[test]
    fn a_bone_swung_round_a_cone_comes_back_untwisted() {
        let skel = figure();
        let defs = parse_cfg(CFG);
        let pose = bind_worlds(&skel);
        let mut trace = floor;
        let mut r = Ragdoll::new(&defs[1], &skel, &pose, None, Dvars::default(), &mut trace)
            .expect("the figure's bones are all there");
        let (p0, p1) = (r.bodies[1].p0, r.bodies[1].p1);
        let (at, len) = (
            r.points[p0].pos,
            r.points[p0].pos.distance(r.points[p1].pos),
        );
        let start = r.bodies[1].rot;
        let tilt = 30f32.to_radians();
        let dir = |t: f32, a: f32| Vec3::new(t.sin() * a.cos(), t.sin() * a.sin(), t.cos());
        let path = (0..=10)
            .map(|k| dir(tilt * k as f32 / 10.0, 0.0))
            .chain((0..=72).map(|k| dir(tilt, std::f32::consts::TAU * k as f32 / 72.0)))
            .chain((0..=10).rev().map(|k| dir(tilt * k as f32 / 10.0, 0.0)));
        for d in path {
            r.points[p1].pos = at + d * len;
            r.update_frames();
        }
        let back = (start.inverse() * r.bodies[1].rot).normalize();
        let deg = 2.0 * back.xyz().length().atan2(back.w.abs()).to_degrees();
        assert!(deg < 0.01, "the neck came back turned {deg} degrees");
    }

    #[test]
    fn a_bone_across_a_wall_fails_the_tunnel_test() {
        let skel = figure();
        let defs = parse_cfg(CFG);
        let pose = bind_worlds(&skel);
        // A wall at y 15 across the left upper arm.
        let mut wall = |a: Vec3, b: Vec3, _: f32| {
            let crosses = (a.y - 15.0) * (b.y - 15.0) < 0.0;
            Sweep {
                fraction: if crosses { 0.5 } else { 1.0 },
                normal: Vec3::NEG_Y,
                startsolid: false,
            }
        };
        assert!(Ragdoll::new(&defs[1], &skel, &pose, None, Dvars::default(), &mut wall).is_none());
    }

    /// FNV-1a over a stream of words: the pins below hash every output's bits.
    fn fnv(words: impl IntoIterator<Item = u32>) -> u64 {
        words.into_iter().fold(0xcbf2_9ce4_8422_2325, |h, w| {
            (h ^ u64::from(w)).wrapping_mul(0x100_0000_01b3)
        })
    }

    /// Each blast's change to every point's last position, bit for bit:
    /// a sphere, a cylinder, a jolt and a blast with an inner radius,
    /// each bone's centre jittered by a fixed LCG.
    #[test]
    fn pins_the_blast_push() {
        let skel = figure();
        let defs = parse_cfg(CFG);
        let pose = bind_worlds(&skel);
        let mut trace = floor;
        let r0 = Ragdoll::new(&defs[1], &skel, &pose, None, Dvars::default(), &mut trace)
            .expect("the figure's bones are all there");
        let root = r0.points[r0.bodies[0].p0].pos;
        let blasts = [
            (
                Vec3::new(-48.0, 10.0, -5.0),
                0.0,
                256.0,
                false,
                Vec3::ZERO,
                1.0,
            ),
            (
                Vec3::new(30.0, -60.0, 70.0),
                0.0,
                120.0,
                true,
                Vec3::ZERO,
                2.5,
            ),
            (
                Vec3::new(5.0, 5.0, 5.0),
                10.0,
                90.0,
                true,
                Vec3::new(0.0, 3.0, 1.0),
                1.0,
            ),
            (
                Vec3::new(-20.0, 0.0, 30.0),
                25.0,
                60.0,
                false,
                Vec3::ZERO,
                0.7,
            ),
            (
                Vec3::new(500.0, 0.0, 0.0),
                0.0,
                100.0,
                false,
                Vec3::ZERO,
                1.0,
            ),
        ];
        let mut out = Vec::new();
        for (off, inner, outer, cylinder, impulse, scale) in blasts {
            let mut r = r0.clone();
            let mut lcg = 77u32;
            let mut rand = || {
                lcg = lcg.wrapping_mul(0x343fd).wrapping_add(0x269ec3);
                ((lcg >> 16) & 0x7fff) as f32 / 16383.5 - 1.0
            };
            let e = Explosion {
                origin: root + off,
                inner,
                outer,
                cylinder,
                impulse,
                scale,
            };
            out.push(u32::from(r.explode(&e, &mut rand)));
            out.push(r.version as u32);
            for p in &r.points {
                out.extend(p.prev.to_array().map(f32::to_bits));
            }
        }
        assert_eq!(fnv(out), PIN_BLAST);
    }
    const PIN_BLAST: u64 = 11647641683859709654;
}
