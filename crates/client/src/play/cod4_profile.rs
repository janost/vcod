//! The CoD4 profile's stats across runs: the active profile's
//! `players/profiles/<profile>/[<fs_game>/]vcod_mpdata`, the stats block
//! stored plain, read at the join, sent in the `stats` packets, and
//! written back once the server's `N` commands have changed it and at
//! the disconnect, as `CL_Disconnect` uploads them (`iw3mp.exe` 0x4698bc).
//! The front end reads the same file for its menus' `stat()` and writes
//! their `statset`s back. Retail's own `mpdata` beside it is never read or
//! written.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use vcod_cod4::net::stats::{STATS_BYTES, Stats};

/// The install root and `fs_game` the next join reads its profile under.
static CONTEXT: Mutex<Option<(PathBuf, String)>> = Mutex::new(None);

/// The install root (where `players/` sits) and `fs_game`, as the client
/// starts or changes mod.
pub fn set_context(root: &Path, fs_game: &str) {
    *CONTEXT.lock().unwrap() = Some((root.to_path_buf(), fs_game.to_string()));
}

/// Writes wait this long after a change, so a burst of `N`s is one write.
const SETTLE_SECS: u64 = 2;

/// The file's name in the profile (or its `fs_game`) directory.
const FILE: &str = "vcod_mpdata";

pub struct ProfileStats {
    /// The profile's directory: once deleted, nothing is written.
    dir: PathBuf,
    path: PathBuf,
    saved: Stats,
    changed_at: Option<Instant>,
    /// The file was there: no `LiveStorage_StatsInit` for this profile.
    found: bool,
}

impl ProfileStats {
    /// The active profile's stats under [`set_context`]'s root: the file's,
    /// a fresh profile's when there is none, it is not one block long, or
    /// its `stat 299` is not `stat_version` 10 (0x579ac9). `None` with no
    /// context or no active profile.
    pub fn load() -> Option<(ProfileStats, Stats)> {
        let (root, fs_game) = CONTEXT.lock().unwrap().clone()?;
        Self::load_from(&root, &fs_game)
    }

    /// [`Self::load`] under `root` and `fs_game`.
    pub(crate) fn load_from(root: &Path, fs_game: &str) -> Option<(ProfileStats, Stats)> {
        let name = super::cod4_menus::front::active_profile(root)?;
        let dir = super::cod4_menus::front::profile_dir(root, &name)?;
        let path = if fs_game.is_empty() {
            dir.join(FILE)
        } else {
            dir.join(fs_game).join(FILE)
        };
        let read = std::fs::read(&path)
            .ok()
            .and_then(|b| <[u8; STATS_BYTES]>::try_from(b).ok())
            .map(|b| Stats(Box::new(b)))
            .filter(|s| s.get(299) == Some(10));
        let found = read.is_some();
        let stats = read.unwrap_or_else(Stats::fresh);
        log::info!("cod4: profile {name}'s stats from {}", path.display());
        let p = ProfileStats {
            dir,
            path,
            saved: stats.clone(),
            changed_at: None,
            found,
        };
        Some((p, stats))
    }

    /// The profile's directory.
    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    /// Whether the stats came from the file, not a fresh profile's.
    pub(crate) fn found(&self) -> bool {
        self.found
    }

    /// `stats` as the session holds them now: written once they have
    /// settled past a change, or at once with `now` (the disconnect).
    pub fn save(&mut self, stats: &Stats, now: bool) {
        if stats.0[4..] == self.saved.0[4..] || !self.dir.is_dir() {
            self.changed_at = None;
            return;
        }
        let since = *self.changed_at.get_or_insert_with(Instant::now);
        if !now && since.elapsed().as_secs() < SETTLE_SECS {
            return;
        }
        let mut sealed = stats.clone();
        sealed.seal();
        let written = self
            .path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|_| std::fs::write(&self.path, &sealed.0[..]));
        match written {
            Ok(()) => {
                log::info!("Successfully wrote stats data");
                self.saved = stats.clone();
                self.changed_at = None;
            }
            Err(e) => log::warn!("Unable to write stats: {} ({e}).", self.path.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A profile with no file joins on a fresh block; a change is written
    /// once settled (or at the disconnect) and read back by the next run.
    #[test]
    fn a_profiles_stats_outlive_the_run() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        std::fs::create_dir_all(root.join("players/profiles/tester")).unwrap();
        std::fs::write(root.join("players/profiles/active.txt"), "tester").unwrap();
        let file = root.join("players/profiles/tester").join(FILE);
        let (mut p, mut stats) = ProfileStats::load_from(root, "").unwrap();
        assert_eq!(stats.0[..], Stats::fresh().0[..]);
        stats.set(2301, 4200);
        p.save(&stats, false);
        assert!(!file.exists());
        p.save(&stats, true);
        let (_, again) = ProfileStats::load_from(root, "").unwrap();
        assert_eq!(again.get(2301), Some(4200));
        // A file of the wrong length reads as a fresh profile.
        std::fs::write(&file, [7u8; 16]).unwrap();
        let (_, fresh) = ProfileStats::load_from(root, "").unwrap();
        assert_eq!(fresh.0[..], Stats::fresh().0[..]);
    }
}
