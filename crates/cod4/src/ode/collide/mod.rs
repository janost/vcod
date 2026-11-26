//! Contacts for the ragdolls' world in iw3mp.exe 1.7: a capsule body
//! against the clip map ([`world`], [`capsule`]), the near callback that
//! turns the contacts into contact joints (0x5a5f20 and its pass
//! 0x5a2760) and the self collision hook (0x5bd9c0, [`selfcol`]);
//! cod4-player-anims.md 10.6.

pub mod capsule;
pub mod selfcol;
pub mod world;

#[cfg(test)]
pub(crate) mod tests;

use glam::Vec3;

use super::joint::contact::{APPROX1_1, APPROX1_2, BOUNCE, Contact, SOFT_CFM, SOFT_ERP, Surface};
use super::math::{V3, normalize3, sub};
use super::world::{BodyId, World};

pub use world::{Capsule, Pieces, collide_world};

#[inline]
fn w(x: f32) -> f64 {
    x as f64
}

/// A contact as a collider leaves it (ODE's `dContactGeom`, 0x30 bytes,
/// with IW's surface flags at +0x2c): position, normal, depth.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContactGeom {
    pub pos: V3,
    pub normal: V3,
    pub depth: f32,
    pub surface: u32,
}

/// Room for a collide's contacts (the 0x80 0x5a5f20 pushes).
pub const MAX_CONTACTS: usize = 0x80;
/// The most joints a world holds and contacts its group holds (the 0x1000
/// at 0x5a2a34 and 0x5a2a43): past either the pass prints `Maximum number
/// of ODE physics contact points exceeded.` and stops.
pub const MAX_JOINTS: usize = 0x1000;
/// The contacts' mode, 0x301c (0x5a6040, 0x5a2ba0): bounce, soft ERP and
/// CFM, the friction pyramid.
pub const MODE: u32 = BOUNCE | SOFT_ERP | SOFT_CFM | APPROX1_1 | APPROX1_2;
/// `bounce_vel` on every contact (the float at 0x6b72dc).
pub const BOUNCE_VEL: f32 = 0.1;
/// Two contacts face each other under this dot of their normals (the
/// double at 0x70b268).
const FACING: f64 = -0.990_000_009_536_743_2;
/// A pinched body drops a contact its point leaves faster than this along
/// the normal (the float at 0x6df494).
const SEPARATING: f32 = 0.01;

/// The contact dvars' defaults (their registrations, cod4-gsc.md 3.6).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dvars {
    pub friction_scale: f32,
    pub contact_cfm: f32,
    pub contact_erp: f32,
    pub contact_cfm_ragdoll: f32,
    pub contact_erp_ragdoll: f32,
    pub self_collision_scale: f32,
}

impl Default for Dvars {
    fn default() -> Dvars {
        Dvars {
            friction_scale: 1.0,
            contact_cfm: 1e-5,
            contact_erp: 0.8,
            contact_cfm_ragdoll: 0.001,
            contact_erp_ragdoll: 0.3,
            self_collision_scale: 1.2,
        }
    }
}

/// What the near callback reads and writes on a body's phys object (its
/// userdata, body +0xc): friction (+0x44), bounce (+0x48) and the state
/// (+0x4c): 2 after a collide with no contact, 1 once two of its contacts
/// face each other (0x5a2280).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Obj {
    pub friction: f32,
    pub bounce: f32,
    pub state: i32,
}

/// The ragdolls' world (`PHYS_WORLD_RAGDOLL`), whose contacts take
/// `phys_contact_cfm_ragdoll` and `_erp_ragdoll` (0x5a603d).
pub const RAGDOLL_WORLD: usize = 2;

/// The near callback's surface (0x5a5fbd..0x5a60a4) for a body against
/// the world: `mu` its friction (plus the world's 0) times
/// `phys_frictionScale`, `mu2` 0, bounce the lesser of its own and 1
/// (0x409400), `bounce_vel` 0.1, and the world's contact ERP and CFM.
pub fn world_surface(obj: &Obj, dv: &Dvars, world: usize) -> Surface {
    let f = (w(obj.friction) + 0.0) as f32;
    let bounce = if ((1.0 - w(obj.bounce)) as f32) < 0.0 {
        1.0
    } else {
        obj.bounce
    };
    let (soft_erp, soft_cfm) = if world == RAGDOLL_WORLD {
        (dv.contact_erp_ragdoll, dv.contact_cfm_ragdoll)
    } else {
        (dv.contact_erp, dv.contact_cfm)
    };
    Surface {
        mode: MODE,
        mu: (w(dv.friction_scale) * w(f)) as f32,
        mu2: 0.0,
        bounce,
        bounce_vel: BOUNCE_VEL,
        soft_erp,
        soft_cfm,
        ..Surface::default()
    }
}

/// 0x5a5f20 for a capsule body against the world: its contacts
/// ([`collide_world`] under the ragdoll mask, room for 0x80); none sets
/// the state 2; five or more go through 0x5a2320's cut (at most 15,
/// [`crate::physics`]'s `reduce`); then [`contact_pass`] with the body as
/// `b1` and the world as `b2`. The impact callback after it (0x5a5de0)
/// and the jitter regions (0x5a23e0) are left out.
pub fn near_world(
    world: &mut World,
    body: BodyId,
    obj: &mut Obj,
    cap: &Capsule,
    clip: &dyn Pieces,
    dv: &Dvars,
    world_index: usize,
) {
    let contacts = collide_world(clip, cap, crate::ragdoll::CLIP_MASK, MAX_CONTACTS);
    if contacts.is_empty() {
        obj.state = 2;
        return;
    }
    let surface = world_surface(obj, dv, world_index);
    let contacts = if contacts.len() >= 5 {
        cut(&contacts)
    } else {
        contacts
    };
    contact_pass(world, contacts, [Some((body, obj)), None], surface);
}

/// 0x5a2320's cut: the contacts it copies out, in its order.
pub fn cut(c: &[ContactGeom]) -> Vec<ContactGeom> {
    let v: Vec<(Vec3, Vec3, f32)> = c
        .iter()
        .map(|c| (Vec3::from(c.pos), Vec3::from(c.normal), c.depth))
        .collect();
    crate::physics::reduce(&v)
        .into_iter()
        .map(|i| c[i])
        .collect()
}

/// The contact pass (0x5a2760) on contacts for `b1` against `b2` (a body
/// and its phys object, or the world): 0x5a2280 sets both bodies' state 1
/// when two contacts face each other; with `b1`'s state 1, else `b2`'s,
/// 0x5a25d0 keeps one of each facing pair by that body's position; then
/// each contact a body in state 1 is leaving (its point's velocity,
/// normalized, along the normal over 0.01) is skipped and the rest become
/// contact joints, until the world's 0x1000 joints or the group's 0x1000
/// contacts are reached.
pub fn contact_pass(
    world: &mut World,
    mut c: Vec<ContactGeom>,
    mut bodies: [Option<(BodyId, &mut Obj)>; 2],
    surface: Surface,
) {
    if pinched(&c) {
        for (_, o) in bodies.iter_mut().flatten() {
            o.state = 1;
        }
    }
    let flags = bodies
        .each_ref()
        .map(|b| b.as_ref().is_some_and(|(_, o)| o.state == 1));
    let ids = bodies.each_ref().map(|b| b.as_ref().map(|(id, _)| *id));
    if let Some(k) = (0..2).find(|&k| flags[k]) {
        let p = world.body(ids[k].expect("flagged body")).pos;
        unwedge(&mut c, p);
    }
    'contacts: for g in c {
        for k in 0..2 {
            if flags[k] {
                let b = world.body(ids[k].expect("flagged body"));
                let v = normalize3(point_vel(b, g.pos));
                if dot_yx(g.normal, v) > SEPARATING {
                    continue 'contacts;
                }
            }
        }
        if world.joint_count() >= MAX_JOINTS || world.contact_count() >= MAX_JOINTS {
            return;
        }
        world.create_contact(
            Contact::new(surface, g.pos, g.normal, g.depth),
            ids[0],
            ids[1],
        );
    }
}

/// `(a1 b1 + a0 b0) + a2 b2` stored, the order 0x5a2874 loads it in.
fn dot_yx(a: V3, b: V3) -> f32 {
    ((w(a[1]) * w(b[1]) + w(a[0]) * w(b[0])) + w(a[2]) * w(b[2])) as f32
}

/// 0x5a2280: two contacts whose normals' stored dot is under -0.99.
fn pinched(c: &[ContactGeom]) -> bool {
    (0..c.len()).any(|i| {
        (i + 1..c.len()).any(|j| {
            let d = ((w(c[j].normal[0]) * w(c[i].normal[0])
                + w(c[j].normal[1]) * w(c[i].normal[1]))
                + w(c[j].normal[2]) * w(c[i].normal[2])) as f32;
            w(d) < FACING
        })
    })
}

/// 0x5a25d0: of a facing pair `i < j` the first stays when `p` lies on
/// its plane or in front (`n_i . p - n_i . x_i` not under 0), else the
/// second takes its slot; the last contact then takes the second's slot,
/// and the walk goes on from the slot after it.
fn unwedge(c: &mut Vec<ContactGeom>, p: V3) {
    let mut i = 0;
    while i + 1 < c.len() {
        let mut j = i + 1;
        while j < c.len() {
            let ni = c[i].normal;
            let d = ((w(c[j].normal[0]) * w(ni[0]) + w(c[j].normal[1]) * w(ni[1]))
                + w(c[j].normal[2]) * w(ni[2])) as f32;
            if w(d) < FACING {
                let a = dot_yx(p, ni);
                let b = dot_yx(ni, c[i].pos);
                if w(a) - w(b) < 0.0 {
                    c[i] = c[j];
                }
                c.swap_remove(j);
            }
            j += 1;
        }
        i += 1;
    }
}

/// `dBodyGetPointVel` (0x5b70d0): `lvel + avel x (p - pos)`, the offset
/// stored.
pub fn point_vel(b: &super::Body, p: V3) -> V3 {
    let r = sub(p, b.pos);
    let (a, l) = (b.avel, b.lvel);
    [
        ((w(a[1]) * w(r[2]) - w(a[2]) * w(r[1])) + w(l[0])) as f32,
        ((w(a[2]) * w(r[0]) - w(r[2]) * w(a[0])) + w(l[1])) as f32,
        ((w(r[1]) * w(a[0]) - w(r[0]) * w(a[1])) + w(l[2])) as f32,
    ]
}
