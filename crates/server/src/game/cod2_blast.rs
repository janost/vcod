//! CoD2's `G_RadiusDamage` (lnxded 0x81022a0, Mac 0x1c45b8) and
//! `CanDamage` (Mac 0x1c3f18) over any clip, and the `radiusDamage`
//! builtin's walk over the host (`GScr_RadiusDamage`, Mac 0x19947a).
//! docs/research/cod2-combat.md 7 and 7.1.
//!
//! The missiles' blast (`Cod2Level::radius_damage`) and the builtin share
//! the measure; each hands the result to its own `G_Damage`.

use glam::Vec3;
use vcod_cod2::collision::MASK_BLAST;
use vcod_cod2::pmove::PlayerState;
use vcod_common::asset_source::RawFile;
use vcod_gsc::{Cx, EntId, Target, Value};

use super::host::GameHost;

/// The mask of the tenth's capsule trace (`G_TraceCapsule(.., 0x811)`).
const TENTH_MASK: u32 = 0x811;
/// `CanDamage`'s side offset (rodata 0x3264c4, 0x326534: 15 and -15).
const SIDE: f32 = 15.0;

/// A playing client as `CanDamage` measures it: the box, and the eye
/// `G_GetPlayerViewOrigin` gives.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BlastBody {
    /// `r.currentOrigin`, which a shot's antilag rewind moves
    /// (cod2-combat.md 2.3.2).
    pub origin: Vec3,
    pub mins: Vec3,
    pub maxs: Vec3,
    pub eye: Vec3,
    /// `ps.origin`, which the rewind leaves alone: `cloneplayer`'s.
    pub ps_origin: Vec3,
}

impl BlastBody {
    pub fn of(ps: &PlayerState) -> BlastBody {
        BlastBody {
            origin: ps.origin,
            mins: ps.mins,
            maxs: ps.maxs,
            eye: super::cod2::Cod2Level::view_origin(ps),
            ps_origin: ps.origin,
        }
    }
}

/// One candidate of the walk: where `G_RadiusDamage` measures it from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Victim {
    /// `r.currentOrigin`.
    pub origin: Vec3,
    /// The link's `absmin`/`absmax`.
    pub absmin: Vec3,
    pub absmax: Vec3,
    /// `r.bmodel`: measured by its box, not its origin.
    pub brush: bool,
    /// A client's body, for `CanDamage`'s client arm.
    pub client: Option<BlastBody>,
}

/// `G_RadiusDamage`'s query box about `at`: `radius` (at least 1) times
/// `sqrt 2` each way (rodata 0x326570, 0x326188).
pub fn search_box(at: Vec3, radius: f32) -> (Vec3, Vec3) {
    let half = radius.max(1.0) * std::f32::consts::SQRT_2;
    (at - Vec3::splat(half), at + Vec3::splat(half))
}

/// What one victim takes: the damage `G_Damage` is handed, rounded (the
/// lnxded's `ROUND` of the product), and the direction, the victim's
/// origin less the blast raised 24 (rodata 0x326470). `None` past the
/// radius, or shielded with no tenth. `clear(a, b, mask)` is a trace that
/// meets nothing.
pub fn victim_damage(
    clear: &dyn Fn(Vec3, Vec3, u32) -> bool,
    v: &Victim,
    at: Vec3,
    inner: f32,
    outer: f32,
    radius: f32,
) -> Option<(i32, Vec3)> {
    let radius = radius.max(1.0);
    let d = if v.brush {
        Vec3::from_array(std::array::from_fn(|k| {
            if at[k] < v.absmin[k] {
                v.absmin[k] - at[k]
            } else if at[k] > v.absmax[k] {
                at[k] - v.absmax[k]
            } else {
                0.0
            }
        }))
    } else {
        v.origin - at
    };
    let dist = d.length();
    if dist >= radius {
        return None;
    }
    let f = 1.0 - dist / radius;
    let share = match &v.client {
        Some(b) => client_share(clear, b, at),
        None => entity_share(clear, v, at),
    };
    let mult = if share > 0.0 {
        share
    } else {
        // A shielded victim takes a tenth (rodata 0x326388) when the
        // capsule trace to its box's centre is cut short and that centre is
        // within a fifth of the radius (0x3262dc).
        let centre = (v.absmin + v.absmax) * 0.5;
        if clear(at, centre, TENTH_MASK) || centre.distance(at) >= 0.2 * radius {
            return None;
        }
        0.1
    };
    let damage = ((outer + f * (inner - outer)) * mult).round_ties_even() as i32;
    let mut dir = v.origin - at;
    dir.z += 24.0;
    Some((damage, dir))
}

/// `CanDamage`'s client arm: five probes from the blast to the middle of
/// eye and feet and four points 15 either side across the line to the
/// blast, half the eye's height up and down; none 0, one to three `n / 3`,
/// four or five 1.
pub fn client_share(clear: &dyn Fn(Vec3, Vec3, u32) -> bool, b: &BlastBody, at: Vec3) -> f32 {
    let centre = (b.eye + b.origin) * 0.5;
    let half = (b.eye.z - b.origin.z) * 0.5;
    let mut d = at - b.origin;
    d.z = 0.0;
    let d = d.normalize_or_zero();
    let side = Vec3::new(-d.y, d.x, 0.0);
    let probes = [
        centre,
        centre + side * SIDE + Vec3::Z * half,
        centre + side * SIDE - Vec3::Z * half,
        centre - side * SIDE + Vec3::Z * half,
        centre - side * SIDE - Vec3::Z * half,
    ];
    match probes.iter().filter(|&&p| clear(at, p, MASK_BLAST)).count() {
        0 => 0.0,
        n @ 1..=3 => n as f32 / 3.0,
        _ => 1.0,
    }
}

/// `CanDamage`'s arm for an entity with no client: its link's centre, then
/// the four corners 15 out on x and y at the centre's height, each traced
/// to the blast; 1 on the first that passes, else 0.
pub fn entity_share(clear: &dyn Fn(Vec3, Vec3, u32) -> bool, v: &Victim, at: Vec3) -> f32 {
    let c = (v.absmin + v.absmax) * 0.5;
    let probes = [
        c,
        c + Vec3::new(SIDE, SIDE, 0.0),
        c + Vec3::new(SIDE, -SIDE, 0.0),
        c + Vec3::new(-SIDE, SIDE, 0.0),
        c + Vec3::new(-SIDE, -SIDE, 0.0),
    ];
    if probes.iter().any(|&p| clear(p, at, MASK_BLAST)) {
        1.0
    } else {
        0.0
    }
}

/// The host's clear-trace for [`victim_damage`]: the world's clip and the
/// linked brush models, no props.
fn host_clear(host: &GameHost) -> impl Fn(Vec3, Vec3, u32) -> bool + '_ {
    move |a, b, mask| {
        host.world
            .as_ref()
            .is_none_or(|w| w.collision.point_trace(a, b, mask, false).fraction >= 1.0)
    }
}

/// The entities with no client a walk about `at` finds, in the area
/// tree's order: those with `takedamage`, each with its link.
pub fn entity_victims(
    host: &mut GameHost,
    cx: &mut Cx,
    at: Vec3,
    radius: f32,
) -> Vec<(EntId, Victim)> {
    use vcod_gsc::Host;
    let (lo, hi) = search_box(at, radius);
    let origin = cx.intern_folded("origin");
    let ids: Vec<u32> = host.area.entities_in_box(lo.into(), hi.into(), -1);
    let mut out = Vec::new();
    for n in ids {
        let Some(id) = host.ents.handle(n) else {
            continue;
        };
        let Some(e) = host.ents.get(id) else {
            continue;
        };
        let brush = e.brush_model.is_some();
        if e.client.is_some() || !host.entity_takes_damage(id) {
            continue;
        }
        let Some(link) = host.area.last_link(n) else {
            continue;
        };
        let at = match host.get_field(cx, id, origin) {
            Value::Vector(v) => Vec3::from(v),
            _ => (Vec3::from(link.absmin) + Vec3::from(link.absmax)) * 0.5,
        };
        out.push((
            id,
            Victim {
                origin: at,
                absmin: Vec3::from(link.absmin),
                absmax: Vec3::from(link.absmax),
                brush,
                client: None,
            },
        ));
    }
    out
}

/// `G_Damage`'s arm for an entity with no client on every entity a blast
/// at `at` reaches: `health`, `damage` and `death` notifies. The
/// attacker is `attacker`, the world when `None`. `engine` (a missile's
/// blast, outside any builtin) queues the notifies on
/// `GameHost::engine_notifies` instead of raising them on `cx`.
#[allow(clippy::too_many_arguments)]
pub fn damage_entities(
    host: &mut GameHost,
    cx: &mut Cx,
    at: Vec3,
    inner: f32,
    outer: f32,
    radius: f32,
    attacker: Option<EntId>,
    inflictor: Option<EntId>,
    mod_: &str,
    engine: bool,
) {
    let victims = entity_victims(host, cx, at, radius);
    for (id, v) in victims {
        if Some(id) == inflictor {
            continue;
        }
        let Some((damage, dir)) = victim_damage(&host_clear(host), &v, at, inner, outer, radius)
        else {
            continue;
        };
        let world = host.ents.world(cx);
        let attacker = attacker.unwrap_or(world);
        let hit = crate::game::host::EntityHit {
            inflictor: inflictor.unwrap_or(attacker),
            mod_: mod_.to_string(),
            dir: dir.to_array(),
            point: at.to_array(),
        };
        for (event, args) in host.damage_nonclient(cx, id, damage, attacker, &hit) {
            if engine {
                // Load-time `with_cx` takes no notify: the engine queue's.
                host.engine_notifies.push((id, event, args));
            } else {
                let event = cx.intern_folded(event);
                cx.notify(Target::Entity(id), event, args);
            }
        }
    }
}

/// `radiusDamage(origin, range, maxDamage, minDamage)` on CoD2
/// (`GScr_RadiusDamage`, Mac 0x19947a): `G_RadiusDamage` with no
/// inflictor, the world (entity 1022) as the attacker and
/// `MOD_EXPLOSIVE` (14), under the level's `setPlayerIgnoreRadiusDamage`
/// flag for the walk only. VERIFIED, the call. Each playing client the
/// blast reaches gets `CodeCallback_PlayerDamage` with an undefined
/// inflictor, the world, `DFLAG_RADIUS`, weapon and hit location
/// `"none"` and a `timeOffset` of 0, its damage times the hit-location
/// table's `none` row (`G_Damage`, Mac 0x1c4272, the world's weapon being
/// 0). Every entity with `takedamage` takes `G_Damage`'s entity arm.
///
/// Retail walks the area tree once and runs each victim's callback before
/// measuring the next; vcod measures every victim first, the clients in
/// slot order ahead of the entities, then starts the callbacks in that
/// order (each runs to its first `wait` before the builtin returns).
/// INFERRED to differ only when a callback moves another victim.
pub fn radius_damage(
    host: &mut GameHost,
    cx: &mut Cx,
    at: Vec3,
    radius: f32,
    max_damage: f32,
    min_damage: f32,
) {
    let ignore = host.ignore_radius_damage;
    host.blast_noises.push(at.to_array());
    let none_mult = hitloc_none(host);
    let mut clients = Vec::new();
    {
        let clear = host_clear(host);
        for (slot, state) in host.cod2().clients.iter().enumerate() {
            let Some(b) = state.body else {
                continue;
            };
            if ignore || !host.client_vitals[slot].takedamage {
                continue;
            }
            let v = Victim {
                origin: b.origin,
                absmin: b.origin + b.mins,
                absmax: b.origin + b.maxs,
                brush: false,
                client: Some(b),
            };
            if let Some((damage, dir)) =
                victim_damage(&clear, &v, at, max_damage, min_damage, radius)
            {
                let damage = (none_mult * damage as f32) as i32;
                if damage > 0 {
                    clients.push((slot, damage, dir));
                }
            }
        }
    }
    let world = host.ents.world(cx);
    let (mod_, none) = (cx.intern_exact("MOD_EXPLOSIVE"), cx.intern_exact("none"));
    let callback = cx.func_ref(
        crate::game::script::CALLBACK_SETUP,
        "CodeCallback_PlayerDamage",
    );
    host.cod2_mut().callback = Default::default();
    for (slot, damage, dir) in clients {
        let Some(victim) = host.ents.handle(slot as u32) else {
            continue;
        };
        let args = vec![
            Value::Undefined,
            Value::Entity(world),
            Value::Int(damage),
            Value::Int(super::cod2_combat::DFLAG_RADIUS),
            Value::String(mod_),
            Value::String(none),
            Value::Vector(at.to_array()),
            Value::Vector(dir.to_array()),
            Value::String(none),
        ];
        let args = host.callback_args("CodeCallback_PlayerDamage", args);
        cx.spawn(callback, Some(Target::Entity(victim)), args);
    }
    damage_entities(
        host,
        cx,
        at,
        max_damage,
        min_damage,
        radius,
        None,
        None,
        "MOD_EXPLOSIVE",
        false,
    );
}

impl super::cod2::Cod2Level {
    /// `G_Damage` on an entity with no client (Mac 0x1c4272): nothing
    /// without `takedamage`; else the damage, at least 1, comes off its
    /// `health` with the `damage` notify (damage, attacker), and `death` at
    /// 0 or below; a `trigger_damage` fires off it. `attacker` is a client
    /// slot.
    #[allow(clippy::too_many_arguments)]
    pub fn entity_damage(
        &mut self,
        id: EntId,
        attacker: usize,
        inflictor: Option<EntId>,
        damage: i32,
        dflags: i32,
        mod_: &'static str,
        point: Vec3,
        dir: Vec3,
    ) {
        if !self.rt.host.entity_takes_damage(id) {
            return;
        }
        self.rt.deliver_entity_hit(&super::combat::EntityHit {
            target: id,
            attacker,
            inflictor,
            damage,
            dflags,
            mod_,
            point: point.to_array(),
            dir: dir.to_array(),
        });
    }

    /// `G_RunMissile`'s impact on an entity with no client: the missile's
    /// weapon `damage` as `MOD_PROJECTILE`, the missile the inflictor
    /// (Mac 0x1ca0e4, `ent+0x19c`).
    pub fn impact_entity(&mut self, i: usize, id: EntId, at: Vec3) {
        let m = self.combat.missiles[i].clone();
        let damage = self
            .combat
            .weapons
            .get(usize::from(m.weapon))
            .and_then(Option::as_ref)
            .map_or(0, |w| w.damage);
        if damage == 0 {
            return;
        }
        let dir = m.delta.normalize_or(Vec3::Z);
        let owner = m
            .owner
            .unwrap_or(crate::game::host::ENTITYNUM_NONE as usize);
        self.entity_damage(id, owner, Some(m.id), damage, 0, "MOD_PROJECTILE", at, dir);
    }
}

/// `g_fHitLocDamageMult[0]`, the `none` row of `info/mp_lochit_dmgtable`
/// (1 when the table is missing).
fn hitloc_none(host: &GameHost) -> f32 {
    host.fs
        .as_deref()
        .and_then(|fs| fs.load::<RawFile>("info/mp_lochit_dmgtable").ok())
        .and_then(|b| super::cod2_combat::parse_hitloc_table(&b.text()))
        .map_or(1.0, |t| t[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(_: Vec3, _: Vec3, _: u32) -> bool {
        true
    }

    fn shut(_: Vec3, _: Vec3, _: u32) -> bool {
        false
    }

    fn standing(at: Vec3) -> Victim {
        let b = BlastBody {
            origin: at,
            mins: Vec3::new(-15.0, -15.0, 0.0),
            maxs: Vec3::new(15.0, 15.0, 70.0),
            eye: at + Vec3::Z * 60.0,
            ps_origin: at,
        };
        Victim {
            origin: at,
            absmin: at + b.mins,
            absmax: at + b.maxs,
            brush: false,
            client: Some(b),
        }
    }

    /// The damage falls from the inner to the outer linearly with the
    /// distance, rounded; nothing at or past the radius.
    #[test]
    fn the_blast_falls_off_and_stops_at_the_radius() {
        let at = Vec3::ZERO;
        let v = standing(Vec3::new(100.0, 0.0, 0.0));
        let (dmg, dir) = victim_damage(&open, &v, at, 200.0, 50.0, 200.0).unwrap();
        assert_eq!(dmg, 125);
        assert_eq!(dir, Vec3::new(100.0, 0.0, 24.0));
        assert!(victim_damage(&open, &standing(Vec3::X * 200.0), at, 200.0, 50.0, 200.0).is_none());
    }

    /// A body nothing reaches takes a tenth only when it stands within a
    /// fifth of the radius of a blast whose centre trace is cut short.
    #[test]
    fn a_shielded_body_takes_a_tenth_up_close() {
        let at = Vec3::ZERO;
        let close = standing(Vec3::new(20.0, 0.0, -35.0));
        let (dmg, _) = victim_damage(&shut, &close, at, 300.0, 100.0, 300.0).unwrap();
        // 1 - 40.31 / 300 of the way from 100 to 300, a tenth.
        assert_eq!(
            dmg,
            (0.1f32 * (100.0 + (1.0 - 40.31129 / 300.0) * 200.0)).round() as i32
        );
        let far = standing(Vec3::new(150.0, 0.0, -35.0));
        assert!(victim_damage(&shut, &far, at, 300.0, 100.0, 300.0).is_none());
    }

    /// A brush model is measured from the nearest point of its box.
    #[test]
    fn a_brush_is_measured_by_its_box() {
        let v = Victim {
            origin: Vec3::new(500.0, 0.0, 0.0),
            absmin: Vec3::new(50.0, -10.0, -10.0),
            absmax: Vec3::new(60.0, 10.0, 10.0),
            brush: true,
            client: None,
        };
        let (dmg, _) = victim_damage(&open, &v, Vec3::ZERO, 100.0, 0.0, 100.0).unwrap();
        assert_eq!(dmg, 50);
    }

    #[test]
    fn the_client_share_counts_its_probes() {
        let b = standing(Vec3::new(100.0, 0.0, 0.0)).client.unwrap();
        assert_eq!(client_share(&open, &b, Vec3::ZERO), 1.0);
        assert_eq!(client_share(&shut, &b, Vec3::ZERO), 0.0);
        // Only the middle probe passes: a third.
        let middle = |_: Vec3, p: Vec3, _: u32| p.y == 0.0;
        assert_eq!(client_share(&middle, &b, Vec3::ZERO), 1.0 / 3.0);
    }
}
