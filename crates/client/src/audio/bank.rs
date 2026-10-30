//! Lazy decoded-sound cache; a failure caches as `None`. Streamed aliases
//! bypass it through [`SoundBank::read_stream`].

use std::collections::HashMap;
use std::io::Cursor;
use vcod_common::asset_source::AssetSource;

use kira::sound::static_sound::StaticSoundData;

use crate::audio::stream::Resampler;

#[derive(Default)]
pub struct SoundBank {
    cache: HashMap<String, Option<StaticSoundData>>,
    /// Read or decode failures, counted once per file.
    pub failures: u64,
    /// The mixer rate `mss_khz` picked ([`mixer_rate`]); a file above it
    /// is resampled down to it once, at decode. 0 keeps every file as is.
    pub rate: u32,
}

/// `mss_khz` as the Miles start-up reads it (CoDMP.exe 0x44ba30): 11, 22
/// and 44 open the output at 11025, 22050 and 44100 Hz; anything else
/// warns and takes 22050.
pub fn mixer_rate(khz: i32) -> u32 {
    match khz {
        11 => 11025,
        22 => 22050,
        44 => 44100,
        _ => {
            crate::console::log::print(&format!(
                "invalid value {khz} for mss_khz, using 22 khz instead"
            ));
            22050
        }
    }
}

/// Retail mixes every voice at the output rate, so a file above it loses
/// what that rate cannot carry. kira mixes at the device's own rate; taking
/// the file down to `rate` once, linearly, stands in for that. How Miles
/// interpolates is not in CoDMP.exe. Streams go through the same
/// [`Resampler`] chunk by chunk (`stream.rs`).
fn to_rate(d: StaticSoundData, rate: u32) -> StaticSoundData {
    if rate == 0 || d.sample_rate <= rate || d.frames.is_empty() {
        return d;
    }
    let mut r = Resampler::new(d.sample_rate, rate);
    let mut frames = Vec::new();
    r.push(&d.frames, &mut frames);
    r.finish(&mut frames);
    let step = f64::from(d.sample_rate) / f64::from(rate);
    let scale = |f: usize| ((f as f64) / step).round() as usize;
    StaticSoundData {
        sample_rate: rate,
        frames: frames.into(),
        settings: d.settings,
        slice: d.slice.map(|(a, b)| (scale(a), scale(b))),
    }
}

/// Alias `file` values are relative to `sound/`; some carry a leading slash.
pub(crate) fn sound_path(file: &str) -> String {
    format!("sound/{}", file.trim_start_matches('/'))
}

impl SoundBank {
    /// An empty bank that takes files down to `rate` (0 keeps them).
    pub fn at_rate(rate: u32) -> SoundBank {
        SoundBank {
            rate,
            ..Default::default()
        }
    }

    /// `None` (cached) when the file is missing or fails to decode.
    pub fn get(&mut self, fs: &dyn AssetSource, file: &str) -> Option<&StaticSoundData> {
        let key = file.to_ascii_lowercase();
        if !self.cache.contains_key(&key) {
            let path = sound_path(file);
            let decoded = match fs.read(&path) {
                Some(bytes) => match StaticSoundData::from_cursor(Cursor::new(bytes)) {
                    Ok(d) => Some(to_rate(d, self.rate)),
                    Err(e) => {
                        log::warn!("sound {path}: decode failed: {e}");
                        None
                    }
                },
                None => {
                    log::warn!("sound {path}: not found in pk3s");
                    None
                }
            };
            if decoded.is_none() {
                self.failures += 1;
            }
            self.cache.insert(key.clone(), decoded);
        }
        self.cache.get(&key).and_then(Option::as_ref)
    }

    /// Raw bytes for a streamed alias; nothing is cached and a missing file
    /// is not counted here.
    pub fn read_stream(fs: &dyn AssetSource, file: &str) -> Option<Vec<u8>> {
        let path = sound_path(file);
        let bytes = fs.read(&path);
        if bytes.is_none() {
            log::warn!("sound {path}: not found in pk3s");
        }
        bytes
    }

    /// Cached entries, failures included. Only the tests ask.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    /// Only the tests ask.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kira::Frame;

    #[test]
    fn decodes_retail_wav_once_and_caches_failures() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut bank = SoundBank::default();
        assert!(bank.is_empty());
        let d = bank.get(&fs, "weapons/kar98k/kar98_01.wav");
        assert!(d.is_some());
        assert!(d.unwrap().duration().as_secs_f32() > 0.1);
        assert!(bank.get(&fs, "Footsteps/foot_stone01.wav").is_some()); // case-insensitive dir
        assert!(bank.get(&fs, "nope/missing.wav").is_none());
        assert!(bank.get(&fs, "nope/missing.wav").is_none());
        assert_eq!(bank.failures, 1);
        assert_eq!(bank.len(), 3);
    }

    fn tone(rate: u32, frames: Vec<f32>) -> StaticSoundData {
        StaticSoundData {
            sample_rate: rate,
            frames: frames.into_iter().map(Frame::from_mono).collect(),
            settings: Default::default(),
            slice: None,
        }
    }

    #[test]
    fn mss_khz_picks_retail_rates() {
        assert_eq!(mixer_rate(11), 11025);
        assert_eq!(mixer_rate(22), 22050);
        assert_eq!(mixer_rate(44), 44100);
        assert_eq!(mixer_rate(48), 22050);
    }

    #[test]
    fn files_above_the_mixer_rate_come_down_to_it() {
        let d = to_rate(tone(22050, vec![0.0, 1.0, 0.0, -1.0, 0.0]), 11025);
        assert_eq!(d.sample_rate, 11025);
        let left: Vec<f32> = d.frames.iter().map(|f| f.left).collect();
        assert_eq!(left, vec![0.0, 0.0, 0.0]);
        // 2.5 source frames a step: frame 1 lerps halfway between 2 and 3
        let d = to_rate(tone(27563, vec![0.0, 0.0, 1.0, 0.0, 0.0]), 11025);
        assert_eq!(d.frames.len(), 2);
        assert!((d.frames[1].left - 0.5).abs() < 0.01);
        // at or below the rate, untouched
        let same = tone(11025, vec![0.25; 4]);
        assert!(to_rate(same.clone(), 22050) == same);
        assert!(to_rate(same.clone(), 0) == same);
    }

    #[test]
    fn streamed_bytes_come_back_raw() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let bytes = SoundBank::read_stream(&fs, "ambient/amb_harbor.mp3").unwrap();
        assert!(bytes.len() > 100_000);
    }
}
