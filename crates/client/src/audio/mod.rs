//! Spectator audio. Engine facts are in docs/research/cod11-sound-system.md.

pub mod alias;
pub mod bank;
pub mod blend;
pub mod cod2;
pub mod cues;
mod handle;
pub mod occlusion;
pub mod spatial;
mod stream;
pub mod voices;

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use vcod_common::asset_source::AssetSource;

use glam::{Mat3, Vec3};
use kira::effect::reverb::{ReverbBuilder, ReverbHandle};
use kira::effect::volume_control::{VolumeControlBuilder, VolumeControlHandle};
use kira::sound::PlaybackState;
use kira::sound::streaming::StreamingSoundData;
use kira::track::MainTrackBuilder;
use kira::{AudioManager, AudioManagerSettings, Decibels, DefaultBackend, Mix, StartTime, Tween};

use crate::audio::alias::{AliasRow, AliasTable, Pick};
use crate::audio::bank::SoundBank;
use crate::audio::cues::{Cue, CueCtx, Source};
use crate::audio::handle::{Handle, start_sound};
use crate::audio::spatial::{Listener, amplitude_db};
use crate::audio::voices::{Admitted, ENDLESS_MS, NewVoice, VoiceId, VoiceTable};
use crate::fx::registry::{EV_AMMO_PICKUP, EV_ITEM_PICKUP};
use crate::fx::sim::{FxSound, Rng};
use vcod_common::collision::CollisionWorld;
use vcod_common::net::events::GameEvent;
use vcod_common::net::flags::{EF_NODRAW, ET_MISSILE};
use vcod_common::net::protocol::Protocol;
use vcod_common::net::snapshot::Snapshot;
use vcod_common::weapon::WeaponSounds;

/// Kira's per-track sound cap. Retail's pools (research doc, section 2) are
/// enforced per class in `VoiceTable`, which evicts a victim instead; this
/// only backstops kira itself.
const SOUND_CAPACITY: usize = 256;

/// Applied to every alias's random volume; `FUN_0044d5c0` @ `CoDMP.exe
/// 0x44d5c0` (research doc, section 3).
const VOLUME_SCALE: f32 = 0.8;

/// CoD2's master gain is `snd_volume` times this (`FLOAT_00326234`, Mac
/// `SND_Init` and `SND_Update`'s `g_snd` +0x24), which every voice's
/// volume multiplies (docs/research/cod2-sound.md 3).
const COD2_VOLUME_SCALE: f32 = 0.75;
/// The secondary aliases a CoD4 row's chain starts at most: the recursion
/// check's 10 links (`cmp [esp+0x10],0xa`, iw3mp.exe 0x5c3b66).
const MAX_SECONDARY_CHAIN: usize = 10;

/// CoD4 1.7's master gain is `snd_volume` (0.8, archived) times this, the
/// double at iw3mp.exe 0x70b0c8 that `SND_Init` (0x5c6ff0) and the
/// master's update (0x5c4fa4) multiply by (docs/research/cod4-sound.md 5).
pub(crate) const COD4_VOLUME_SCALE: f32 = 0.75;

/// `ENTITYNUM_NONE`, the 0x3ff the cgame starts its shellshock sounds on
/// (`0x3002f2d0`).
const SHOCK_ENTITY: u32 = 1023;

/// The key [`AudioSystem::hold`] keeps a CoD2 scope's heartbeat under
/// (`play::cod2_shock::HoldBreath`); retail plays it on the client's own
/// entity, which vcod's holds may already key.
pub const BREATH_ENTITY: u32 = 1022;

/// The shellshock room. Retail hands the room type to Miles and every
/// voice's reverb levels as dry 1, wet the level (syscall 0xd9, CoDMP.exe
/// `0x44f3c0`, `0x44f430`); vcod's stand-in is one dark reverb on the main
/// track for any room but `generic`, its equal-power mix and a gain set so
/// the dry path stays at 1 (docs/research/cod11-shellshock.md, "Sound").
const ROOM_FEEDBACK: f64 = 0.85;
const ROOM_DAMPING: f64 = 0.9;

pub struct AudioOpts {
    pub enabled: bool,
    /// Master volume, linear 0..1.
    pub volume: f32,
    /// `mss_khz`'s mixer rate in Hz (`bank::mixer_rate`).
    pub rate: u32,
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
    /// Sounds refused for lack of capacity: every pool slot was held by
    /// voices the newcomer may not steal, or kira itself refused.
    pub drops: u64,
    /// Voices ended early to make room (`FUN_0044c350`, research doc,
    /// section 2).
    pub steals: u64,
    pub step_ms: f32,
}

pub struct AudioSystem {
    /// The connection's `CS_SOUNDS` (`Protocol::cs`), 1.1's until a
    /// gamestate names another.
    cs_sounds: usize,
    /// `None` when disabled or no device opened; cues still resolve, nothing
    /// starts.
    manager: Option<AudioManager<DefaultBackend>>,
    aliases_all: AliasTable,
    /// `aliases_all` filtered by loadspec for the current map; empty before
    /// the first gamestate.
    aliases: AliasTable,
    /// `aliases_all` filtered for the name `menu`: CoDMP.exe's alias set 0,
    /// which `CL_InitUI` loads (0x418f10) and the UI's sounds play from
    /// (docs/research/cod11-front-end.md section 20).
    menu_aliases: AliasTable,
    /// The map `aliases` was filtered for.
    map: Option<String>,
    bank: SoundBank,
    table: VoiceTable,
    handles: HashMap<VoiceId, Handle>,
    /// Line-of-sight gain per spatial voice (`occlusion.rs`); empty without
    /// a collision world.
    occlusion: occlusion::Occlusion,
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
    /// Live missile `projectileSound` voices, keyed by entity.
    projectile_voices: HashMap<u32, LoopVoice>,
    /// United Offensive's per-body MG fire loops, keyed by entity.
    fire_loops: HashMap<u32, FireLoop>,
    /// United Offensive's sound blend voices, keyed by entity.
    blends: HashMap<u32, BlendVoice>,
    /// Voices a caller holds on an entity frame by frame ([`Self::hold`]).
    held: HashMap<u32, VoiceId>,
    /// The connection is United Offensive's, whose cgame plays
    /// `loopFireSound` (1.1's ignores it).
    uo: bool,
    /// What `snd_restart` needs to reopen a stream where it was, per
    /// streamed voice outside the menu set.
    resumable: HashMap<VoiceId, Resumable>,
    /// `musicPlay`'s voice (`o`).
    music: Option<Music>,
    /// `soundFade`'s master fade (`q`).
    fade: SoundFade,
    /// When `step` last advanced the fades; they run on wall time.
    last_step: Option<Instant>,
    /// Aliases already warned about on the `loopSound` path, which retries
    /// every snapshot.
    warned_loops: HashSet<String>,
    /// The shellshock room's stand-in ([`ROOM_FEEDBACK`]) and its current
    /// wet level.
    reverb: Option<(ReverbHandle, VolumeControlHandle)>,
    room_wet: f32,
    /// `shellshock_loop`'s voice and its picked volume, and the end cue's.
    shock_loop: Option<(VoiceId, f32)>,
    shock_end: Option<VoiceId>,
    /// [`VOLUME_SCALE`] for CoD 1, [`COD2_VOLUME_SCALE`] for CoD2.
    volume_scale: f32,
    /// CoD2's current ambient track (`SND_PlayAmbientAlias`, Mac 0x4c806):
    /// its row and the slot entity it plays on, [`cues::AMBIENT_ENTITY`]
    /// or [`cues::AMBIENT_ENTITY_2`]. `ambient` holds its voice.
    cod2_ambient: Option<(AliasRow, u32)>,
    /// Configstring 3 as [`Self::set_cod2_ambient`] last applied it.
    cod2_ambient_key: Option<String>,
    /// Each CoD2 ambient voice's target volume and change per ms
    /// (`SND_UpdateBackgroundVolume`, Mac 0x4a5d2).
    ambient_ramps: HashMap<VoiceId, (f32, f32)>,
    /// CoD2's environment effects, `r` and `D` (`setReverb`).
    env: cod2::EnvEffects,
    /// CoD2 subtitles of the rows started since the last
    /// [`Self::take_subtitles`]: the `subtitle.str` key and how long it
    /// shows, ms.
    subtitles: Vec<(String, i32)>,
    /// Each started voice's row, for the tests that follow a cue to the
    /// mixer ([`Self::mix`]).
    #[cfg(test)]
    rows: HashMap<VoiceId, AliasRow>,
    /// A silent system's looping voices play on until stopped, as a
    /// device's do ([`Self::keep_silent_loops`]).
    #[cfg(test)]
    keep_loops: bool,
    /// CoD4's rule for secondary aliases: each starts its own in turn
    /// down the chain (`SND_PlaySoundAlias_Internal`, iw3mp.exe
    /// 0x5c3b5f..0x5c3bdb). Off, CoD2's: one secondary, refused when it
    /// names its own.
    chain_secondaries: bool,
    /// CoD4's entity voices keep their start's offset from the entity
    /// (`SND_SetChannelStartInfo`, cod4-sound.md 4): a loop starts as a
    /// [`Source::Tag`], so a brush model's sounds at its bounds' middle.
    entity_offsets: bool,
    /// Set across [`Self::play_as_master`]: every voice the call starts
    /// plays as a master.
    as_master: bool,
}

/// `snd_slaveFadeTime`'s default, ms (Mac `SND_Init`, 0x4b496).
const SLAVE_FADE_MS: i32 = 500;
/// `cg_subtitleMinTime`'s default, 3 s (`CG_Init`, Mac 0x145214).
const SUBTITLE_MIN_MS: i32 = 3000;

/// `musicPlay`'s track. `fade` is `musicStop`'s per-ms change of the voice's
/// volume while it fades out, retail's music slot ramp (CoDMP.exe 0x44e7a0,
/// research doc, section 10, "`musicPlay` and `musicStop`").
#[derive(Clone, Copy, Debug)]
struct Music {
    id: VoiceId,
    fade: Option<f32>,
}

/// The master fade `soundFade` drives (CoDMP.exe 0x8e0790..0x8e0798):
/// `level` scales every voice, moving `rate` per ms toward `target`
/// (research doc, section 10, "`soundFade`").
#[derive(Clone, Copy, Debug, PartialEq)]
struct SoundFade {
    level: f32,
    target: f32,
    rate: f32,
}

impl SoundFade {
    /// `S_Init`'s values (0x450029), which `snd_restart` also lands on.
    const FULL: SoundFade = SoundFade {
        level: 1.0,
        target: 1.0,
        rate: 0.0,
    };
}

/// A playing stream's start parameters, replayed by `restart`.
#[derive(Clone, Debug)]
struct Resumable {
    row: AliasRow,
    source: Source,
    volume: f32,
    pitch: f64,
    /// Where it started, in seconds; the position when no handle reports one.
    from_s: f64,
}

/// One live `es.loopSound` or `projectileSound`; `idx` is the `CS_SOUNDS`
/// index or the weapon. `looping` is the alias row's flag. Only a
/// looping voice is restarted when it loses its voice; a non-looping row (a
/// data error, retail would machine-gun it) plays once.
#[derive(Clone, Debug)]
struct LoopVoice {
    idx: i32,
    id: VoiceId,
    looping: bool,
    /// The voices the row's secondary aliases started beside it, which
    /// live and end with it.
    secondary: Vec<VoiceId>,
}

/// A sound blend entity's voice: its alias pair and the pitch it started
/// at, which a refresh keeps unless both rows have a fixed pitch.
struct BlendVoice {
    aliases: [String; 2],
    id: VoiceId,
    pitch: f64,
}

/// A body's MG fire loop (uo cgame 0x300490b0): `left` ms of the loop
/// alias, then the stop alias once.
struct FireLoop {
    weapon: i32,
    left: f32,
    voice: Option<VoiceId>,
}

impl AudioSystem {
    pub fn new(fs: &dyn AssetSource, opts: AudioOpts) -> AudioSystem {
        let aliases = match fs.game() == vcod_common::game::Game::Cod2 {
            true => cod2::load(fs),
            false => AliasTable::load(fs),
        };
        Self::with_aliases(fs, opts, aliases)
    }

    /// [`Self::new`] over an alias table read elsewhere (CoD4's zones).
    pub fn with_aliases(
        fs: &dyn AssetSource,
        opts: AudioOpts,
        aliases_all: AliasTable,
    ) -> AudioSystem {
        let mut reverb = None;
        let manager = if opts.enabled {
            let mut main = MainTrackBuilder::new().sound_capacity(SOUND_CAPACITY);
            reverb = Some((
                main.add_effect(
                    ReverbBuilder::new()
                        .feedback(ROOM_FEEDBACK)
                        .damping(ROOM_DAMPING)
                        .mix(Mix::DRY),
                ),
                main.add_effect(VolumeControlBuilder::new(Decibels::IDENTITY)),
            ));
            let settings = AudioManagerSettings {
                main_track_builder: main,
                ..Default::default()
            };
            match AudioManager::<DefaultBackend>::new(settings) {
                Ok(m) => Some(m),
                Err(e) => {
                    log::warn!("audio: cannot open an output device ({e}); running silent");
                    reverb = None;
                    None
                }
            }
        } else {
            None
        };
        let cod2 = fs.game() == vcod_common::game::Game::Cod2;
        let mut table = VoiceTable::new();
        if cod2 {
            table.use_slave_lerp();
        }
        AudioSystem {
            volume_scale: if cod2 {
                COD2_VOLUME_SCALE
            } else {
                VOLUME_SCALE
            },
            manager,
            cs_sounds: cues::CS_SOUNDS,
            menu_aliases: aliases_all.for_map("menu"),
            aliases_all,
            aliases: AliasTable::default(),
            map: None,
            bank: SoundBank::at_rate(opts.rate),
            table,
            handles: HashMap::new(),
            occlusion: occlusion::Occlusion::default(),
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
            projectile_voices: HashMap::new(),
            fire_loops: HashMap::new(),
            blends: HashMap::new(),
            held: HashMap::new(),
            uo: false,
            resumable: HashMap::new(),
            music: None,
            fade: SoundFade::FULL,
            last_step: None,
            warned_loops: HashSet::new(),
            reverb,
            room_wet: 0.0,
            shock_loop: None,
            shock_end: None,
            cod2_ambient: None,
            cod2_ambient_key: None,
            ambient_ramps: HashMap::new(),
            env: cod2::EnvEffects::default(),
            subtitles: Vec::new(),
            #[cfg(test)]
            rows: HashMap::new(),
            #[cfg(test)]
            keep_loops: false,
            chain_secondaries: false,
            entity_offsets: false,
            as_master: false,
        }
    }

    pub fn enabled(&self) -> bool {
        self.manager.is_some()
    }

    /// `mss_volume`, linear 0..1; live voices pick it up on their next step.
    pub fn set_master_volume(&mut self, v: f32) {
        self.master_volume = v.clamp(0.0, 1.0);
    }

    /// `snd_restart`'s Miles shutdown and start-up at `rate`: every voice
    /// stops, the sounds decode again for the new mixer rate and the
    /// ambient starts over; entity loops restart with the next snapshot.
    /// Every other stream (dialog, quick chat, `musicPlay`, an in-game
    /// menu's) reopens at its position and current volume, and a `musicStop`
    /// fade goes on; `soundFade`'s master fade is back at full. With no game
    /// up nothing is saved: the main menu stops every sound and replays its
    /// music (research doc, section 10, "Mixer rate and `snd_restart`").
    pub fn restart(&mut self, fs: &dyn AssetSource, rate: u32) {
        let ambient = self.ambient.as_ref().map(|(alias, _)| alias.clone());
        let ambient_id = self.ambient.as_ref().map(|(_, id)| *id);
        let loops: HashSet<VoiceId> = self
            .loop_voices
            .values()
            .chain(self.projectile_voices.values())
            .flat_map(|v| std::iter::once(v.id).chain(v.secondary.iter().copied()))
            .collect();
        let music = self.music;
        let mut saved: Vec<(VoiceId, Resumable)> = self
            .table
            .ids()
            .into_iter()
            .filter(|id| {
                self.in_game()
                    && Some(*id) != ambient_id
                    && !self.ambient_ramps.contains_key(id)
                    && !loops.contains(id)
                    && self.shock_loop.is_none_or(|(l, _)| l != *id)
            })
            .filter_map(|id| {
                let mut r = self.resumable.get(&id)?.clone();
                match self.handles.get(&id) {
                    Some(h) if h.state() == PlaybackState::Stopped => return None,
                    Some(h) => r.from_s = h.position(),
                    None => {}
                }
                r.volume = self.table.volume(id)?;
                Some((id, r))
            })
            .collect();
        // Retail's save and restore walk the slots in order.
        saved.sort_by_key(|(id, _)| *id);
        self.table.clear();
        self.stop_all();
        self.fade = SoundFade::FULL;
        self.bank = SoundBank::at_rate(rate);
        // CoD2's comes back with the next `set_cod2_ambient`: `stop_all`
        // forgot configstring 3.
        if !self.aliases_all.cod2 {
            self.set_ambient(fs, ambient.as_deref());
        }
        for (old, r) in saved {
            let new = self.start_row(fs, r, 0.0);
            if let Some(m) = music.filter(|m| m.id == old) {
                self.music = new.map(|id| Music { id, ..m });
            }
        }
    }

    /// A game is up: the map set is a level's, not the main menu's.
    fn in_game(&self) -> bool {
        self.map.as_deref().is_some_and(|m| m != "menu")
    }

    /// New map. Refilters the aliases by loadspec and drops everything keyed
    /// to the old table.
    /// Where the connection's sound aliases sit, which United Offensive
    /// moved (docs/research/coduo-diff.md, section 7).
    pub fn set_protocol(&mut self, p: &vcod_common::net::protocol::Protocol) {
        self.cs_sounds = p.cs.sounds();
        self.uo = p.is_uo();
    }

    pub fn on_gamestate(&mut self, map: &str) {
        self.aliases = self.aliases_all.for_map(map);
        self.map = Some(map.to_string());
        self.table.clear();
        self.stop_all();
        self.last_pick.clear();
        self.weapon_sounds.clear();
        log::info!("audio: {} aliases apply on {map}", self.aliases.len());
    }

    /// Stops every voice, the ambient and the entity loops included:
    /// `CG_MapRestart`'s syscall 0xd3 (`S_StopAllSounds`). The caller
    /// restarts the ambient from CS 3; a loop restarts with the next
    /// snapshot that carries it.
    pub fn stop_all(&mut self) {
        // A dropped kira handle keeps playing.
        for (_, mut h) in self.handles.drain() {
            h.stop();
        }
        self.ambient = None;
        self.cod2_ambient = None;
        self.cod2_ambient_key = None;
        self.ambient_ramps.clear();
        self.loop_voices.clear();
        self.projectile_voices.clear();
        self.fire_loops.clear();
        self.blends.clear();
        self.held.clear();
        self.resumable.clear();
        self.music = None;
        self.shock_loop = None;
        self.shock_end = None;
        self.set_room_wet(0.0);
        self.table.set_channel_volumes([1.0; 11]);
        self.stats.voices = 0;
    }

    /// The room's wet level over a dry path held at 1: kira's mix crossfades
    /// `sqrt(1 - m)` dry against `sqrt(m)` wet, so `m = w^2 / (1 + w^2)`
    /// under a gain of `sqrt(1 + w^2)`.
    fn set_room_wet(&mut self, wet: f32) {
        if wet == self.room_wet {
            return;
        }
        self.room_wet = wet;
        if let Some((reverb, gain)) = &mut self.reverb {
            let w2 = wet * wet;
            let tween = Tween {
                duration: Duration::from_millis(30),
                ..Default::default()
            };
            reverb.set_mix(Mix(w2 / (1.0 + w2)), tween);
            gain.set_volume(Decibels(10.0 * (1.0 + w2).log10()), tween);
        }
    }

    /// A frame of the shellshock's sound half (`play::shock::ShockSound`):
    /// the channel mix, the room, `shellshock_loop` blended toward its
    /// silent twin while the blend lasts, and the end or abort cue.
    pub fn set_shellshock(&mut self, fs: &dyn AssetSource, s: &crate::play::shock::ShockSound) {
        self.shock_sound(
            fs,
            s.volumes,
            (&s.room, 1.0, s.wet),
            s.loop_blend.map(|b| ("shellshock_loop", b)),
            s.cue,
            "shellshock_end_abort",
        );
    }

    /// A frame of a CoD4 shellshock's sound half (`play::cod4_shock::Sound`):
    /// the record's own loop, end and abort aliases (the flashbang's
    /// tinnitus), its room's dry and wet levels. The abort is an alias it
    /// plays (`SND_PlaySoundAlias`), in the end's place.
    pub fn set_cod4_shellshock(
        &mut self,
        fs: &dyn AssetSource,
        s: &crate::play::cod4_shock::Sound,
    ) {
        self.shock_sound(
            fs,
            s.volumes,
            (&s.room, s.dry, s.wet),
            s.loop_blend.as_ref().map(|(a, _, b)| (a.as_str(), *b)),
            s.cue.as_ref().map(|(a, ms)| (a.as_str(), *ms)),
            "",
        );
    }

    /// The shellshock mix: `volumes` by channel, the room at `(name, dry,
    /// wet)`, the `loop` alias at `1 - blend` of its volume while `Some`,
    /// and a one-shot `cue` (`abort`, the end's abort, stops the end).
    fn shock_sound(
        &mut self,
        fs: &dyn AssetSource,
        volumes: [f32; 11],
        (room, dry, wet): (&str, f32, f32),
        looping: Option<(&str, f32)>,
        cue: Option<(&str, i32)>,
        abort: &str,
    ) {
        self.table.set_channel_volumes(volumes);
        let wet = if room.eq_ignore_ascii_case("generic") {
            0.0
        } else {
            wet
        };
        if self.aliases_all.cod2 {
            // CoD2's shellshock is the environment's priority 2
            // (`CG_UpdateShellShock`, Mac 0x1d7528, cod2-sound.md 7.12):
            // set each frame it blends, deactivated when it ends; `step`
            // applies the level in force.
            if wet > 0.0 {
                self.env.set(2, room, dry, wet, 0);
            } else {
                self.env.deactivate(2, 0);
            }
        } else {
            self.set_room_wet(wet);
        }
        let shock_cue = |alias: &str, pos: Vec3| Cue {
            alias: alias.to_string(),
            source: Source::Entity {
                num: SHOCK_ENTITY,
                pos,
            },
            delay_s: 0.0,
        };
        match looping {
            Some((alias, blend)) => {
                let live = self
                    .shock_loop
                    .filter(|(id, _)| self.table.looping(*id).is_some());
                let started = live.or_else(|| {
                    let id = self.play(fs, shock_cue(alias, self.listener.pos))?;
                    Some((id, self.table.volume(id)?))
                });
                // The silent twin's volume is 0: the blend scales the row's.
                if let Some((id, base)) = started {
                    self.table.set_volume(id, base * (1.0 - blend));
                }
                self.shock_loop = started;
            }
            None => {
                if let Some((id, _)) = self.shock_loop.take() {
                    self.stop_voice(id);
                }
            }
        }
        match cue {
            // `null.wav` on the end's channel and entity replaces it.
            Some((alias, _)) if alias == abort => {
                if let Some(id) = self.shock_end.take() {
                    self.stop_voice(id);
                }
            }
            Some((alias, ms)) => {
                if let Some(id) = self.shock_end.take() {
                    self.stop_voice(id);
                }
                let cue = shock_cue(alias, self.listener.pos);
                self.shock_end = self.play_from(fs, cue, ms.max(0) as f32 / 1000.0);
            }
            None => {}
        }
    }

    /// A download reopened the search path: reload the alias csvs, drop every
    /// decoded sound, and redo the current map's `on_gamestate` against the
    /// new table.
    pub fn reopen(&mut self, fs: &dyn AssetSource) {
        self.aliases_all = match self.aliases_all.cod2 {
            true => cod2::load(fs),
            false => AliasTable::load(fs),
        };
        self.menu_aliases = self.aliases_all.for_map("menu");
        self.bank = SoundBank::at_rate(self.bank.rate);
        self.missed.clear();
        self.warned_loops.clear();
        match self.map.clone() {
            Some(map) => self.on_gamestate(&map),
            None => self.weapon_sounds.clear(),
        }
    }

    /// A CoD4 session's alias table, built from its zones (`cod4_audio`),
    /// played by 1.7's master scale and CoD2's slave rules
    /// (docs/research/cod4-sound.md 5); the current map's gamestate filters
    /// it again.
    pub fn set_cod4_aliases(&mut self, table: AliasTable) {
        self.aliases_all = table;
        self.volume_scale = COD4_VOLUME_SCALE;
        self.chain_secondaries = true;
        self.entity_offsets = true;
        self.table.use_slave_lerp();
        self.missed.clear();
        match self.map.clone() {
            Some(map) => self.on_gamestate(&map),
            None => self.aliases = self.aliases_all.for_map(""),
        }
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
    pub(crate) fn ensure_weapon(
        &mut self,
        fs: &dyn AssetSource,
        configstrings: &[String],
        index: i32,
    ) {
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
        fs: &dyn AssetSource,
        ev: &GameEvent,
        configstrings: &[String],
        muzzles: &HashMap<u32, (Vec3, Vec3)>,
        shot_fallbacks: &HashMap<u32, Vec3>,
    ) {
        self.ensure_weapon(fs, configstrings, ev.weapon);
        let fired = crate::fx::registry::fire_weapon(ev);
        if fired != ev.weapon {
            self.ensure_weapon(fs, configstrings, fired);
        }
        // `parm` is a CS7 weapon index on the pickup events alone.
        if matches!(ev.event, EV_ITEM_PICKUP | EV_AMMO_PICKUP) {
            self.ensure_weapon(fs, configstrings, ev.parm);
        }
        self.start_fire_loop(ev);
        let cues = {
            let ctx = CueCtx {
                configstrings,
                sounds: self.cs_sounds,
                weapon_sounds: &self.weapon_sounds,
                muzzles,
                shot_fallbacks,
                listener_pos: self.listener.pos,
                ps_entity: self.ps_entity,
                uo: self.uo,
            };
            cues::resolve(ev, &ctx)
        };
        for c in cues {
            self.play(fs, c);
        }
    }

    /// `CG_FireWeapon` arms a body's fire loop for `fireTime` + 51 ms when
    /// its weapon has a `loopFireSound` (uo cgame 0x30047d20); only a player
    /// body off a turret and out of a vehicle runs it (0x30045ca0).
    fn start_fire_loop(&mut self, ev: &GameEvent) {
        if !self.uo
            || !cues::fires_weapon(ev.event)
            || ev.e_type != vcod_common::net::flags::ET_PLAYER
            || ev.eflags & (crate::turret::EF_MOUNTED | vcod_common::net::flags::EF_IN_VEHICLE_V22)
                != 0
        {
            return;
        }
        let weapon = crate::fx::registry::fire_weapon(ev);
        let Some(w) = self.weapon_sounds.get(&weapon).and_then(Option::as_ref) else {
            return;
        };
        if w.loop_fire.is_none() {
            return;
        }
        let ent = match ev.entity_num {
            u32::MAX => self.ps_entity,
            n => n,
        };
        let left = w.fire_time_ms + 51.0;
        let entry = self.fire_loops.entry(ent).or_insert(FireLoop {
            weapon,
            left,
            voice: None,
        });
        entry.weapon = weapon;
        entry.left = left;
    }

    /// Where uo cgame 0x300490b0 plays a fire loop's frame: the body's world
    /// `tag_flash` (syscall 0xa5, 0x3001fdf0), the ridden body's viewmodel
    /// one (0x3001fec0; `muzzles` keys it under the ridden number), else
    /// the body's position (0x30005f30). The voice rides the entity at
    /// that offset as a `Source::Tag`.
    fn fire_loop_source(
        &self,
        ent: u32,
        entity_pos: &HashMap<u32, Vec3>,
        muzzles: &HashMap<u32, (Vec3, Vec3)>,
    ) -> Source {
        if let Some(&(pos, _)) = muzzles.get(&ent) {
            return Source::Tag { num: ent, pos };
        }
        let pos = match ent {
            e if e == self.ps_entity => self.listener.pos,
            e => entity_pos.get(&e).copied().unwrap_or(self.listener.pos),
        };
        Source::Entity { num: ent, pos }
    }

    /// Runs the fire loops `ms` on (`cg.frametime`): each keeps its loop
    /// alias playing on the body while time is left, then plays the stop
    /// alias once and ends. Both start at `fire_loop_source`.
    pub fn step_fire_loops(
        &mut self,
        fs: &dyn AssetSource,
        ms: f32,
        entity_pos: &HashMap<u32, Vec3>,
        muzzles: &HashMap<u32, (Vec3, Vec3)>,
    ) {
        let ents: Vec<u32> = self.fire_loops.keys().copied().collect();
        for ent in ents {
            let Some(l) = self.fire_loops.get_mut(&ent) else {
                continue;
            };
            l.left -= ms;
            let (left, weapon, voice) = (l.left, l.weapon, l.voice);
            let sounds = self.weapon_sounds.get(&weapon).and_then(Option::as_ref);
            let (looped, stop) =
                sounds.map_or((None, None), |w| (w.loop_fire.clone(), w.stop_fire.clone()));
            let source = self.fire_loop_source(ent, entity_pos, muzzles);
            if left <= 0.0 {
                if let Some(id) = voice {
                    self.stop_voice(id);
                }
                self.fire_loops.remove(&ent);
                if let Some(alias) = stop {
                    self.play(
                        fs,
                        Cue {
                            alias,
                            source,
                            delay_s: 0.0,
                        },
                    );
                }
                continue;
            }
            let live = voice.is_some_and(|id| self.table.looping(id).is_some());
            if !live && let Some(alias) = looped {
                let id = self.play(
                    fs,
                    Cue {
                        alias,
                        source,
                        delay_s: 0.0,
                    },
                );
                if let Some(l) = self.fire_loops.get_mut(&ent) {
                    l.voice = id;
                }
            }
        }
    }

    /// The cgame's case 9 for this frame's snapshot pair: every sound blend
    /// entity on a United Offensive connection ([`blend::sources`]) and
    /// every shell in flight ([`blend::shell_sources`], heard from the
    /// listener), then [`Self::set_sound_blends`].
    pub fn step_sound_blends(
        &mut self,
        fs: &dyn AssetSource,
        p: &vcod_common::net::protocol::Protocol,
        pair: (
            &vcod_common::net::snapshot::Snapshot,
            &vcod_common::net::snapshot::Snapshot,
            f32,
        ),
        (configstrings, weapons): (&[String], &[Option<vcod_common::weapon::WeaponDef>]),
        entity_pos: &HashMap<u32, Vec3>,
    ) {
        let mut sources = Vec::new();
        if self.uo {
            sources = blend::sources(p, pair, self.cs_sounds, configstrings, entity_pos);
            sources.extend(blend::shell_sources(
                p,
                pair.0,
                weapons,
                entity_pos,
                self.listener.pos,
            ));
        }
        self.set_sound_blends(fs, &sources);
    }

    /// This frame's sound blend entities (`blend::sources`), syscall 199
    /// (`CoDUOMP.exe` 0x452ba0, docs/research/coduo-diff.md 16). A voice
    /// already playing the pair on the entity is refreshed: the volume to
    /// the blend of both rows' `vol_min`, the pitch to the blend of their
    /// `pitch_min` when both rows fix it and not both at 1 (0x4523c0).
    /// Otherwise the first row starts, culled past the blended `dist_max`,
    /// at a volume and pitch drawn between the blended bounds (0x452870,
    /// 0x4527c0). A blend absent this frame ends, as the end-of-frame
    /// reaper ends a loop no one refreshed.
    pub fn set_sound_blends(&mut self, fs: &dyn AssetSource, sources: &[blend::BlendSource]) {
        let gone: Vec<u32> = self
            .blends
            .keys()
            .filter(|e| !sources.iter().any(|s| s.entity == **e))
            .copied()
            .collect();
        for ent in gone {
            if let Some(v) = self.blends.remove(&ent) {
                self.stop_voice(v.id);
            }
        }
        for s in sources {
            let source = Source::Entity {
                num: s.entity,
                pos: s.pos,
            };
            let rows = s.aliases.clone().map(|alias| {
                self.pick_row(
                    &Cue {
                        alias,
                        source,
                        delay_s: 0.0,
                    },
                    false,
                )
            });
            let [Some(r1), Some(r2)] = rows else {
                continue;
            };
            let b = s.blend;
            let lerp = |x: f32, y: f32| b * y + (1.0 - b) * x;
            let same = |v: &BlendVoice| {
                v.aliases
                    .iter()
                    .zip(&s.aliases)
                    .all(|(x, y)| x.eq_ignore_ascii_case(y))
            };
            if let Some(v) = self.blends.get_mut(&s.entity)
                && same(v)
                && self.table.looping(v.id).is_some()
            {
                // The refresh places the voice too (0x4510d0): the blend
                // entity is never drawn, so `step` has no position for it.
                self.table.set_entity_pos(v.id, s.pos);
                self.table
                    .set_volume(v.id, lerp(r1.vol.0, r2.vol.0) * self.volume_scale);
                let fixed = |r: &AliasRow| r.pitch.0 == r.pitch.1;
                if (r1.pitch.0 != 1.0 || r2.pitch.0 != 1.0) && fixed(&r1) && fixed(&r2) {
                    v.pitch = f64::from(lerp(r1.pitch.0, r2.pitch.0));
                }
                if let Some(h) = self.handles.get_mut(&v.id) {
                    h.set_playback_rate(v.pitch * f64::from(s.pitch_scale));
                }
                continue;
            }
            if let Some(v) = self.blends.remove(&s.entity) {
                self.stop_voice(v.id);
            }
            if !r1.file.eq_ignore_ascii_case(&r2.file) || r1.looping != r2.looping {
                let key = format!("{}+{}", r1.name, r2.name);
                if self.warned_loops.insert(key) {
                    log::warn!(
                        "audio: blend of {:?} and {:?}: not one sound file and loop status",
                        r1.name,
                        r2.name
                    );
                }
            }
            let dist = (lerp(r1.dist.0, r2.dist.0), lerp(r1.dist.1, r2.dist.1));
            if r1.channel.is_spatial() && s.pos.distance(self.listener.pos) > dist.1 {
                continue;
            }
            let volume = self
                .rng
                .range(lerp(r1.vol.0, r2.vol.0), lerp(r1.vol.1, r2.vol.1))
                * self.volume_scale;
            let pitch = f64::from(
                self.rng
                    .range(lerp(r1.pitch.0, r2.pitch.0), lerp(r1.pitch.1, r2.pitch.1)),
            );
            let row = AliasRow { dist, ..r1 };
            let started = self.start_row(
                fs,
                Resumable {
                    row,
                    source,
                    volume,
                    pitch: pitch * f64::from(s.pitch_scale),
                    from_s: 0.0,
                },
                0.0,
            );
            if let Some(id) = started {
                self.blends.insert(
                    s.entity,
                    BlendVoice {
                        aliases: s.aliases.clone(),
                        id,
                        pitch,
                    },
                );
            }
        }
    }

    /// Keeps `alias` going on entity `ent` while called each frame: started
    /// when it is not playing, restarted when its row ran out.
    pub fn hold(&mut self, fs: &dyn AssetSource, ent: u32, alias: &str, pos: Vec3) {
        if self
            .held
            .get(&ent)
            .is_some_and(|&id| self.table.looping(id).is_some())
        {
            return;
        }
        let cue = Cue {
            alias: alias.to_string(),
            source: Source::Entity { num: ent, pos },
            delay_s: 0.0,
        };
        match self.play(fs, cue) {
            Some(id) => {
                self.held.insert(ent, id);
            }
            None => {
                self.held.remove(&ent);
            }
        }
    }

    /// Starts a sound vcod builds itself on the main track (CoD4 voice
    /// chat's talker streams, `cod4_voice`); nothing without a device.
    pub fn play_custom<D: kira::sound::SoundData>(&mut self, d: D) -> Option<D::Handle> {
        self.manager.as_mut()?.play(d).ok()
    }

    /// A playing voice's expected ms left, what `SND_PlaySoundAlias`
    /// returns at the start; `None` for a stream or a voice gone.
    pub fn voice_ms(&self, id: VoiceId) -> Option<i32> {
        self.table.duration_ms(id)
    }

    /// Stops what [`Self::hold`] kept on `ent`.
    pub fn release(&mut self, ent: u32) {
        if let Some(id) = self.held.remove(&ent) {
            self.stop_voice(id);
        }
    }

    /// The listener distance United Offensive's pick weighs `lod` by: -1 for
    /// an emitter at the origin (`CoDUOMP.exe` 0x438300).
    fn lod_distance(&self, source: Source) -> f32 {
        let pos = source.pos();
        if pos == Vec3::ZERO {
            -1.0
        } else {
            pos.distance(self.listener.pos)
        }
    }

    /// `None` for an unknown alias (a miss) and for a `null.wav` row (not a
    /// miss).
    fn pick_row(&mut self, cue: &Cue, menu: bool) -> Option<AliasRow> {
        let roll = self.rng.next_f32();
        let key = cue.alias.to_ascii_lowercase();
        let last = self.last_pick.get(&key).copied();
        let set = if menu {
            &self.menu_aliases
        } else {
            &self.aliases
        };
        // United Offensive's pick weighs only the rows whose `lod` admits
        // the emitter's distance (syscall 0xc4).
        let (pick, index) = match self.uo && !menu {
            true => set.pick_lod(&cue.alias, roll, last, self.lod_distance(cue.source)),
            false => set.pick(&cue.alias, roll, last),
        };
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
    pub fn play(&mut self, fs: &dyn AssetSource, cue: Cue) -> Option<VoiceId> {
        self.play_inner(fs, cue, true, false)
    }

    /// `cue` as `SND_PlaySoundAliasAsMaster` plays it (CoD4's
    /// `EV_SOUND_ALIAS_AS_MASTER`, cod4-sound.md 4): its voices and their
    /// secondaries play as masters whatever their rows say, ducking the
    /// slaves while they last.
    pub fn play_as_master(&mut self, fs: &dyn AssetSource, cue: Cue) -> Option<VoiceId> {
        self.as_master = true;
        let id = self.play(fs, cue);
        self.as_master = false;
        id
    }

    /// `SND_StopSoundsOnEnt` (CoD4's `EV_STOPSOUNDS`, cod4-sound.md 4):
    /// every voice on entity `num`, its loops' with them; a loop the
    /// entity still asks for starts again on the next refresh. Returns how
    /// many it stopped.
    pub fn stop_entity(&mut self, num: u32) -> usize {
        let ids: Vec<VoiceId> = self
            .table
            .ids()
            .into_iter()
            .filter(|&id| self.table.entity(id) == Some(num))
            .collect();
        for &id in &ids {
            self.stop_voice(id);
        }
        ids.len()
    }

    /// A `Sound` emitter inside an effect is a world-point cue.
    pub fn play_fx(&mut self, fs: &dyn AssetSource, sounds: Vec<FxSound>) {
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

    /// A 2D cue started `from_s` seconds in: the shellshock end cue, which
    /// starts as far in as the frame came late (syscall 0xc2's last
    /// argument).
    fn play_from(&mut self, fs: &dyn AssetSource, cue: Cue, from_s: f32) -> Option<VoiceId> {
        crate::frame_digest::cue(&cue);
        let row = self.pick_row(&cue, false)?;
        let volume = self.rng.range(row.vol.0, row.vol.1) * self.volume_scale;
        let pitch = self.rng.range(row.pitch.0, row.pitch.1) as f64;
        self.start_row(
            fs,
            Resumable {
                row,
                source: cue.source,
                volume,
                pitch,
                from_s: f64::from(from_s),
            },
            0.0,
        )
    }

    /// `count_cull` is false on the loop-sound path, which retries every
    /// snapshot and would otherwise tick `culled` per snapshot.
    /// `menu` picks from the menu set.
    fn play_inner(
        &mut self,
        fs: &dyn AssetSource,
        cue: Cue,
        count_cull: bool,
        menu: bool,
    ) -> Option<VoiceId> {
        self.play_pair(fs, cue, count_cull, menu).0
    }

    /// [`Self::play_inner`], with the voices its row's secondary aliases
    /// started beside it, which the loop path stops with the primary.
    fn play_pair(
        &mut self,
        fs: &dyn AssetSource,
        cue: Cue,
        count_cull: bool,
        menu: bool,
    ) -> (Option<VoiceId>, Vec<VoiceId>) {
        crate::frame_digest::cue(&cue);
        let emitter = cue.source.pos();
        // A NaN origin passes every distance test, and a NaN pan poisons
        // kira's mix for good.
        if !emitter.is_finite() {
            log::debug!("audio: {:?} at a non-finite position {emitter}", cue.alias);
            if count_cull {
                self.stats.culled += 1;
            }
            return (None, Vec::new());
        }
        let Some(row) = self.pick_row(&cue, menu) else {
            return (None, Vec::new());
        };
        // CoD2's secondary alias starts first, from the same place; one of
        // its own is refused (`SND_PlaySoundAlias_Internal`, Mac 0x4b7b8,
        // docs/research/cod2-sound.md 3). CoD4's starts its own in turn,
        // down the chain; a chain back to an alias already started ends
        // there (iw3mp.exe 0x5c3b5f, cod4-sound.md 4).
        let mut secondary = Vec::new();
        let mut seen = vec![row.name.to_ascii_lowercase()];
        let mut next = row.secondary.clone();
        while let Some(second) = next.take() {
            let sub = Cue {
                alias: second.clone(),
                ..cue.clone()
            };
            let Some(r) = self.pick_row(&sub, menu) else {
                break;
            };
            if self.chain_secondaries {
                let name = r.name.to_ascii_lowercase();
                if seen.contains(&name) || seen.len() > MAX_SECONDARY_CHAIN {
                    log::debug!(
                        "audio: secondary aliases of {:?} loop at {name:?}",
                        row.name
                    );
                    break;
                }
                seen.push(name);
                next = r.secondary.clone();
            } else if r.secondary.is_some() {
                log::debug!(
                    "audio: secondary alias {:?} of {:?} has its own",
                    r.name,
                    row.name
                );
                break;
            }
            secondary.extend(self.start_picked(fs, r, sub, count_cull));
        }
        // Over a game the cgame's end-of-frame reaper (syscall 0xd2,
        // 0x44e600) ends a looping voice nobody refreshes, so a menu's
        // looping music stops the frame it starts.
        if menu && row.looping && self.in_game() {
            return (None, secondary);
        }
        let subtitle = row.subtitle.clone();
        let Some(started) = self.start_picked(fs, row, cue, count_cull) else {
            return (None, secondary);
        };
        // `CG_PlaySoundAliasByName` and its kin (Mac 0x144474..0x144b3c)
        // print a started row's subtitle for `cg_subtitleMinTime` or the
        // sound's length, whichever is longer.
        if let Some(key) = subtitle.filter(|_| !menu) {
            let ms = self.table.duration_ms(started).unwrap_or(0);
            self.subtitles.push((key, ms.max(SUBTITLE_MIN_MS)));
        }
        (Some(started), secondary)
    }

    /// The subtitles of the CoD2 rows started since the last call, oldest
    /// first: the `subtitle.str` key and the ms it shows
    /// (`CL_SubtitlePrint`, Mac 0x17ab76).
    pub fn take_subtitles(&mut self) -> Vec<(String, i32)> {
        std::mem::take(&mut self.subtitles)
    }

    /// [`Self::play_inner`] past the pick: the volume and pitch drawn, the
    /// start-time cull, then [`Self::start_row`] after the cue's delay and
    /// a CoD2 row's `startdelay`.
    fn start_picked(
        &mut self,
        fs: &dyn AssetSource,
        row: AliasRow,
        cue: Cue,
        count_cull: bool,
    ) -> Option<VoiceId> {
        let emitter = cue.source.pos();
        // Sampled before the device check so the rng path is device
        // independent.
        let volume = self.rng.range(row.vol.0, row.vol.1) * self.volume_scale;
        let pitch = self.rng.range(row.pitch.0, row.pitch.1) as f64;
        let row_delay_s = row.start_delay_ms as f32 / 1000.0;
        // Start-time cull, `FUN_0044d670` @ `CoDMP.exe 0x44d670` (research
        // doc, section 3). There is no later cull.
        if row.channel.is_spatial() && emitter.distance(self.listener.pos) > row.dist.1 {
            if count_cull {
                self.stats.culled += 1;
            }
            return None;
        }
        self.start_row(
            fs,
            Resumable {
                row,
                source: cue.source,
                volume,
                pitch,
                from_s: 0.0,
            },
            cue.delay_s + row_delay_s,
        )
    }

    /// Admits and starts a picked row after the cull, `r.from_s` seconds in.
    /// Every streamed voice gets a `resumable` entry, kept on a silent
    /// system too.
    fn start_row(&mut self, fs: &dyn AssetSource, r: Resumable, delay_s: f32) -> Option<VoiceId> {
        let Resumable {
            ref row,
            source,
            volume,
            pitch,
            from_s,
        } = r;
        let spatial = row.channel.is_spatial();
        let master = self.master();
        // Static files are decoded before admission so a failed load never
        // steals a pool slot; their length feeds the steal tiebreak.
        // Streams have no length before playback (research doc approximation,
        // section 10).
        let static_data = (!row.streamed)
            .then(|| self.bank.get(fs, &row.file))
            .flatten();
        if !row.streamed && static_data.is_none() {
            // The bank already counted and warned about the failure.
            return None;
        }
        let duration_ms = static_data
            .as_ref()
            .map(|d| u32::try_from(d.duration().as_millis()).unwrap_or(ENDLESS_MS))
            .unwrap_or(ENDLESS_MS);
        let (id, replaced, stolen) = match self.table.add(
            NewVoice {
                source,
                channel: row.channel,
                volume,
                dist: row.dist,
                curve: row.curve.clone(),
                master_slave: if self.as_master {
                    alias::MasterSlave::Master
                } else {
                    row.master_slave
                },
                spatial,
                looping: row.looping,
                streamed: row.streamed,
                duration_ms,
                protected: matches!(
                    source,
                    Source::Entity {
                        num: cues::AMBIENT_ENTITY | cues::AMBIENT_ENTITY_2 | cues::MUSIC_ENTITY,
                        ..
                    }
                ),
            },
            |id| {
                self.handles
                    .get(&id)
                    .is_some_and(|h| h.state() == PlaybackState::Stopped)
            },
        ) {
            Err(_) => {
                // Every slot was held by voices this sound may not steal;
                // retail's allocator comes up empty in the same case.
                self.stats.drops += 1;
                return None;
            }
            Ok(Admitted::Started {
                id,
                replaced,
                stolen,
            }) => (id, replaced, stolen),
        };
        self.stats.steals += stolen.len() as u64;
        // Replacement and stealing already removed the victims from the
        // table; only their kira handles remain.
        for r in replaced.iter().chain(stolen.iter()) {
            if let Some(mut h) = self.handles.remove(r) {
                h.stop();
            }
        }
        // Initial gain/pan so the first audio chunk is already placed. The
        // occlusion map is empty here; a voice born behind a wall settles
        // over the next few frames like any other change.
        let (db, pan) = self
            .table
            .update(
                &self.listener,
                &HashMap::new(),
                &HashMap::new(),
                &HashMap::new(),
                master,
            )
            .into_iter()
            .find(|u| u.id == id)
            .map(|u| (amplitude_db(u.gain), u.pan))
            // Unreachable; silence if it ever is.
            .unwrap_or((-60.0, 0.0));
        // `from_secs_f32` panics on a negative or NaN delay, and effect files
        // can carry one.
        let start = match Duration::try_from_secs_f32(delay_s) {
            Ok(d) if !d.is_zero() => StartTime::Delayed(d),
            _ => StartTime::Immediate,
        };

        // No device. The voice stays in the table (ambient and loop
        // bookkeeping are table state) and `step` reaps it next frame.
        // `manager` is borrowed mutably across the field accesses below.
        if row.streamed {
            self.resumable.insert(id, r.clone());
        }
        #[cfg(test)]
        self.rows.insert(id, row.clone());
        let Some(manager) = self.manager.as_mut() else {
            return Some(id);
        };
        let started = if row.streamed {
            // `read_stream` already warned on a missing file.
            match SoundBank::read_stream(fs, &row.file).map(|b| stream::open(b, self.bank.rate)) {
                Some(Ok(decoder)) => Some(start_sound!(
                    manager,
                    StreamingSoundData::from_decoder(decoder),
                    Stream,
                    db,
                    pan,
                    pitch,
                    start,
                    from_s,
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
            // The bank already counted and warned about a failure.
            static_data.map(|data| {
                start_sound!(
                    manager,
                    data,
                    Static,
                    db,
                    pan,
                    pitch,
                    start,
                    from_s,
                    row.looping
                )
            })
        };

        match started {
            Some(Ok(h)) => {
                self.handles.insert(id, h);
                self.stats.plays += 1;
                Some(id)
            }
            Some(Err(e)) => {
                log::debug!("audio: play {}: {e}", row.name);
                self.stats.drops += 1;
                self.table.remove(id);
                self.resumable.remove(&id);
                None
            }
            None => {
                self.table.remove(id);
                self.resumable.remove(&id);
                None
            }
        }
    }

    /// Ends a voice [`Self::play`] started (CoD4's `stopLocalSound`).
    pub fn stop(&mut self, id: VoiceId) {
        self.stop_voice(id);
    }

    /// CoD4's looping rule (cod4-sound.md 4, INFERRED, not yet confirmed
    /// on 1.7): once a frame, a looping voice nothing refreshed
    /// stops. The refreshed ones are the entity loops and their
    /// secondaries, the missiles', the ambient, the music, the held and
    /// blended voices and the shellshock's; a looping row a one-shot path
    /// started (an event's `playSound`, an effect's sound) plays a frame.
    /// Returns how many it stopped.
    pub fn reap_loops(&mut self) -> usize {
        let mut kept: HashSet<VoiceId> = self
            .loop_voices
            .values()
            .chain(self.projectile_voices.values())
            .flat_map(|v| std::iter::once(v.id).chain(v.secondary.iter().copied()))
            .collect();
        kept.extend(self.ambient.as_ref().map(|a| a.1));
        kept.extend(self.music.as_ref().map(|m| m.id));
        kept.extend(self.held.values().copied());
        kept.extend(self.blends.values().map(|b| b.id));
        kept.extend(self.fire_loops.values().filter_map(|f| f.voice));
        kept.extend(self.shock_loop.map(|s| s.0));
        kept.extend(self.shock_end);
        let gone: Vec<VoiceId> = self
            .table
            .ids()
            .into_iter()
            .filter(|&id| self.table.looping(id) == Some(true) && !kept.contains(&id))
            .collect();
        for &id in &gone {
            log::debug!("audio: looping voice {id:?} not refreshed; stopped");
            self.stop_voice(id);
        }
        gone.len()
    }

    /// A loop's voice and its secondary's.
    fn stop_loop_voice(&mut self, v: &LoopVoice) {
        self.stop_voice(v.id);
        for &s in &v.secondary {
            self.stop_voice(s);
        }
    }

    fn stop_voice(&mut self, id: VoiceId) {
        self.table.remove(id);
        self.resumable.remove(&id);
        if let Some(mut h) = self.handles.remove(&id) {
            h.stop();
        }
    }

    /// `SND_UpdateBackgroundVolume` (Mac 0x4a5d2) for each CoD2 ambient
    /// voice over `ms`: the volume moves by its rate and stops on its
    /// target; one falling below a target of 0 ends.
    fn advance_ambient_ramps(&mut self, ms: f32) {
        let ramps: Vec<(VoiceId, (f32, f32))> =
            self.ambient_ramps.iter().map(|(&i, &r)| (i, r)).collect();
        for (id, (target, rate)) in ramps {
            let Some(cur) = self.table.volume(id) else {
                self.ambient_ramps.remove(&id);
                continue;
            };
            let next = cur + rate * ms;
            let v = if rate <= 0.0 {
                if next < target && target == 0.0 {
                    self.stop_voice(id);
                    self.ambient_ramps.remove(&id);
                    continue;
                }
                if next < target { target } else { next }
            } else if target < next {
                target
            } else {
                next
            };
            self.table.set_volume(id, v);
        }
    }

    /// CoD2's `CG_StartAmbient` (Mac 0x143de6) on configstring 3, `key`
    /// its text: run once per new text (and again after a stop), its
    /// alias `n` played as `SND_PlayAmbientAlias` (Mac 0x4c806) does with
    /// `fade_ms` the time left to `t`. The same row again does nothing; a
    /// row of the same file, loop flag, pitch range and channel only
    /// ramps the volume to a new draw; anything else fades the playing
    /// track out over `fade_ms` (stopped at once for 0) and starts the new
    /// one on the other of two slots, ramping up from 0. An alias that
    /// names no row, a blank one too (`ambientStop`'s `t\<ms>`), changes
    /// nothing (cod2-sound.md 7.3).
    pub fn set_cod2_ambient(
        &mut self,
        fs: &dyn AssetSource,
        key: &str,
        alias: Option<&str>,
        fade_ms: i32,
    ) {
        if self.cod2_ambient_key.as_deref() == Some(key) {
            return;
        }
        self.cod2_ambient_key = Some(key.to_string());
        let Some(alias) = alias.map(str::trim).filter(|a| !a.is_empty()) else {
            return;
        };
        let slot = self
            .cod2_ambient
            .as_ref()
            .map_or(cues::AMBIENT_ENTITY_2, |(_, e)| *e);
        let cue = Cue {
            alias: alias.to_string(),
            source: Source::Entity {
                num: slot,
                pos: self.listener.pos,
            },
            delay_s: 0.0,
        };
        let Some(row) = self.pick_row(&cue, false) else {
            return;
        };
        let live = self
            .ambient
            .as_ref()
            .map(|(_, id)| *id)
            .filter(|id| self.table.looping(*id).is_some());
        let fade = fade_ms.max(0);
        if let (Some(id), Some((cur, _))) = (live, self.cod2_ambient.clone()) {
            if cur == row {
                return;
            }
            if cur.file.eq_ignore_ascii_case(&row.file)
                && cur.looping == row.looping
                && cur.pitch == row.pitch
                && cur.channel == row.channel
            {
                let vol = self.rng.range(row.vol.0, row.vol.1) * self.volume_scale;
                let old = self.ambient_ramps.get(&id).map_or(vol, |r| r.0);
                let rate = if fade == 0 {
                    0.0
                } else {
                    (vol - old) / fade as f32
                };
                self.ambient_ramps.insert(id, (vol, rate));
                self.cod2_ambient = Some((row, slot));
                self.ambient = Some((alias.to_string(), id));
                return;
            }
            match self.ambient_ramps.get(&id).copied() {
                _ if fade == 0 => {
                    self.stop_voice(id);
                    self.ambient_ramps.remove(&id);
                }
                Some((target, _)) if target > 0.0 => {
                    self.ambient_ramps.insert(id, (0.0, -target / fade as f32));
                }
                _ => {}
            }
        }
        // `SND_StartBackground` (Mac 0x4c5c8) on the other slot, ending
        // whatever still fades there.
        let next = match slot {
            cues::AMBIENT_ENTITY => cues::AMBIENT_ENTITY_2,
            _ => cues::AMBIENT_ENTITY,
        };
        let stale: Vec<VoiceId> = self
            .ambient_ramps
            .keys()
            .copied()
            .filter(|id| self.table.entity(*id) == Some(next))
            .collect();
        for id in stale {
            self.stop_voice(id);
            self.ambient_ramps.remove(&id);
        }
        let vol = self.rng.range(row.vol.0, row.vol.1) * self.volume_scale;
        let pitch = f64::from(self.rng.range(row.pitch.0, row.pitch.1));
        let (start, rate) = if fade < 1 {
            (vol, 0.0)
        } else {
            (0.0, vol / fade as f32)
        };
        self.ambient = None;
        self.cod2_ambient = None;
        let started = self.start_row(
            fs,
            Resumable {
                row: row.clone(),
                source: Source::Entity {
                    num: next,
                    pos: self.listener.pos,
                },
                volume: start,
                pitch,
                from_s: 0.0,
            },
            0.0,
        );
        if let Some(id) = started {
            log::info!("audio: ambient {alias}");
            self.ambient_ramps.insert(id, (vol, rate));
            self.ambient = Some((alias.to_string(), id));
            self.cod2_ambient = Some((row, next));
        }
    }

    /// CoD2's `r` (`setReverb`, `CG_ServerCommand` Mac 0x1e2ea4):
    /// [`cod2::EnvEffects::set`].
    pub fn set_environment(&mut self, prio: usize, room: &str, dry: f32, wet: f32, fade_ms: i32) {
        self.env.set(prio, room, dry, wet, fade_ms);
    }

    /// CoD2's `D` (`deactivateReverb`): [`cod2::EnvEffects::deactivate`].
    pub fn deactivate_environment(&mut self, prio: usize, fade_ms: i32) {
        self.env.deactivate(prio, fade_ms);
    }

    /// Configstring 3's `n` key, `ambientPlay`'s wire form (research doc,
    /// section 9). The row is `local`, `streamed`, `looping`, so `play` makes
    /// it a 2D loop. A round restart resends CS 3 with the same alias and a
    /// new `t`; an unchanged alias with a live voice is a no-op, a lost voice
    /// is restarted. It sits on [`cues::AMBIENT_ENTITY`] so no cue can replace
    /// it (research doc, "vcod divergences from retail"). The `t` crossfade
    /// deadline is ignored; vcod cuts over.
    pub fn set_ambient(&mut self, fs: &dyn AssetSource, alias: Option<&str>) {
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
        fs: &dyn AssetSource,
        configstrings: &[String],
        loops: &HashMap<u32, (i32, Vec3)>,
    ) {
        // Resolved per snapshot; the `CS_SOUNDS` block fills lazily
        // (research doc, section 9).
        let wanted = loops
            .iter()
            .filter(|(_, (idx, _))| *idx > 0)
            .map(|(&ent, &(idx, pos))| {
                let alias = configstrings
                    .get(self.cs_sounds + idx as usize)
                    .filter(|a| !a.is_empty())
                    .cloned();
                (ent, (idx, alias, pos))
            })
            .collect();
        let mut voices = std::mem::take(&mut self.loop_voices);
        self.reconcile_loops(fs, &mut voices, &wanted, "loopSound");
        self.loop_voices = voices;
    }

    /// This snapshot's missiles as `(CS7 weapon index, origin)`
    /// ([`missile_sounds`]): each plays its weapon's `projectileSound` on
    /// itself, the way `set_loop_sounds` plays `es.loopSound`, and stops
    /// when it leaves the map or the snapshot or turns into its explode
    /// (research doc, section 8, "`projectileSound`").
    pub fn set_projectile_sounds(
        &mut self,
        fs: &dyn AssetSource,
        configstrings: &[String],
        missiles: &HashMap<u32, (i32, Vec3)>,
    ) {
        for &(weapon, _) in missiles.values() {
            self.ensure_weapon(fs, configstrings, weapon);
        }
        let wanted = missiles
            .iter()
            .map(|(&ent, &(weapon, pos))| {
                let alias = self
                    .weapon_sounds
                    .get(&weapon)
                    .and_then(Option::as_ref)
                    .and_then(|s| s.projectile.clone());
                (ent, (weapon, alias, pos))
            })
            .collect();
        let mut voices = std::mem::take(&mut self.projectile_voices);
        self.reconcile_loops(fs, &mut voices, &wanted, "projectileSound");
        self.projectile_voices = voices;
    }

    /// [`Self::set_loop_sounds`] with the aliases resolved by the caller,
    /// per entity `(key, alias, origin)`: a CoD2 session's
    /// (`play::cod2_sound::Cod2Sounds::loops`).
    pub fn set_loop_aliases(
        &mut self,
        fs: &dyn AssetSource,
        wanted: &HashMap<u32, (i32, Option<String>, Vec3)>,
    ) {
        let mut voices = std::mem::take(&mut self.loop_voices);
        self.reconcile_loops(fs, &mut voices, wanted, "loopSound");
        self.loop_voices = voices;
    }

    /// [`Self::set_projectile_sounds`] with the aliases resolved by the
    /// caller, keyed by weapon.
    pub fn set_projectile_aliases(
        &mut self,
        fs: &dyn AssetSource,
        wanted: &HashMap<u32, (i32, Option<String>, Vec3)>,
    ) {
        let mut voices = std::mem::take(&mut self.projectile_voices);
        self.reconcile_loops(fs, &mut voices, wanted, "projectileSound");
        self.projectile_voices = voices;
    }

    /// Brings `voices` to `wanted`, per entity `(key, alias, origin)`: a
    /// voice whose entity is gone or whose key changed stops, and a wanted
    /// alias with no live voice starts. The cgame plays these every frame
    /// and the engine's looping dedupe keeps one voice (research doc,
    /// section 2), so only a looping row is restarted once it ends.
    fn reconcile_loops(
        &mut self,
        fs: &dyn AssetSource,
        voices: &mut HashMap<u32, LoopVoice>,
        wanted: &HashMap<u32, (i32, Option<String>, Vec3)>,
        what: &str,
    ) {
        crate::frame_digest::loops(what, wanted);
        voices.retain(|ent, v| {
            let keep = matches!(wanted.get(ent), Some((key, _, _)) if *key == v.idx);
            if !keep {
                log::debug!("audio: {what} off entity {ent}");
                self.stop_loop_voice(v);
            }
            keep
        });

        for (&ent, (idx, alias, pos)) in wanted {
            let (idx, pos) = (*idx, *pos);
            let Some(alias) = alias else {
                continue;
            };
            if let Some(&LoopVoice { id, looping, .. }) = voices.get(&ent) {
                // Still live, or a non-looping row already played once.
                if !looping || self.table.looping(id).is_some() {
                    continue;
                }
                if let Some(v) = voices.remove(&ent) {
                    self.stop_loop_voice(&v);
                }
            }
            // Dropped here rather than counted as a miss every snapshot.
            if self.aliases.get(alias).is_none() {
                if self.warned_loops.insert(alias.to_ascii_lowercase()) {
                    log::warn!("audio: {what} alias {alias:?} is not in the alias table");
                }
                continue;
            }
            let source = if self.entity_offsets {
                Source::Tag { num: ent, pos }
            } else {
                Source::Entity { num: ent, pos }
            };
            let cue = Cue {
                alias: alias.clone(),
                source,
                delay_s: 0.0,
            };
            // Silent pick, cull or decode failure; the next snapshot retries.
            // When a voice starts rides its last one's end and the picks'
            // draws, so the digest takes `wanted` instead.
            let (started, secondary) =
                crate::frame_digest::clocked(|| self.play_pair(fs, cue, false, false));
            let Some(id) = started else {
                // Nothing would ever stop it: the next snapshot retries
                // both.
                for s in secondary {
                    self.stop_voice(s);
                }
                continue;
            };
            log::debug!("audio: {what} {alias:?} on entity {ent} at {pos}");
            let looping = self.table.looping(id).unwrap_or(false);
            if !looping && self.warned_loops.insert(alias.to_ascii_lowercase()) {
                log::warn!("audio: {what} alias {alias:?} is not a looping row; playing it once");
            }
            voices.insert(
                ent,
                LoopVoice {
                    idx,
                    id,
                    looping,
                    secondary,
                },
            );
        }
    }

    /// `alias` on the viewer, the way `CG_LocalSound` and the chat and game
    /// message beeps play.
    /// Whether the menu set has `alias` (UI syscall 0x26).
    pub fn has_menu_alias(&self, alias: &str) -> bool {
        self.menu_aliases.get(alias).is_some()
    }

    /// A menu's sound: `alias` from the menu set on the listener, as UI
    /// syscall 0x27 (CoDMP.exe 0x44d9c0) plays it.
    pub fn play_menu(&mut self, fs: &dyn AssetSource, alias: &str) -> Option<VoiceId> {
        let cue = Cue {
            alias: alias.to_string(),
            source: Source::Entity {
                num: self.ps_entity,
                pos: self.listener.pos,
            },
            delay_s: 0.0,
        };
        self.play_inner(fs, cue, true, true)
    }

    pub fn play_local(&mut self, fs: &dyn AssetSource, alias: &str) -> Option<VoiceId> {
        let cue = Cue {
            alias: alias.to_string(),
            source: Source::Entity {
                num: self.ps_entity,
                pos: self.listener.pos,
            },
            delay_s: 0.0,
        };
        self.play(fs, cue)
    }

    /// A server command the net layer did not consume; returns whether it was
    /// a sound command. `s <idx>` is `playLocalSound` (research doc, section
    /// 9). The alias is resolved at receive time since the `CS_SOUNDS` slot
    /// is often created by the `d` update just before it.
    pub fn on_server_command(
        &mut self,
        fs: &dyn AssetSource,
        tokens: &[String],
        configstrings: &[String],
    ) -> bool {
        let Some(cmd) = tokens.first() else {
            return false;
        };
        match cmd.as_str() {
            // United Offensive's `playLocalAnnouncerSound` sends `S`, read
            // the same way (coduo-diff.md, section 8).
            "s" | "S" => {
                let idx = tokens
                    .get(1)
                    .and_then(|t| t.parse::<usize>().ok())
                    .filter(|i| (1..=255).contains(i));
                let Some(idx) = idx else {
                    log::debug!("audio: malformed playLocalSound command {tokens:?}");
                    return true;
                };
                match configstrings.get(self.cs_sounds + idx) {
                    Some(a) if !a.is_empty() => {
                        log::debug!("audio: playLocalSound {idx} = {a:?}");
                        self.play_local(fs, a);
                    }
                    _ => log::debug!("audio: playLocalSound {idx}: no alias in that configstring"),
                }
                true
            }
            "o" => {
                if let Some(alias) = tokens.get(1) {
                    self.music_play(fs, alias);
                }
                true
            }
            "p" => {
                let fade_ms = tokens.get(1).and_then(|t| t.parse::<i64>().ok());
                self.music_stop(fade_ms.unwrap_or(0));
                true
            }
            "q" => {
                // `atof` and `atol`: an unparsable word reads as 0.
                let target = tokens.get(1).and_then(|t| t.parse::<f32>().ok());
                let ms = tokens.get(2).and_then(|t| t.parse::<i32>().ok());
                self.sound_fade(target.unwrap_or(0.0), ms.unwrap_or(0));
                true
            }
            // Quick chat (`j`/`k`/`l`) is handled by `quick_chat::QuickChat`
            // before commands reach here.
            _ => false,
        }
    }

    /// `musicPlay`'s `o <alias>` (cgame 0x3002e0d0 case `o`, syscall 0xd4 =
    /// CoDMP.exe 0x44dc40): starts the track only while the music slot is
    /// idle, so a second `o` during a track is dropped (research doc,
    /// section 10, "`musicPlay` and `musicStop`").
    pub fn music_play(&mut self, fs: &dyn AssetSource, alias: &str) {
        if self
            .music
            .is_some_and(|m| self.table.looping(m.id).is_some())
        {
            return;
        }
        self.music = None;
        if let Some(row) = self.aliases.get(alias).and_then(|rows| rows.first()) {
            if !row.streamed {
                log::warn!(
                    "audio: alias {} sound {} played as an ambient / music track is not streamed",
                    row.name,
                    row.file
                );
            }
            if row.channel.is_spatial() {
                log::warn!(
                    "audio: alias {} sound {} played as an ambient / music track uses a 3D channel",
                    row.name,
                    row.file
                );
            }
        }
        let cue = Cue {
            alias: alias.to_string(),
            source: Source::Entity {
                num: cues::MUSIC_ENTITY,
                pos: self.listener.pos,
            },
            delay_s: 0.0,
        };
        self.music = self.play(fs, cue).map(|id| Music { id, fade: None });
    }

    /// `musicStop`'s `p <fade ms>` (syscall 0xd5 = CoDMP.exe 0x44db90): 0
    /// closes the track at once, more ramps its volume to 0 over that many
    /// ms, and a second `p` with a fade leaves a fade in progress alone. A
    /// negative fade, which the server refuses to send, stops at once.
    pub fn music_stop(&mut self, fade_ms: i64) {
        let Some(m) = self.music else {
            return;
        };
        if fade_ms <= 0 {
            self.stop_voice(m.id);
            self.music = None;
            return;
        }
        if m.fade.is_some() {
            return;
        }
        let volume = self.table.volume(m.id).unwrap_or(0.0);
        self.music = Some(Music {
            fade: Some(-volume / fade_ms as f32),
            ..m
        });
    }

    /// `soundFade`'s `q <volume> <ms>` (cgame 0x30031fc0, syscall 0xd7 =
    /// CoDMP.exe 0x44e0f0): the master fade heads for `target` over `ms`.
    /// A time of 0 lands on the next step, and a target of 0 then stops
    /// every sound at once (research doc, section 10, "`soundFade`").
    pub fn sound_fade(&mut self, target: f32, ms: i32) {
        let delta = target - self.fade.level;
        self.fade.target = target;
        if ms != 0 {
            self.fade.rate = delta / ms as f32;
            return;
        }
        self.fade.rate = delta;
        if target == 0.0 {
            self.stop_everything();
        }
    }

    /// `S_StopAllSounds(0)` (0x44fa40): every voice ends, the ambient and
    /// the music with it.
    fn stop_everything(&mut self) {
        self.table.clear();
        self.stop_all();
    }

    /// Linear master gain: `mss_volume` times the `soundFade` level, as
    /// retail's 0x8e078c is `s_volume` times 0x8e0790 (0x44ece8).
    fn master(&self) -> f32 {
        self.master_volume * self.fade.level
    }

    /// Advances the master fade and a `musicStop` fade by `ms` of wall
    /// time, as `S_Update` does before it sets any voice's volume
    /// (0x44e890 for the master, 0x44e7a0 for the music slot). The master
    /// landing on 0 stops every sound; the music reaching 0 closes it.
    fn advance_fades(&mut self, ms: f32) {
        self.advance_ambient_ramps(ms);
        let f = &mut self.fade;
        if f.rate != 0.0 {
            f.level += f.rate * ms;
            let past = if f.rate >= 0.0 {
                f.level > f.target
            } else {
                f.level < f.target
            };
            if past {
                f.level = f.target;
                f.rate = 0.0;
            }
            if f.level == 0.0 && f.rate == 0.0 {
                self.stop_everything();
            }
        }
        if let Some(Music {
            id,
            fade: Some(rate),
        }) = self.music
        {
            let volume = self.table.volume(id).unwrap_or(0.0) + rate * ms;
            if volume < 0.0 {
                self.stop_voice(id);
                self.music = None;
            } else {
                self.table.set_volume(id, volume);
            }
        }
    }

    /// Once per frame, after the event drain. `entity_angles` are the drawn
    /// entities' `lerpAngles`, the frame a tag voice turns with. `world` is
    /// the collision world in game mode and `None` where there is none (fly
    /// mode), which disables occlusion.
    pub fn step(
        &mut self,
        entity_pos: &HashMap<u32, Vec3>,
        entity_angles: &HashMap<u32, Vec3>,
        world: Option<&CollisionWorld>,
    ) {
        let t0 = Instant::now();
        let ms = self
            .last_step
            .map_or(0.0, |t| t0.duration_since(t).as_secs_f32() * 1000.0);
        self.last_step = Some(t0);
        self.advance_fades(ms);
        self.table.advance_slave_lerp(ms, SLAVE_FADE_MS);
        if self.aliases_all.cod2 {
            self.env.advance(ms);
            self.set_room_wet(self.env.wet());
        }
        let master = self.master();
        #[cfg(test)]
        let kept: HashSet<VoiceId> = if self.keep_loops {
            let t = &self.table;
            t.ids()
                .into_iter()
                .filter(|&id| t.looping(id) == Some(true))
                .collect()
        } else {
            HashSet::new()
        };
        #[cfg(not(test))]
        let kept = HashSet::<VoiceId>::new();
        let handles = &mut self.handles;
        self.table.retain(|id| match handles.get(&id) {
            Some(h) => h.state() != PlaybackState::Stopped,
            None => kept.contains(&id),
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
        self.resumable.retain(|id, _| live.contains(id));
        // A test binary's row log follows the live voices too, or a soak
        // run under it grows by every row it ever played.
        #[cfg(test)]
        self.rows.retain(|id, _| live.contains(id));

        if !self.table.is_empty() {
            // The ridden entity has no drawn body, so `entity_pos` never
            // carries it.
            let mut positions = entity_pos.clone();
            positions.insert(self.ps_entity, self.listener.pos);
            // A 3D music row stays on the listener, as a 3D ambient would.
            positions.insert(cues::MUSIC_ENTITY, self.listener.pos);
            // cgame `vmMain` 13: the centity's origin and `AnglesToAxis` of
            // its `lerpAngles`; the ridden body's are the view's.
            let mut axes: HashMap<u32, Mat3> = entity_angles
                .iter()
                .map(|(&n, a)| {
                    let [f, l, u] = vcod_common::pmove::aim::angles_to_axis_msvc(a.to_array());
                    (n, Mat3::from_cols(f.into(), l.into(), u.into()))
                })
                .collect();
            let l = &self.listener;
            axes.insert(self.ps_entity, Mat3::from_cols(l.forward, -l.right, l.up));
            let occluded = match world {
                Some(w) => {
                    self.occlusion
                        .compute(&self.table.spatial_positions(), self.listener.pos, w)
                }
                None => {
                    self.occlusion.reset();
                    HashMap::new()
                }
            };
            for u in self
                .table
                .update(&self.listener, &positions, &axes, &occluded, master)
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

    /// A silent system keeps its looping voices until something stops
    /// them, where it would drop every voice at the next `step`: what a
    /// device would still play.
    #[cfg(test)]
    pub(crate) fn keep_silent_loops(&mut self) {
        self.keep_loops = true;
    }

    /// Where a live voice stands after the last [`Self::step`].
    #[cfg(test)]
    pub(crate) fn voice_source(&self, id: voices::VoiceId) -> Option<Source> {
        self.table.source(id)
    }

    /// What the mixer would play now: each live voice's row and the gain
    /// `step` would hand kira, at the listener as it stands. A silent
    /// system's voices last until its next `step`.
    #[cfg(test)]
    pub(crate) fn mix(&mut self) -> Vec<(AliasRow, f32)> {
        let master = self.master();
        let gains = self.table.update(
            &self.listener,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            master,
        );
        gains
            .into_iter()
            .filter_map(|u| Some((self.rows.get(&u.id)?.clone(), u.gain)))
            .collect()
    }

    /// The peak of a row's decoded frames as the mixer gets them: the
    /// bank's for a loaded sound, the stream decoder's first chunks for a
    /// streamed one.
    #[cfg(test)]
    pub(crate) fn decoded_peak(&mut self, fs: &dyn AssetSource, row: &AliasRow) -> Option<f32> {
        use kira::sound::streaming::Decoder;
        let peak = |f: &[kira::Frame]| {
            f.iter()
                .map(|f| f.left.abs().max(f.right.abs()))
                .fold(0.0, f32::max)
        };
        if row.streamed {
            let bytes = SoundBank::read_stream(fs, &row.file)?;
            let mut d = stream::open(bytes, self.bank.rate).ok()?;
            let mut p = 0.0f32;
            for _ in 0..64 {
                p = p.max(peak(&d.decode().ok()?));
            }
            return Some(p);
        }
        self.bank.get(fs, &row.file).map(|d| peak(&d.frames))
    }

    pub fn stats(&self) -> AudioStats {
        AudioStats {
            decode_failures: self.bank.failures + self.stream_failures,
            ..self.stats
        }
    }
}

/// The cvar the frame's master volume reads: CoD2's `snd_volume` where
/// the shell registered it (its options slider), else CoD1's
/// `mss_volume`. Keyed on the shell, not the search path: CoD4's `.iwd`
/// path reads as CoD2's while its shell has no `snd_volume`, and an
/// unregistered cvar reads 0, a silent master.
pub fn volume_cvar(shell: &crate::console::shell::Shell) -> &'static str {
    match shell.cvar("snd_volume") {
        Some(_) => "snd_volume",
        None => "mss_volume",
    }
}

/// Every missile `snap` draws, as `(CS7 weapon index, origin)`, for
/// [`AudioSystem::set_projectile_sounds`]. The cgame's `ET_MISSILE` case
/// (1.1 cgame 0x3001b3f0, 1.5's 0x3001c670, UO's 0x3001edb0) skips one
/// with `EF_NODRAW`; the explode rides the missile's own entity as
/// `eType` 0, so the frame it blows stops the sound.
pub fn missile_sounds(p: &Protocol, snap: &Snapshot) -> HashMap<u32, (i32, Vec3)> {
    snap.entities
        .iter()
        .filter(|(_, e)| {
            e.field_i32(p, "eType") == ET_MISSILE && e.field_i32(p, "eFlags") & EF_NODRAW == 0
        })
        .map(|(&num, e)| (num, (e.field_i32(p, "weapon"), Vec3::from(e.origin(p)))))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::pk3::Pk3Fs;

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
                rate: 0,
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
        a.step(&HashMap::new(), &HashMap::new(), None);
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
                rate: 0,
            },
        );
        assert!(a.aliases.is_empty());
        a.on_gamestate("mp_carentan");
        assert!(a.aliases.len() > 1000, "aliases: {}", a.aliases.len());
    }

    #[test]
    fn menu_sounds_play_from_the_menu_set_over_a_game() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let opts = AudioOpts {
            enabled: false,
            volume: 1.0,
            rate: 0,
        };
        let mut a = AudioSystem::new(&fs, opts);
        a.on_gamestate("mp_carentan");
        // The `menu` loadspec rows are not the map's.
        assert!(a.aliases.get("mouse_over").is_none());
        assert!(a.has_menu_alias("mouse_over"));
        assert!(a.play_menu(&fs, "mouse_over").is_some());
        assert!(a.play_menu(&fs, "mouse_click").is_some());
        assert_eq!(a.stats().misses, 0);
        // The looping menu music is reaped over a game, not in the front end.
        assert!(a.play_menu(&fs, "music_mainmenu").is_none());
        a.on_gamestate("menu");
        assert!(a.play_menu(&fs, "music_mainmenu").is_some());
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
                rate: 0,
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
                rate: 0,
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
            scale: 0,
            other_entity_num: u32::MAX,
            attacker_entity_num: -1,
            e_type: 0,
            eflags: 0,
        };
        a.on_game_event(&fs, &ev, &cs, &HashMap::new(), &HashMap::new());
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
                rate: 0,
            },
        );
        a.on_gamestate("mp_carentan");
        let mut cs = vec![String::new(); 800];
        cs[cues::CS_SOUNDS + 3] = "bomb_tick".to_string();
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
        a.step(&HashMap::new(), &HashMap::new(), None);
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

    /// A loop whose row names a looping secondary (CoD4's
    /// `cobra_helicopter_dying_loop` and its `_layer`): the secondary
    /// starts beside it and stops with it, and one whose primary is out of
    /// range starts no secondary that nothing would stop. Both once rang
    /// on forever after the helicopter was gone.
    #[test]
    fn a_loops_secondary_ends_with_it() {
        let mut fs = Pk3Fs::empty();
        fs.overlay("sound/a.wav", silent_wav(64));
        let csv = "name,sequence,file,vol_min,vol_max,pitch_min,pitch_max,dist_min,dist_max,lod_min,lod_max,channel,type,probability,loop,masterslave,loadspec,subtitle\n\
              veh_loop,,a.wav,1,,1,,10,500,,,auto,,,looping\n\
              veh_loop_layer,,a.wav,1,,1,,10,5000,,,auto,,,looping\n";
        let mut a = AudioSystem::new(
            &fs,
            AudioOpts {
                enabled: false,
                volume: 1.0,
                rate: 0,
            },
        );
        a.aliases_all = AliasTable::parse_csv(csv, "loops.csv");
        for r in a.aliases_all.rows.get_mut("veh_loop").unwrap() {
            r.secondary = Some("veh_loop_layer".into());
        }
        a.on_gamestate("mp_crash");
        let wanted = |x: f32| {
            HashMap::from([(
                5u32,
                (3, Some("veh_loop".to_string()), Vec3::new(x, 0.0, 0.0)),
            )])
        };
        a.set_loop_aliases(&fs, &wanted(100.0));
        assert_eq!(a.table.len(), 2, "the loop and its secondary");
        a.set_loop_aliases(&fs, &wanted(100.0));
        assert_eq!(a.table.len(), 2, "refreshed, not started again");
        a.set_loop_aliases(&fs, &HashMap::new());
        assert_eq!(a.table.len(), 0, "the entity left: both stop");
        // Past the primary's 500, inside the secondary's 5000: each
        // snapshot's retry leaves nothing behind.
        for _ in 0..3 {
            a.set_loop_aliases(&fs, &wanted(1000.0));
        }
        assert_eq!(a.table.len(), 0);
        assert!(a.loop_voices.is_empty());
    }

    /// CoD4's jets' chain (`veh_mig29_dist_loop` to `mid_loop` to
    /// `mid_loop2` to `close_loop` to `close_loop2`, cod4-sound.md 4):
    /// every link starts beside the loop and stops with it; one naming an
    /// alias already started ends the chain there. CoD2's rule starts
    /// only a secondary with none of its own.
    #[test]
    fn a_cod4_loop_starts_its_whole_secondary_chain() {
        let mut fs = Pk3Fs::empty();
        fs.overlay("sound/a.wav", silent_wav(64));
        let csv = "name,sequence,file,vol_min,vol_max,pitch_min,pitch_max,dist_min,dist_max,lod_min,lod_max,channel,type,probability,loop,masterslave,loadspec,subtitle\n\
              jet_dist,,a.wav,1,,1,,10,5000,,,auto,,,looping\n\
              jet_mid,,a.wav,1,,1,,10,5000,,,auto,,,looping\n\
              jet_mid2,,a.wav,1,,1,,10,5000,,,auto,,,looping\n\
              jet_close,,a.wav,1,,1,,10,5000,,,auto,,,looping\n";
        let chain = |a: &mut AudioSystem, close_to: &str| {
            for (from, to) in [
                ("jet_dist", "jet_mid"),
                ("jet_mid", "jet_mid2"),
                ("jet_mid2", "jet_close"),
                ("jet_close", close_to),
            ] {
                for r in a.aliases_all.rows.get_mut(from).unwrap() {
                    r.secondary = (!to.is_empty()).then(|| to.into());
                }
            }
        };
        let wanted = HashMap::from([(
            5u32,
            (3, Some("jet_dist".to_string()), Vec3::new(100.0, 0.0, 0.0)),
        )]);
        let mut a = AudioSystem::new(
            &fs,
            AudioOpts {
                enabled: false,
                volume: 1.0,
                rate: 0,
            },
        );
        a.aliases_all = AliasTable::parse_csv(csv, "loops.csv");
        chain(&mut a, "");
        a.on_gamestate("mp_crash");
        a.set_loop_aliases(&fs, &wanted);
        assert_eq!(
            a.table.len(),
            1,
            "CoD2's rule: jet_mid has its own, refused"
        );
        a.set_loop_aliases(&fs, &HashMap::new());
        a.chain_secondaries = true;
        a.set_loop_aliases(&fs, &wanted);
        assert_eq!(a.table.len(), 4, "the loop and its three links");
        assert_eq!(a.loop_voices[&5].secondary.len(), 3);
        a.set_loop_aliases(&fs, &HashMap::new());
        assert_eq!(a.table.len(), 0, "the entity left: all stop");
        // The last link back to the second: the chain ends there.
        chain(&mut a, "jet_mid");
        a.on_gamestate("mp_crash");
        a.set_loop_aliases(&fs, &wanted);
        assert_eq!(a.table.len(), 4);
    }

    /// CoD4's reaper: a looping row a one-shot path started stops at the
    /// frame's end, an entity loop and its secondary play on.
    #[test]
    fn cod4_reaps_the_loops_nothing_refreshed() {
        let mut fs = Pk3Fs::empty();
        fs.overlay("sound/a.wav", silent_wav(64));
        let csv = "name,sequence,file,vol_min,vol_max,pitch_min,pitch_max,dist_min,dist_max,lod_min,lod_max,channel,type,probability,loop,masterslave,loadspec,subtitle\n\
              veh_loop,,a.wav,1,,1,,10,5000,,,auto,,,looping\n\
              veh_loop_layer,,a.wav,1,,1,,10,5000,,,auto,,,looping\n\
              shot,,a.wav,1,,1,,10,5000,,,auto,,,\n";
        let mut a = AudioSystem::new(
            &fs,
            AudioOpts {
                enabled: false,
                volume: 1.0,
                rate: 0,
            },
        );
        a.aliases_all = AliasTable::parse_csv(csv, "loops.csv");
        for r in a.aliases_all.rows.get_mut("veh_loop").unwrap() {
            r.secondary = Some("veh_loop_layer".into());
        }
        a.on_gamestate("mp_crash");
        let cue = |alias: &str| Cue {
            alias: alias.into(),
            source: Source::Point(Vec3::X * 100.0),
            delay_s: 0.0,
        };
        let stray = a.play(&fs, cue("veh_loop")).unwrap();
        let shot = a.play(&fs, cue("shot")).unwrap();
        a.set_loop_aliases(
            &fs,
            &HashMap::from([(5u32, (3, Some("veh_loop".to_string()), Vec3::X * 100.0))]),
        );
        // The stray loop and its secondary go; the shot plays out.
        assert_eq!(a.reap_loops(), 2);
        assert!(a.table.looping(stray).is_none());
        assert!(a.table.looping(shot).is_some());
        assert_eq!(a.table.len(), 3, "the shot, the entity loop, its secondary");
        assert_eq!(a.reap_loops(), 0);
    }

    /// CoD4's `EV_STOPSOUNDS` stops every voice on its entity, the loop's
    /// with them, and the next refresh starts the loop again;
    /// `EV_SOUND_ALIAS_AS_MASTER` plays a plain row as a master; a loop
    /// starts at its entity offset and keeps it as the entity moves.
    #[test]
    fn cod4_stops_an_entity_plays_a_master_and_offsets_a_loop() {
        let mut fs = Pk3Fs::empty();
        fs.overlay("sound/a.wav", silent_wav(64));
        let csv = "name,sequence,file,vol_min,vol_max,pitch_min,pitch_max,dist_min,dist_max,lod_min,lod_max,channel,type,probability,loop,masterslave,loadspec,subtitle\n\
            veh_loop,,a.wav,1,,1,,10,5000,,,auto,,,looping\n\
            shot,,a.wav,1,,1,,10,5000,,,auto,,,\n";
        let mut a = AudioSystem::new(
            &fs,
            AudioOpts {
                enabled: false,
                volume: 1.0,
                rate: 0,
            },
        );
        a.aliases_all = AliasTable::parse_csv(csv, "loops.csv");
        a.entity_offsets = true;
        a.on_gamestate("mp_crash");
        let on = |num: u32| Cue {
            alias: "shot".into(),
            source: Source::Entity {
                num,
                pos: Vec3::X * 100.0,
            },
            delay_s: 0.0,
        };
        // A brush model's loop 32 above its origin at 0.
        let wanted = HashMap::from([(5u32, (3, Some("veh_loop".to_string()), Vec3::Z * 32.0))]);
        a.keep_silent_loops();
        a.set_loop_aliases(&fs, &wanted);
        a.step(&HashMap::from([(5, Vec3::ZERO)]), &HashMap::new(), None);
        a.step(&HashMap::from([(5, Vec3::X * 10.0)]), &HashMap::new(), None);
        let shot = a.play(&fs, on(5)).unwrap();
        let other = a.play(&fs, on(6)).unwrap();
        let lp = a.loop_voices[&5].id;
        assert_eq!(
            a.table.source(lp),
            Some(Source::Tag {
                num: 5,
                pos: Vec3::new(10.0, 0.0, 32.0)
            })
        );
        assert_eq!(a.stop_entity(5), 2, "the loop and the shot");
        assert!(a.table.looping(shot).is_none() && a.table.looping(lp).is_none());
        assert!(a.table.looping(other).is_some());
        a.set_loop_aliases(&fs, &wanted);
        assert_ne!(a.loop_voices[&5].id, lp, "the loop starts again");
        let m = a.play_as_master(&fs, on(6)).unwrap();
        assert_eq!(a.table.master_slave(m), Some(alias::MasterSlave::Master));
        let plain = a.play(&fs, on(7)).unwrap();
        assert_eq!(a.table.master_slave(plain), Some(alias::MasterSlave::None));
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
                rate: 0,
            },
        );
        a.on_gamestate("mp_carentan");
        let mut cs = vec![String::new(); 800];
        // `MP_bomb_plant` is `all_mp`, one-shot, channel `auto`.
        cs[cues::CS_SOUNDS + 4] = "MP_bomb_plant".to_string();
        let mut loops = HashMap::new();
        loops.insert(13u32, (4, Vec3::ZERO));
        a.set_loop_sounds(&fs, &cs, &loops);
        let id = a.loop_voices[&13].id;
        assert!(!a.loop_voices[&13].looping);
        a.step(&HashMap::new(), &HashMap::new(), None);
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
                rate: 0,
            },
        );
        a.on_gamestate("mp_carentan");
        let mut cs = vec![String::new(); 800];
        cs[cues::CS_SOUNDS + 3] = "bomb_tick".to_string();
        cs[cues::CS_SOUNDS + 4] = "ambient_mp_carentan".to_string();
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

    /// `snd_restart` reopens the ambient from the top on a new voice and
    /// takes later decodes and streams down to the new mixer rate (research
    /// doc, section 10, "Mixer rate and `snd_restart`").
    #[test]
    fn snd_restart_restarts_the_ambient_at_the_new_rate() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut a = AudioSystem::new(
            &fs,
            AudioOpts {
                enabled: false,
                volume: 1.0,
                rate: 44100,
            },
        );
        a.on_gamestate("mp_carentan");
        a.set_ambient(&fs, Some("ambient_mp_carentan"));
        let id = a.ambient.as_ref().unwrap().1;
        a.restart(&fs, 11025);
        let (alias, new_id) = a.ambient.clone().unwrap();
        assert_eq!(alias, "ambient_mp_carentan");
        assert_ne!(new_id, id);
        assert_eq!(a.bank.rate, 11025);
    }

    fn silent(fs: &dyn AssetSource, rate: u32) -> AudioSystem {
        let mut a = AudioSystem::new(
            fs,
            AudioOpts {
                enabled: false,
                volume: 1.0,
                rate,
            },
        );
        a.on_gamestate("mp_carentan");
        a
    }

    fn cmd(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    /// Retail's `snd_restart` saves every stream but the ambient slots and
    /// reopens each at its position (research doc, section 10).
    #[test]
    fn snd_restart_resumes_dialog_and_restarts_the_ambient_from_the_top() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut a = silent(&fs, 44100);
        a.set_ambient(&fs, Some("ambient_mp_carentan"));
        let ambient = a.ambient.as_ref().unwrap().1;
        let line = a.play_local(&fs, "MP_announcer_allies_win").unwrap();
        // A silent system has no handle to ask; stand in for 1.25 s played.
        a.resumable.get_mut(&line).unwrap().from_s = 1.25;
        a.resumable.get_mut(&ambient).unwrap().from_s = 9.0;
        a.restart(&fs, 11025);
        assert_eq!(a.bank.rate, 11025);
        let new_ambient = a.ambient.as_ref().unwrap().1;
        assert_eq!(a.resumable[&new_ambient].from_s, 0.0);
        let resumed: Vec<&Resumable> = a
            .resumable
            .iter()
            .filter(|(id, _)| **id != new_ambient)
            .map(|(_, r)| r)
            .collect();
        assert_eq!(resumed.len(), 1);
        assert!(
            resumed[0]
                .row
                .name
                .eq_ignore_ascii_case("MP_announcer_allies_win")
        );
        assert_eq!(resumed[0].from_s, 1.25);
        assert_eq!(a.table.len(), 2);
    }

    /// A loaded (non-streamed) sound is not saved; it ends at the restart.
    #[test]
    fn snd_restart_drops_loaded_sounds() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut a = silent(&fs, 22050);
        a.play_local(&fs, "weap_thompson_fire");
        assert_eq!(a.table.len(), 1);
        assert!(a.resumable.is_empty());
        a.restart(&fs, 22050);
        assert_eq!(a.table.len(), 0);
    }

    /// `o` plays only into an idle music slot; `p 0` frees it
    /// (CoDMP.exe 0x44dc40, 0x44db90). The track survives `snd_restart`.
    #[test]
    fn music_play_waits_for_an_idle_slot_and_survives_snd_restart() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut a = silent(&fs, 22050);
        let cs = Vec::new();
        assert!(a.on_server_command(&fs, &cmd(&["o", "MP_announcer_allies_win"]), &cs));
        let first = a.music.unwrap().id;
        assert!(a.on_server_command(&fs, &cmd(&["o", "MP_announcer_axis_win"]), &cs));
        assert_eq!(a.music.unwrap().id, first);
        assert_eq!(a.table.len(), 1);
        a.resumable.get_mut(&first).unwrap().from_s = 0.5;
        a.restart(&fs, 44100);
        let Music { id: resumed, fade } = a.music.unwrap();
        assert!(fade.is_none());
        assert_ne!(resumed, first);
        assert_eq!(a.resumable[&resumed].from_s, 0.5);
        assert!(a.on_server_command(&fs, &cmd(&["p", "0"]), &cs));
        assert!(a.music.is_none());
        assert_eq!(a.table.len(), 0);
        a.on_server_command(&fs, &cmd(&["o", "MP_announcer_axis_win"]), &cs);
        let id = a.music.unwrap().id;
        assert!(
            a.resumable[&id]
                .row
                .name
                .eq_ignore_ascii_case("MP_announcer_axis_win")
        );
    }

    /// `p N` ramps the track's volume to 0 over N ms and closes it there; a
    /// second `p N` leaves the ramp alone (CoDMP.exe 0x44db90, 0x44e7a0).
    #[test]
    fn music_stop_ramps_the_track_out_and_closes_it() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut a = silent(&fs, 22050);
        let cs = Vec::new();
        a.on_server_command(&fs, &cmd(&["o", "MP_announcer_allies_win"]), &cs);
        let id = a.music.unwrap().id;
        let start = a.table.volume(id).unwrap();
        a.on_server_command(&fs, &cmd(&["p", "1000"]), &cs);
        a.advance_fades(250.0);
        a.on_server_command(&fs, &cmd(&["p", "100"]), &cs);
        a.advance_fades(250.0);
        let half = a.table.volume(id).unwrap();
        assert!((half - start * 0.5).abs() < 1e-5, "{half} vs {start}");
        a.advance_fades(600.0);
        assert!(a.music.is_none());
        assert_eq!(a.table.len(), 0);
    }

    /// `q <volume> <ms>` moves the master fade linearly and scales every
    /// voice; landing on 0 stops every sound, the ambient and the music
    /// included, and a later `q 1 0` brings the level back on the next step
    /// (CoDMP.exe 0x44e0f0, 0x44e890).
    #[test]
    fn sound_fade_ramps_the_master_and_stops_everything_at_zero() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut a = silent(&fs, 22050);
        let cs = Vec::new();
        a.set_master_volume(0.8);
        a.set_ambient(&fs, Some("ambient_mp_carentan"));
        a.on_server_command(&fs, &cmd(&["o", "MP_announcer_allies_win"]), &cs);
        assert!(a.on_server_command(&fs, &cmd(&["q", "0.000000", "1000"]), &cs));
        a.advance_fades(400.0);
        assert!((a.master() - 0.8 * 0.6).abs() < 1e-5);
        assert_eq!(a.table.len(), 2);
        a.advance_fades(700.0);
        assert_eq!(a.fade.level, 0.0);
        assert_eq!(a.table.len(), 0);
        assert!(a.ambient.is_none() && a.music.is_none());
        a.on_server_command(&fs, &cmd(&["q", "1.000000", "0"]), &cs);
        assert_eq!(a.fade.level, 0.0);
        a.advance_fades(16.0);
        assert_eq!(a.fade, SoundFade::FULL);
    }

    /// `q 0 0` stops every sound in the command itself.
    #[test]
    fn sound_fade_to_zero_in_no_time_stops_at_once() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut a = silent(&fs, 22050);
        a.play_local(&fs, "MP_announcer_allies_win");
        a.on_server_command(&fs, &cmd(&["q", "0.000000", "0"]), &[]);
        assert_eq!(a.table.len(), 0);
    }

    /// `snd_restart` runs `S_Init`, which sets the master fade back to full,
    /// but saves the music slot's ramp with the track's current volume, so a
    /// `musicStop` fade goes on where it was (CoDMP.exe 0x450029, 0x4510f0,
    /// 0x451330).
    #[test]
    fn snd_restart_resets_the_master_fade_and_keeps_a_music_fade() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut a = silent(&fs, 22050);
        let cs = Vec::new();
        a.on_server_command(&fs, &cmd(&["o", "MP_announcer_allies_win"]), &cs);
        let start = a.table.volume(a.music.unwrap().id).unwrap();
        a.on_server_command(&fs, &cmd(&["p", "1000"]), &cs);
        a.on_server_command(&fs, &cmd(&["q", "0.500000", "1000"]), &cs);
        a.advance_fades(250.0);
        let rate = a.music.unwrap().fade;
        a.restart(&fs, 44100);
        assert_eq!(a.fade, SoundFade::FULL);
        let m = a.music.unwrap();
        assert_eq!(m.fade, rate);
        let resumed = a.table.volume(m.id).unwrap();
        assert!((resumed - start * 0.75).abs() < 1e-5);
        assert_eq!(a.resumable[&m.id].volume, resumed);
        a.advance_fades(800.0);
        assert!(a.music.is_none());
    }

    /// With no game up the main menu replays its own music, so a restart
    /// saves nothing (research doc, section 10).
    #[test]
    fn snd_restart_saves_nothing_at_the_main_menu() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut a = silent(&fs, 22050);
        a.on_gamestate("menu");
        assert!(a.play_menu(&fs, "music_mainmenu").is_some());
        assert_eq!(a.resumable.len(), 1);
        a.restart(&fs, 44100);
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
                rate: 0,
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
                rate: 0,
            },
        );
        a.on_gamestate("mp_carentan");
        let mut cs = vec![String::new(); 800];
        cs[cues::CS_SOUNDS + 5] = "MP_announcer_allies_win".to_string();
        let tok = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();

        assert!(a.on_server_command(&fs, &tok(&["s", "5"]), &cs));
        assert_eq!(a.table.len(), 1);
        assert_eq!(a.stats().misses, 0);

        // Unparsable, empty slot, out of range; recognised, silent.
        assert!(a.on_server_command(&fs, &tok(&["s", "x"]), &cs));
        assert!(a.on_server_command(&fs, &tok(&["s", "6"]), &cs));
        assert!(a.on_server_command(&fs, &tok(&["s", "0"]), &cs));
        assert_eq!(a.table.len(), 1);

        // Quick chat (`j`/`k`/`l`) is owned by `quick_chat::QuickChat`
        // upstream; the audio layer no longer recognises it.
        assert!(!a.on_server_command(&fs, &tok(&["j", "0", "0", "0", "yes_sir"]), &cs));
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
                rate: 0,
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
        a.step(&HashMap::new(), &HashMap::new(), None);
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
                rate: 0,
            },
        );
        a.on_gamestate("mp_carentan");
        let far = Vec3::splat(50_000.0);
        let row = &a.aliases.get("bomb_tick").unwrap()[0];
        assert!(row.channel.is_spatial() && row.dist.1 < far.length());

        let mut cs = vec![String::new(); 800];
        cs[cues::CS_SOUNDS + 3] = "bomb_tick".to_string();
        cs[cues::CS_SOUNDS + 4] = "no_such_loop_alias".to_string();
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
                rate: 0,
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
                rate: 0,
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

    /// A mono 16-bit wav of `n` silent samples.
    fn silent_wav(n: u32) -> Vec<u8> {
        let mut w = Vec::new();
        let data = n * 2;
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&22050u32.to_le_bytes());
        w.extend_from_slice(&44100u32.to_le_bytes());
        w.extend_from_slice(&2u16.to_le_bytes());
        w.extend_from_slice(&16u16.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&data.to_le_bytes());
        w.resize(w.len() + data as usize, 0);
        w
    }

    /// The vehicle captures (`net/snapshots-uo-{tank,jeep,overheat}.bin`,
    /// the last the tank's crew with the gunner holding its trigger): every
    /// frame their sound blend entities (coduo-diff.md 19, "Captures") hold
    /// one voice each on their vehicle, a listener beside it, the engine
    /// pairs' volume and the jeep's pitch follow `angles2[1]` between the
    /// rows of UO's `gmi_mp_vehicles_ge.csv`, and a blend gone from the
    /// snapshot is silent the same frame.
    #[test]
    fn the_uo_vehicle_captures_blend_the_engines_and_the_mg_loops() {
        let p = &vcod_common::net::protocol::PROTOCOL_V22;
        let mut fs = Pk3Fs::empty();
        // The stock rows, `pakuo` `soundaliases/gmi_mp_vehicles_ge.csv`.
        let csv = "name,sequence,file,vol_min,vol_max,pitch_min,pitch_max,dist_min,dist_max,lod_min,lod_max,channel,type,probability,loop,masterslave,loadspec,subtitle\n\
              panzerIV_idle_low,,vehicles/tank_02_idle.wav,0,,1,,300,3500,,,,,,looping,,,\n\
              panzerIV_idle_high,,vehicles/tank_02_idle.wav,0.75,,1,,300,3500,,,,,,looping,,,\n\
              panzerIV_engine_low,,vehicles/tank_03_move.wav,0,,1,,400,4500,,,,,,looping\n\
              panzerIV_engine_high,,vehicles/tank_03_move.wav,1.5,,1,,400,4500,,,,,,looping\n\
              jeep_idle_low,,vehicles/jeep_idle01.wav,0,,1,,150,2000,,,voice,,,looping\n\
              jeep_idle_high,,vehicles/jeep_idle01.wav,0.8,,1,,150,2000,,,voice,,,looping\n\
              jeep_engine_low,,vehicles/jeep_move01.wav,0.2,,0.75,,350,3500,,,voice,,,looping\n\
              jeep_engine_high,,vehicles/jeep_move01.wav,2.5,,1.25,,350,3500,,,voice,,,looping\n\
              weap_mg42_loop,1,weapons/mg42/mg42_loop02.wav,1.25,1.4,1,1.07,10,3500,0,1800,weapon,,,looping\n\
              weap_mg42_loop,2,weapons/mg42/mg42_loop.wav,1,1.2,1,1.07,10,8500,1800,8500,weapon,,,looping\n\
              weap_mg34_loop,1,weapons/mg34/mg34_loop01.wav,1.4,1.5,1,1.1,50,3500,0,1800,weapon,,,looping\n\
              weap_mg34_loop,2,weapons/mg34/mg34_loop01_distant.wav,0.85,0.95,1,1.1,7,8500,1800,8500,weapon,,,looping\n";
        for f in [
            "vehicles/tank_02_idle.wav",
            "vehicles/tank_03_move.wav",
            "vehicles/jeep_idle01.wav",
            "vehicles/jeep_move01.wav",
            "weapons/mg42/mg42_loop02.wav",
            "weapons/mg34/mg34_loop01.wav",
        ] {
            fs.overlay(&format!("sound/{f}"), silent_wav(64));
        }
        let mut seen = HashMap::<String, usize>::new();
        for (tag, map) in [
            ("uo-tank", "mp_cassino"),
            ("uo-jeep", "mp_peaks"),
            ("uo-overheat", "mp_cassino"),
        ] {
            let mut a = AudioSystem::new(
                &fs,
                AudioOpts {
                    enabled: false,
                    volume: 1.0,
                    rate: 0,
                },
            );
            a.aliases_all = AliasTable::parse_csv(csv, "gmi_mp_vehicles_ge.csv");
            a.set_protocol(p);
            a.on_gamestate(map);
            let h = vcod_common::net::huffman::Huffman::new();
            let gs_data = vcod_common::testing::fixture(&format!("net/gamestate-{tag}.bin"));
            let gs = vcod_common::net::gamestate::parse(
                &mut vcod_common::net::msg::MsgReader::new(&gs_data[4..], &h),
                p,
            )
            .unwrap();
            let frames = vcod_common::testing::fixture_frames(
                p,
                &format!("net/gamestate-{tag}.bin"),
                &format!("net/snapshots-{tag}.bin"),
            );
            for pair in frames.windows(2) {
                let (older, newer) = (&pair[0].snapshot, &pair[1].snapshot);
                // Beside the blends' vehicle, inside every row's `dist_max`.
                let eye = older
                    .entities
                    .values()
                    .find(|e| e.field_i32(p, "eType") == 9 && e.field_i32(p, "eventParm") != 0)
                    .and_then(|e| {
                        older
                            .entities
                            .get(&(e.field_i32(p, "otherEntityNum") as u32))
                    })
                    .map_or(Vec3::ZERO, |e| Vec3::from(e.origin(p)) + Vec3::Z * 60.0);
                a.set_listener(eye, Vec3::X, Vec3::NEG_Y, Vec3::Z, 0);
                let pos: HashMap<u32, Vec3> = older
                    .entities
                    .iter()
                    .map(|(&n, e)| (n, Vec3::from(e.origin(p))))
                    .collect();
                a.step_sound_blends(&fs, p, (older, newer, 0.0), (&gs.configstrings, &[]), &pos);
                let t = older.server_time;
                // A blend linked with no aliases yet plays nothing.
                let blends: Vec<u32> = older
                    .entities
                    .iter()
                    .filter(|(_, e)| {
                        e.field_i32(p, "eType") == 9
                            && e.field_i32(p, "eventParm") != 0
                            && e.field_i32(p, "_union.scale") != 0
                    })
                    .map(|(&n, _)| n)
                    .collect();
                let mut live: Vec<u32> = a.blends.keys().copied().collect();
                live.sort();
                assert_eq!(live, blends, "{tag} at {t}");
                for n in blends {
                    let e = &older.entities[&n];
                    let v = &a.blends[&n];
                    let owner = e.field_i32(p, "otherEntityNum") as u32;
                    let at = match a.table.source(v.id) {
                        Some(Source::Entity { num, pos }) if num == n => Some(pos),
                        _ => None,
                    };
                    assert_eq!(at, pos.get(&owner).copied(), "{tag} {n} at {t}");
                    *seen.entry(v.aliases.join("+")).or_default() += 1;
                    let b = e.field_f32(p, "angles2[1]");
                    let (vol, pitch) = match v.aliases[0].as_str() {
                        "panzerIV_idle_low" => (0.75 * b, 1.0),
                        "panzerIV_engine_low" => (1.5 * b, 1.0),
                        "jeep_idle_low" => (0.8 * b, 1.0),
                        "jeep_engine_low" => (0.2 + 2.3 * b, 0.75 + 0.5 * b),
                        _ => continue,
                    };
                    let got = a.table.volume(v.id).unwrap();
                    let want = vol * VOLUME_SCALE;
                    assert!((got - want).abs() < 1e-5, "{tag} {n} at {t}: {got} {want}");
                    assert!(
                        (v.pitch - f64::from(pitch)).abs() < 1e-5,
                        "{tag} {n} at {t}"
                    );
                }
            }
        }
        for pair in [
            "panzerIV_idle_low+panzerIV_idle_high",
            "panzerIV_engine_low+panzerIV_engine_high",
            "jeep_idle_low+jeep_idle_high",
            "jeep_engine_low+jeep_engine_high",
            "weap_mg42_loop+weap_mg42_loop",
            "weap_mg34_loop+weap_mg34_loop",
        ] {
            assert!(
                seen.get(pair).copied().unwrap_or(0) > 10,
                "{pair}: {seen:?}"
            );
        }
    }

    /// The tank capture's shell (`net/snapshots-uo-tank.bin`, the
    /// Panzer IV's main gun, configstring 7's 42): every frame it is in
    /// flight it holds one voice on itself, the stock `artillery_whiz_low`
    /// and `_high` rows of `gmi_mp_artillery.csv` at the blended volume of
    /// its closing rate on the listener, and the frame it explodes
    /// (`eType` 0) the voice ends.
    #[test]
    fn the_uo_tank_capture_shell_whizzes_in_flight() {
        use crate::fx::registry::uo_captures::weapons;
        let p = &vcod_common::net::protocol::PROTOCOL_V22;
        let mut fs = Pk3Fs::empty();
        let csv = "name,sequence,file,vol_min,vol_max,pitch_min,pitch_max,dist_min,dist_max,lod_min,lod_max,channel,type,probability,loop,masterslave,loadspec,subtitle\n\
              artillery_whiz_low,,whizbys/artillery_whiz14.wav,1.3,,0.97,,200,6000,,,item,,,looping\n\
              artillery_whiz_high,,whizbys/artillery_whiz14.wav,0.4,,1,,200,6000,,,item,,,looping\n";
        fs.overlay("sound/whizbys/artillery_whiz14.wav", silent_wav(64));
        let mut a = AudioSystem::new(
            &fs,
            AudioOpts {
                enabled: false,
                volume: 1.0,
                rate: 0,
            },
        );
        a.aliases_all = AliasTable::parse_csv(csv, "gmi_mp_artillery.csv");
        a.set_protocol(p);
        a.on_gamestate("mp_cassino");
        let weapons = weapons("uo-tank");
        let frames = vcod_common::testing::fixture_frames(
            p,
            "net/gamestate-uo-tank.bin",
            "net/snapshots-uo-tank.bin",
        );
        let (mut flying, mut ended) = (0, 0);
        let mut last: Vec<u32> = Vec::new();
        for pair in frames.windows(2) {
            let (older, newer) = (&pair[0].snapshot, &pair[1].snapshot);
            let shells: Vec<u32> = older
                .entities
                .iter()
                .filter(|(_, e)| {
                    e.field_i32(p, "eType") == vcod_common::net::flags::ET_MISSILE
                        && e.field_i32(p, "weapon") == 42
                })
                .map(|(&n, _)| n)
                .collect();
            // The listener stands 1000 units off the first shell's line.
            let eye = shells
                .first()
                .map(|n| Vec3::from(older.entities[n].origin(p)) + Vec3::Y * 1000.0)
                .unwrap_or(Vec3::ZERO);
            a.set_listener(eye, Vec3::X, Vec3::NEG_Y, Vec3::Z, 0);
            let pos: HashMap<u32, Vec3> = older
                .entities
                .iter()
                .map(|(&n, e)| (n, Vec3::from(e.origin(p))))
                .collect();
            a.step_sound_blends(&fs, p, (older, newer, 0.0), (&[], &weapons), &pos);
            let mut live: Vec<u32> = a.blends.keys().copied().collect();
            live.sort();
            assert_eq!(live, shells, "at {}", older.server_time);
            for n in &shells {
                let v = &a.blends[n];
                assert_eq!(v.aliases, ["artillery_whiz_low", "artillery_whiz_high"]);
                let e = &older.entities[n];
                let vel = vcod_common::net::trajectory::Trajectory::read(e, p, "pos").delta;
                let b = blend::shell_blend(pos[n], vel, eye);
                let want = (1.3 + (0.4 - 1.3) * b) * VOLUME_SCALE;
                let got = a.table.volume(v.id).unwrap();
                assert!((got - want).abs() < 1e-5, "{n}: {got} {want}");
                flying += 1;
            }
            ended += last.iter().filter(|n| !shells.contains(n)).count();
            last = shells;
        }
        assert!(flying >= 1 && ended >= 1, "{flying} {ended}");
    }

    /// The bipod capture's 3 s burst (`net/snapshots-uo-mg.bin`): the
    /// mg34's 70 ms `fireTime` plus 51 ms bridges every gap between the
    /// snapshots' shots, so one loop runs from the first shot until 121 ms
    /// past the last, and then ends.
    #[test]
    fn the_uo_mg_capture_holds_one_fire_loop_through_the_burst() {
        use crate::fx::registry::uo_captures::weapons;
        let p = &vcod_common::net::protocol::PROTOCOL_V22;
        let fs = Pk3Fs::empty();
        let mut a = AudioSystem::new(
            &fs,
            AudioOpts {
                enabled: false,
                volume: 1.0,
                rate: 0,
            },
        );
        a.set_protocol(p);
        a.weapon_sounds
            .insert(26, weapons("uo-mg")[26].clone().map(|d| d.sounds));
        let none = HashMap::new();
        let mut tracker = vcod_common::net::events::EventTracker::new();
        let mut last_time = None;
        let mut shots = Vec::new();
        let mut running = Vec::new();
        for f in vcod_common::testing::fixture_frames(
            p,
            "net/gamestate-uo-mg.bin",
            "net/snapshots-uo-mg.bin",
        ) {
            let t = f.snapshot.server_time;
            if let Some(last) = last_time {
                a.step_fire_loops(&fs, (t - last) as f32, &none, &HashMap::new());
            }
            last_time = Some(t);
            for ev in tracker.drain(&f.snapshot, p) {
                if ev.event == crate::fx::registry::EV_FIRE_WEAPON && ev.weapon == 26 {
                    shots.push(t);
                    a.start_fire_loop(&ev);
                }
            }
            running.push((t, !a.fire_loops.is_empty()));
        }
        let (first, last) = (shots[0], *shots.last().unwrap());
        assert!(shots.len() >= 30, "{} shots", shots.len());
        for (t, on) in running {
            assert_eq!(on, (first..=last + 100).contains(&t), "at {t}");
        }
    }

    /// UO's MG fire loop: a shot holds it `fireTime` + 51 ms, a later shot
    /// re-arms it, and it ends on the stop alias; 1.1 never starts one.
    #[test]
    fn a_uo_body_holds_its_fire_loop_past_the_last_shot() {
        let fs = Pk3Fs::empty();
        let mut a = AudioSystem::new(
            &fs,
            AudioOpts {
                enabled: false,
                volume: 1.0,
                rate: 0,
            },
        );
        a.weapon_sounds.insert(
            4,
            Some(WeaponSounds {
                loop_fire: Some("weap_mg34_loop".into()),
                stop_fire: Some("weap_mg34_cooldown".into()),
                fire_time_ms: 100.0,
                ..Default::default()
            }),
        );
        let shot = GameEvent {
            event: crate::fx::registry::EV_FIRE_WEAPON,
            parm: 0,
            entity_num: 3,
            client_num: 3,
            weapon: 4,
            surf_type: 0,
            pos: [0.0; 3],
            dir: [0.0; 3],
            scale: 0,
            other_entity_num: u32::MAX,
            attacker_entity_num: -1,
            e_type: vcod_common::net::flags::ET_PLAYER,
            eflags: 0,
        };
        let none = HashMap::new();
        a.start_fire_loop(&shot);
        assert!(a.fire_loops.is_empty(), "1.1 plays no loop");
        a.set_protocol(&vcod_common::net::protocol::PROTOCOL_V22);
        a.start_fire_loop(&shot);
        a.step_fire_loops(&fs, 100.0, &none, &HashMap::new());
        a.start_fire_loop(&shot);
        a.step_fire_loops(&fs, 150.0, &none, &HashMap::new());
        assert_eq!(a.fire_loops[&3].left, 1.0);
        a.step_fire_loops(&fs, 1.0, &none, &HashMap::new());
        assert!(a.fire_loops.is_empty());
        // The loop starts at the body's `tag_flash`, else where it stands.
        let flash = Vec3::new(40.0, 10.0, 50.0);
        let muzzles = HashMap::from([(3, (flash, Vec3::X))]);
        let at = HashMap::from([(3, Vec3::new(0.0, 0.0, 0.0)), (6, Vec3::ONE)]);
        assert_eq!(
            a.fire_loop_source(3, &at, &muzzles),
            Source::Tag { num: 3, pos: flash }
        );
        assert_eq!(
            a.fire_loop_source(6, &at, &muzzles),
            Source::Entity {
                num: 6,
                pos: Vec3::ONE
            }
        );
        // Mounted, the turret's own loop plays instead.
        a.start_fire_loop(&GameEvent {
            eflags: crate::turret::EF_MOUNTED,
            ..shot
        });
        assert!(a.fire_loops.is_empty());
    }

    /// The bazooka capture (`net/snapshots-uo-bazooka.bin`, coduo-diff.md
    /// 19): the round rides entity 158 as `eType` 4 for eight snapshots and
    /// blows on it as `eType` 0. Its `projectileSound` holds one voice on
    /// the missile for exactly those snapshots and stops on the explode.
    /// The stock row is `null.wav`, so retail's round flies silent: with it
    /// the same snapshots start no voice and count no miss.
    #[test]
    fn the_uo_bazooka_capture_plays_its_projectile_sound_in_flight() {
        let p = &vcod_common::net::protocol::PROTOCOL_V22;
        let mut fs = Pk3Fs::empty();
        fs.overlay(
            "weapons/mp/bazooka_mp",
            b"WEAPONFILE\\weaponType\\projectile\\projectileSound\\weap_panzerfaust_projectile"
                .to_vec(),
        );
        fs.overlay("sound/weapons/rocket_loop.wav", silent_wav(64));
        let header = "name,sequence,file,vol_min,vol_max,pitch_min,pitch_max,dist_min,dist_max,lod_min,lod_max,channel,type,probability,loop,masterslave,loadspec,subtitle\n";
        // The stock row, `pakuo` `soundaliases/generic_mp_sound.csv`.
        let stock = "weap_panzerfaust_projectile,,null.wav,1.14,1.25,,,7,500,,,auto,,,looping\n";
        let audible = stock.replace("null.wav", "weapons/rocket_loop.wav");
        let gs = vcod_common::testing::fixture_gamestate(p, "net/gamestate-uo-bazooka.bin");
        let frames = vcod_common::testing::fixture_frames(
            p,
            "net/gamestate-uo-bazooka.bin",
            "net/snapshots-uo-bazooka.bin",
        );
        for (row, heard) in [(audible.as_str(), true), (stock, false)] {
            let mut a = AudioSystem::new(
                &fs,
                AudioOpts {
                    enabled: false,
                    volume: 1.0,
                    rate: 0,
                },
            );
            a.aliases_all =
                AliasTable::parse_csv(&format!("{header}{row}"), "generic_mp_sound.csv");
            a.set_protocol(p);
            a.on_gamestate("mp_cassino");
            let (mut flying, mut voice) = (0, None);
            for f in &frames {
                let missiles = missile_sounds(p, &f.snapshot);
                // On the round, inside the row's 500 `dist_max`.
                let eye = missiles.get(&158).map_or(Vec3::ZERO, |&(_, pos)| pos);
                a.set_listener(eye, Vec3::X, Vec3::NEG_Y, Vec3::Z, 0);
                a.set_projectile_sounds(&fs, &gs.configstrings, &missiles);
                assert!(missiles.keys().all(|&n| n == 158));
                if missiles.is_empty() || !heard {
                    assert!(a.projectile_voices.is_empty());
                    assert_eq!(a.table.len(), 0);
                    continue;
                }
                flying += 1;
                let id = a.projectile_voices[&158].id;
                assert_eq!(*voice.get_or_insert(id), id, "one voice for the flight");
                assert!(matches!(
                    a.table.source(id),
                    Some(Source::Entity { num: 158, .. })
                ));
            }
            assert_eq!(flying, if heard { 8 } else { 0 });
            assert_eq!(a.stats.misses, 0);
        }
    }

    /// A CoD4 table plays every row at 1.7's master scale, 0.75 under the
    /// shell's 0.8 master: `snd_volume`'s default times iw3mp.exe's double
    /// at 0x70b0c8 (cod4-sound.md 5).
    #[test]
    fn a_cod4_table_plays_at_the_1_7_master_scale() {
        let mut a = silent(&Pk3Fs::empty(), 0);
        a.set_cod4_aliases(AliasTable::default());
        assert_eq!(a.volume_scale, COD4_VOLUME_SCALE);
        assert_eq!(COD4_VOLUME_SCALE, 0.75);
        let shell = crate::console::shell::Shell::new();
        assert_eq!(shell.cvar_f32("mss_volume") * a.volume_scale, 0.8 * 0.75);
    }

    /// A silent system over a CoD2 table of `csv`, on mp_toujane.
    fn cod2_silent(fs: &dyn AssetSource, csv: &str) -> AudioSystem {
        let mut a = silent(fs, 0);
        let ctx = cod2::Ctx {
            groups: &[],
            curves: &[],
        };
        a.aliases_all = cod2::parse_csv(csv, "t.csv", &ctx);
        a.table.use_slave_lerp();
        a.volume_scale = 1.0;
        a.on_gamestate("mp_toujane");
        a
    }

    /// `SND_PlayAmbientAlias`: a new track fades in on the other slot
    /// while the old one fades out and ends; the same row is a no-op, a
    /// row of the same file only ramps the volume, and a blank alias
    /// (`ambientStop`) changes nothing.
    #[test]
    fn cod2_ambients_crossfade() {
        let fs = Pk3Fs::empty();
        let csv = "name,file,vol_min,channel,type,loop\n\
                   amb_a,ambient/a.mp3,0.8,local,streamed,looping\n\
                   amb_b,ambient/b.mp3,0.6,local,streamed,looping\n\
                   amb_b_quiet,ambient/b.mp3,0.2,local,streamed,looping\n";
        let mut a = cod2_silent(&fs, csv);
        a.set_cod2_ambient(&fs, "n\\amb_a\\t\\0", Some("amb_a"), 0);
        let (_, first) = a.ambient.clone().unwrap();
        assert_eq!(a.table.volume(first), Some(0.8));
        assert_eq!(a.table.entity(first), Some(cues::AMBIENT_ENTITY));
        // The same text again is not a new `CG_StartAmbient`.
        a.set_cod2_ambient(&fs, "n\\amb_a\\t\\0", Some("amb_a"), 0);
        assert_eq!(a.table.len(), 1);
        a.set_cod2_ambient(&fs, "n\\amb_b\\t\\2000", Some("amb_b"), 1000);
        let (_, second) = a.ambient.clone().unwrap();
        assert_eq!(a.table.entity(second), Some(cues::AMBIENT_ENTITY_2));
        assert_eq!(a.table.volume(second), Some(0.0));
        a.advance_fades(500.0);
        assert!((a.table.volume(first).unwrap() - 0.4).abs() < 1e-5);
        assert!((a.table.volume(second).unwrap() - 0.3).abs() < 1e-5);
        a.advance_fades(600.0);
        assert_eq!(a.table.volume(first), None);
        assert_eq!(a.table.volume(second), Some(0.6));
        // Same file, loop, pitch and channel: the volume ramps, no restart.
        a.set_cod2_ambient(&fs, "n\\amb_b_quiet\\t\\4000", Some("amb_b_quiet"), 400);
        assert_eq!(a.ambient.as_ref().map(|x| x.1), Some(second));
        a.advance_fades(200.0);
        assert!((a.table.volume(second).unwrap() - 0.4).abs() < 1e-5);
        a.advance_fades(300.0);
        assert!((a.table.volume(second).unwrap() - 0.2).abs() < 1e-5);
        // `ambientStop`: nothing to pick, nothing changes.
        a.set_cod2_ambient(&fs, "t\\5000", None, 1000);
        a.advance_fades(2000.0);
        assert_eq!(a.table.volume(second), Some(0.2));
        // A cut (fade 0) stops the old track at once.
        a.set_cod2_ambient(&fs, "n\\amb_a\\t\\6000", Some("amb_a"), 0);
        assert_eq!(a.table.volume(second), None);
        assert_eq!(a.table.len(), 1);
    }

    /// A started CoD2 row with a `subtitle` cell queues its key for the
    /// HUD, on for at least `cg_subtitleMinTime`; a row without one, and
    /// the menu's sounds, queue nothing.
    #[test]
    fn cod2_rows_queue_their_subtitles() {
        let mut fs = Pk3Fs::empty();
        fs.overlay("sound/vo/a.wav", silent_wav(64));
        let csv = "name,file,subtitle,channel\n\
                   vo_a,vo/a.wav,SUBTITLE_VO_A,local\n\
                   vo_b,vo/a.wav,,local\n";
        let mut a = cod2_silent(&fs, csv);
        let cue = |alias: &str| Cue {
            alias: alias.to_string(),
            source: Source::Point(Vec3::ZERO),
            delay_s: 0.0,
        };
        assert!(a.play(&fs, cue("vo_a")).is_some());
        assert!(a.play(&fs, cue("vo_b")).is_some());
        assert_eq!(
            a.take_subtitles(),
            [("SUBTITLE_VO_A".to_string(), SUBTITLE_MIN_MS)]
        );
        assert!(a.take_subtitles().is_empty());
    }
}
