//! The default run's per-second checks over the newest snapshot (`ProbeWatch`).

use super::fixture::vec_str;
use super::*;

/// One trajectory group as one line, `trDuration` included: a bounded type
/// is only readable with it, and the missile capture's `traj_str` leaves it
/// out because a missile's trajectory is unbounded.
fn mover_traj_str(t: &Trajectory) -> String {
    format!(
        "trType {} trTime {} trDuration {} base [{:.1},{:.1},{:.1}] delta [{:.1},{:.1},{:.1}]",
        t.tr_type,
        t.tr_time,
        t.tr_duration,
        t.base.x,
        t.base.y,
        t.base.z,
        t.delta.x,
        t.delta.y,
        t.delta.z,
    )
}

/// Per-second checks over the newest snapshot, for chasing wrong-model and
/// sound-path reports headlessly.
#[derive(Default)]
pub(super) struct ProbeWatch {
    client_models: std::collections::HashMap<u32, i32>,
    /// Live eType-2 corpses: entity -> (clientNum, first-seen serverTime).
    corpses: std::collections::HashMap<u32, (i32, i32)>,
    prop_origins: std::collections::HashMap<u32, (String, [f32; 3])>,
    /// The last `pos`/`apos` trajectory seen per entity, so a change is
    /// printed once rather than every snapshot it survives.
    trajectories: std::collections::HashMap<u32, (Trajectory, Trajectory)>,
    loop_sounds: std::collections::HashMap<u32, i32>,
    events: vcod_common::net::events::EventTracker,
    /// Empty until the gamestate names the map, or when there is no game
    /// data; the audio line stays quiet then.
    pub(super) aliases: crate::audio::alias::AliasTable,
    /// Since the last audio line.
    cues: u64,
    misses: u64,
    first_miss: Option<String>,
}

impl ProbeWatch {
    pub(super) fn check(
        &mut self,
        s: &net::snapshot::Snapshot,
        configstrings: &[String],
        p: &'static net::protocol::Cod1Wire,
    ) {
        use crate::entities::{ET_GENERAL, EntityVisual, resolve_visual};
        if !self.aliases.is_empty() {
            println!(
                "audio: {} cues, {} alias misses{}",
                self.cues,
                self.misses,
                match self.first_miss.take() {
                    Some(m) => format!(" (first: {m})"),
                    None => String::new(),
                }
            );
            self.cues = 0;
            self.misses = 0;
        }
        for (&num, cl) in &s.clients {
            let mi = cl.field_i32(p, "modelindex");
            if let Some(old) = self.client_models.insert(num, mi)
                && old != mi
            {
                let name = |i: i32| {
                    configstrings
                        .get(p.cs.models() + i as usize)
                        .cloned()
                        .unwrap_or_default()
                };
                println!(
                    "client {num}: modelindex {old} ({}) -> {mi} ({})",
                    name(old),
                    name(mi)
                );
            }
        }
        // Corpse lifecycle (1 Hz, so times are +-1 s): a corpse resolves
        // through the dead client's roster entry, so log appear/vanish with
        // that entry's modelindex - the evidence for how long the wire keeps
        // bodies and whether the roster clears first.
        let mi_of = |cn: i32| {
            s.clients
                .get(&(cn as u32))
                .map_or(-1, |c| c.field_i32(p, "modelindex"))
        };
        self.corpses.retain(|&num, &mut (cn, t0)| {
            if s.entities.contains_key(&num) {
                return true;
            }
            println!(
                "corpse {num} gone after {} ms (clientNum {cn} modelindex {})",
                s.server_time - t0,
                mi_of(cn)
            );
            false
        });
        for (&num, ent) in &s.entities {
            if ent.field_i32(p, "eType") == crate::entities::ET_CORPSE {
                let cn = ent.field_i32(p, "clientNum");
                self.corpses.entry(num).or_insert_with(|| {
                    println!(
                        "corpse {num} appeared clientNum {cn} modelindex {}",
                        mi_of(cn)
                    );
                    (cn, s.server_time)
                });
                // The trajectory decides whether a closed-form evaluation sinks
                // the body: TR_GRAVITY with a frozen trTime drops z by
                // 400*age^2 per second. `eFlags` bit 0x800 and
                // `groundEntityNum` ride along because they are the other two
                // halves of the settle: retail sets 0x800 for 250 ms and
                // rewrites the ground entity to 1022 when the body lands
                // (`docs/research/cod11-combat.md` 5.3).
                let tr = net::trajectory::Trajectory::read(ent, p, "pos");
                let eval = tr.evaluate_f32(s.server_time);
                println!(
                    "corpse {num} trType {} trTime {} (age {} ms) base z {:.0} delta {:?} eval z {:.0} eFlags {:#x} groundEntityNum {}",
                    tr.tr_type,
                    tr.tr_time,
                    s.server_time - tr.tr_time,
                    tr.base.z,
                    tr.delta,
                    eval.z,
                    ent.field_i32(p, "eFlags"),
                    ent.field_i32(p, "groundEntityNum"),
                );
            }
        }
        for (&num, ent) in &s.entities {
            let etype = ent.field_i32(p, "eType");
            match resolve_visual(ent, &s.clients, configstrings, p) {
                EntityVisual::Player { body, .. } if !body.contains("playerbody") => {
                    println!(
                        "entity {num} eType {etype} clientNum {} index {} resolves to {body}",
                        ent.field_i32(p, "clientNum"),
                        ent.field_i32(p, "index")
                    );
                }
                EntityVisual::Model(m) if etype == ET_GENERAL => {
                    let o = ent.origin(p);
                    if let Some((old_m, old_o)) = self.prop_origins.insert(num, (m.clone(), o)) {
                        let d = ((o[0] - old_o[0]).powi(2)
                            + (o[1] - old_o[1]).powi(2)
                            + (o[2] - old_o[2]).powi(2))
                        .sqrt();
                        if old_m != m || d > 8.0 {
                            println!(
                                "entity {num} ET_GENERAL index {}: {old_m} @[{:.0},{:.0},{:.0}] -> {m} @[{:.0},{:.0},{:.0}]",
                                ent.field_i32(p, "index"),
                                old_o[0],
                                old_o[1],
                                old_o[2],
                                o[0],
                                o[1],
                                o[2]
                            );
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Every change to an entity's `pos` or `apos` trajectory group, which is
    /// what says whether retail ships a mover as per-frame origins (`trType`
    /// stationary, a new `trBase` every snapshot) or as a trajectory the
    /// client extrapolates (one non-stationary group per verb). Runs per
    /// snapshot rather than per second: a two-second move is 40 frames and a
    /// once-a-second sample cannot tell the two shapes apart.
    pub(super) fn check_movers(
        &mut self,
        s: &net::snapshot::Snapshot,
        p: &'static net::protocol::Cod1Wire,
    ) {
        for (&num, ent) in &s.entities {
            let now = (
                Trajectory::read(ent, p, "pos"),
                Trajectory::read(ent, p, "apos"),
            );
            let was = self.trajectories.insert(num, now);
            if was.as_ref() == Some(&now) {
                continue;
            }
            // A stationary entity that has never moved is the map's static
            // set; only report one once it has moved at least once.
            if was.is_none() && now.0.tr_type == 0 && now.1.tr_type == 0 {
                continue;
            }
            println!(
                "entity {num} serverTime {} eType {} pos {} apos {}",
                s.server_time,
                ent.field_i32(p, "eType"),
                mover_traj_str(&now.0),
                mover_traj_str(&now.1)
            );
        }
        self.trajectories.retain(|n, _| s.entities.contains_key(n));
    }

    /// `EventTracker::drain` is idempotent per `message_num`, so re-reading
    /// the same snapshot is free.
    pub(super) fn check_sounds(
        &mut self,
        s: &net::snapshot::Snapshot,
        configstrings: &[String],
        p: &'static net::protocol::Cod1Wire,
    ) {
        for (&num, ent) in &s.entities {
            let ls = ent.field_i32(p, "loopSound");
            let old = self.loop_sounds.insert(num, ls);
            // Starts and stops only; most entities sit at 0.
            if old != Some(ls) && (ls != 0 || old.is_some_and(|o| o != 0)) {
                let o = ent.origin(p);
                println!(
                    "entity {num} loopSound {} -> {ls} {} at [{:.0},{:.0},{:.0}]",
                    old.unwrap_or(0),
                    sound_cs_alias_paren(configstrings, p, ls),
                    o[0],
                    o[1],
                    o[2]
                );
            }
        }
        // Forget entities that left, so a reused slot reports again.
        self.loop_sounds.retain(|n, _| s.entities.contains_key(n));

        let ps_client = s.ps.field_i32(p, "clientNum");
        // No weapon cache or drawn entities headlessly; fire, reload and
        // whizby cues resolve to nothing.
        let no_weapons = Default::default();
        let no_muzzles = Default::default();
        let ctx = crate::audio::cues::CueCtx {
            configstrings,
            sounds: p.cs.sounds(),
            weapon_sounds: &no_weapons,
            muzzles: &no_muzzles,
            shot_fallbacks: &Default::default(),
            listener_pos: glam::Vec3::from(s.ps.origin(p)),
            ps_entity: crate::audio::cues::ps_entity(ps_client),
            uo: p.is_uo(),
        };
        for ev in drain_v1(&mut self.events, s, p) {
            for cue in crate::audio::cues::resolve(&ev, &ctx) {
                self.cues += 1;
                if self.aliases.get(&cue.alias).is_none() {
                    self.misses += 1;
                    self.first_miss.get_or_insert(cue.alias);
                }
            }
            if ev.event == vcod_common::net::event_ids::EV_EARTHQUAKE
                && let Some(es) = s.entities.get(&ev.entity_num)
            {
                println!(
                    "EV_EARTHQUAKE entity {} angles2 {:?} time {} at {}",
                    ev.entity_num,
                    [
                        es.field_f32(p, "angles2[0]"),
                        es.field_f32(p, "angles2[1]"),
                        es.field_f32(p, "angles2[2]"),
                    ],
                    es.field_i32(p, "time"),
                    vec_str(ev.pos),
                );
            }
            if ev.event != crate::fx::registry::EV_SOUND_ALIAS {
                continue;
            }
            let who = if ev.entity_num == u32::MAX {
                "playerState ring (the followed player)".to_string()
            } else {
                format!("entity {}", ev.entity_num)
            };
            println!(
                "EV_SOUND_ALIAS {who} clientNum {} parm {} {} at [{:.0},{:.0},{:.0}]",
                ev.client_num,
                ev.parm,
                sound_cs_alias_paren(configstrings, p, ev.parm),
                ev.pos[0],
                ev.pos[1],
                ev.pos[2]
            );
        }
    }
}

/// `G_SoundAliasIndex`'s hard limit, past `CsLayout::sounds` (sound doc,
/// section 9); index 0 in the block is "no sound".
pub(super) const CS_SOUNDS_COUNT: usize = 256;

/// `None` when the index names no alias (0 is "no sound", a garbled command
/// can carry anything), so callers print the raw index.
pub(super) fn sound_cs_alias<'a>(
    configstrings: &'a [String],
    p: &net::protocol::Cod1Wire,
    idx: i32,
) -> Option<(usize, &'a str)> {
    if idx <= 0 || idx as usize >= CS_SOUNDS_COUNT {
        return None;
    }
    let cs = p.cs.sounds() + idx as usize;
    Some((cs, configstrings.get(cs).map_or("<unset>", String::as_str)))
}

fn sound_cs_alias_paren(configstrings: &[String], p: &net::protocol::Cod1Wire, idx: i32) -> String {
    match sound_cs_alias(configstrings, p, idx) {
        Some((cs, name)) => format!("(cs {cs}) = {name:?}"),
        None => format!("(index {idx}: no alias)"),
    }
}
