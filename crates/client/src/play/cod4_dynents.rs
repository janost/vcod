//! The fired events' pushes on the map's dynamic entities
//! ([`vcod_cod4::dynent`], cod4-world.md 13): each explode arm of
//! `CG_EntityEvent` calls `DynEntCl_ExplosionEvent` (iw3mp.exe 0x48e670)
//! after `Ragdoll_ExplosionEvent`, each client-side round
//! `DynEntCl_EntityImpactEvent` (0x48e0f0), `EV_FIRE_MELEE` the swing's
//! rays (0x48e3a0) and `EV_PHYS_JITTER` the jitter (0x48eb50).

use glam::Vec3;
use vcod_cod4::dynent::{Broken, DynEnts, Struck};
use vcod_cod4::ode::collide::JitterRegion;

use super::cod4::{ET_PLAYER, Fired, ent_f32, ent_u32, weapon_name};
use super::cod4_predict::Cod4World;
use crate::cod4_fx::OwnView;

/// `eFlags`' prone and crouch bits, which pick the eye height of another
/// player's swing (0x4585b4, 0x4585c4).
use vcod_cod4::net::flags::{EF_CROUCH, EF_PRONE};

/// What the fired events did to the dynamic entities.
#[derive(Default)]
pub struct Done {
    /// Those that broke.
    pub broken: Vec<Broken>,
    /// Where a round met one, by the fired event's index: its impact
    /// effect plays there (0x48dd00).
    pub struck: Vec<(usize, Struck)>,
    /// Where a swing met one: `melee_knife_hit_other` plays there on the
    /// world (0x48e2c8).
    pub swung: Vec<Vec3>,
}

/// Where a swing starts and looks (0x458550, 0x4584f0): the drawn view's
/// for our own (or the followed) player, else his origin raised 11
/// prone, 40 crouched or 60 standing (the doubles at 0x70b370, 0x70b380,
/// 0x70b2f8) and his angles.
fn swing_view(f: &Fired, own: Option<&OwnView>) -> Option<(Vec3, Vec3)> {
    if let Some(v) = own.filter(|v| f.entity.is_none() || f.entity == v.client) {
        return Some((v.eye, v.angles));
    }
    let e = f.state.as_ref().filter(|e| e.etype() == ET_PLAYER)?;
    let flags = ent_u32(e, "lerp.eFlags");
    let height = if flags & EF_PRONE != 0 {
        11.0
    } else if flags & EF_CROUCH != 0 {
        40.0
    } else {
        60.0
    };
    let angles = Vec3::new(
        ent_f32(e, "lerp.apos.trBase[0]"),
        ent_f32(e, "lerp.apos.trBase[1]"),
        0.0,
    );
    Some((f.origin + Vec3::Z * height, angles))
}

/// `fired`'s blasts, swings, jitters and `rounds` (per fired event, each
/// leg the dynamic entities see) on `d` at `now`; `own` is the drawn
/// view a swing of ours starts from.
pub fn on_fired(
    d: &mut DynEnts,
    world: &Cod4World,
    fired: &[Fired],
    weapons: &[String],
    rounds: &[Vec<(Vec3, Vec3)>],
    own: Option<&OwnView>,
    now: i32,
) -> Done {
    let mut done = Done::default();
    let broken = &mut done.broken;
    for (i, f) in fired.iter().enumerate() {
        let def = weapon_name(weapons, f.weapon)
            .and_then(|w| world.def(w))
            .map(|d| &d.0);
        let radius = def.map(|w| w.explosion_radius as f32);
        if let Some(e) = super::cod4_players::explosion_at(f, radius) {
            // A weapon's explode does its inner to outer damage
            // (0x436b4e); a script's push none (0x437254).
            let damage = match (f.name.starts_with("EV_PHYS_"), def) {
                (false, Some(w)) => [w.explosion_inner_damage, w.explosion_outer_damage],
                _ => [0, 0],
            };
            broken.extend(d.explode(&e, damage, now));
        }
        match f.name {
            // The swinger's weapon (none, nothing: 0x48e3ff) does its
            // `meleeDamage` (`WeaponDef` +0x354, 0x48e40e).
            "EV_FIRE_MELEE" if f.weapon != 0 => {
                let (Some(w), Some((eye, angles))) = (def, swing_view(f, own)) else {
                    continue;
                };
                let view = vcod_common::x87::angle_vectors(angles);
                let (s, b) = d.melee(eye, view, w.melee_damage, now);
                broken.extend(b);
                done.swung.extend(s.map(|s| s.at));
            }
            // `physicsJitter`: the outer radius as the parm (an unsigned
            // `fild`, 0x43738d), the inner and the two pushes in `lerp.u`
            // words 0 to 2 (0x437374..0x4373a3).
            "EV_PHYS_JITTER" => {
                let word = |k: usize| {
                    f.state
                        .as_ref()
                        .map_or(0.0, |e| ent_f32(e, &format!("lerp.u.anonymous.data[{k}]")))
                };
                let j =
                    JitterRegion::new(f.origin.into(), word(0), f.parm as f32, word(1), word(2));
                d.jitter(j, now);
            }
            _ => {}
        }
        // A round does its weapon's `damage` (0x455b9e).
        let damage = def.map_or(0, |w| w.damage);
        for &(start, end) in rounds.get(i).into_iter().flatten() {
            let (struck, b) = d.bullet(start, end, damage, now);
            broken.extend(b);
            done.struck.extend(struck.map(|s| (i, s)));
        }
    }
    done
}
