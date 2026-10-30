//! `--probe-flame`: a United Offensive flamethrower victim. Runs the play
//! client's flame chunks and report (`fx::flame`) off every snapshot, with
//! each firing body's nozzle at its eye along its view in place of the
//! posed `tag_flash`, holds `wbuttons` 0x1 while the report does and sends
//! the `fdc` it names. Prints every `fdc`, every flag change and every
//! health change; pairs with `client-probes/probe_flame` (recipe in
//! docs/research/coduo-diff.md 19, "The flame report"). Writes no fixture.

use std::collections::HashMap;
use std::time::Instant;

use glam::Vec3;
use vcod_common::collision::CollisionWorld;
use vcod_common::net::msg::{NULL_USERCMD, UserCmd, WBUTTON_IN_FLAME_V22};
use vcod_common::net::protocol::Protocol;
use vcod_common::net::snapshot::Snapshot;
use vcod_common::weapon::WeaponDef;

use crate::fx::flame::{ChunkWorld, FlameSystem, Victim, nozzles};

/// A standing body's view height, where the nozzle is put.
const EYE: f32 = 60.0;
/// How far ahead of the eye the nozzle sits.
const NOZZLE: f32 = 16.0;

#[derive(Default)]
pub struct FlameProbe {
    flame: FlameSystem,
    world: Option<Box<CollisionWorld>>,
    weapons: Vec<Option<WeaponDef>>,
    in_flame: bool,
    t0: Option<Instant>,
    last_ms: f32,
    health: Option<i32>,
    fdcs: u32,
    /// `sv_maxclients`, which the chunks' residue test reads.
    max_clients: u32,
}

impl FlameProbe {
    /// The map's collision, `sv_maxclients` and configstring 7's weapons,
    /// at a gamestate.
    pub fn load(&mut self, fs: Option<&vcod_common::pk3::Pk3Fs>, serverinfo: &str, cs7: &str) {
        self.max_clients = crate::fx::flame::max_clients(serverinfo);
        let map = vcod_common::net::info_value_for_key(serverinfo, "mapname").unwrap_or_default();
        let Some(fs) = fs else {
            println!("FLAME: no game data, no weapons or collision");
            return;
        };
        self.weapons = vcod_common::weapon_table::from_configstring(fs, cs7);
        if let Ok(bsp) = vcod_common::mapworld::find(fs, map) {
            let tris = vcod_common::props::collision_tris(fs, &bsp.entities);
            self.world = Some(Box::new(CollisionWorld::build(&bsp, &tris)));
        }
        println!(
            "FLAME: {} weapons, collision {}",
            self.weapons.len(),
            self.world.is_some()
        );
    }

    /// One frame at `now`: steps the chunks, runs the report and returns
    /// the `fdc` command to send, if any.
    pub fn frame(
        &mut self,
        now: Instant,
        p: &Protocol,
        snap: &Snapshot,
        flood_open: bool,
    ) -> Option<String> {
        let ms = now
            .duration_since(*self.t0.get_or_insert(now))
            .as_secs_f32()
            * 1000.0;
        let dt = (ms - self.last_ms).max(0.0);
        self.last_ms = ms;
        let own = snap.ps.field_i32(p, "clientNum");
        let mut muzzles = HashMap::new();
        for (&num, ent) in &snap.entities {
            let [pitch, yaw, _] = ent.angles(p).map(f32::to_radians);
            let dir = Vec3::new(
                pitch.cos() * yaw.cos(),
                pitch.cos() * yaw.sin(),
                -pitch.sin(),
            );
            let eye = Vec3::from(ent.origin(p)) + Vec3::Z * EYE;
            muzzles.insert(num, (eye + dir * NOZZLE, dir));
        }
        let firing = nozzles(p, snap, own, &self.weapons, &muzzles);
        let origin = Vec3::from(snap.ps.origin(p));
        let world = self.world.as_deref();
        let v = |n: &str| {
            Vec3::from_array([0, 1, 2].map(|i| snap.ps.field_f32(p, &format!("{n}[{i}]"))))
        };
        let victim = u32::try_from(own).ok().map(|client| Victim {
            client,
            origin,
            mins: v("mins"),
            maxs: v("maxs"),
        });
        let bodies = victim
            .map(|v| {
                crate::play::predict::solid_bodies(p, &snap.entities, v.client, &HashMap::new())
            })
            .unwrap_or_default();
        let chunk_world = world.map(|world| ChunkWorld {
            world,
            bodies: &bodies,
            local: victim,
            max_clients: self.max_clients,
        });
        self.flame.step(
            ms,
            dt,
            &firing,
            origin + Vec3::Z * EYE,
            chunk_world.as_ref(),
        );
        let trace = world.map(|w| {
            move |s: Vec3, e: Vec3| {
                let mw = vcod_common::movetrace::MoveWorld::new(w, &bodies, u32::MAX);
                let t = mw.box_trace(
                    s,
                    e,
                    Vec3::ZERO,
                    Vec3::ZERO,
                    crate::fx::client_bullet::MASK_CLIENT_SHOT,
                );
                (t.fraction, mw.entity_num(&t))
            }
        });
        let report = self.flame.report(
            ms,
            victim,
            flood_open,
            trace.as_ref().map(|t| t as crate::fx::flame::ShotTrace),
        );
        if report.in_flame != self.in_flame {
            println!(
                "FLAME t={} in_flame={} firing={}",
                snap.server_time,
                report.in_flame,
                firing.len()
            );
        }
        self.in_flame = report.in_flame;
        let health = snap.ps.arrays.stats[0];
        if self.health != Some(health) {
            println!("FLAME t={} health={health}", snap.server_time);
            self.health = Some(health);
        }
        report.fdc.map(|n| {
            self.fdcs += 1;
            println!(
                "FLAME t={} ms={ms:.0} fdc {n} (#{})",
                snap.server_time, self.fdcs
            );
            format!("fdc {n}")
        })
    }

    /// The cmd: still, with the flag the report holds.
    pub fn cmd(&self) -> UserCmd {
        let mut cmd = NULL_USERCMD;
        if self.in_flame {
            cmd.wbuttons |= WBUTTON_IN_FLAME_V22;
        }
        cmd
    }
}
