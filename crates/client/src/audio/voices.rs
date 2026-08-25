//! Live-voice bookkeeping, independent of kira
//! (docs/research/cod11-sound-system.md, sections 2 and 4).

use std::collections::HashMap;

use glam::Vec3;

use crate::audio::alias::{Channel, MasterSlave};
use crate::audio::cues::Source;
use crate::audio::spatial::{falloff, pan, Listener};

pub type VoiceId = u64;

/// Retail's world-space entity; `Source::Point` voices carry it for the
/// replacement rule (research doc, section 2).
pub const ENTITYNUM_WORLD: u32 = 1022;

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

    /// A non-`Auto` channel ends every live voice with the same (entity,
    /// channel), `FUN_0044c7e0` @ `CoDMP.exe 0x44c7e0` (research doc, section
    /// 2); `Auto` never replaces. Returns the replaced ids for the caller to
    /// stop.
    pub fn add(&mut self, v: NewVoice) -> (VoiceId, Vec<VoiceId>) {
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
        let id = self.next_id;
        self.next_id += 1;
        self.voices.push(Voice {
            id,
            source: v.source,
            channel: v.channel,
            volume: v.volume,
            dist: v.dist,
            master_slave: v.master_slave,
            spatial: v.spatial,
            looping: v.looping,
        });
        (id, replaced)
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

    /// Per-frame gain and pan. `Entity` sources take the entity's current
    /// position when known, else the last one. A non-spatial voice gets
    /// `volume * duck * master_volume` at pan 0. A `Slave(level)` voice is
    /// capped at `level` while any `Master` is live, before `master_volume`
    /// (research doc, section 4).
    pub fn update(
        &mut self,
        listener: &Listener,
        entity_pos: &HashMap<u32, Vec3>,
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
                    falloff(p.distance(listener.pos), v.dist.0, v.dist.1),
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
        }
    }

    /// A 2D voice; the source is a placeholder.
    fn nv2d(channel: Channel) -> NewVoice {
        let mut v = nv(Source::Point(Vec3::ZERO), channel);
        v.spatial = false;
        v
    }

    #[test]
    fn same_entity_and_channel_replaces_except_auto() {
        let mut t = VoiceTable::new();
        let ent = Source::Entity {
            num: 3,
            pos: Vec3::ZERO,
        };
        let (a, r) = t.add(nv(ent, Channel::Weapon));
        assert!(r.is_empty());
        let (_b, r) = t.add(nv(ent, Channel::Weapon));
        assert_eq!(r, vec![a]);
        let (_c, r) = t.add(nv(ent, Channel::Body));
        assert!(r.is_empty());
        let (_d, r) = t.add(nv(ent, Channel::Auto));
        assert!(r.is_empty());
        let (_e, r) = t.add(nv(ent, Channel::Auto));
        assert!(r.is_empty());
        let other = Source::Entity {
            num: 4,
            pos: Vec3::ZERO,
        };
        let (_f, r) = t.add(nv(other, Channel::Weapon));
        assert!(r.is_empty());
        assert_eq!(t.len(), 5);
    }

    #[test]
    fn world_points_replace_on_non_auto_channel_but_not_on_auto() {
        let mut t = VoiceTable::new();
        let (a, r) = t.add(nv(Source::Point(Vec3::ZERO), Channel::Voice));
        assert!(r.is_empty());
        let (_b, r) = t.add(nv(Source::Point(Vec3::new(10.0, 0.0, 0.0)), Channel::Voice));
        assert_eq!(r, vec![a]);
        assert_eq!(t.len(), 1);

        let (_c, r) = t.add(nv(Source::Point(Vec3::ZERO), Channel::Auto));
        assert!(r.is_empty());
        let (_d, r) = t.add(nv(Source::Point(Vec3::ZERO), Channel::Auto));
        assert!(r.is_empty());
        assert_eq!(t.len(), 3);
    }

    #[test]
    fn update_spatializes_points_and_tracks_entities() {
        let mut t = VoiceTable::new();
        let l = Listener::default();
        let (p, _) = t.add(nv(Source::Point(l.right * 600.0), Channel::Auto));
        let (e, _) = t.add(nv(
            Source::Entity {
                num: 3,
                pos: Vec3::ZERO,
            },
            Channel::Auto,
        ));
        let (loc, _) = t.add(nv2d(Channel::Auto));
        let mut pos = HashMap::new();
        pos.insert(3u32, -l.right * 100.0);
        let ups = t.update(&l, &pos, 1.0);
        let get = |id| ups.iter().find(|u| u.id == id).unwrap();
        assert!((get(p).gain - 0.5).abs() < 1e-5); // 600 into a 100..1100 range
        assert!((get(p).pan - 1.0).abs() < 1e-5);
        assert_eq!(get(e).gain, 1.0);
        assert!((get(e).pan + 1.0).abs() < 1e-5);
        assert_eq!((get(loc).gain, get(loc).pan), (1.0, 0.0));
        // Entity gone from the map: keeps its last position.
        let ups = t.update(&l, &HashMap::new(), 1.0);
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
        let (id, _) = t.add(v);
        let ups = t.update(&Listener::default(), &HashMap::new(), 0.5);
        assert!((get_from(&ups, id).gain - 0.75).abs() < 1e-5);
    }

    #[test]
    fn master_ducks_slaves_while_playing() {
        let mut t = VoiceTable::new();
        let mut slave = nv2d(Channel::Auto);
        slave.master_slave = MasterSlave::Slave(0.25);
        let (s, _) = t.add(slave);
        let ups = t.update(&Listener::default(), &HashMap::new(), 1.0);
        assert_eq!(get_from(&ups, s).gain, 1.0);
        let mut master = nv2d(Channel::Auto);
        master.master_slave = MasterSlave::Master;
        let (m, _) = t.add(master);
        let ups = t.update(&Listener::default(), &HashMap::new(), 1.0);
        assert!((get_from(&ups, s).gain - 0.25).abs() < 1e-5);
        assert_eq!(get_from(&ups, m).gain, 1.0);
        t.remove(m);
        let ups = t.update(&Listener::default(), &HashMap::new(), 1.0);
        assert_eq!(get_from(&ups, s).gain, 1.0);
    }

    #[test]
    fn slave_below_the_cap_is_unaffected() {
        let mut t = VoiceTable::new();
        let mut slave = nv2d(Channel::Auto);
        slave.volume = 0.2;
        slave.master_slave = MasterSlave::Slave(0.25);
        let (s, _) = t.add(slave);
        let mut master = nv2d(Channel::Auto);
        master.master_slave = MasterSlave::Master;
        t.add(master);
        let ups = t.update(&Listener::default(), &HashMap::new(), 1.0);
        assert!((get_from(&ups, s).gain - 0.2).abs() < 1e-5);
    }

    #[test]
    fn clear_returns_every_id() {
        let mut t = VoiceTable::new();
        let (a, _) = t.add(nv2d(Channel::Auto));
        let (b, _) = t.add(nv2d(Channel::Auto));
        let mut ids = t.clear();
        ids.sort();
        assert_eq!(ids, vec![a, b]);
        assert_eq!(t.len(), 0);
        // Ids keep counting up across a clear rather than reusing freed ones.
        let (c, _) = t.add(nv2d(Channel::Auto));
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
        let (s, _) = t.add(slave);
        let mut master = nv2d(Channel::Auto);
        master.master_slave = MasterSlave::Master;
        t.add(master);
        // falloff(600, 100, 1100) = 0.5; min(0.5, 0.25) * 0.5 = 0.125.
        let ups = t.update(&l, &HashMap::new(), 0.5);
        assert!((get_from(&ups, s).gain - 0.125).abs() < 1e-5);

        // Cap above scale*v; min(0.5, 0.75) * 0.5 = 0.25.
        let mut t = VoiceTable::new();
        let mut slave = nv(Source::Point(l.right * 600.0), Channel::Auto);
        slave.volume = 1.0;
        slave.master_slave = MasterSlave::Slave(0.75);
        let (s, _) = t.add(slave);
        let mut master = nv2d(Channel::Auto);
        master.master_slave = MasterSlave::Master;
        t.add(master);
        let ups = t.update(&l, &HashMap::new(), 0.5);
        assert!((get_from(&ups, s).gain - 0.25).abs() < 1e-5);
    }

    #[test]
    fn retain_ids_and_is_empty() {
        let mut t = VoiceTable::new();
        assert!(t.is_empty());
        let (a, _) = t.add(nv2d(Channel::Auto));
        let (b, _) = t.add(nv2d(Channel::Auto));
        let mut ids = t.ids();
        ids.sort();
        assert_eq!(ids, vec![a, b]);
        assert!(!t.is_empty());
        t.retain(|id| id == a);
        assert_eq!(t.ids(), vec![a]);
        assert!(!t.is_empty());
    }
}
