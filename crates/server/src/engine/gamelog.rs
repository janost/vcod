//! `games_mp.log`: `G_LogPrintf` and the file `G_InitGame` opens, the
//! same on CoD1 1.1, 1.5, UO, CoD2 1.3 and CoD4 1.7 (CoD4 1.7 0x80b43c4,
//! docs/research/cod4-server-gaps.md, "games_mp.log"; CoD1 and CoD2 in
//! cod11-map-cycle.md 9). Each line is the level clock as `%3i:%i%i `
//! (minutes, then the seconds' two digits) and the text as raised; the
//! level clock is the server's (`G_InitGame` starts `level.time` at
//! `svs.time`), so the stamps run on across map changes. The game queues
//! its lines with their clock (`GameHost::log_printf`); each engine hands
//! them to its [`LevelLog`].

use std::io::Write;
use std::path::{Path, PathBuf};

/// The separator `G_InitGame` and `G_ShutdownGame` write (0x821ae48).
pub const SEPARATOR: &str = "------------------------------------------------------------\n";

/// The open log: `g_logSync 1` opens it `FS_APPEND_SYNC` (mode 3), each
/// line flushed; 0 `FS_APPEND` (mode 2), buffered until the close.
pub struct GameLog {
    out: std::io::BufWriter<std::fs::File>,
    sync: bool,
    pub path: PathBuf,
}

impl GameLog {
    /// `FS_FOpenFileByMode` of `g_log` in the write directory: the home
    /// path's `game` directory (`fs_game`, else the base game's), `main/`
    /// when `game` is empty.
    /// `None`, with retail's warning, when it cannot be opened.
    pub fn open(home: &Path, game: &str, name: &str, sync: bool) -> Option<GameLog> {
        let dir = home.join(if game.is_empty() { "main" } else { game });
        let path = dir.join(name);
        let file = std::fs::create_dir_all(&dir).and_then(|_| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
        });
        match file {
            Ok(f) => Some(GameLog {
                out: std::io::BufWriter::new(f),
                sync,
                path,
            }),
            Err(e) => {
                log::warn!("WARNING: Couldn't open logfile: {name} ({e})");
                None
            }
        }
    }

    /// `G_LogPrintf`: the stamp, then `text` as given (the caller's `\n`).
    pub fn print(&mut self, level_time_ms: i32, text: &str) {
        let line = format!("{}{text}", stamp(level_time_ms));
        if let Err(e) = self.out.write_all(line.as_bytes()) {
            log::warn!("{}: {e}", self.path.display());
        }
        if self.sync {
            let _ = self.out.flush();
        }
    }
}

impl Drop for GameLog {
    fn drop(&mut self) {
        let _ = self.out.flush();
    }
}

/// `G_ShutdownGame`'s line ending a level.
pub const SHUTDOWN: &str = "ShutdownGame:";
/// CoD1's `G_ShutdownGame(1)`, a `map_restart` (1.1, 1.5 and UO game
/// modules; VERIFIED live on 1.1d, cod11-map-cycle.md 9).
pub const RESTART: &str = "RestartGame:";

/// `level.logFile` over a level's life: [`LevelLog::init`] at
/// `G_InitGame`, [`LevelLog::write`] for the game's queued lines and
/// [`LevelLog::shutdown`] at `G_ShutdownGame`.
#[derive(Default)]
pub struct LevelLog {
    file: Option<GameLog>,
}

impl LevelLog {
    /// `G_InitGame`'s open: `g_log` empty prints `Not logging to disk.`;
    /// else the file opens under `<fs_homepath>/<game>` (`g_logSync`'s
    /// mode) and takes the separator and `InitGame: <serverinfo>` at `now`,
    /// the new `level.time`. With no home path (a test's server) nothing is
    /// written.
    pub fn init(&mut self, cvars: &crate::cvars::Cvars, game: &str, info: &str, now: i32) {
        self.file = None;
        let name = cvars.get("g_log");
        if name.is_empty() {
            log::info!("Not logging to disk.");
            return;
        }
        let home = cvars.get("fs_homepath");
        if home.is_empty() {
            return;
        }
        let sync = crate::game::builtins::cvar::atoi(cvars.get("g_logSync")) != 0;
        self.file = GameLog::open(Path::new(home), game, name, sync);
        if let Some(log) = self.file.as_mut() {
            log.print(now, SEPARATOR);
            log.print(now, &format!("InitGame: {info}\n"));
        }
    }

    /// The game's `G_LogPrintf` lines, each at its own clock; without a
    /// file they are dropped, as `G_LogPrintf` returns.
    pub fn write(&mut self, lines: Vec<(i32, String)>) {
        if let Some(log) = self.file.as_mut() {
            for (at, text) in lines {
                log.print(at, &text);
            }
        }
    }

    /// `G_ShutdownGame`: `lines` first, then `word` (`ShutdownGame:`, or
    /// CoD1's `RestartGame:` for a `map_restart`) and the separator at the
    /// level's last `level.time`, and the file closed.
    pub fn shutdown(&mut self, lines: Vec<(i32, String)>, at: i32, word: &str) {
        self.write(lines);
        if let Some(mut log) = self.file.take() {
            log.print(at, &format!("{word}\n"));
            log.print(at, SEPARATOR);
        }
    }

    /// The open file's path.
    pub fn path(&self) -> Option<&Path> {
        self.file.as_ref().map(|f| f.path.as_path())
    }
}

/// `%3i:%i%i `: whole minutes right-aligned in three, the seconds' tens and
/// ones (0x80b4403..0x80b4483, C division: toward zero).
pub fn stamp(level_time_ms: i32) -> String {
    let secs = level_time_ms / 1000;
    let (min, sec) = (secs / 60, secs - secs / 60 * 60);
    format!("{min:3}:{}{} ", sec / 10, sec % 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The retail log's stamps: `  0:43`, ` 10:58` (VERIFIED live).
    #[test]
    fn stamps_read_minutes_and_two_second_digits() {
        assert_eq!(stamp(0), "  0:00 ");
        assert_eq!(stamp(43_950), "  0:43 ");
        assert_eq!(stamp(658_000), " 10:58 ");
        assert_eq!(stamp(5_095_000), " 84:55 ");
    }

    #[test]
    fn opens_under_the_mod_or_main_and_appends() {
        let home = std::env::temp_dir().join(format!("vcod-gamelog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        for _ in 0..2 {
            let mut log = GameLog::open(&home, "", "games_mp.log", false).unwrap();
            log.print(1000, "x\n");
        }
        let text = std::fs::read_to_string(home.join("main/games_mp.log")).unwrap();
        assert_eq!(text, "  0:01 x\n  0:01 x\n");
        let log = GameLog::open(&home, "mods/m", "games_mp.log", true).unwrap();
        assert_eq!(log.path, home.join("mods/m/games_mp.log"));
        let _ = std::fs::remove_dir_all(&home);
    }
}
