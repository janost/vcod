//! Download-and-load state machine for one map. Pure: the caller pumps the
//! net client, feeds the events in, and performs the returned `Action`.
//! Replaces the blocking loops that ran before the window existed.

use std::path::PathBuf;
use std::time::{Duration, Instant};
use vcod_common::net::NetEvent;

/// Bytes so far of pak `pak` (1-based) of `paks`; `size` is 0 until the
/// first block names it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Progress {
    pub pak: usize,
    pub paks: usize,
    pub received: u64,
    pub size: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    BeginDownload {
        remote: String,
        dest: PathBuf,
    },
    /// A pak landed: reopen the pk3 search path, then step again.
    Reopen,
    /// Send `donedl`; the server answers with the gamestate again.
    FinishDownloads,
    /// The map resolves: parse and load it.
    Ready,
    Wait(Option<Progress>),
    Failed(String),
}

/// The retail client gives up on a stalled download after this long.
const STALL: Duration = Duration::from_secs(30);
/// How long to wait for the re-sent gamestate after `donedl`.
const REGAMESTATE: Duration = Duration::from_secs(10);
/// Referenced paks tried per map, as before.
const MAX_CANDIDATES: usize = 4;

enum State {
    Resolve,
    Downloading {
        idx: usize,
        last_progress: Instant,
        received: u64,
    },
    Reopening,
    AwaitGamestate {
        since: Instant,
    },
    Done,
}

pub struct MapLoader {
    map: String,
    candidates: Vec<(String, PathBuf)>,
    next: usize,
    downloaded: usize,
    state: State,
}

impl MapLoader {
    /// `candidates` are `(remote name, local path)` pairs the server
    /// references and the client lacks, in the order to try them.
    pub fn new(map: String, mut candidates: Vec<(String, PathBuf)>) -> Self {
        candidates.truncate(MAX_CANDIDATES);
        MapLoader {
            map,
            candidates,
            next: 0,
            downloaded: 0,
            state: State::Resolve,
        }
    }

    pub fn map(&self) -> &str {
        &self.map
    }

    /// `events` are this frame's pumped net events, `progress` the net
    /// client's `download_progress()`, `map_resolves` whether the pk3 search
    /// path finds the map right now.
    pub fn step(
        &mut self,
        events: &[NetEvent],
        progress: Option<(u64, u32)>,
        map_resolves: bool,
        now: Instant,
    ) -> Action {
        for ev in events {
            if let NetEvent::Dropped(why) = ev {
                self.state = State::Done;
                return Action::Failed(format!("disconnected: {why}"));
            }
        }
        match self.state {
            State::Resolve => {
                if map_resolves {
                    if self.downloaded > 0 {
                        self.state = State::AwaitGamestate { since: now };
                        return Action::FinishDownloads;
                    }
                    self.state = State::Done;
                    return Action::Ready;
                }
                let Some((remote, dest)) = self.candidates.get(self.next).cloned() else {
                    self.state = State::Done;
                    return Action::Failed(format!("map {} is not on the server", self.map));
                };
                let idx = self.next;
                self.next += 1;
                self.state = State::Downloading {
                    idx,
                    last_progress: now,
                    received: 0,
                };
                Action::BeginDownload { remote, dest }
            }
            State::Downloading {
                idx,
                ref mut last_progress,
                ref mut received,
            } => {
                if events
                    .iter()
                    .any(|e| matches!(e, NetEvent::DownloadComplete(_)))
                {
                    self.downloaded += 1;
                    self.state = State::Reopening;
                    return Action::Reopen;
                }
                let (got, size) = progress.unwrap_or((0, 0));
                if got > *received {
                    *received = got;
                    *last_progress = now;
                }
                if now.duration_since(*last_progress) > STALL {
                    self.state = State::Done;
                    return Action::Failed(format!(
                        "download of {} stalled",
                        self.candidates[idx].0
                    ));
                }
                Action::Wait(Some(Progress {
                    pak: idx + 1,
                    paks: self.candidates.len(),
                    received: got,
                    size,
                }))
            }
            State::Reopening => {
                // the caller reopened before this step; decide on the new fs
                self.state = State::Resolve;
                self.step(&[], None, map_resolves, now)
            }
            State::AwaitGamestate { since } => {
                if events.iter().any(|e| matches!(e, NetEvent::GamestateReady)) {
                    self.state = State::Done;
                    return Action::Ready;
                }
                if now.duration_since(since) > REGAMESTATE {
                    self.state = State::Done;
                    return Action::Failed("no gamestate after the download".into());
                }
                Action::Wait(None)
            }
            State::Done => Action::Wait(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cands(n: usize) -> Vec<(String, PathBuf)> {
        (0..n)
            .map(|i| {
                (
                    format!("main/zzz_{i}.pk3"),
                    PathBuf::from(format!("/tmp/zzz_{i}.pk3")),
                )
            })
            .collect()
    }

    #[test]
    fn a_present_map_is_ready_at_once() {
        let mut l = MapLoader::new("mp_x".into(), vec![]);
        assert_eq!(l.step(&[], None, true, Instant::now()), Action::Ready);
    }

    #[test]
    fn no_candidates_and_no_map_fails() {
        let mut l = MapLoader::new("mp_x".into(), vec![]);
        assert!(matches!(
            l.step(&[], None, false, Instant::now()),
            Action::Failed(_)
        ));
    }

    #[test]
    fn one_pak_downloads_then_waits_for_the_gamestate() {
        let t0 = Instant::now();
        let mut l = MapLoader::new("mp_x".into(), cands(1));
        assert_eq!(
            l.step(&[], None, false, t0),
            Action::BeginDownload {
                remote: "main/zzz_0.pk3".into(),
                dest: PathBuf::from("/tmp/zzz_0.pk3")
            }
        );
        assert_eq!(
            l.step(&[], Some((1000, 5000)), false, t0 + Duration::from_secs(1)),
            Action::Wait(Some(Progress {
                pak: 1,
                paks: 1,
                received: 1000,
                size: 5000
            }))
        );
        let done = [NetEvent::DownloadComplete("main/zzz_0.pk3".into())];
        assert_eq!(
            l.step(
                &done,
                Some((5000, 5000)),
                false,
                t0 + Duration::from_secs(2)
            ),
            Action::Reopen
        );
        assert_eq!(
            l.step(&[], None, true, t0 + Duration::from_secs(2)),
            Action::FinishDownloads
        );
        assert_eq!(
            l.step(&[], None, true, t0 + Duration::from_secs(3)),
            Action::Wait(None)
        );
        assert_eq!(
            l.step(
                &[NetEvent::GamestateReady],
                None,
                true,
                t0 + Duration::from_secs(4)
            ),
            Action::Ready
        );
    }

    #[test]
    fn a_pak_without_the_map_moves_to_the_next_candidate() {
        let t0 = Instant::now();
        let mut l = MapLoader::new("mp_x".into(), cands(2));
        assert!(matches!(
            l.step(&[], None, false, t0),
            Action::BeginDownload { .. }
        ));
        let done = [NetEvent::DownloadComplete("main/zzz_0.pk3".into())];
        assert_eq!(l.step(&done, None, false, t0), Action::Reopen);
        assert_eq!(
            l.step(&[], None, false, t0),
            Action::BeginDownload {
                remote: "main/zzz_1.pk3".into(),
                dest: PathBuf::from("/tmp/zzz_1.pk3")
            }
        );
    }

    #[test]
    fn a_stalled_download_fails() {
        let t0 = Instant::now();
        let mut l = MapLoader::new("mp_x".into(), cands(1));
        l.step(&[], None, false, t0);
        l.step(&[], Some((10, 100)), false, t0 + Duration::from_secs(1));
        assert!(matches!(
            l.step(&[], Some((10, 100)), false, t0 + Duration::from_secs(32)),
            Action::Failed(_)
        ));
    }

    #[test]
    fn a_drop_fails_in_any_state() {
        let t0 = Instant::now();
        let mut l = MapLoader::new("mp_x".into(), cands(1));
        l.step(&[], None, false, t0);
        assert!(matches!(
            l.step(&[NetEvent::Dropped("bye".into())], None, false, t0),
            Action::Failed(_)
        ));
    }

    #[test]
    fn no_gamestate_after_donedl_fails() {
        let t0 = Instant::now();
        let mut l = MapLoader::new("mp_x".into(), cands(1));
        l.step(&[], None, false, t0);
        l.step(
            &[NetEvent::DownloadComplete("main/zzz_0.pk3".into())],
            None,
            false,
            t0,
        );
        assert_eq!(l.step(&[], None, true, t0), Action::FinishDownloads);
        assert!(matches!(
            l.step(&[], None, true, t0 + Duration::from_secs(11)),
            Action::Failed(_)
        ));
    }
}
