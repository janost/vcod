//! Live-voice bookkeeping, independent of kira
//! (docs/research/cod11-sound-system.md, sections 2 and 4).

use std::collections::HashMap;
use std::time::Instant;

use glam::Vec3;

use crate::audio::alias::{Channel, MasterSlave};
use crate::audio::cues::Source;
use crate::audio::spatial::{falloff, pan, Listener};

pub type VoiceId = u64;

/// Retail's world-space entity; `Source::Point` voices carry it for the
/// replacement rule (research doc, section 2).
pub const ENTITYNUM_WORLD: u32 = 1022;

/// Retail's voice pools: 32 3D samples, 32 2D samples, 13 streams with slots
/// 0-4 reserved, so 8 general (`FUN_0044ba30`/`FUN_0044bb70`, research doc,
/// section 2).
pub const SPATIAL_POOL: usize = 32;
pub const FLAT_POOL: usize = 32;
pub const STREAM_POOL: usize = 8;

/// A stream's length is unknowable before decode; its voice never wins the
/// earliest-end tiebreak.
pub const ENDLESS_MS: u32 = u32::MAX;

/// Which retail pool a voice occupies: the type decides, then the channel
/// (research doc, sections 2 and 10).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Pool {
    Spatial,
    Flat,
    Stream,
}

fn pool_of(streamed: bool, spatial: bool) -> Pool {
    if streamed {
        Pool::Stream
    } else if spatial {
        Pool::Spatial
    } else {
        Pool::Flat
    }
}

/// Outcome of admitting a cue to the table.
#[derive(Debug)]
pub enum Admitted {
    /// The voice is in the table. `replaced` are the same-(entity, channel)
    /// voices the channel rule ended, `stolen` the capacity victims; the
    /// caller stops both sets.
    Started {
        id: VoiceId,
        replaced: Vec<VoiceId>,
        stolen: Vec<VoiceId>,
    },
}

/// `spatial` comes from the alias row's channel, not the source. A 2D voice
/// ignores its position but keeps the entity identity for replacement.
#[derive(Debug)]
pub struct NewVoice {
    pub source: Source,
    pub channel: Channel,
    /// Linear, already sampled from the row's `vol` range.
    pub volume: f32,
    pub dist: (f32, f32),
    pub master_slave: MasterSlave,
    pub spatial: bool,
    pub looping: bool,
    /// Streamed rows play through kira's streaming path and occupy the
    /// stream pool.
    pub streamed: bool,
    /// Expected playback length; [`ENDLESS_MS`] when unknowable (streams).
    pub duration_ms: u32,
    /// Never chosen as a steal victim. The ambient carries this so a full
    /// stream pool cannot evict it (research doc, "vcod divergences").
    pub protected: bool,
}

struct Voice {
    id: VoiceId,
    source: Source,
    channel: Channel,
    volume: f32,
    dist: (f32, f32),
    master_slave: MasterSlave,
    spatial: bool,
    looping: bool,
    streamed: bool,
    protected: bool,
    /// `started + duration_ms`; `None` when the length is unknown.
    ends_at: Option<Instant>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VoiceUpdate {
    pub id: VoiceId,
    /// Linear gain, may exceed 1.0 (alias volumes go to 1.5).
    pub gain: f32,
    pub pan: f32,
}

#[derive(Default)]
pub struct VoiceTable {
    voices: Vec<Voice>,
    next_id: VoiceId,
}

/// Entity identity for the replacement rule; a fixed point is
/// [`ENTITYNUM_WORLD`].
fn entity_of(source: &Source) -> u32 {
    match source {
        Source::Entity { num, .. } => *num,
        Source::Point(_) => ENTITYNUM_WORLD,
    }
}

impl VoiceTable {
    pub fn new() -> VoiceTable {
        VoiceTable::default()
    }

    /// Admits a cue: same-(entity, channel) replacement first
    /// (`FUN_0044c7e0`, research doc, section 2), then capacity. A full pool
    /// steals per `FUN_0044c350`: candidates are same-pool voices whose
    /// channel index is `<=` the newcomer's, preferring a finished voice,
    /// then one on the newcomer's entity, then the lowest channel index,
    /// then the earliest end time. `is_finished` reports voices whose sound
    /// has stopped but not been reaped yet.
    pub fn add(
        &mut self,
        v: NewVoice,
        is_finished: impl Fn(VoiceId) -> bool,
    ) -> Result<Admitted, NewVoice> {
        let mut replaced = Vec::new();
        if v.channel != Channel::Auto {
            let ent = entity_of(&v.source);
            self.voices.retain(|x| {
                let same = x.channel == v.channel && entity_of(&x.source) == ent;
                if same {
                    replaced.push(x.id);
                }
                !same
            });
        }
        let pool = pool_of(v.streamed, v.spatial);
        let cap = match pool {
            Pool::Spatial => SPATIAL_POOL,
            Pool::Flat => FLAT_POOL,
            Pool::Stream => STREAM_POOL,
        };
        let in_pool = |x: &Voice| pool_of(x.streamed, x.spatial) == pool;
        let count = self.voices.iter().filter(|x| in_pool(x)).count();
        let stolen = if count >= cap {
            let ch = v.channel as u8;
            let ent = entity_of(&v.source);
            let eligible = |x: &Voice| !x.protected && x.channel as u8 <= ch;
            let rank = |x: &Voice| {
                (
                    entity_of(&x.source) != ent,
                    x.channel as u8,
                    x.ends_at.is_none(),
                    x.ends_at,
                )
            };
            let finished: Vec<&Voice> = self
                .voices
                .iter()
                .filter(|x| in_pool(x) && eligible(x) && is_finished(x.id))
                .collect();
            // Retail takes a finished voice before running the preference
            // scan at all (research doc, section 2).
            let scan: Vec<&Voice> = if finished.is_empty() {
                self.voices
                    .iter()
                    .filter(|x| in_pool(x) && eligible(x))
                    .collect()
            } else {
                finished
            };
            match scan.iter().min_by_key(|x| rank(x)) {
                Some(victim) => vec![victim.id],
                None => return Err(v),
            }
        } else {
            Vec::new()
        };
        for id in &stolen {
            self.voices.retain(|x| x.id != *id);
        }
        let id = self.next_id;
        self.next_id += 1;
        let started = Instant::now();
        let ends_at = (v.duration_ms != ENDLESS_MS)
            .then(|| started + std::time::Duration::from_millis(u64::from(v.duration_ms)));
        self.voices.push(Voice {
            id,
            source: v.source,
            channel: v.channel,
            volume: v.volume,
            dist: v.dist,
            master_slave: v.master_slave,
            spatial: v.spatial,
            looping: v.looping,
            streamed: v.streamed,
            protected: v.protected,
            ends_at,
        });
        Ok(Admitted::Started {
            id,
            replaced,
            stolen,
        })
    }

    /// `None` once `id` has left the table (reaped or replaced), which
    /// is how loop bookkeeping tells running from lost.
    pub fn looping(&self, id: VoiceId) -> Option<bool> {
        self.voices.iter().find(|v| v.id == id).map(|v| v.looping)
    }

    pub fn remove(&mut self, id: VoiceId) {
        self.voices.retain(|v| v.id != id);
    }

    pub fn retain(&mut self, mut keep: impl FnMut(VoiceId) -> bool) {
        self.voices.retain(|v| keep(v.id));
    }

    pub fn clear(&mut self) -> Vec<VoiceId> {
        self.voices.drain(..).map(|v| v.id).collect()
    }

    pub fn len(&self) -> usize {
        self.voices.len()
    }

    pub fn is_empty(&self) -> bool {
        self.voices.is_empty()
    }

    pub fn ids(&self) -> Vec<VoiceId> {
        self.voices.iter().map(|v| v.id).collect()
    }

    /// Current emitter position of every spatial voice, for the occlusion
    /// traces.
    pub fn spatial_positions(&self) -> Vec<(VoiceId, Vec3)> {
        self.voices
            .iter()
            .filter(|v| v.spatial)
            .map(|v| {
                let p = match v.source {
                    Source::Point(p) => p,
                    Source::Entity { pos, .. } => pos,
                };
                (v.id, p)
            })
            .collect()
    }

    /// Per-frame gain and pan. `Entity` sources take the entity's current
    /// position when known, else the last one. A non-spatial voice gets
    /// `volume * duck * master_volume` at pan 0. A `Slave(level)` voice is
    /// capped at `level` while any `Master` is live, before `master_volume`
    /// (research doc, section 4). The per-voice `occlusion` factor from
    /// [`crate::audio::occlusion`] scales into the distance term, so the
    /// duck cap still bounds it.
    pub fn update(
        &mut self,
        listener: &Listener,
        entity_pos: &HashMap<u32, Vec3>,
        occlusion: &HashMap<VoiceId, f32>,
        master_volume: f32,
    ) -> Vec<VoiceUpdate> {
        let master_live = self
            .voices
            .iter()
            .any(|v| v.master_slave == MasterSlave::Master);
        let mut out = Vec::with_capacity(self.voices.len());
        for v in &mut self.voices {
            // Unconditional so a 2D voice's tracked position stays current
            // too.
            if let Source::Entity { num, pos } = &mut v.source {
                if let Some(p) = entity_pos.get(num) {
                    *pos = *p;
                }
            }
            let (spatial_scale, pan_v) = if v.spatial {
                let p = match v.source {
                    Source::Point(p) => p,
                    Source::Entity { pos, .. } => pos,
                };
                (
                    falloff(p.distance(listener.pos), v.dist.0, v.dist.1)
                        * occlusion.get(&v.id).copied().unwrap_or(1.0),
                    pan(listener, p),
                )
            } else {
                (1.0, 0.0)
            };
            let scaled = v.volume * spatial_scale;
            let capped = match v.master_slave {
                MasterSlave::Slave(level) if master_live => scaled.min(level),
                _ => scaled,
            };
            out.push(VoiceUpdate {
                id: v.id,
                gain: capped * master_volume,
                pan: pan_v,
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nv(source: Source, channel: Channel) -> NewVoice {
        NewVoice {
            source,
            channel,
            volume: 1.0,
            dist: (100.0, 1100.0),
            master_slave: MasterSlave::None,
            spatial: true,
            looping: false,
            streamed: false,
            duration_ms: 500,
            protected: false,
        }
    }

    /// A 2D voice; the source is a placeholder.
    fn nv2d(channel: Channel) -> NewVoice {
        let mut v = nv(Source::Point(Vec3::ZERO), channel);
        v.spatial = false;
        v
    }

    fn put(t: &mut VoiceTable, v: NewVoice) -> VoiceId {
        match t.add(v, |_| false).unwrap() {
            Admitted::Started {
                id,
                replaced,
                stolen,
            } => {
                assert!(replaced.is_empty() && stolen.is_empty());
                id
            }
        }
    }

    #[test]
    fn same_entity_and_channel_replaces_except_auto() {
        let mut t = VoiceTable::new();
        let ent = Source::Entity {
            num: 3,
            pos: Vec3::ZERO,
        };
        let a = put(&mut t, nv(ent, Channel::Weapon));
        match t.add(nv(ent, Channel::Weapon), |_| false).unwrap() {
            Admitted::Started {
                replaced, stolen, ..
            } => {
                assert_eq!((replaced, stolen), (vec![a], Vec::new()))
            }
        }
        put(&mut t, nv(ent, Channel::Body));
        put(&mut t, nv(ent, Channel::Auto));
        put(&mut t, nv(ent, Channel::Auto));
        let other = Source::Entity {
            num: 4,
            pos: Vec3::ZERO,
        };
        put(&mut t, nv(other, Channel::Weapon));
        assert_eq!(t.len(), 5);
    }

    #[test]
    fn world_points_replace_on_non_auto_channel_but_not_on_auto() {
        let mut t = VoiceTable::new();
        let a = put(&mut t, nv(Source::Point(Vec3::ZERO), Channel::Voice));
        match t
            .add(
                nv(Source::Point(Vec3::new(10.0, 0.0, 0.0)), Channel::Voice),
                |_| false,
            )
            .unwrap()
        {
            Admitted::Started {
                replaced, stolen, ..
            } => {
                assert_eq!((replaced, stolen), (vec![a], Vec::new()))
            }
        }
        assert_eq!(t.len(), 1);

        put(&mut t, nv(Source::Point(Vec3::ZERO), Channel::Auto));
        put(&mut t, nv(Source::Point(Vec3::ZERO), Channel::Auto));
        assert_eq!(t.len(), 3);
    }

    #[test]
    fn update_spatializes_points_and_tracks_entities() {
        let mut t = VoiceTable::new();
        let l = Listener::default();
        let p = put(&mut t, nv(Source::Point(l.right * 600.0), Channel::Auto));
        let e = put(
            &mut t,
            nv(
                Source::Entity {
                    num: 3,
                    pos: Vec3::ZERO,
                },
                Channel::Auto,
            ),
        );
        let loc = put(&mut t, nv2d(Channel::Auto));
        let mut pos = HashMap::new();
        pos.insert(3u32, -l.right * 100.0);
        let ups = t.update(&l, &pos, &HashMap::new(), 1.0);
        let get = |id| ups.iter().find(|u| u.id == id).unwrap();
        assert!((get(p).gain - 0.5).abs() < 1e-5); // 600 into a 100..1100 range
        assert!((get(p).pan - 1.0).abs() < 1e-5);
        assert_eq!(get(e).gain, 1.0);
        assert!((get(e).pan + 1.0).abs() < 1e-5);
        assert_eq!((get(loc).gain, get(loc).pan), (1.0, 0.0));
        // Entity gone from the map: keeps its last position.
        let ups = t.update(&l, &HashMap::new(), &HashMap::new(), 1.0);
        assert!((get_from(&ups, e).pan + 1.0).abs() < 1e-5);
    }

    fn get_from(ups: &[VoiceUpdate], id: VoiceId) -> &VoiceUpdate {
        ups.iter().find(|u| u.id == id).unwrap()
    }

    #[test]
    fn master_volume_and_row_volume_scale_gain() {
        let mut t = VoiceTable::new();
        let mut v = nv2d(Channel::Auto);
        v.volume = 1.5;
        let id = put(&mut t, v);
        let ups = t.update(&Listener::default(), &HashMap::new(), &HashMap::new(), 0.5);
        assert!((get_from(&ups, id).gain - 0.75).abs() < 1e-5);
    }

    #[test]
    fn master_ducks_slaves_while_playing() {
        let mut t = VoiceTable::new();
        let mut slave = nv2d(Channel::Auto);
        slave.master_slave = MasterSlave::Slave(0.25);
        let s = put(&mut t, slave);
        let ups = t.update(&Listener::default(), &HashMap::new(), &HashMap::new(), 1.0);
        assert_eq!(get_from(&ups, s).gain, 1.0);
        let mut master = nv2d(Channel::Auto);
        master.master_slave = MasterSlave::Master;
        let m = put(&mut t, master);
        let ups = t.update(&Listener::default(), &HashMap::new(), &HashMap::new(), 1.0);
        assert!((get_from(&ups, s).gain - 0.25).abs() < 1e-5);
        assert_eq!(get_from(&ups, m).gain, 1.0);
        t.remove(m);
        let ups = t.update(&Listener::default(), &HashMap::new(), &HashMap::new(), 1.0);
        assert_eq!(get_from(&ups, s).gain, 1.0);
    }

    #[test]
    fn slave_below_the_cap_is_unaffected() {
        let mut t = VoiceTable::new();
        let mut slave = nv2d(Channel::Auto);
        slave.volume = 0.2;
        slave.master_slave = MasterSlave::Slave(0.25);
        let s = put(&mut t, slave);
        let mut master = nv2d(Channel::Auto);
        master.master_slave = MasterSlave::Master;
        put(&mut t, master);
        let ups = t.update(&Listener::default(), &HashMap::new(), &HashMap::new(), 1.0);
        assert!((get_from(&ups, s).gain - 0.2).abs() < 1e-5);
    }

    #[test]
    fn clear_returns_every_id() {
        let mut t = VoiceTable::new();
        let a = put(&mut t, nv2d(Channel::Auto));
        let b = put(&mut t, nv2d(Channel::Auto));
        let mut ids = t.clear();
        ids.sort();
        assert_eq!(ids, vec![a, b]);
        assert_eq!(t.len(), 0);
        // Ids keep counting up across a clear rather than reusing freed ones.
        let c = put(&mut t, nv2d(Channel::Auto));
        assert!(c > b);
    }

    /// The 2D tests cannot tell `min(scale * v, level)` from `min(v, level) *
    /// scale`; this one uses a spatial voice at falloff 0.5.
    #[test]
    fn duck_cap_applies_before_spatial_scale_and_master_volume() {
        let mut t = VoiceTable::new();
        let l = Listener::default();
        let mut slave = nv(Source::Point(l.right * 600.0), Channel::Auto);
        slave.volume = 1.0;
        slave.master_slave = MasterSlave::Slave(0.25);
        let s = put(&mut t, slave);
        let mut master = nv2d(Channel::Auto);
        master.master_slave = MasterSlave::Master;
        put(&mut t, master);
        // falloff(600, 100, 1100) = 0.5; min(0.5, 0.25) * 0.5 = 0.125.
        let ups = t.update(&l, &HashMap::new(), &HashMap::new(), 0.5);
        assert!((get_from(&ups, s).gain - 0.125).abs() < 1e-5);

        // Cap above scale*v; min(0.5, 0.75) * 0.5 = 0.25.
        let mut t = VoiceTable::new();
        let mut slave = nv(Source::Point(l.right * 600.0), Channel::Auto);
        slave.volume = 1.0;
        slave.master_slave = MasterSlave::Slave(0.75);
        let s = put(&mut t, slave);
        let mut master = nv2d(Channel::Auto);
        master.master_slave = MasterSlave::Master;
        put(&mut t, master);
        let ups = t.update(&l, &HashMap::new(), &HashMap::new(), 0.5);
        assert!((get_from(&ups, s).gain - 0.25).abs() < 1e-5);
    }

    #[test]
    fn retain_ids_and_is_empty() {
        let mut t = VoiceTable::new();
        assert!(t.is_empty());
        let a = put(&mut t, nv2d(Channel::Auto));
        let b = put(&mut t, nv2d(Channel::Auto));
        let mut ids = t.ids();
        ids.sort();
        assert_eq!(ids, vec![a, b]);
        assert!(!t.is_empty());
        t.retain(|id| id == a);
        assert_eq!(t.ids(), vec![a]);
        assert!(!t.is_empty());
    }

    /// Fills the spatial pool with `auto` voices on entity 9 and returns
    /// their ids in add order.
    fn fill_auto(t: &mut VoiceTable, n: usize) -> Vec<VoiceId> {
        (0..n)
            .map(|_| {
                put(
                    t,
                    nv(
                        Source::Entity {
                            num: 9,
                            pos: Vec3::ZERO,
                        },
                        Channel::Auto,
                    ),
                )
            })
            .collect()
    }

    #[test]
    fn full_pool_steals_the_earliest_ending_candidate() {
        let mut t = VoiceTable::new();
        let ids = fill_auto(&mut t, SPATIAL_POOL);
        let first = ids[0];
        // All candidates are auto and end at the same duration; the earliest
        // added ends earliest, so the first voice dies.
        let new_id;
        match t
            .add(
                nv(
                    Source::Entity {
                        num: 10,
                        pos: Vec3::ZERO,
                    },
                    Channel::Auto,
                ),
                |_| false,
            )
            .unwrap()
        {
            Admitted::Started {
                id,
                replaced,
                stolen,
            } => {
                new_id = id;
                assert!(replaced.is_empty(), "auto never replaces");
                assert_eq!(stolen, vec![first]);
            }
        }
        assert!(t.ids().contains(&new_id));
        assert_eq!(t.len(), SPATIAL_POOL);
    }

    #[test]
    fn steal_prefers_the_newcomers_entity_then_lowest_channel_then_end() {
        let mut t = VoiceTable::new();
        // Same-entity voice but latest channel among candidates.
        let same_ent_low_channel = put(
            &mut t,
            nv(
                Source::Entity {
                    num: 7,
                    pos: Vec3::ZERO,
                },
                Channel::Weapon,
            ),
        );
        for i in 0..SPATIAL_POOL - 1 {
            put(
                &mut t,
                nv(
                    Source::Entity {
                        num: 100 + i as u32,
                        pos: Vec3::ZERO,
                    },
                    Channel::Auto,
                ),
            );
        }
        match t
            .add(
                nv(
                    Source::Entity {
                        num: 7,
                        pos: Vec3::ZERO,
                    },
                    Channel::Body,
                ),
                |_| false,
            )
            .unwrap()
        {
            Admitted::Started { stolen, .. } => assert_eq!(stolen, vec![same_ent_low_channel]),
        }
    }

    #[test]
    fn finished_voices_are_stolen_first() {
        let mut t = VoiceTable::new();
        let ids = fill_auto(&mut t, SPATIAL_POOL);
        let finished = ids[SPATIAL_POOL - 1];
        let victim = |id: VoiceId| id == finished;
        match t
            .add(nv(Source::Point(Vec3::ZERO), Channel::Auto), victim)
            .unwrap()
        {
            Admitted::Started { stolen, .. } => assert_eq!(stolen, vec![finished]),
        }
    }

    #[test]
    fn a_higher_channel_cannot_steal_down_and_a_lower_pool_is_untouched() {
        // The flat pool is full of 2D auto voices.
        let mut t = VoiceTable::new();
        for _ in 0..FLAT_POOL {
            put(&mut t, nv2d(Channel::Auto));
        }
        // A spatial newcomer finds its own pool empty: no cross-pool stealing.
        let v = put(&mut t, nv(Source::Point(Vec3::ZERO), Channel::Body));
        assert_eq!(t.len(), FLAT_POOL + 1);

        // Fill the spatial pool with body voices only.
        let mut t2 = VoiceTable::new();
        for i in 0..SPATIAL_POOL {
            put(
                &mut t2,
                nv(
                    Source::Entity {
                        num: i as u32,
                        pos: Vec3::ZERO,
                    },
                    Channel::Body,
                ),
            );
        }
        // An auto sound may not touch them.
        assert!(t2
            .add(nv(Source::Point(Vec3::ZERO), Channel::Auto), |_| false)
            .is_err());
        assert_eq!(t2.len(), SPATIAL_POOL);
        let _ = v;
    }

    #[test]
    fn streams_and_protected_voices() {
        let mut t = VoiceTable::new();
        let mut ambient = nv2d(Channel::Local);
        ambient.streamed = true;
        ambient.duration_ms = ENDLESS_MS;
        ambient.protected = true;
        let amb_id = put(&mut t, ambient);
        // Distinct entities: world-space same-channel sounds would replace
        // each other before capacity ever comes into play.
        for i in 0..STREAM_POOL - 1 {
            let mut s = nv(
                Source::Entity {
                    num: 100 + i as u32,
                    pos: Vec3::ZERO,
                },
                Channel::Voice,
            );
            s.streamed = true;
            s.duration_ms = ENDLESS_MS;
            s.spatial = false;
            put(&mut t, s);
        }
        assert_eq!(t.len(), STREAM_POOL);
        // The pool is full of endless voices on other entities; the protected
        // ambient survives every steal.
        let mut s = nv2d(Channel::Voice);
        s.streamed = true;
        s.duration_ms = ENDLESS_MS;
        match t.add(s, |_| false).unwrap() {
            Admitted::Started { stolen, .. } => assert_eq!(stolen.len(), 1),
        }
        assert!(t.ids().contains(&amb_id));
        // A loaded 2D voice goes to the flat pool instead; a different
        // channel so the (entity, channel) replacement cannot touch the
        // world-space streams.
        let mut other = nv2d(Channel::Menu);
        put(&mut t, other);
        assert_eq!(t.ids().len(), STREAM_POOL + 1);
    }
}
