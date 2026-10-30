//! CoD4 1.7's sound aliases as a player reads them: the zones' alias
//! lists ([`crate::assets::sound`]) flattened into [`Alias`] rows, the
//! entity channels of `soundaliases/channels.def`, and the zone's loaded
//! sounds decoded to PCM and wrapped as `.wav` bytes
//! (docs/research/cod4-sound.md).

use crate::assets::Assets;
use crate::assets::sound::{LoadedSound, SoundAlias, SoundFile, SpeakerMap};
use anyhow::{Result, bail, ensure};
use std::collections::HashMap;
use std::sync::Arc;

/// `snd_alias_t.flags` bit 0: the alias loops (cod4-sound.md 2).
pub const FLAG_LOOPING: u32 = 0x1;
/// Bit 1: a master alias, which ducks the slaves while it plays.
pub const FLAG_MASTER: u32 = 0x2;
/// Bit 2: a slave alias.
pub const FLAG_SLAVE: u32 = 0x4;
/// Bit 4: no reverb send (`nowetlevel`).
pub const FLAG_NO_WET: u32 = 0x10;
/// Bits 6-7: the sound file's type, [`TYPE_LOADED`] or [`TYPE_STREAMED`].
const TYPE_SHIFT: u32 = 6;
pub const TYPE_LOADED: u32 = 1;
pub const TYPE_STREAMED: u32 = 2;
/// Bits 8-13: the entity channel, a row of `channels.def`.
const CHANNEL_SHIFT: u32 = 8;
const CHANNEL_MASK: u32 = 0x3f;

/// The 29 surface types, in `infoParms` order (`iw3mp.exe` 0x71fbbc):
/// the suffix of every per-surface alias and the index footsteps,
/// landings and impacts carry.
pub const SURFACES: [&str; 29] = [
    "default",
    "bark",
    "brick",
    "carpet",
    "cloth",
    "concrete",
    "dirt",
    "flesh",
    "foliage",
    "glass",
    "grass",
    "gravel",
    "ice",
    "metal",
    "mud",
    "paper",
    "plaster",
    "rock",
    "sand",
    "snow",
    "water",
    "wood",
    "asphalt",
    "ceramic",
    "plastic",
    "rubber",
    "cushion",
    "fruit",
    "paintedmetal",
];

/// Where an alias's samples are.
#[derive(Clone, Debug, PartialEq)]
pub enum AliasFile {
    /// A loaded sound, by its name in the zone (`footsteps/foot_stone01.wav`).
    Loaded(String),
    /// A streamed file under `sound/` in the iwds (`ambient/x.mp3`).
    Streamed(String),
}

impl AliasFile {
    /// The path under `sound/` either lives at.
    pub fn path(&self) -> &str {
        match self {
            AliasFile::Loaded(p) | AliasFile::Streamed(p) => p,
        }
    }
}

/// One `snd_alias_t` (0x5C), the fields a player reads.
#[derive(Clone, Debug, PartialEq)]
pub struct Alias {
    pub name: String,
    pub file: AliasFile,
    pub sequence: i32,
    pub vol: (f32, f32),
    pub pitch: (f32, f32),
    pub dist: (f32, f32),
    pub flags: u32,
    pub slave_percentage: f32,
    /// A weight among the list's rows.
    pub probability: f32,
    pub start_delay_ms: i32,
    /// The falloff curve's knots `(x, y)`; `None` for no curve.
    pub curve: Option<Vec<[f32; 2]>>,
    pub secondary: Option<String>,
    /// `lfePercentage` (60) and `centerPercentage` (64): a multichannel
    /// output's sends only (cod4-sound.md 2).
    pub lfe: f32,
    pub center: f32,
    /// The speaker map a 2D voice plays through.
    pub speaker_map: Option<Arc<SpeakerMap>>,
}

impl Alias {
    pub fn looping(&self) -> bool {
        self.flags & FLAG_LOOPING != 0
    }

    pub fn master(&self) -> bool {
        self.flags & FLAG_MASTER != 0
    }

    pub fn slave(&self) -> bool {
        self.flags & FLAG_SLAVE != 0
    }

    pub fn streamed(&self) -> bool {
        (self.flags >> TYPE_SHIFT) & 3 == TYPE_STREAMED
    }

    /// The row of `channels.def` it plays on.
    pub fn channel(&self) -> usize {
        ((self.flags >> CHANNEL_SHIFT) & CHANNEL_MASK) as usize
    }

    /// The alias at `al` of zone `a`; `None` with no file.
    pub fn of(a: &Assets, list: &str, al: &SoundAlias) -> Option<Alias> {
        let f = |o: usize| f32::from_le_bytes(al.raw[o..o + 4].try_into().unwrap());
        let i = |o: usize| i32::from_le_bytes(al.raw[o..o + 4].try_into().unwrap());
        let file = match al.file.as_ref()? {
            SoundFile::Loaded(id) => AliasFile::Loaded(
                loaded_name(&a.loaded_sounds[id.as_ref()?.index()].name).to_string(),
            ),
            SoundFile::Streamed { dir, name } => {
                let name = name.as_deref().filter(|n| !n.is_empty())?;
                AliasFile::Streamed(match dir.as_deref().filter(|d| !d.is_empty()) {
                    Some(d) => format!("{d}/{name}"),
                    None => name.to_string(),
                })
            }
        };
        let curve = al
            .falloff
            .map(|c| &a.snd_curves[c.index()])
            .filter(|c| c.knots.len() >= 2)
            .map(|c| c.knots.clone());
        Some(Alias {
            name: al.name.clone().unwrap_or_else(|| list.to_string()),
            file,
            sequence: i(20),
            vol: (f(24), f(28)),
            pitch: (f(32), f(36)),
            dist: (f(40), f(44)),
            flags: i(48) as u32,
            slave_percentage: f(52),
            probability: f(56),
            start_delay_ms: i(68),
            curve,
            secondary: al.secondary.clone().filter(|s| !s.is_empty()),
            lfe: f(60),
            center: f(64),
            speaker_map: al.speaker_map.clone().map(Arc::new),
        })
    }
}

/// A loaded sound's name without the `,` that marks a reference to one
/// an earlier zone loaded (cod4-sound.md 3).
pub fn loaded_name(name: &str) -> &str {
    name.strip_prefix(',').unwrap_or(name)
}

/// One row of `channels.def`.
#[derive(Clone, Debug, PartialEq)]
pub struct ChannelDef {
    pub name: String,
    pub priority: i32,
    pub is3d: bool,
    /// One sound per channel per entity (the file's own word for it), the
    /// default; `unrestricted` lets them stack.
    pub restricted: bool,
}

/// `soundaliases/channels.def`: `name, priority, is3d, restricted,
/// pausable, maxvoices`, `#` comments; a row's index is the alias
/// flags' channel.
pub fn parse_channels(text: &str) -> Vec<ChannelDef> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let cells: Vec<&str> = l.split(',').map(str::trim).collect();
            ChannelDef {
                name: cells[0].to_ascii_lowercase(),
                priority: cells.get(1).and_then(|c| c.parse().ok()).unwrap_or(0),
                is3d: cells.get(2).is_some_and(|c| c.eq_ignore_ascii_case("3d")),
                restricted: !cells
                    .get(3)
                    .is_some_and(|c| c.eq_ignore_ascii_case("unrestricted")),
            }
        })
        .collect()
}

/// The aliases and loaded sounds of a set of zones, by name.
#[derive(Default)]
pub struct SoundSet {
    /// Lowercased list name to its rows; a later zone's list replaces an
    /// earlier one's.
    pub aliases: HashMap<String, Vec<Alias>>,
    /// Lowercased loaded-sound name to the sound holding its samples.
    pub loaded: HashMap<String, Arc<LoadedSound>>,
    pub channels: Vec<ChannelDef>,
    /// Alias rows dropped for no file.
    pub skipped: usize,
}

impl SoundSet {
    /// Adds zone `a`'s alias lists and the loaded sounds that carry data.
    pub fn add(&mut self, a: &Assets) {
        for list in &a.sounds {
            let rows: Vec<Alias> = list
                .aliases
                .iter()
                .filter_map(|al| {
                    let r = Alias::of(a, &list.name, al);
                    self.skipped += usize::from(r.is_none());
                    r
                })
                .collect();
            self.aliases.insert(list.name.to_ascii_lowercase(), rows);
        }
        for l in &a.loaded_sounds {
            if l.data.is_some() && !l.name.starts_with(',') {
                self.loaded
                    .insert(l.name.to_ascii_lowercase(), Arc::new(l.clone()));
            }
        }
        if let Some(r) = a
            .raw_files
            .iter()
            .find(|r| r.name.eq_ignore_ascii_case("soundaliases/channels.def"))
        {
            self.channels = parse_channels(&String::from_utf8_lossy(&r.data));
        }
    }

    pub fn alias(&self, name: &str) -> Option<&[Alias]> {
        self.aliases
            .get(&name.to_ascii_lowercase())
            .map(Vec::as_slice)
    }

    /// The `.wav` bytes of loaded sound `name` (no leading `,`, any case).
    pub fn wav(&self, name: &str) -> Option<Result<Vec<u8>>> {
        let l = self.loaded.get(&name.to_ascii_lowercase())?;
        Some(wav(l))
    }
}

/// `WAVE_FORMAT_PCM`, the `AILSOUNDINFO.format` of every stock loaded
/// sound that carries data (cod4-sound.md 3).
pub const FORMAT_PCM: u32 = 1;

/// A loaded sound's samples as a RIFF `.wav`: the zone keeps the bare
/// sample data and its `AILSOUNDINFO`, no header.
pub fn wav(l: &LoadedSound) -> Result<Vec<u8>> {
    let data = l.data.as_deref().unwrap_or_default();
    ensure!(
        l.format == FORMAT_PCM,
        "{}: format {} is not PCM",
        l.name,
        l.format
    );
    if !(l.bits == 8 || l.bits == 16) || !(1..=2).contains(&l.channels) || l.rate == 0 {
        bail!(
            "{}: {} bits, {} channels at {} Hz",
            l.name,
            l.bits,
            l.channels,
            l.rate
        );
    }
    let block = l.channels * l.bits / 8;
    let len = (data.len() as u32 / block) * block;
    let mut out = Vec::with_capacity(44 + len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(l.channels as u16).to_le_bytes());
    out.extend_from_slice(&l.rate.to_le_bytes());
    out.extend_from_slice(&(l.rate * block).to_le_bytes());
    out.extend_from_slice(&(block as u16).to_le_bytes());
    out.extend_from_slice(&(l.bits as u16).to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&data[..len as usize]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_def_rows_index_the_flags() {
        let text = "# c\nphysics,0,3d,unrestricted,pause,6\nauto,1,3d,unrestricted\nauto2d,1,2d,unrestricted\nlocal,7,2d\n";
        let c = parse_channels(text);
        assert_eq!(c.len(), 4);
        assert!(!c[2].restricted && c[3].restricted);
        assert_eq!(c[1].name, "auto");
        assert!(c[1].is3d && !c[2].is3d);
        assert_eq!(c[2].priority, 1);
    }

    #[test]
    fn a_pcm_sound_gets_a_header() {
        let l = LoadedSound {
            name: "x.wav".into(),
            format: 1,
            rate: 22050,
            bits: 16,
            channels: 1,
            samples: 2,
            block_size: 2,
            data_len: 4,
            data: Some(vec![1, 0, 2, 0]),
        };
        let w = wav(&l).unwrap();
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(w.len(), 48);
        assert_eq!(u32::from_le_bytes(w[24..28].try_into().unwrap()), 22050);
        assert_eq!(&w[44..], &[1, 0, 2, 0]);
    }
}
