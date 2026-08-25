//! Spectator audio. Engine facts are in docs/research/cod11-sound-system.md.

pub mod alias;
pub mod bank;
pub mod cues;
mod handle;
pub mod spatial;
pub mod voices;

use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::time::{Duration, Instant};

use glam::Vec3;
use kira::sound::streaming::StreamingSoundData;
use kira::sound::PlaybackState;
use kira::track::MainTrackBuilder;
use kira::{AudioManager, AudioManagerSettings, DefaultBackend, StartTime};

use crate::audio::alias::{AliasRow, AliasTable, Pick};
use crate::audio::bank::SoundBank;
use crate::audio::cues::{Cue, CueCtx, Source};
use crate::audio::handle::{start_sound, Handle};
use crate::audio::spatial::{amplitude_db, Listener};
use crate::audio::voices::{NewVoice, VoiceId, VoiceTable};
use crate::fx::registry::{EV_AMMO_PICKUP, EV_ITEM_PICKUP};
use crate::fx::sim::{FxSound, Rng};
use vcod_common::net::events::GameEvent;
use vcod_common::pk3::Pk3Fs;
use vcod_common::weapon::WeaponSounds;

/// Kira's per-track sound cap. Retail's pools are smaller (research doc,
/// section 2) but steal voices; vcod refuses the new sound instead.
const SOUND_CAPACITY: usize = 256;

/// Applied to every alias's random volume; `FUN_0044d5c0` @ `CoDMP.exe
/// 0x44d5c0` (research doc, section 3).
const VOLUME_SCALE: f32 = 0.8;

pub struct AudioOpts {
    pub enabled: bool,
    /// Master volume, linear 0..1.
    pub volume: f32,
}

#[derive(Default, Clone, Copy, Debug)]
pub struct AudioStats {
    pub voices: usize,
    pub plays: u64,
    /// Cues whose alias is not in the table.
    pub misses: u64,
    /// 3D cues whose emitter was already past `dist_max` at start.
    pub culled: u64,
    /// Read or decode failures; a static file counts once (the bank caches
    /// the failure), a stream once per attempt. Only the tests read it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub decode_failures: u64,
    /// Sounds kira refused for lack of capacity.
    pub drops: u64,
    pub step_ms: f32,
}

pub struct AudioSystem {
    /// `None` when disabled or no device opened; cues still resolve, nothing
    /// starts.
    manager: Option<AudioManager<DefaultBackend>>,
    aliases_all: AliasTable,
    /// `aliases_all` filtered by loadspec for the current map; empty before
    /// the first gamestate.
    aliases: AliasTable,
    bank: SoundBank,
    table: VoiceTable,
    handles: HashMap<VoiceId, Handle>,
    listener: Listener,
    /// Entity whose playerState we ride. Its body is never drawn, so `step`
    /// pins its voices to the camera. `u32::MAX` before the first
    /// `set_listener`.
    ps_entity: u32,
    /// Lowercased alias name -> last picked row index, for the no-repeat
    /// rule (research doc, section 1e).
    last_pick: HashMap<String, usize>,
    /// Per 1-based CS7 index, filled lazily; `None` = load failed.
    pub(crate) weapon_sounds: HashMap<i32, Option<WeaponSounds>>,
    rng: Rng,
    master_volume: f32,
    stats: AudioStats,
    /// Streamed read or decode failures; the bank counts the static ones.
    stream_failures: u64,
    missed: HashSet<String>,
    /// Configstring 3 ambient and its voice; the name lets a same-alias
    /// resend be a no-op.
    ambient: Option<(String, VoiceId)>,
    /// Live `es.loopSound` voices, keyed by entity.
    loop_voices: HashMap<u32, LoopVoice>,
    /// Aliases already warned about on the `loopSound` path, which retries
    /// every snapshot.
    warned_loops: HashSet<String>,
    /// `j`/`k`/`l` letters already logged as unhandled.
    quick_chat_seen: HashSet<String>,
}

/// One live `es.loopSound`. `looping` is the alias row's flag. Only a
/// looping voice is restarted when it loses its voice; a non-looping row (a
/// data error, retail would machine-gun it) plays once.
#[derive(Clone, Copy, Debug)]
struct LoopVoice {
    idx: i32,
    id: VoiceId,
    looping: bool,
}

impl AudioSystem {
    pub fn new(fs: &Pk3Fs, opts: AudioOpts) -> AudioSystem {
        let manager = if opts.enabled {
            let settings = AudioManagerSettings {
                main_track_builder: MainTrackBuilder::new().sound_capacity(SOUND_CAPACITY),
                ..Default::default()
            };
            match AudioManager::<DefaultBackend>::new(settings) {
                Ok(m) => Some(m),
                Err(e) => {
                    log::warn!("audio: cannot open an output device ({e}); running silent");
                    None
                }
            }
        } else {
            None
        };
        AudioSystem {
            manager,
            aliases_all: AliasTable::load(fs),
            aliases: AliasTable::default(),
            bank: SoundBank::default(),
            table: VoiceTable::new(),
            handles: HashMap::new(),
            listener: Listener::default(),
            ps_entity: u32::MAX,
            last_pick: HashMap::new(),
            weapon_sounds: HashMap::new(),
            rng: Rng::new(0x5eed_a11a5),
            master_volume: opts.volume.clamp(0.0, 1.0),
            stats: AudioStats::default(),
            stream_failures: 0,
            missed: HashSet::new(),
            ambient: None,
            loop_voices: HashMap::new(),
            warned_loops: HashSet::new(),
            quick_chat_seen: HashSet::new(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.manager.is_some()
    }

    /// New map. Refilters the aliases by loadspec and drops everything keyed
    /// to the old table.
    pub fn on_gamestate(&mut self, map: &str) {
        self.aliases = self.aliases_all.for_map(map);
        self.table.clear();
        // A dropped kira handle keeps playing.
        for (_, mut h) in self.handles.drain() {
            h.stop();
        }
        self.last_pick.clear();
        self.weapon_sounds.clear();
        // Both index the cleared table; the caller restarts the ambient from
        // CS 3.
        self.ambient = None;
        self.loop_voices.clear();
        self.stats.voices = 0;
        log::info!("audio: {} aliases apply on {map}", self.aliases.len());
    }

    pub fn set_listener(
        &mut self,
        pos: Vec3,
        forward: Vec3,
        right: Vec3,
        up: Vec3,
        ps_entity: u32,
    ) {
        self.listener = Listener {
            pos,
            forward,
            right,
            up,
        };
        self.ps_entity = ps_entity;
    }

    /// Loads the weapon file behind CS7 index `index` once; `None` cached on
    /// failure.
    pub(crate) fn ensure_weapon(&mut self, fs: &Pk3Fs, configstrings: &[String], index: i32) {
        if index <= 0 || self.weapon_sounds.contains_key(&index) {
            return;
        }
        let names = crate::entities::split_weapon_list(
            configstrings.get(7).map(String::as_str).unwrap_or(""),
        );
        let sounds = crate::entities::weapon_name_for_index(&names, index).and_then(|name| {
            match vcod_common::weapon::load(fs, name) {
                Ok(def) => Some(def.sounds),
                Err(e) => {
                    log::warn!("audio: weapon {name}: {e:#}");
                    None
                }
            }
        });
        self.weapon_sounds.insert(index, sounds);
    }

    pub fn on_game_event(
        &mut self,
        fs: &Pk3Fs,
        ev: &GameEvent,
        configstrings: &[String],
        muzzles: &HashMap<u32, (Vec3, Vec3)>,
    ) {
        self.ensure_weapon(fs, configstrings, ev.weapon);
        // `parm` is a CS7 weapon index on the pickup events alone.
        if matches!(ev.event, EV_ITEM_PICKUP | EV_AMMO_PICKUP) {
            self.ensure_weapon(fs, configstrings, ev.parm);
        }
        let cues = {
            let ctx = CueCtx {
                configstrings,
                weapon_sounds: &self.weapon_sounds,
                muzzles,
                listener_pos: self.listener.pos,
                ps_entity: self.ps_entity,
            };
            cues::resolve(ev, &ctx)
        };
        for c in cues {
            self.play(fs, c);
        }
    }

    /// `None` for an unknown alias (a miss) and for a `null.wav` row (not a
    /// miss).
    fn pick_row(&mut self, cue: &Cue) -> Option<AliasRow> {
        let roll = self.rng.next_f32();
        let key = cue.alias.to_ascii_lowercase();
        let last = self.last_pick.get(&key).copied();
        let (pick, index) = self.aliases.pick(&cue.alias, roll, last);
        let row = match pick {
            Pick::Row(r) => Some(r.clone()),
            Pick::Silent => None,
            Pick::Unknown => {
                self.stats.misses += 1;
                if self.missed.insert(key) {
                    log::debug!("audio: unknown sound alias {:?}", cue.alias);
                }
                return None;
            }
        };
        if let Some(i) = index {
            self.last_pick.insert(key, i);
        }
        row
    }

    /// Starts `cue`. A disabled system still resolves and allocates the voice
    /// (counters and loop bookkeeping are table state); `step` reaps the
    /// handleless voice next frame.
    pub fn play(&mut self, fs: &Pk3Fs, cue: Cue) -> Option<VoiceId> {
        self.play_inner(fs, cue, true)
    }

    /// A `Sound` emitter inside an effect is a world-point cue.
    pub fn play_fx(&mut self, fs: &Pk3Fs, sounds: Vec<FxSound>) {
        for s in sounds {
            self.play(
                fs,
                Cue {
                    alias: s.alias,
                    source: Source::Point(s.pos),
                    delay_s: s.delay_s,
                },
            );
        }
    }

    /// `count_cull` is false on the loop-sound path, which retries every
    /// snapshot and would otherwise tick `culled` per snapshot.
    fn play_inner(&mut self, fs: &Pk3Fs, cue: Cue, count_cull: bool) -> Option<VoiceId> {
        let emitter = match cue.source {
            Source::Point(p) => p,
            Source::Entity { pos, .. } => pos,
        };
        // A NaN origin passes every distance test, and a NaN pan poisons
        // kira's mix for good.
        if !emitter.is_finite() {
            log::debug!("audio: {:?} at a non-finite position {emitter}", cue.alias);
            if count_cull {
                self.stats.culled += 1;
            }
            return None;
        }
        let row = self.pick_row(&cue)?;
        // Sampled before the device check so the rng path is device
        // independent.
        let volume = self.rng.range(row.vol.0, row.vol.1) * VOLUME_SCALE;
        let pitch = self.rng.range(row.pitch.0, row.pitch.1) as f64;
        let spatial = row.channel.is_spatial();
        // Start-time cull, `FUN_0044d670` @ `CoDMP.exe 0x44d670` (research
        // doc, section 3). There is no later cull.
        if spatial && emitter.distance(self.listener.pos) > row.dist.1 {
            if count_cull {
                self.stats.culled += 1;
            }
            return None;
        }
        let (id, replaced) = self.table.add(NewVoice {
            source: cue.source,
            channel: row.channel,
            volume,
            dist: row.dist,
            master_slave: row.master_slave,
            spatial,
            looping: row.looping,
        });
        for r in replaced {
            self.stop_voice(r);
        }
        // Initial gain/pan so the first audio chunk is already placed.
        let (db, pan) = self
            .table
            .update(&self.listener, &HashMap::new(), self.master_volume)
            .into_iter()
            .find(|u| u.id == id)
            .map(|u| (amplitude_db(u.gain), u.pan))
            // Unreachable; silence if it ever is.
            .unwrap_or((-60.0, 0.0));
        // `from_secs_f32` panics on a negative or NaN delay, and effect files
        // can carry one.
        let start = match Duration::try_from_secs_f32(cue.delay_s) {
            Ok(d) if !d.is_zero() => StartTime::Delayed(d),
            _ => StartTime::Immediate,
        };

        // No device. The voice stays in the table (ambient and loop
        // bookkeeping are table state) and `step` reaps it next frame.
        // `manager` is borrowed mutably across the field accesses below.
        let Some(manager) = self.manager.as_mut() else {
            return Some(id);
        };
        let started = if row.streamed {
            // `read_stream` already warned on a missing file.
            match SoundBank::read_stream(fs, &row.file)
                .map(|b| StreamingSoundData::from_cursor(Cursor::new(b)))
            {
                Some(Ok(data)) => Some(start_sound!(
                    manager,
                    data,
                    Stream,
                    db,
                    pan,
                    pitch,
                    start,
                    row.looping
                )),
                Some(Err(e)) => {
                    log::warn!("audio: stream {}: decode failed: {e}", row.file);
                    self.stream_failures += 1;
                    None
                }
                None => {
                    self.stream_failures += 1;
                    None
                }
            }
        } else {
            match self.bank.get(fs, &row.file) {
                // The bank already counted and warned about a failure.
                None => None,
                Some(data) => Some(start_sound!(
                    manager,
                    data,
                    Static,
                    db,
                    pan,
                    pitch,
                    start,
                    row.looping
                )),
            }
        };

        match started {
            Some(Ok(h)) => {
                self.handles.insert(id, h);
                self.stats.plays += 1;
                Some(id)
            }
            Some(Err(e)) => {
                log::debug!("audio: play {}: {e}", cue.alias);
                self.stats.drops += 1;
                self.table.remove(id);
                None
            }
            None => {
                self.table.remove(id);
                None
            }
        }
    }

    fn stop_voice(&mut self, id: VoiceId) {
        self.table.remove(id);
        if let Some(mut h) = self.handles.remove(&id) {
            h.stop();
        }
    }

    /// Configstring 3's `n` key, `ambientPlay`'s wire form (research doc,
    /// section 9). The row is `local`, `streamed`, `looping`, so `play` makes
    /// it a 2D loop. A round restart resends CS 3 with the same alias and a
    /// new `t`; an unchanged alias with a live voice is a no-op, a lost voice
    /// is restarted. It sits on [`cues::AMBIENT_ENTITY`] so no cue can replace
    /// it (research doc, "vcod divergences from retail"). The `t` crossfade
    /// deadline is ignored; vcod cuts over.
    pub fn set_ambient(&mut self, fs: &Pk3Fs, alias: Option<&str>) {
        let alias = alias.unwrap_or("").trim();
        if self
            .ambient
            .as_ref()
            .is_some_and(|(cur, id)| cur == alias && self.table.looping(*id).is_some())
        {
            return;
        }
        if let Some((_, id)) = self.ambient.take() {
            self.stop_voice(id);
        }
        if alias.is_empty() {
            return;
        }
        let cue = Cue {
            alias: alias.to_string(),
            // The row is 2D; the position is never read.
            source: Source::Entity {
                num: cues::AMBIENT_ENTITY,
                pos: self.listener.pos,
            },
            delay_s: 0.0,
        };
        if let Some(id) = self.play(fs, cue) {
            log::info!("audio: ambient {alias}");
            self.ambient = Some((alias.to_string(), id));
        }
    }

    /// This snapshot's `es.loopSound` per entity as `(CS_SOUNDS index,
    /// origin)` (research doc, section 9). Called per snapshot; `step` keeps
    /// the voices moving. An entity absent from `loops` (or at index 0) has
    /// its loop stopped; live, loops end by the entity leaving the snapshot,
    /// never by `loopSound -> 0`.
    pub fn set_loop_sounds(
        &mut self,
        fs: &Pk3Fs,
        configstrings: &[String],
        loops: &HashMap<u32, (i32, Vec3)>,
    ) {
        let gone: Vec<u32> = self
            .loop_voices
            .iter()
            .filter(|(ent, v)| !matches!(loops.get(ent), Some(&(idx, _)) if idx == v.idx))
            .map(|(ent, _)| *ent)
            .collect();
        for ent in gone {
            if let Some(v) = self.loop_voices.remove(&ent) {
                log::debug!("audio: loop off entity {ent}");
                self.stop_voice(v.id);
            }
        }

        for (&ent, &(idx, pos)) in loops {
            if idx <= 0 {
                continue;
            }
            if let Some(&LoopVoice { id, looping, .. }) = self.loop_voices.get(&ent) {
                // Still live, or a non-looping row already played once.
                if !looping || self.table.looping(id).is_some() {
                    continue;
                }
                self.loop_voices.remove(&ent);
            }
            // Resolved per snapshot; the `CS_SOUNDS` block fills lazily
            // (research doc, section 9).
            let alias = match configstrings.get(cues::CS_SOUND_ALIASES + idx as usize) {
                Some(a) if !a.is_empty() => a.clone(),
                _ => continue,
            };
            // Dropped here rather than counted as a miss every snapshot.
            if self.aliases.get(&alias).is_none() {
                if self.warned_loops.insert(alias.to_ascii_lowercase()) {
                    log::warn!("audio: loopSound alias {alias:?} is not in the alias table");
                }
                continue;
            }
            let cue = Cue {
                alias: alias.clone(),
                source: Source::Entity { num: ent, pos },
                delay_s: 0.0,
            };
            // Silent pick, cull or decode failure; the next snapshot retries.
            let Some(id) = self.play_inner(fs, cue, false) else {
                continue;
            };
            log::debug!("audio: loop {alias:?} on entity {ent} at {pos}");
            let looping = self.table.looping(id).unwrap_or(false);
            if !looping && self.warned_loops.insert(alias.to_ascii_lowercase()) {
                log::warn!(
                    "audio: loopSound alias {alias:?} is not a looping row; playing it once"
                );
            }
            self.loop_voices.insert(ent, LoopVoice { idx, id, looping });
        }
    }

    /// A server command the net layer did not consume; returns whether it was
    /// a sound command. `s <idx>` is `playLocalSound` (research doc, section
    /// 9). The alias is resolved at receive time since the `CS_SOUNDS` slot
    /// is often created by the `d` update just before it.
    pub fn on_server_command(
        &mut self,
        fs: &Pk3Fs,
        tokens: &[String],
        configstrings: &[String],
    ) -> bool {
        let Some(cmd) = tokens.first() else {
            return false;
        };
        match cmd.as_str() {
            "s" => {
                let idx = tokens
                    .get(1)
                    .and_then(|t| t.parse::<usize>().ok())
                    .filter(|i| (1..=255).contains(i));
                let Some(idx) = idx else {
                    log::debug!("audio: malformed playLocalSound command {tokens:?}");
                    return true;
                };
                match configstrings.get(cues::CS_SOUND_ALIASES + idx) {
                    Some(a) if !a.is_empty() => {
                        log::debug!("audio: playLocalSound {idx} = {a:?}");
                        let cue = Cue {
                            alias: a.clone(),
                            source: Source::Entity {
                                num: self.ps_entity,
                                pos: self.listener.pos,
                            },
                            delay_s: 0.0,
                        };
                        self.play(fs, cue);
                    }
                    _ => log::debug!("audio: playLocalSound {idx}: no alias in that configstring"),
                }
                true
            }
            // Quick chat (research doc, section 9). Recognised, not played;
            // the id-to-alias table is an open item.
            "j" | "k" | "l" => {
                if self.quick_chat_seen.insert(cmd.clone()) {
                    log::debug!("audio: quick chat {cmd:?} not played (no id->alias table yet)");
                }
                true
            }
            _ => false,
        }
    }

    /// Once per frame, after the event drain.
    pub fn step(&mut self, entity_pos: &HashMap<u32, Vec3>) {
        let t0 = Instant::now();
        let handles = &mut self.handles;
        self.table.retain(|id| match handles.get(&id) {
            Some(h) => h.state() != PlaybackState::Stopped,
            None => false,
        });
        let live: HashSet<VoiceId> = self.table.ids().into_iter().collect();
        // A dropped kira handle keeps playing.
        handles.retain(|id, h| {
            let keep = live.contains(id);
            if !keep {
                h.stop();
            }
            keep
        });

        if !self.table.is_empty() {
            // The ridden entity has no drawn body, so `entity_pos` never
            // carries it.
            let mut positions = entity_pos.clone();
            positions.insert(self.ps_entity, self.listener.pos);
            for u in self
                .table
                .update(&self.listener, &positions, self.master_volume)
            {
                if let Some(h) = handles.get_mut(&u.id) {
                    h.set_volume(amplitude_db(u.gain));
                    h.set_panning(u.pan);
                }
            }
        }
        self.stats.voices = self.table.len();
        self.stats.step_ms = t0.elapsed().as_secs_f32() * 1000.0;
    }

    pub fn stats(&self) -> AudioStats {
        AudioStats {
            decode_failures: self.bank.failures + self.stream_failures,
            ..self.stats
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_system_is_inert_but_counts_resolution() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut a = AudioSystem::new(
            &fs,
            AudioOpts {
                enabled: false,
                volume: 1.0,
            },
        );
        assert!(!a.enabled());
        a.on_gamestate("mp_carentan");
        a.play(
            &fs,
            Cue {
                alias: "weap_thompson_fire".into(),
                source: Source::Point(Vec3::ZERO),
                delay_s: 0.0,
            },
        );
        a.play(
            &fs,
            Cue {
                alias: "no_such_alias".into(),
                source: Source::Point(Vec3::ZERO),
                delay_s: 0.0,
            },
        );
        // Out of range, so culled rather than missed.
        a.play(
            &fs,
            Cue {
                alias: "step_run_dirt".into(),
                source: Source::Point(Vec3::splat(100_000.0)),
                delay_s: 0.0,
            },
        );
        a.step(&HashMap::new());
        let s = a.stats();
        assert_eq!((s.plays, s.misses, s.voices, s.culled), (0, 1, 0, 1));
    }

    #[test]
    fn on_gamestate_filters_the_full_table() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut a = AudioSystem::new(
            &fs,
            AudioOpts {
                enabled: false,
                volume: 1.0,
            },
        );
        assert!(a.aliases.is_empty());
        a.on_gamestate("mp_carentan");
        assert!(a.aliases.len() > 1000, "aliases: {}", a.aliases.len());
    }

    #[test]
    fn repeated_plays_thread_the_no_repeat_index() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut a = AudioSystem::new(
            &fs,
            AudioOpts {
                enabled: false,
                volume: 1.0,
            },
        );
        a.on_gamestate("mp_carentan");
        // At least 3 variants, so the no-repeat rule applies.
        assert!(a.aliases.get("step_run_dirt").unwrap().len() >= 3);
        let mut picks = Vec::new();
        for _ in 0..3 {
            a.play(
                &fs,
                Cue {
                    alias: "step_run_dirt".into(),
                    source: Source::Point(Vec3::ZERO),
                    delay_s: 0.0,
                },
            );
            picks.push(a.last_pick["step_run_dirt"]);
        }
        assert_ne!(picks[0], picks[1], "picks: {picks:?}");
        assert_ne!(picks[1], picks[2], "picks: {picks:?}");
        let s = a.stats();
        assert_eq!((s.plays, s.misses), (0, 0));
    }

    #[test]
    fn game_event_resolves_a_weapon_fire_cue() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut a = AudioSystem::new(
            &fs,
            AudioOpts {
                enabled: false,
                volume: 1.0,
            },
        );
        a.on_gamestate("mp_carentan");
        a.set_listener(Vec3::ZERO, Vec3::X, -Vec3::Y, Vec3::Z, 5);
        let mut cs = vec![String::new(); 8];
        cs[7] = "kar98k_mp thompson_mp".to_string();
        let ev = GameEvent {
            event: crate::fx::registry::EV_FIRE_WEAPON,
            // Not a pickup, so `parm` must not be loaded as a weapon index.
            parm: 1,
            entity_num: 5,
            client_num: 5,
            weapon: 2,
            surf_type: 0,
            pos: [0.0; 3],
            dir: [0.0; 3],
            other_entity_num: u32::MAX,
            attacker_entity_num: -1,
        };
        a.on_game_event(&fs, &ev, &cs, &HashMap::new());
        assert_eq!(
            a.weapon_sounds[&2].as_ref().unwrap().fire.as_deref(),
            Some("weap_thompson_fire")
        );
        assert!(a.last_pick.contains_key("weap_thompson_fire"));
        assert!(!a.weapon_sounds.contains_key(&1));
        let s = a.stats();
        assert_eq!(
            (s.plays, s.misses, s.decode_failures, s.culled),
            (0, 0, 0, 0)
        );
    }

    #[test]
    fn loop_sounds_start_stop_with_entities() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut a = AudioSystem::new(
            &fs,
            AudioOpts {
                enabled: false,
                volume: 1.0,
            },
        );
        a.on_gamestate("mp_carentan");
        let mut cs = vec![String::new(); 800];
        cs[cues::CS_SOUND_ALIASES + 3] = "bomb_tick".to_string();
        let mut loops = HashMap::new();
        loops.insert(12u32, (3, Vec3::new(1.0, 2.0, 3.0)));
        a.set_loop_sounds(&fs, &cs, &loops);
        assert_eq!(a.loop_voices.len(), 1);
        let id = a.loop_voices[&12].id;
        a.set_loop_sounds(&fs, &cs, &loops); // same state, no second voice
        assert_eq!(a.loop_voices.len(), 1);
        assert_eq!(a.loop_voices[&12].id, id);
        assert_eq!(a.table.len(), 1);

        // A looping voice that lost its table entry is restarted next
        // snapshot.
        a.step(&HashMap::new());
        assert_eq!(a.table.len(), 0);
        a.set_loop_sounds(&fs, &cs, &loops);
        assert_eq!(a.loop_voices.len(), 1);
        assert_ne!(a.loop_voices[&12].id, id);
        assert_eq!(a.table.len(), 1);

        // The entity leaves the snapshot; live stops arrive this way
        // (research doc, section 9).
        a.set_loop_sounds(&fs, &cs, &HashMap::new());
        assert!(a.loop_voices.is_empty());
        assert_eq!(a.table.len(), 0);
    }

    #[test]
    fn non_looping_loop_alias_plays_once_and_is_not_restarted() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut a = AudioSystem::new(
            &fs,
            AudioOpts {
                enabled: false,
                volume: 1.0,
            },
        );
        a.on_gamestate("mp_carentan");
        let mut cs = vec![String::new(); 800];
        // `MP_bomb_plant` is `all_mp`, one-shot, channel `auto`.
        cs[cues::CS_SOUND_ALIASES + 4] = "MP_bomb_plant".to_string();
        let mut loops = HashMap::new();
        loops.insert(13u32, (4, Vec3::ZERO));
        a.set_loop_sounds(&fs, &cs, &loops);
        let id = a.loop_voices[&13].id;
        assert!(!a.loop_voices[&13].looping);
        a.step(&HashMap::new());
        assert_eq!(a.table.len(), 0);
        a.set_loop_sounds(&fs, &cs, &loops);
        assert_eq!(a.loop_voices[&13].id, id); // not restarted
        assert_eq!(a.table.len(), 0);
        a.set_loop_sounds(&fs, &cs, &HashMap::new());
        assert!(a.loop_voices.is_empty());
    }

    #[test]
    fn loop_sound_index_change_restarts_and_zero_stops() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut a = AudioSystem::new(
            &fs,
            AudioOpts {
                enabled: false,
                volume: 1.0,
            },
        );
        a.on_gamestate("mp_carentan");
        let mut cs = vec![String::new(); 800];
        cs[cues::CS_SOUND_ALIASES + 3] = "bomb_tick".to_string();
        cs[cues::CS_SOUND_ALIASES + 4] = "ambient_mp_carentan".to_string();
        let mut loops = HashMap::new();
        loops.insert(12u32, (3, Vec3::ZERO));
        a.set_loop_sounds(&fs, &cs, &loops);
        let first = a.loop_voices[&12].id;
        loops.insert(12u32, (4, Vec3::ZERO));
        a.set_loop_sounds(&fs, &cs, &loops);
        assert_eq!(a.loop_voices.len(), 1);
        assert_ne!(a.loop_voices[&12].id, first);
        assert_eq!(a.table.len(), 1); // the old voice was stopped, not left
        loops.insert(12u32, (0, Vec3::ZERO));
        a.set_loop_sounds(&fs, &cs, &loops);
        assert!(a.loop_voices.is_empty());
        assert_eq!(a.table.len(), 0);
    }

    /// A round restart resends CS 3 with the same alias and a new `t`
    /// (research doc, section 9).
    #[test]
    fn ambient_restarts_only_on_a_changed_alias() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut a = AudioSystem::new(
            &fs,
            AudioOpts {
                enabled: false,
                volume: 1.0,
            },
        );
        a.on_gamestate("mp_carentan");
        a.set_ambient(&fs, Some("ambient_mp_carentan"));
        let id = a.ambient.as_ref().unwrap().1;
        a.set_ambient(&fs, Some("ambient_mp_carentan"));
        assert_eq!(a.ambient.as_ref().unwrap().1, id);
        assert_eq!(a.table.len(), 1);
        a.set_ambient(&fs, Some("ambient_mp_harbor"));
        assert_ne!(a.ambient.as_ref().unwrap().1, id);
        assert_eq!(a.table.len(), 1);
        a.set_ambient(&fs, None);
        assert!(a.ambient.is_none());
        assert_eq!(a.table.len(), 0);
    }

    #[test]
    fn server_command_s_plays_the_announcer_alias() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut a = AudioSystem::new(
            &fs,
            AudioOpts {
                enabled: false,
                volume: 1.0,
            },
        );
        a.on_gamestate("mp_carentan");
        let mut cs = vec![String::new(); 800];
        cs[cues::CS_SOUND_ALIASES + 5] = "MP_announcer_allies_win".to_string();
        let tok = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();

        assert!(a.on_server_command(&fs, &tok(&["s", "5"]), &cs));
        assert_eq!(a.table.len(), 1);
        assert_eq!(a.stats().misses, 0);

        // Unparsable, empty slot, out of range; recognised, silent.
        assert!(a.on_server_command(&fs, &tok(&["s", "x"]), &cs));
        assert!(a.on_server_command(&fs, &tok(&["s", "6"]), &cs));
        assert!(a.on_server_command(&fs, &tok(&["s", "0"]), &cs));
        assert_eq!(a.table.len(), 1);

        // Quick chat is recognised but not played.
        for c in ["j", "k", "l"] {
            assert!(a.on_server_command(&fs, &tok(&[c, "0", "0", "0", "yes_sir"]), &cs));
        }
        assert_eq!(a.table.len(), 1);

        // The HUD's scoreboard command, not ours.
        assert!(!a.on_server_command(&fs, &tok(&["b", "1"]), &cs));
        assert!(!a.on_server_command(&fs, &[], &cs));
    }

    #[test]
    fn ambient_keeps_a_reserved_identity_and_restarts_when_lost() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut a = AudioSystem::new(
            &fs,
            AudioOpts {
                enabled: false,
                volume: 1.0,
            },
        );
        a.on_gamestate("mp_carentan");
        a.set_listener(Vec3::ZERO, Vec3::X, -Vec3::Y, Vec3::Z, 5);
        a.set_ambient(&fs, Some("ambient_mp_carentan"));
        let id = a.ambient.as_ref().unwrap().1;

        // `player_out_of_ammo` has a `local` row; on a shared
        // `(ps_entity, Local)` identity it would replace the ambient.
        for _ in 0..8 {
            a.play(
                &fs,
                Cue {
                    alias: "player_out_of_ammo".into(),
                    source: Source::Entity {
                        num: 5,
                        pos: Vec3::ZERO,
                    },
                    delay_s: 0.0,
                },
            );
        }
        assert!(a.table.looping(id).is_some(), "the ambient was replaced");

        // No handles on a silent system, so `step` reaps every voice.
        a.step(&HashMap::new());
        assert!(a.table.looping(id).is_none());
        a.set_ambient(&fs, Some("ambient_mp_carentan"));
        assert_ne!(a.ambient.as_ref().unwrap().1, id);
        assert_eq!(a.table.len(), 1);
    }

    /// An unknown alias and an out-of-range emitter are permanent; the
    /// per-snapshot retry must not count them.
    #[test]
    fn loop_retries_do_not_inflate_the_counters() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut a = AudioSystem::new(
            &fs,
            AudioOpts {
                enabled: false,
                volume: 1.0,
            },
        );
        a.on_gamestate("mp_carentan");
        let far = Vec3::splat(50_000.0);
        let row = &a.aliases.get("bomb_tick").unwrap()[0];
        assert!(row.channel.is_spatial() && row.dist.1 < far.length());

        let mut cs = vec![String::new(); 800];
        cs[cues::CS_SOUND_ALIASES + 3] = "bomb_tick".to_string();
        cs[cues::CS_SOUND_ALIASES + 4] = "no_such_loop_alias".to_string();
        let mut loops = HashMap::new();
        loops.insert(12u32, (3, far));
        loops.insert(13u32, (4, Vec3::ZERO));
        for _ in 0..3 {
            a.set_loop_sounds(&fs, &cs, &loops);
        }
        let s = a.stats();
        assert_eq!((s.culled, s.misses), (0, 0));
        assert!(a.loop_voices.is_empty());
        assert_eq!(a.table.len(), 0);
    }

    #[test]
    fn non_finite_cue_positions_are_rejected() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut a = AudioSystem::new(
            &fs,
            AudioOpts {
                enabled: false,
                volume: 1.0,
            },
        );
        a.on_gamestate("mp_carentan");
        a.play(
            &fs,
            Cue {
                alias: "step_run_dirt".into(),
                source: Source::Point(Vec3::NAN),
                delay_s: 0.0,
            },
        );
        a.play(
            &fs,
            Cue {
                alias: "step_run_dirt".into(),
                source: Source::Entity {
                    num: 7,
                    pos: Vec3::new(0.0, f32::INFINITY, 0.0),
                },
                delay_s: 0.0,
            },
        );
        let s = a.stats();
        assert_eq!((s.culled, s.misses, s.plays), (2, 0, 0));
        assert_eq!(a.table.len(), 0);
        // Rejected ahead of the pick, so the no-repeat state is untouched.
        assert!(!a.last_pick.contains_key("step_run_dirt"));
    }

    #[test]
    fn weapon_sounds_cache_fills_from_cs7() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut a = AudioSystem::new(
            &fs,
            AudioOpts {
                enabled: false,
                volume: 1.0,
            },
        );
        let mut cs = vec![String::new(); 8];
        // CS7 is 1-based with no leading `none`, so index 1 is the first
        // token.
        cs[7] = "kar98k_mp thompson_mp".to_string();
        a.ensure_weapon(&fs, &cs, 2);
        assert_eq!(
            a.weapon_sounds[&2].as_ref().unwrap().fire.as_deref(),
            Some("weap_thompson_fire")
        );
        a.ensure_weapon(&fs, &cs, 9);
        assert!(a.weapon_sounds[&9].is_none());
    }
}
