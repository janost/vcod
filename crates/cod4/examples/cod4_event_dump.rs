//! Prints a CoD4 1.7 capture's events with their 1.7 names
//! (`net::events`): each new event on the playerstate's ring, every event
//! entity (eType 17 and up) and every player or corpse ring entry the
//! snapshot names, damage feedback, entity types as they first appear, and
//! the server commands. The line format of `cod4_probe` captures.
//!
//!     cargo run -p vcod-cod4 --example cod4_event_dump -- CAPTURE.txt

use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use vcod_cod4::net::capture::{self, Item, Session};
use vcod_cod4::net::delta::EntityState;
use vcod_cod4::net::events::event_name;
use vcod_cod4::net::parse::Op;

const ET_EVENTS: u32 = 17;

fn name(id: u32) -> String {
    format!("{id} {}", event_name(id).unwrap_or("?"))
}

/// The word of entity field `n` in eType `et`'s table.
fn word(et: u32, n: &str) -> Option<usize> {
    EntityState::table(et)
        .iter()
        .find(|f| f.name == n)
        .map(|f| f.word())
}

fn main() -> Result<()> {
    let path = std::env::args().nth(1).context("usage: CAPTURE.txt")?;
    let lines = capture::parse(&std::fs::read_to_string(&path)?)?;
    let mut session = Session::new();
    let mut seen_types = BTreeSet::new();
    let mut last_seq = 0u32;
    let mut last_damage = 0u32;
    let mut ent_seq: BTreeMap<u32, u32> = BTreeMap::new();
    for l in &lines {
        let Some(Item::Server { parsed, .. }) = session.feed(l)? else {
            continue;
        };
        let client = &session.client;
        for op in &parsed.ops {
            match op {
                Op::ServerCommand { text, .. } => {
                    let t = String::from_utf8_lossy(text);
                    if !t.starts_with("v ") {
                        println!("{} cmd {t}", l.ms);
                    }
                }
                Op::Snapshot(i) => {
                    let s = &client.snapshots[*i];
                    let ps = &s.ps;
                    let seq = ps.get("eventSequence").unwrap_or(0);
                    if seq != last_seq {
                        for k in last_seq.max(seq.saturating_sub(4))..seq {
                            let ev = ps.get(&format!("events[{}]", k % 4)).unwrap_or(0);
                            let parm = ps.get(&format!("eventParms[{}]", k % 4)).unwrap_or(0);
                            println!(
                                "{} t={} ps event {} parm {parm}",
                                l.ms,
                                s.server_time,
                                name(ev)
                            );
                        }
                        last_seq = seq;
                    }
                    let dmg = ps.get("damageEvent").unwrap_or(0);
                    if dmg != last_damage {
                        println!(
                            "{} t={} ps damageEvent {dmg} damageCount {} yaw {} pitch {}",
                            l.ms,
                            s.server_time,
                            ps.get("damageCount").unwrap_or(0),
                            ps.get("damageYaw").unwrap_or(0),
                            ps.get("damagePitch").unwrap_or(0),
                        );
                        last_damage = dmg;
                    }
                    for e in &s.entities {
                        let et = e.etype();
                        if seen_types.insert(et.min(ET_EVENTS)) {
                            println!(
                                "{} t={} first eType {et} (entity {}) at {:?}",
                                l.ms,
                                s.server_time,
                                e.number(),
                                e.origin()
                            );
                        }
                    }
                    // Only what this message changed.
                    for (n, _) in &s.wire.entities {
                        let Some(e) = s.entities.iter().find(|e| e.number() == *n) else {
                            continue;
                        };
                        let et = e.etype();
                        let get = |f: &str| word(et, f).map_or(0, |w| e.0[w]);
                        if et >= ET_EVENTS {
                            println!(
                                "{} t={} entity {n} event {} parm {} other {} attacker {} at {:?}",
                                l.ms,
                                s.server_time,
                                name(et - ET_EVENTS),
                                get("eventParm"),
                                get("otherEntityNum"),
                                get("attackerEntityNum"),
                                e.origin()
                            );
                        } else if matches!(et, 1 | 2) {
                            let seq = get("eventSequence");
                            let last = ent_seq.insert(*n, seq).unwrap_or(seq);
                            for k in last.max(seq.saturating_sub(4))..seq {
                                println!(
                                    "{} t={} entity {n} eType {et} event {} parm {}",
                                    l.ms,
                                    s.server_time,
                                    name(get(&format!("events[{}]", k % 4))),
                                    get(&format!("eventParms[{}]", k % 4)),
                                );
                            }
                        } else if et == 3 {
                            println!(
                                "{} t={} entity {n} item index {} at {:?}",
                                l.ms,
                                s.server_time,
                                get("index"),
                                e.origin()
                            );
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}
