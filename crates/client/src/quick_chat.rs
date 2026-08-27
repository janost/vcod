//! Quick chat: the `j`/`k`/`l` server commands resolved against the
//! mod-provided `.voice` tables, queued and drained at retail's
//! one-line-per-second cadence (docs/research/cod11-quick-chat.md).
//! Without those files this is inert, exactly like stock retail.

use std::collections::VecDeque;

use glam::Vec3;
use vcod_common::pk3::Pk3Fs;
use vcod_common::voicechat::{self, VoiceChat};

use crate::audio::cues::{Cue, Source};
use crate::fx::sim::Rng;

const AXIS_FILE: &str = "mp/axis_chat.voice";
const ALLIES_FILE: &str = "mp/allies_chat.voice";
/// Retail's ring holds 32 entries.
const QUEUE_CAP: usize = 32;
/// `FUN_3002d5d0`: one entry per 1000 ms.
const DRAIN_INTERVAL_S: f32 = 1.0;
/// Retail reads the file through a 0x4000-byte cap ("voice chat file too large").
const FILE_LIMIT: u64 = 0x4000;

struct Tables {
    axis: Option<VoiceChat>,
    allies: Option<VoiceChat>,
}

struct Queued {
    client_num: u32,
    pos: Vec3,
    alias: String,
    text: String,
}

/// One drained line: play the cue, show the text as `<name>: <text>`.
pub struct DrainedLine {
    pub client_num: u32,
    pub cue: Cue,
    pub text: String,
}

pub struct QuickChat {
    tables: Option<Tables>,
    queue: VecDeque<Queued>,
    next_due_s: f32,
    rng: Rng,
}

impl QuickChat {
    pub fn new(seed: u64) -> QuickChat {
        QuickChat {
            tables: None,
            queue: VecDeque::new(),
            next_due_s: 0.0,
            rng: Rng::new(seed),
        }
    }

    /// The `j`/`k`/`l` handler; true when the command was quick chat. argv
    /// layout per the research doc: `[letter, scope, ?, clientNum, category,
    /// originX, originY, originZ]`.
    pub fn on_server_command(
        &mut self,
        fs: &Pk3Fs,
        tokens: &[String],
        team_of: impl Fn(u32) -> Option<i32>,
    ) -> bool {
        match tokens.first().map(String::as_str) {
            Some("j") | Some("k") | Some("l") => {}
            _ => return false,
        }
        let int = |i: usize| -> i32 {
            tokens
                .get(i)
                .and_then(|t| t.trim().parse().ok())
                .unwrap_or(0)
        }; // Retail clamps to its 64-slot clientinfo array.
        let client_num = int(3).clamp(0, 63) as u32;
        let pos = Vec3::new(int(5) as f32, int(6) as f32, int(7) as f32);
        let Some(category) = tokens.get(4) else {
            return true;
        };

        let tables = self.tables.get_or_insert_with(|| load_tables(fs));
        // Retail keeps the axis table when clientinfo.team == 1, allies otherwise.
        let table = match team_of(client_num) {
            Some(1) => tables.axis.as_ref(),
            _ => tables.allies.as_ref(),
        };
        let Some(variants) = table
            .and_then(|t| t.category(category))
            .map(|c| &c.variants)
        else {
            log::debug!("quick chat: no variants for {category:?}");
            return true;
        };
        if variants.is_empty() {
            return true;
        }
        let idx = (self.rng.next_u64() % variants.len() as u64) as usize;
        self.queue.push_back(Queued {
            client_num,
            pos,
            alias: variants[idx].alias.clone(),
            text: category.clone(),
        });
        while self.queue.len() > QUEUE_CAP {
            self.queue.pop_front();
        }
        true
    }

    /// At most one line per second, oldest first. The first line is immediate.
    pub fn drain(&mut self, now: f32) -> Option<DrainedLine> {
        if now < self.next_due_s || self.queue.is_empty() {
            return None;
        }
        self.next_due_s = now + DRAIN_INTERVAL_S;
        let q = self.queue.pop_front()?;
        Some(DrainedLine {
            client_num: q.client_num,
            text: q.text,
            cue: Cue {
                alias: q.alias,
                source: Source::Entity {
                    num: q.client_num,
                    pos: q.pos,
                },
                delay_s: 0.0,
            },
        })
    }

    #[cfg(test)]
    fn with_tables(allies: Option<VoiceChat>, axis: Option<VoiceChat>) -> QuickChat {
        let mut qc = QuickChat::new(1);
        qc.tables = Some(Tables { allies, axis });
        qc
    }
}

fn load_tables(fs: &Pk3Fs) -> Tables {
    Tables {
        axis: load_table(fs, AXIS_FILE),
        allies: load_table(fs, ALLIES_FILE),
    }
}

fn load_table(fs: &Pk3Fs, path: &str) -> Option<VoiceChat> {
    // Called once per session, so these warns fire once like retail's do.
    let Some(data) = fs.read(path) else {
        log::warn!("audio: {path} not found; quick chat stays inert (stock parity)");
        return None;
    };
    if data.len() as u64 > FILE_LIMIT {
        log::warn!(
            "audio: {path} is {} bytes, over the {} cap; ignored",
            data.len(),
            FILE_LIMIT
        );
        return None;
    }
    match std::str::from_utf8(&data)
        .map_err(|e| e.to_string())
        .and_then(voicechat::parse)
    {
        Ok(v) => Some(v),
        Err(e) => {
            log::warn!("audio: {path}: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::voicechat::Category;

    fn cat(name: &str, aliases: &[&str]) -> Category {
        Category {
            name: name.to_string(),
            variants: aliases
                .iter()
                .map(|a| voicechat::Variant {
                    alias: a.to_string(),
                    head_icon: None,
                })
                .collect(),
        }
    }

    fn vc(categories: Vec<Category>) -> VoiceChat {
        VoiceChat {
            gender: voicechat::Gender::Male,
            categories,
        }
    }

    fn cmd(letter: &str, client: i32, category: &str) -> Vec<String> {
        [
            letter,
            "0",
            "0",
            &client.to_string(),
            category,
            "10",
            "20",
            "30",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    #[test]
    fn consumes_jkl_and_leaves_other_commands() {
        let mut qc = QuickChat::with_tables(Some(vc(vec![])), None);
        assert!(!qc.on_server_command(&fs(), &["s".to_string(), "5".to_string()], |_| None));
        assert!(qc.on_server_command(&fs(), &cmd("j", 2, "praise"), |_| None));
        assert!(qc.on_server_command(&fs(), &cmd("l", 2, "praise"), |_| None));
    }

    // Every test here injects its tables, so the fs is never read.
    fn fs() -> Pk3Fs {
        Pk3Fs::empty()
    }

    #[test]
    fn speaker_team_picks_the_table() {
        let mut qc = QuickChat::with_tables(
            Some(vc(vec![cat("praise", &["allies_snd"])])),
            Some(vc(vec![cat("praise", &["axis_snd"])])),
        );
        assert!(qc.on_server_command(&fs(), &cmd("j", 3, "praise"), |n| {
            if n == 3 {
                Some(1)
            } else {
                None
            }
        }));
        assert_eq!(qc.drain(0.0).unwrap().cue.alias, "axis_snd");

        let mut qc = QuickChat::with_tables(
            Some(vc(vec![cat("praise", &["allies_snd"])])),
            Some(vc(vec![cat("praise", &["axis_snd"])])),
        );
        assert!(qc.on_server_command(&fs(), &cmd("j", 3, "praise"), |n| {
            if n == 3 {
                Some(2)
            } else {
                None
            }
        }));
        assert_eq!(qc.drain(0.0).unwrap().cue.alias, "allies_snd");
    }

    #[test]
    fn missing_category_is_inert_but_consumed() {
        let mut qc = QuickChat::with_tables(Some(vc(vec![])), None);
        assert!(qc.on_server_command(&fs(), &cmd("j", 0, "nothing"), |_| None));
        assert!(qc.drain(0.0).is_none());
    }

    #[test]
    fn drain_is_one_per_second() {
        let mut qc = QuickChat::with_tables(Some(vc(vec![cat("c", &["a", "b", "c"])])), None);
        for n in 0..3 {
            assert!(qc.on_server_command(&fs(), &cmd("j", n, "c"), |_| None));
        }
        assert_eq!(qc.queue.len(), 3);
        assert!(qc.drain(0.0).is_some(), "first line immediate");
        assert!(qc.drain(0.5).is_none());
        assert!(qc.drain(0.999).is_none());
        assert!(qc.drain(1.0).is_some());
        assert!(qc.drain(1.5).is_none());
        assert!(qc.drain(2.0).is_some());
        assert!(qc.drain(99.0).is_none(), "queue empty");
    }

    #[test]
    fn clamped_client_and_origin_land_in_the_cue() {
        let mut qc = QuickChat::with_tables(Some(vc(vec![cat("c", &["a"])])), None);
        assert!(qc.on_server_command(&fs(), &cmd("j", 500, "c"), |_| None));
        let line = qc.drain(0.0).unwrap();
        assert_eq!(line.client_num, 63);
        assert_eq!(
            line.cue.source,
            Source::Entity {
                num: 63,
                pos: Vec3::new(10.0, 20.0, 30.0)
            }
        );
    }

    #[test]
    fn missing_files_warn_once_and_stay_inert() {
        // No stock install ships .voice files (research doc), so a real game
        // dir exercises the inert path; without one there is nothing to test.
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut qc = QuickChat::new(1);
        assert!(qc.on_server_command(&fs, &cmd("j", 0, "praise"), |_| None));
        assert!(qc.drain(0.0).is_none());
        // Tables cached: a second command does not retry the load.
        assert!(qc.on_server_command(&fs, &cmd("j", 0, "praise"), |_| None));
        assert!(qc.drain(0.0).is_none());
    }
}
