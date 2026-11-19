//! The events a snapshot brings ([`Fired`]): the playerstate's ring, the
//! entities' rings and the temp entities as they enter, and the other
//! players' aim.

use std::collections::{HashMap, HashSet};

use glam::Vec3;
use vcod_cod4::net::delta::EntityState;
use vcod_cod4::net::events::event_name;
use vcod_cod4::net::parse::{Client, Snapshot};

use super::scene::{predicts, trajectory, weapon_name};
use super::{
    Cod4Session, ET_EVENTS, ET_GENERAL, ET_MG42, ET_MISSILE, ET_PLAYER, EV_OBITUARY, Source,
    client_name, client_perks, ent_f32, ent_u32, ps_f32, vec3, weapon_list,
};
use crate::play::cod4_input::Cod4Input;
use crate::play::cod4_predict::Ring;

/// One event fired this step.
#[derive(Clone, Debug, PartialEq)]
pub struct Fired {
    pub id: u32,
    pub name: &'static str,
    /// The entity it rode, `None` for the playerstate's ring.
    pub entity: Option<u32>,
    pub server_time: i32,
    /// The ring entry's `eventParms[k]`, or an event entity's `eventParm`.
    pub parm: u32,
    /// The entity's `lerp.pos.trBase`, or the playerstate's origin.
    pub origin: Vec3,
    /// The entity's state in the snapshot that carried the event; `None`
    /// for the playerstate's ring.
    pub state: Option<EntityState>,
    /// The entity's weapon, or the playerstate's.
    pub weapon: u32,
    /// The perks of the client the entity names ([`client_perks`]), or
    /// of the playerstate's client.
    pub perks: u32,
    /// A bullet shot's seed and spread (`cod4_fx::client_bullets`), set
    /// where the session knows them.
    pub aim: Option<crate::cod4_fx::Aim>,
}

/// `CG_EntityEvent`'s `EV_STANCE_FORCE_*` off our playerstate's ring
/// (`iw3mp.exe` 0x436155, 0x436195, 0x4361d5): `CL_SetStance` (0x462870)
/// makes the stance keys' state the forced one unless `+prone` or
/// `+movedown` is held, so a sprint out of a crouch stays standing
/// (cod4-pmove.md 8).
pub(super) fn force_stance(input: &mut Cod4Input, fired: &[Fired]) {
    use crate::play::cod4_input::Stance;
    for f in fired.iter().filter(|f| f.entity.is_none()) {
        input.set_stance(match f.name {
            "EV_STANCE_FORCE_STAND" => Stance::Stand,
            "EV_STANCE_FORCE_CROUCH" => Stance::Crouch,
            "EV_STANCE_FORCE_PRONE" => Stance::Prone,
            _ => continue,
        });
    }
}

impl Cod4Session {
    pub(super) fn walk(&mut self) {
        let n = self.client().snapshots.len();
        while self.walked < n {
            let i = self.walked;
            self.walked += 1;
            let world = self.world.is_some() && !self.is_demo();
            let (fired, lines, ps_ring, rings, temps) = {
                let c = self.client();
                let s = &c.snapshots[i];
                let predicted = c
                    .gamestate
                    .as_ref()
                    .is_some_and(|g| predicts(world, g, &s.ps));
                walk_snapshot(c, s, self.ps_ring, predicted, &self.rings, &self.temps)
            };
            let fired = self.aim_others(i, fired);
            force_stance(&mut self.input, &fired);
            self.fired.extend(fired);
            self.lines.extend(lines);
            self.ps_ring = ps_ring;
            self.rings = rings;
            self.temps = temps;
        }
    }

    /// The level's weapon def `n` (0 the empty hand).
    pub(super) fn level_def(&self, n: u32) -> Option<&vcod_cod4::pmove::weapon::WeaponDef> {
        self.level_defs
            .as_ref()?
            .1
            .get(n as usize)
            .filter(|_| n != 0)
    }

    /// Another player's fire events in snapshot `i` take their spread
    /// ([`crate::cod4_fx::Aim::other`]): his stance and sight off his legs
    /// anim, his speed off his `trBase` in the snapshot before, seeded by
    /// our own `commandTime`, the predicted one where we predict. A gun's
    /// (`ET_MG42`) take its weapon's `playerSpread` on the same seed
    /// ([`crate::cod4_fx::Aim::turret`]).
    fn aim_others(&self, i: usize, mut fired: Vec<Fired>) -> Vec<Fired> {
        use vcod_cod4::pmove::anim::LegsPose;
        let c = self.client();
        let (Some(now), Some(prev)) = (
            c.snapshots.get(i),
            i.checked_sub(1).and_then(|k| c.snapshots.get(k)),
        ) else {
            return fired;
        };
        let seed = self
            .last_predicted
            .as_ref()
            .map_or(now.ps.get("commandTime").unwrap_or(0) as i32, |p| {
                p.command_time
            });
        let dt = now.server_time.wrapping_sub(prev.server_time);
        for f in fired
            .iter_mut()
            .filter(|f| f.name.starts_with("EV_FIRE_WEAPON"))
        {
            let (Some(n), Some(e)) = (f.entity, f.state.as_ref()) else {
                continue;
            };
            let Some(d) = self.level_def(f.weapon) else {
                continue;
            };
            if e.etype() == ET_MG42 {
                f.aim = Some(crate::cod4_fx::Aim::turret(d, seed));
                continue;
            }
            if e.etype() != ET_PLAYER {
                continue;
            }
            let base = |e: &EntityState| vec3(|k| ent_f32(e, &format!("lerp.pos.trBase[{k}]")));
            let mut v = prev
                .entities
                .iter()
                .find(|p| p.number() == n)
                .map_or(Vec3::ZERO, |p| base(e) - base(p));
            if dt > 0 {
                v *= 1000.0 / dt as f32;
            }
            let legs = match &self.legs_movetypes {
                Some(t) => {
                    let wire = ent_u32(e, "legsAnim") & !vcod_cod4::pmove::anim::ANIM_TOGGLEBIT;
                    LegsPose::of(t.get(wire as usize).copied().unwrap_or(0))
                }
                None => {
                    let flags = ent_u32(e, "lerp.eFlags");
                    LegsPose {
                        prone: flags & 8 != 0,
                        crouched: flags & 4 != 0,
                        sighted: false,
                    }
                }
            };
            f.aim = Some(crate::cod4_fx::Aim::other(d, legs, v.length(), seed));
        }
        fired
    }
}

type Walked = (
    Vec<Fired>,
    Vec<String>,
    Option<Ring>,
    HashMap<u32, u32>,
    HashSet<u32>,
);

/// The client a source reads, borrowed apart from the session's other
/// fields.
pub(super) fn source_client(source: &Source) -> &Client {
    match source {
        Source::Capture { session, .. } => &session.client,
        Source::Live { conn, .. } => &conn.client,
        Source::Demo(d) => &d.client,
    }
}

/// One snapshot's events against the state the last one left: the
/// playerstate ring past `ps_ring` unless the frame predicts it
/// (`predicted`: the predicted ring fires instead, [`Cod4Session::scene`]),
/// each entity's ring past its last `eventSequence` (an entity entering
/// starts its count), and each event entity that was not in the last
/// snapshot.
fn walk_snapshot(
    c: &Client,
    s: &Snapshot,
    ps_ring: Option<Ring>,
    predicted: bool,
    rings: &HashMap<u32, u32>,
    temps: &HashSet<u32>,
) -> Walked {
    let mut fired = Vec::new();
    let mut lines = Vec::new();
    let t = s.server_time;
    // `ev(k)` is ring slot `k`'s event and parm.
    let ring =
        |seq_from: u32, seq_to: u32, ev: &dyn Fn(u32) -> (u32, u32), ent: Option<&EntityState>| {
            let mut out = Vec::new();
            // At most the ring's four.
            let from = seq_from.max(seq_to.saturating_sub(4));
            for q in from..seq_to {
                let (id, parm) = ev(q & 3);
                if let Some(name) = event_name(id).filter(|_| id != 0) {
                    out.push(Fired {
                        id,
                        name,
                        entity: ent.map(EntityState::number),
                        server_time: t,
                        parm,
                        origin: ent.map_or_else(
                            || vec3(|k| ps_f32(&s.ps, &format!("origin[{k}]"))),
                            origin_of,
                        ),
                        state: ent.copied(),
                        weapon: ent
                            .map_or(s.ps.get("weapon").unwrap_or(0), |e| ent_u32(e, "weapon")),
                        perks: client_perks(
                            s,
                            ent.map_or(s.ps.get("clientNum").unwrap_or(0), |e| {
                                ent_u32(e, "clientNum")
                            }),
                        ),
                        aim: None,
                    });
                }
            }
            out
        };
    let seq = s.ps.get("eventSequence").unwrap_or(0) & 0xff;
    let get = |n: String| s.ps.get(&n).unwrap_or(0);
    let mut own_ring = ps_ring;
    // A predicted view with no ring yet (no scene since the gamestate)
    // starts from the snapshot's, so the first predicted frame fires what
    // its cmds raised past it instead of taking them as the baseline.
    if !predicted || ps_ring.is_none() {
        if let Some(prev) = ps_ring.map(|r| r.sequence as u32) {
            let to = if seq < prev { seq + 256 } else { seq };
            let ev = |k: u32| (get(format!("events[{k}]")), get(format!("eventParms[{k}]")));
            fired.extend(ring(prev, to, &ev, None));
        }
        own_ring = Some(Ring {
            sequence: seq as i32,
            events: std::array::from_fn(|k| get(format!("events[{k}]"))),
        });
    }
    let mut new_rings = HashMap::new();
    let mut new_temps = HashSet::new();
    let names: HashMap<u32, String> = s
        .clients
        .iter()
        .map(|cl| (cl.0[0], client_name(cl)))
        .collect();
    let weapons = c.gamestate.as_ref().map(weapon_list).unwrap_or_default();
    for e in &s.entities {
        let n = e.number();
        if e.etype() >= ET_EVENTS {
            new_temps.insert(n);
            if temps.contains(&n) {
                continue;
            }
            let id = e.etype() - ET_EVENTS;
            if let Some(name) = event_name(id) {
                fired.push(Fired {
                    id,
                    name,
                    entity: Some(n),
                    server_time: t,
                    parm: ent_u32(e, "eventParm"),
                    origin: origin_of(e),
                    state: Some(*e),
                    weapon: ent_u32(e, "weapon"),
                    perks: client_perks(s, ent_u32(e, "clientNum")),
                    aim: None,
                });
            }
            if id == EV_OBITUARY {
                let who = |k: u32| {
                    names
                        .get(&k)
                        .cloned()
                        .unwrap_or_else(|| format!("entity {k}"))
                };
                let victim = ent_u32(e, "otherEntityNum");
                let attacker = ent_u32(e, "attackerEntityNum");
                let weapon = weapon_name(&weapons, ent_u32(e, "eventParm")).unwrap_or_default();
                lines.push(if attacker == victim || attacker >= 64 {
                    format!("{} died ({weapon})", who(victim))
                } else {
                    format!("{} killed {} ({weapon})", who(attacker), who(victim))
                });
            }
            continue;
        }
        let seq = ent_u32(e, "eventSequence") & 0xff;
        new_rings.insert(n, seq);
        // A general entity or a missile entering replays its ring from
        // 0, unless it is a missile launched over 200 ms ago (`eFlags`
        // 0x10000); the frag's explode rides such an entity
        // (cod4-effects.md 8; the rule INFERRED, not yet confirmed on 1.7).
        let entering = match e.etype() {
            ET_GENERAL | ET_MISSILE => {
                let launched = ent_u32(e, "lerp.u.anonymous.data[0]") as i32;
                let old = ent_u32(e, "lerp.eFlags") & 0x10000 != 0 && t - launched > 200;
                (!old).then_some(0)
            }
            _ => None,
        };
        if let Some(prev) = rings.get(&n).copied().or(entering) {
            let to = if seq < prev { seq + 256 } else { seq };
            let ev = |k: u32| {
                (
                    ent_u32(e, &format!("events[{k}]")),
                    ent_u32(e, &format!("eventParms[{k}]")),
                )
            };
            fired.extend(ring(prev, to, &ev, Some(e)));
        }
    }
    (fired, lines, own_ring, new_rings, new_temps)
}

/// An entity's `lerp.pos.trBase`.
pub fn origin_of(e: &EntityState) -> Vec3 {
    vec3(|k| ent_f32(e, &format!("lerp.pos.trBase[{k}]")))
}

/// Where `e`'s trajectory puts it at `time` (`BG_EvaluateTrajectory`).
pub fn position_at(e: &EntityState, time: i32) -> Vec3 {
    trajectory(e, "pos").evaluate_f32(time)
}

/// `e`'s yaw at `time` off its angle trajectory.
pub fn yaw_at(e: &EntityState, time: i32) -> f32 {
    angles_at(e, time).y
}

/// `e`'s angles at `time` off its angle trajectory.
pub fn angles_at(e: &EntityState, time: i32) -> Vec3 {
    trajectory(e, "apos").evaluate_f32(time)
}
