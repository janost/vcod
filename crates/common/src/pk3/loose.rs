//! The loose files under a search path's directories, and which of them
//! `FS_FOpenFileRead` takes: every one on an unpure path, a few types on a
//! pure one, CoD4's `main/` always as a pure path
//! (docs/research/cod11-server-handshake.md, "Loose files";
//! cod2-iwi-iwd.md 1; cod4-handshake.md 14.1).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Whose directory test applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Rule {
    /// No loose files at all: `--vanilla` and the stock trees.
    Off,
    /// CoD1 1.1 (CoDMP.exe 0x429d1a): a pure path reads a name ending in
    /// `.cfg`, `.menu`, `.svg`, `.game`, `.dm_PROTOCOL_VERSION` or `.dat`.
    Cod11,
    /// 1.5 and United Offensive (CoDMP.exe 0x42bc00, CoDUOMP.exe 0x42d6f0):
    /// the extension `cfg`, `menu` or `dat`.
    Cod15,
    /// CoD2 1.3 (CoD2MP_s.exe 0x422530): `cfg` or `menu`.
    Cod2,
    /// CoD4 1.7 (iw3mp.exe 0x55b770, cod4_lnxded 0x8186928): `ban.txt` or
    /// `cfg`, and `main/` takes that test on any path (iw3mp.exe 0x55df87).
    Cod4,
}

/// One loose file: its directory's index in the search path, lowest
/// first, its path on disk and its name under the directory as spelled.
pub(super) struct File {
    pub dir: usize,
    pub path: PathBuf,
    pub name: String,
}

pub(super) struct Loose {
    rule: Rule,
    pure: bool,
    /// Parallel to the directories: CoD4's `main`.
    main: Vec<bool>,
    /// Lookup key -> the files under it, the highest directory first.
    files: HashMap<String, Vec<File>>,
}

/// Deeper trees and more files than this are left unindexed: a loose tree
/// that size is a mistake, not game data.
const MAX_DEPTH: usize = 16;
const MAX_FILES: usize = 1 << 17;

/// The extension `FS_FOpenFileRead`'s test reads: what follows the last
/// `.` of the last path component, empty without one (CoDUOMP.exe 0x42d6b0).
fn extension(key: &str) -> &str {
    let file = key.rsplit('/').next().unwrap_or(key);
    file.rfind('.').map_or("", |i| &file[i + 1..])
}

/// CoD4 1.7's test for a loose file off `main/` or a pure path.
pub(super) fn cod4_main_reads(key: &str) -> bool {
    key == "ban.txt" || extension(key) == "cfg"
}

impl Loose {
    pub fn off() -> Self {
        Loose {
            rule: Rule::Off,
            pure: false,
            main: Vec::new(),
            files: HashMap::new(),
        }
    }

    /// Every file under `dirs` (lowest priority first) but `skip_ext`'s
    /// archives, which the search path opens as archives.
    pub fn scan(dirs: &[PathBuf], skip_ext: &str, rule: Rule, pure: bool) -> Self {
        let mut loose = Loose {
            main: dirs
                .iter()
                .map(|d| {
                    rule == Rule::Cod4
                        && d.file_name()
                            .is_some_and(|n| n.eq_ignore_ascii_case("main"))
                })
                .collect(),
            rule,
            pure,
            files: HashMap::new(),
        };
        if rule == Rule::Off {
            return loose;
        }
        let mut count = 0;
        for (i, dir) in dirs.iter().enumerate().rev() {
            loose.walk(i, dir, "", 0, skip_ext, &mut count);
        }
        if count >= MAX_FILES {
            log::warn!("more than {MAX_FILES} loose files on the search path; indexed the first");
        }
        loose
    }

    fn walk(&mut self, i: usize, dir: &Path, rel: &str, depth: usize, skip: &str, n: &mut usize) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            if *n >= MAX_FILES {
                return;
            }
            let Some(name) = e.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let name = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            let path = e.path();
            // Follows a link, as the engine's fopen does.
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                if depth < MAX_DEPTH {
                    self.walk(i, &path, &name, depth + 1, skip, n);
                }
                continue;
            }
            if depth == 0 && extension(&name).eq_ignore_ascii_case(skip) {
                continue;
            }
            *n += 1;
            let key = super::path_key(&name);
            self.files
                .entry(key)
                .or_default()
                .push(File { dir: i, path, name });
        }
    }

    /// Whether a read of `key` takes a loose file in directory `dir`.
    fn allowed(&self, dir: usize, key: &str) -> bool {
        let ext = extension(key);
        let held = self.pure || self.main.get(dir).copied().unwrap_or(false);
        match self.rule {
            Rule::Off => false,
            _ if !held => true,
            Rule::Cod11 => [
                ".cfg",
                ".menu",
                ".svg",
                ".game",
                ".dm_protocol_version",
                ".dat",
            ]
            .iter()
            .any(|e| key.ends_with(e)),
            Rule::Cod15 => ["cfg", "menu", "dat"].contains(&ext),
            Rule::Cod2 => ["cfg", "menu"].contains(&ext),
            Rule::Cod4 => cod4_main_reads(key),
        }
    }

    /// Whether a loose read of `key` sets `fs_fakeChkSum`: a type off the
    /// list, 1.1's being its pure one less `.svg` (CoDMP.exe 0x42a0b3), 1.5's
    /// and UO's their pure test (CoDMP.exe 0x42c293, CoDUOMP.exe 0x42ddf5).
    /// CoD2's and CoD4's `cp` are built elsewhere and carry none.
    pub fn fakes_checksum(&self, key: &str) -> bool {
        match self.rule {
            Rule::Cod11 => ![".cfg", ".menu", ".game", ".dm_protocol_version", ".dat"]
                .iter()
                .any(|e| key.ends_with(e)),
            Rule::Cod15 => !["cfg", "menu", "dat"].contains(&extension(key)),
            Rule::Off | Rule::Cod2 | Rule::Cod4 => false,
        }
    }

    /// Whose clear points the fake checksum keeps: true for 1.1's.
    pub fn cod11(&self) -> bool {
        self.rule == Rule::Cod11
    }

    /// The file a read of `key` takes from the directories, if any.
    pub fn readable(&self, key: &str) -> Option<&File> {
        self.files
            .get(key)?
            .iter()
            .find(|f| self.allowed(f.dir, key))
    }

    /// The keys [`Self::readable`] finds.
    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.files.keys().filter(|k| self.readable(k).is_some())
    }

    /// Whether `FS_ListFilteredFiles` lists the directories for `ext`: on
    /// an unpure path, and on a pure CoD1 one for saved games (CoDMP.exe
    /// 0x42b59c, CoDUOMP.exe 0x42f194; CoD2MP_s.exe 0x423d60 and iw3mp.exe
    /// 0x55cf7a list them only for a caller that asks, which no list does).
    pub fn lists(&self, ext: &str) -> bool {
        match self.rule {
            Rule::Off => false,
            _ if !self.pure => true,
            Rule::Cod11 | Rule::Cod15 => ext.trim_start_matches('.').eq_ignore_ascii_case("svg"),
            Rule::Cod2 | Rule::Cod4 => false,
        }
    }

    /// The names directly under `prefix` (lowercased, ending in `/`) in
    /// directory `dir`, as spelled, without the prefix; sorted case-blind
    /// as a Windows directory lists.
    pub fn in_dir(&self, dir: usize, prefix: &str) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .files
            .iter()
            .filter(|(k, _)| k.strip_prefix(prefix).is_some_and(|r| !r.contains('/')))
            .flat_map(|(_, fs)| fs.iter().filter(|f| f.dir == dir))
            .map(|f| &f.name[prefix.len()..])
            .collect();
        names.sort_by_cached_key(|n| n.to_lowercase());
        names
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loose(rule: Rule, pure: bool, dirs: usize) -> Loose {
        Loose {
            rule,
            pure,
            main: vec![false; dirs],
            files: HashMap::new(),
        }
    }

    #[test]
    fn a_pure_path_takes_each_games_types() {
        let names = [
            "a.cfg",
            "ui/b.menu",
            "save/c.svg",
            "d.game",
            "e.dat",
            "f.gsc",
            "ban.txt",
            "g.dm_1",
            "h.dm_protocol_version",
            "i.cfg/j",
        ];
        let take = |l: &Loose| -> Vec<&str> {
            names.iter().copied().filter(|n| l.allowed(0, n)).collect()
        };
        assert_eq!(
            take(&loose(Rule::Cod11, true, 1)),
            [
                "a.cfg",
                "ui/b.menu",
                "save/c.svg",
                "d.game",
                "e.dat",
                "h.dm_protocol_version"
            ]
        );
        assert_eq!(
            take(&loose(Rule::Cod15, true, 1)),
            ["a.cfg", "ui/b.menu", "e.dat"]
        );
        assert_eq!(take(&loose(Rule::Cod2, true, 1)), ["a.cfg", "ui/b.menu"]);
        assert_eq!(take(&loose(Rule::Cod4, true, 1)), ["a.cfg", "ban.txt"]);
        for rule in [Rule::Cod11, Rule::Cod15, Rule::Cod2, Rule::Cod4] {
            assert_eq!(take(&loose(rule, false, 1)).len(), names.len(), "{rule:?}");
        }
        assert!(take(&loose(Rule::Off, false, 1)).is_empty());
    }

    #[test]
    fn an_off_list_type_fakes_the_checksum() {
        let names = [
            "a.cfg", "b.menu", "c.svg", "d.game", "e.dat", "f.tga", "g.dm_1",
        ];
        let fakes = |l: &Loose| -> Vec<&str> {
            names
                .iter()
                .copied()
                .filter(|n| l.fakes_checksum(n))
                .collect()
        };
        assert_eq!(
            fakes(&loose(Rule::Cod11, false, 1)),
            ["c.svg", "f.tga", "g.dm_1"]
        );
        assert_eq!(
            fakes(&loose(Rule::Cod15, false, 1)),
            ["c.svg", "d.game", "f.tga", "g.dm_1"]
        );
        for rule in [Rule::Off, Rule::Cod2, Rule::Cod4] {
            assert!(fakes(&loose(rule, false, 1)).is_empty(), "{rule:?}");
        }
    }

    #[test]
    fn cod4s_main_reads_as_a_pure_path_does() {
        let mut l = loose(Rule::Cod4, false, 2);
        l.main[0] = true;
        assert!(l.allowed(0, "x.cfg"));
        assert!(!l.allowed(0, "images/x.iwi"));
        assert!(l.allowed(1, "images/x.iwi"));
    }
}
