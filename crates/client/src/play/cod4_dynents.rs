//! The fired events' pushes on the map's dynamic entities
//! ([`vcod_cod4::dynent`], cod4-world.md 13): each explode arm of
//! `CG_EntityEvent` calls `DynEntCl_ExplosionEvent` (iw3mp.exe 0x48e670)
//! after `Ragdoll_ExplosionEvent`, and each client-side round
//! `DynEntCl_EntityImpactEvent` (0x48e0f0).

use glam::Vec3;
use vcod_cod4::dynent::{Broken, DynEnts};

use super::cod4::{Fired, weapon_name};
use super::cod4_predict::Cod4World;

/// `fired`'s blasts and `rounds` (per fired event, each round's start and
/// end) on `d` at `now`; returns what broke.
pub fn on_fired(
    d: &mut DynEnts,
    world: &Cod4World,
    fired: &[Fired],
    weapons: &[String],
    rounds: &[Vec<(Vec3, Vec3)>],
    now: i32,
) -> Vec<Broken> {
    let mut broken = Vec::new();
    for (i, f) in fired.iter().enumerate() {
        let def = weapon_name(weapons, f.weapon)
            .and_then(|w| world.defs.get(w))
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
        // A round does its weapon's `damage` (0x455b9e).
        let damage = def.map_or(0, |w| w.damage);
        for &(start, end) in rounds.get(i).into_iter().flatten() {
            broken.extend(d.bullet(start, end, damage, now).1);
        }
    }
    broken
}
