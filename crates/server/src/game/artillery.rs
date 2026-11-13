//! United Offensive's artillery strike: what `Weapon_ArtilleryStrike_Fire`
//! (game.mp.uo.i386.so 0x7b758) starts once the binoculars' trace finds
//! ground. `fire_artillery_barrage` (0x601fb) spawns an unsent entity at the
//! strike point whose think, `G_BarrageThink` (0x5e7b5), raises the launch
//! and incoming temp events and hangs one shell per incoming over a
//! scattered point (`fire_artillery`, 0x5ff4a; `crate::game::missile`).
//! docs/research/coduo-diff.md 21 holds the measurements.

use glam::Vec3;
use vcod_common::net::event_ids::{EV_UO_PROJECTILE_INCOMING, EV_UO_PROJECTILE_LAUNCH};
use vcod_gsc::EntId;

/// `fire_artillery_barrage`'s first think, and each think's re-arm.
pub const THINK_MS: i32 = 50;
/// The first launch's lead over the barrage's spawn.
const FIRST_LAUNCH_MS: i32 = 250;
/// `G_BarrageThink` hands `fire_artillery` the scattered point this much
/// up, and `fire_artillery` adds as much again.
const DROP_LIFT: f32 = 1000.0;
/// `G_BarrageThink`'s delay from a shell's incoming to its drop.
pub const DROP_DELAY_MS: i32 = 1750;

/// One strike between its spawn and its free.
#[derive(Clone, Debug)]
pub struct Barrage {
    pub id: EntId,
    /// The caller's slot, `r.ownerNum`, which his shells take; `None` once
    /// he left (`G_FreeEntityRefs`' sweep, combat doc 13.7).
    pub owner: Option<usize>,
    pub weapon: u8,
    pub at: Vec3,
    /// The weapon file's `projectileCount`, `projectileRadius`,
    /// `projectileSpacingMin/Max`, `projExplosionType` index, the drop's
    /// speed and the shells' damage.
    pub count: i32,
    pub radius: f32,
    pub spacing: (i32, i32),
    pub explosion_type: i32,
    pub speed: f32,
    pub damage: i32,
    pub launched: i32,
    pub launch_next_ms: i32,
    pub dropped: i32,
    pub incoming_next_ms: i32,
    pub next_think_ms: i32,
    /// Both counts ran out: the next think frees it.
    pub done: bool,
}

/// What one think did, in the order it did it.
#[derive(Debug, PartialEq)]
pub enum Out {
    /// `EV_PROJECTILE_LAUNCH` at the strike point, parm the explosion type.
    Launch(Vec3),
    /// `EV_PROJECTILE_INCOMING` at the scattered point, then the shell
    /// hung over it.
    Incoming(Vec3),
}

impl Barrage {
    /// `fire_artillery_barrage` at `level_ms`, the time the firing cmd ran
    /// under.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: EntId,
        owner: usize,
        weapon: u8,
        at: Vec3,
        def: &vcod_common::weapon::WeaponDef,
        level_ms: i32,
    ) -> Barrage {
        Barrage {
            id,
            owner: Some(owner),
            weapon,
            at,
            count: def.projectile_count,
            radius: def.projectile_radius as f32,
            spacing: def.projectile_spacing,
            explosion_type: crate::game::missile::explosion_type(&def.proj_explosion_type),
            speed: def.projectile_speed,
            damage: def.damage,
            launched: 0,
            launch_next_ms: level_ms + FIRST_LAUNCH_MS,
            dropped: 0,
            incoming_next_ms: level_ms + def.projectile_delay,
            next_think_ms: level_ms + THINK_MS,
            done: false,
        }
    }

    /// One `G_BarrageThink` at `now_ms`: its `irand`s off the level's
    /// `holdrand`, its `crandom`s off `rand_unit`, the host's draw in
    /// [0, 1). `None` when its think is not due.
    pub fn think(
        &mut self,
        now_ms: i32,
        hold: &mut vcod_common::rng::HoldRand,
        mut rand_unit: impl FnMut() -> f32,
    ) -> Option<Vec<Out>> {
        if now_ms < self.next_think_ms {
            return None;
        }
        self.next_think_ms += THINK_MS;
        let mut out = Vec::new();
        if self.launched < self.count && self.launch_next_ms < now_ms {
            self.launched += 1;
            let gap = hold.irand(self.spacing.0, self.spacing.1);
            // `(int)(0.5 * irand + next)`, truncated.
            self.launch_next_ms = (0.5 * gap as f32 + self.launch_next_ms as f32) as i32;
            out.push(Out::Launch(self.at));
        }
        if self.dropped < self.count && self.incoming_next_ms < now_ms {
            self.dropped += 1;
            self.incoming_next_ms += hold.irand(self.spacing.0, self.spacing.1);
            let yaw = (hold.irand(0, 360) as f32).to_radians();
            // One `crandom` per axis along a random yaw's forward; its z is
            // 0, so the point keeps the strike's height.
            let mut crandom = || 2.0 * rand_unit() - 1.0;
            let at = Vec3::new(
                self.at.x + self.radius * crandom() * yaw.cos(),
                self.at.y + self.radius * crandom() * yaw.sin(),
                self.at.z + self.radius * crandom() * 0.0,
            );
            out.push(Out::Incoming(at));
        }
        if self.launched >= self.count && self.dropped >= self.count {
            self.done = true;
        }
        Some(out)
    }
}

/// The temp entity one [`Out`] raises.
pub fn temp_entity(out: &Out, explosion_type: i32) -> crate::game::temp_entity::TempEntity {
    let (event, at) = match out {
        Out::Launch(at) => (EV_UO_PROJECTILE_LAUNCH, at),
        Out::Incoming(at) => (EV_UO_PROJECTILE_INCOMING, at),
    };
    crate::game::temp_entity::TempEntity {
        event,
        parm: explosion_type,
        surf_type: 0,
        other: 0,
        attacker: 0,
        weapon: 0,
        client_num: 0,
        scale: 0,
        origin: at.to_array(),
        scope: crate::game::temp_entity::Scope::Pvs,
        quake: None,
        origin2: None,
    }
}

/// Where `fire_artillery` hangs the shell for an incoming at `at`: the
/// barrage's lift (the missile adds its own).
pub fn drop_point(at: Vec3) -> Vec3 {
    at + Vec3::Z * DROP_LIFT
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arty() -> vcod_common::weapon::WeaponDef {
        vcod_common::weapon::WeaponDef {
            projectile_count: 15,
            projectile_radius: 850,
            projectile_delay: 6000,
            projectile_spacing: (300, 1800),
            projectile_speed: 10_000.0,
            damage: 2000,
            proj_explosion_type: "artillery".into(),
            ..Default::default()
        }
    }

    /// The capture's shape: the first launch 250 ms past the cmd's level
    /// time, the first incoming `projectileDelay` past it, 15 of each, the
    /// launches about twice as dense as the incomings, every incoming
    /// inside `projectileRadius` at the strike's height, then the free.
    #[test]
    fn a_barrage_launches_and_drops_fifteen_shells() {
        let at = Vec3::new(3071.0, -2176.0, 0.0);
        let mut b = Barrage::new(EntId(160, 0), 0, 6, at, &arty(), 13_600);
        let mut seed = 1u64;
        let mut hold = vcod_common::rng::HoldRand(1);
        let mut launches = Vec::new();
        let mut incomings = Vec::new();
        let mut t = 13_600;
        while !b.done {
            t += 50;
            for o in b
                .think(t, &mut hold, || crate::game::host::rand_unit(&mut seed))
                .unwrap()
            {
                match o {
                    Out::Launch(p) => {
                        assert_eq!(p, at);
                        launches.push(t);
                    }
                    Out::Incoming(p) => {
                        assert!((p - at).truncate().length() <= 850.0 * 2f32.sqrt());
                        assert_eq!(p.z, at.z);
                        incomings.push(t);
                    }
                }
            }
        }
        assert_eq!((launches.len(), incomings.len()), (15, 15));
        assert_eq!(launches[0], 13_900, "the first launch");
        assert_eq!(incomings[0], 19_650, "the first incoming");
        for w in incomings.windows(2) {
            assert!((300..=1850).contains(&(w[1] - w[0])), "{w:?}");
        }
        assert!(launches.last() < incomings.last());
    }
}
