//! Lazy decoded-sound cache; a failure caches as `None`. Streamed aliases
//! bypass it through [`SoundBank::read_stream`].

use std::collections::HashMap;
use std::io::Cursor;

use kira::sound::static_sound::StaticSoundData;

use vcod_common::pk3::Pk3Fs;

#[derive(Default)]
pub struct SoundBank {
    cache: HashMap<String, Option<StaticSoundData>>,
    /// Read or decode failures, counted once per file.
    pub failures: u64,
}

/// Alias `file` values are relative to `sound/`; some carry a leading slash.
fn sound_path(file: &str) -> String {
    format!("sound/{}", file.trim_start_matches('/'))
}

impl SoundBank {
    /// `None` (cached) when the file is missing or fails to decode.
    pub fn get(&mut self, fs: &Pk3Fs, file: &str) -> Option<&StaticSoundData> {
        let key = file.to_ascii_lowercase();
        if !self.cache.contains_key(&key) {
            let path = sound_path(file);
            let decoded = match fs.read(&path) {
                Some(bytes) => match StaticSoundData::from_cursor(Cursor::new(bytes)) {
                    Ok(d) => Some(d),
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
    pub fn read_stream(fs: &Pk3Fs, file: &str) -> Option<Vec<u8>> {
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

    #[test]
    fn streamed_bytes_come_back_raw() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let bytes = SoundBank::read_stream(&fs, "ambient/amb_harbor.mp3").unwrap();
        assert!(bytes.len() > 100_000);
    }
}
