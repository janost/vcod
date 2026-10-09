//! pk3 downloads over the Q3/RTCW UDP download protocol (docs/protocol-1.1.md,
//! svc_download). Pak bookkeeping and the file spool; the wire handling is on
//! `NetClient`.

use anyhow::{Context, bail};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Pak names from `sv_referencedPakNames`, e.g. `main/bellicourt_v1_1`.
pub fn referenced_pak_names(systeminfo: &str) -> Vec<String> {
    super::info_value_for_key(systeminfo, "sv_referencedPakNames")
        .map(|v| v.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

/// Paks the server refuses to serve (`FS_idPak`): `main/pak0`..`9`, `localized_*`.
pub fn is_stock_pak(name: &str) -> bool {
    let base = name.rsplit('/').next().unwrap_or(name);
    if base.starts_with("localized_") {
        return true;
    }
    matches!(name.strip_prefix("main/pak"),
             Some(d) if d.len() == 1 && d.bytes().all(|b| b.is_ascii_digit()))
}

/// A directory or file name a server may name: ASCII letters, digits, `_`,
/// `-` and `.`, not empty and not hidden, so it cannot climb out of its parent.
fn safe_name(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('.')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
}

/// Validate a server-supplied pak name into `<dir>/<file>.pk3`. The name gets
/// joined onto a local directory, so traversal and odd characters are
/// rejected, and the directory must be one of `dirs` (the client's base dir
/// and the connection's `fs_game`) so a server cannot drop files anywhere
/// else under the install.
pub fn safe_rel_path(name: &str, dirs: &[&str]) -> Option<PathBuf> {
    let (dir, file) = name.split_once('/')?;
    if !dirs.contains(&dir) || !safe_name(file) {
        return None;
    }
    Some(PathBuf::from(dir).join(format!("{file}.pk3")))
}

/// The systeminfo's `fs_game`, which `CL_SystemInfoChanged` sets on the
/// client (docs/research/cod11-front-end.md, section 16). `None` when unset,
/// when it names `base` itself, or when it is not a plain directory name.
pub fn fs_game(systeminfo: &str, base: &str) -> Option<String> {
    let v = super::info_value_for_key(systeminfo, "fs_game")?;
    (safe_name(v) && !v.eq_ignore_ascii_case(base)).then(|| v.to_string())
}

/// The systeminfo's `cl_allowDownload`, which the server forces on the
/// client; `None` when the server leaves it to the client.
pub fn server_allows_download(systeminfo: &str) -> Option<bool> {
    super::info_value_for_key(systeminfo, "cl_allowDownload")
        .map(|v| v.trim().parse::<i32>().unwrap_or(0) != 0)
}

/// One `sv_referencedPakNames` entry with its `sv_referencedPaks` checksum
/// (`None` when the server's lists don't pair up).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReferencedPak {
    pub name: String,
    pub checksum: Option<i32>,
}

/// The systeminfo's referenced paks, names paired with checksums.
pub fn referenced_paks(systeminfo: &str) -> Vec<ReferencedPak> {
    let sums = checksum_list(systeminfo, "sv_referencedPaks");
    let names = referenced_pak_names(systeminfo);
    let paired = sums.len() == names.len();
    names
        .into_iter()
        .enumerate()
        .map(|(i, name)| ReferencedPak {
            name,
            checksum: paired.then(|| sums[i]),
        })
        .collect()
}

/// `sv_paks`, the checksums a pure server lets the client load; `None` when
/// the server is not pure (the list is empty or absent).
pub fn pure_paks(systeminfo: &str) -> Option<Vec<i32>> {
    let sums = checksum_list(systeminfo, "sv_paks");
    (!sums.is_empty()).then_some(sums)
}

/// A space-separated list of signed checksums (`"%i "` each).
fn checksum_list(systeminfo: &str, key: &str) -> Vec<i32> {
    super::info_value_for_key(systeminfo, key)
        .map(|v| {
            v.split_whitespace()
                .map(|t| t.parse::<i64>().map_or(0, |n| n as i32))
                .collect()
        })
        .unwrap_or_default()
}

/// What a client lacks of one referenced pak: `None` when it has it, else the
/// relative path the download lands in.
///
/// `FS_ComparePaks` (CoDMP.exe 0x43b830) skips stock paks and calls a pak
/// present when any loaded pak has its checksum, whatever the name. When a
/// file of that name exists with another checksum, the download is saved as
/// `<name>.<checksum %08x>.pk3` instead. `have` asks whether a local pak has
/// a checksum, `exists` whether a relative path exists. A server whose lists
/// don't pair up is matched by name.
fn lacking(
    pak: &ReferencedPak,
    dirs: &[&str],
    have: &impl Fn(i32) -> bool,
    exists: &impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    if is_stock_pak(&pak.name) {
        return None;
    }
    let rel = safe_rel_path(&pak.name, dirs)?;
    match pak.checksum {
        Some(c) if have(c) => None,
        Some(c) if exists(&rel) => {
            let file = pak.name.rsplit('/').next().unwrap_or(&pak.name);
            Some(rel.with_file_name(format!("{file}.{:08x}.pk3", c as u32)))
        }
        Some(_) => Some(rel),
        None => (!exists(&rel)).then_some(rel),
    }
}

/// Referenced non-stock paks the client lacks, `name.pk3` each: what
/// retail's "You are missing some files" warning lists when it may not
/// download them. A pak in a directory the client won't write to counts as
/// missing.
pub fn missing_paks(
    systeminfo: &str,
    dirs: &[&str],
    have: impl Fn(i32) -> bool,
    exists: impl Fn(&Path) -> bool,
) -> Vec<String> {
    referenced_paks(systeminfo)
        .into_iter()
        .filter(|p| !is_stock_pak(&p.name))
        .filter(|p| {
            safe_rel_path(&p.name, dirs).is_none() || lacking(p, dirs, &have, &exists).is_some()
        })
        .map(|p| format!("{}.pk3", p.name))
        .collect()
}

/// Referenced paks worth downloading for `map`, most likely first, each with
/// the relative path it lands in (see `lacking`); paks sharing a name token
/// with the map sort ahead of the rest.
pub fn candidates_for_map(
    systeminfo: &str,
    map: &str,
    dirs: &[&str],
    have: impl Fn(i32) -> bool,
    exists: impl Fn(&Path) -> bool,
) -> Vec<(String, PathBuf)> {
    // Pak names rarely match the map exactly (`main/n_dufresne` holds
    // `dufresne_final`, `main/BunkerCourt2AvsG` holds `BunkerCourt2_A_vs_G`),
    // so match on a shared `_`-separated token or on the names with their
    // separators dropped.
    let map = map.to_lowercase();
    let map_tokens: Vec<&str> = map.split('_').filter(|t| t.len() >= 4).collect();
    let squash = |s: &str| s.replace(['_', '-'], "");
    let map_flat = squash(map.strip_prefix("mp_").unwrap_or(&map));
    let mut like_map = Vec::new();
    let mut rest = Vec::new();
    for pak in referenced_paks(systeminfo) {
        let Some(dest) = lacking(&pak, dirs, &have, &exists) else {
            continue;
        };
        let name = pak.name;
        let base = name.rsplit('/').next().unwrap_or(&name).to_lowercase();
        let base_flat = squash(&base);
        let entry = (name.clone(), dest);
        if base
            .split('_')
            .any(|t| t.len() >= 4 && map_tokens.contains(&t))
            || (map_flat.len() >= 4 && base_flat.contains(&map_flat))
        {
            like_map.push(entry);
        } else {
            rest.push(entry);
        }
    }
    like_map.extend(rest);
    like_map
}

/// One transfer, spooled into `<dest>.tmp` until the EOF block validates it as
/// a zip and renames it into place. Never overwrites an existing destination.
pub struct Download {
    /// As requested, e.g. `main/foo.pk3`.
    pub remote: String,
    dest: PathBuf,
    tmp: PathBuf,
    file: File,
    /// Next block accepted; retransmits are ignored.
    pub next_block: u16,
    /// From block 0; 0 until then.
    pub size: u32,
    pub received: u64,
}

impl Download {
    pub fn create(remote: &str, dest: &Path) -> anyhow::Result<Self> {
        if dest.exists() {
            bail!("{} already exists, refusing to overwrite", dest.display());
        }
        if let Some(dir) = dest.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("cannot create {}", dir.display()))?;
        }
        let tmp = dest.with_extension("pk3.tmp");
        let file =
            File::create(&tmp).with_context(|| format!("cannot create {}", tmp.display()))?;
        Ok(Download {
            remote: remote.to_string(),
            dest: dest.to_path_buf(),
            tmp,
            file,
            next_block: 0,
            size: 0,
            received: 0,
        })
    }

    pub fn accept_block(&mut self, data: &[u8]) -> anyhow::Result<()> {
        self.file
            .write_all(data)
            .with_context(|| format!("cannot write {}", self.tmp.display()))?;
        self.received += data.len() as u64;
        self.next_block = self.next_block.wrapping_add(1);
        Ok(())
    }

    /// Validate the spooled file as a zip, then rename it into place. A file
    /// that fails is deleted so a bad transfer never poisons the search path.
    pub fn finish(self) -> anyhow::Result<()> {
        drop(self.file);
        let checked = File::open(&self.tmp)
            .map_err(anyhow::Error::from)
            .and_then(|f| zip::ZipArchive::new(f).map_err(anyhow::Error::from));
        if let Err(e) = checked {
            let _ = std::fs::remove_file(&self.tmp);
            bail!("{} is not a valid pk3: {e}", self.remote);
        }
        std::fs::rename(&self.tmp, &self.dest)
            .with_context(|| format!("cannot rename into {}", self.dest.display()))
    }

    pub fn abort(self) {
        drop(self.file);
        let _ = std::fs::remove_file(&self.tmp);
    }
}

/// A one-entry stored zip, for tests that need a transfer to pass validation.
#[cfg(test)]
pub(crate) fn test_zip_bytes() -> Vec<u8> {
    use std::io::Cursor;
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    w.start_file("maps/mp/foo.bsp", opts).unwrap();
    w.write_all(b"IBSP").unwrap();
    w.finish().unwrap().into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Trimmed from a live server (167.235.192.175:23120).
    const SYSTEMINFO: &str = "\\sv_referencedPakNames\\main/zzz_zfunmod main/shipment \
         main/pak6 main/pak0 main/farm main/bellicourt_v1_1 main/localized_english_pak0\
         \\sv_referencedPaks\\1 2 3 4 5 6 7\\sv_serverid\\225";

    #[test]
    fn parses_referenced_pak_names() {
        let names = referenced_pak_names(SYSTEMINFO);
        assert_eq!(names.len(), 7);
        assert_eq!(names[0], "main/zzz_zfunmod");
        assert_eq!(names[5], "main/bellicourt_v1_1");
        assert!(referenced_pak_names("\\foo\\bar").is_empty());
    }

    #[test]
    fn stock_paks_are_recognised() {
        assert!(is_stock_pak("main/pak0"));
        assert!(is_stock_pak("main/pak9"));
        assert!(is_stock_pak("main/localized_english_pak1"));
        assert!(!is_stock_pak("main/pak10")); // custom, downloadable
        assert!(!is_stock_pak("main/bellicourt_v1_1"));
        assert!(!is_stock_pak("revive/pak0")); // a mod's own pak0 is fair game
    }

    #[test]
    fn safe_rel_path_accepts_normal_names() {
        assert_eq!(
            safe_rel_path("main/bellicourt_v1_1", &["main"]),
            Some(PathBuf::from("main/bellicourt_v1_1.pk3"))
        );
        assert_eq!(
            safe_rel_path("main/z1.2map", &["main"]),
            Some(PathBuf::from("main/z1.2map.pk3"))
        );
        assert_eq!(
            safe_rel_path("uo/foo", &["uo"]),
            Some(PathBuf::from("uo/foo.pk3"))
        );
    }

    #[test]
    fn safe_rel_path_only_writes_into_the_base_and_game_dirs() {
        assert_eq!(safe_rel_path("uo/foo", &["main"]), None);
        assert_eq!(safe_rel_path("Main/foo", &["main"]), None);
        assert_eq!(safe_rel_path("mainx/foo", &["main"]), None);
        assert_eq!(
            safe_rel_path("genesis/zzz_revive", &["main", "genesis"]),
            Some(PathBuf::from("genesis/zzz_revive.pk3"))
        );
    }

    // Seen live on 199.247.2.228:28960 and 63.176.159.145:28960, trimmed.
    const MOD_SYSTEMINFO: &str = "\\cl_allowDownload\\0\\fs_game\\genesis\\sv_referencedPakNames\\\
         genesis/zzz_revive main/pak6 main/zzz_mosinfix main/localized_english_pak0";

    #[test]
    fn fs_game_names_a_plain_directory_other_than_the_base() {
        assert_eq!(fs_game(MOD_SYSTEMINFO, "main").as_deref(), Some("genesis"));
        assert_eq!(
            fs_game("\\fs_game\\BO7MEDX-MOD", "main").as_deref(),
            Some("BO7MEDX-MOD")
        );
        for v in ["", "main", "Main", "..", "../x", "a/b", ".hidden"] {
            assert_eq!(fs_game(&format!("\\fs_game\\{v}"), "main"), None, "{v:?}");
        }
        assert_eq!(fs_game(SYSTEMINFO, "main"), None);
    }

    #[test]
    fn the_server_can_force_downloads_either_way() {
        assert_eq!(server_allows_download(MOD_SYSTEMINFO), Some(false));
        assert_eq!(server_allows_download("\\cl_allowDownload\\1"), Some(true));
        assert_eq!(server_allows_download(SYSTEMINFO), None);
    }

    #[test]
    fn missing_paks_lists_what_is_absent_in_either_dir() {
        let got = missing_paks(
            MOD_SYSTEMINFO,
            &["main", "genesis"],
            |_| false,
            |rel| rel == Path::new("main/zzz_mosinfix.pk3"),
        );
        assert_eq!(got, vec!["genesis/zzz_revive.pk3"]);
        // A directory the client won't write to is missing however it's named.
        let got = missing_paks(MOD_SYSTEMINFO, &["main"], |_| false, |_| true);
        assert_eq!(got, vec!["genesis/zzz_revive.pk3"]);
    }

    #[test]
    fn safe_rel_path_rejects_hostile_names() {
        for bad in [
            "../etc/cron",
            "main/../../etc/cron",
            "/etc/cron",
            "main/..",
            "main/.hidden",
            "main/sub/deep",
            "main\\evil",
            "main/sp ace",
            "main/",
            "/",
            "",
        ] {
            assert_eq!(
                safe_rel_path(bad, &["main"]),
                None,
                "{bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn candidates_prefer_the_map_pak_and_skip_stock_and_present() {
        // `farm` (checksum 5) is here under another name; `shipment` (2)
        // is here by name with another checksum.
        let got = candidates_for_map(
            SYSTEMINFO,
            "mp_bellicourt_v1_1",
            &["main"],
            |c| c == 5,
            |rel| rel == Path::new("main/shipment.pk3"),
        );
        assert_eq!(
            got,
            vec![
                (
                    "main/bellicourt_v1_1".into(),
                    "main/bellicourt_v1_1.pk3".into()
                ),
                ("main/zzz_zfunmod".into(), "main/zzz_zfunmod.pk3".into()),
                ("main/shipment".into(), "main/shipment.00000002.pk3".into()),
            ]
        );
    }

    #[test]
    fn referenced_paks_pair_names_with_checksums() {
        let paks = referenced_paks(SYSTEMINFO);
        assert_eq!(paks[1].name, "main/shipment");
        assert_eq!(paks[1].checksum, Some(2));
        // Lists of different lengths fall back to the names alone.
        assert!(
            referenced_paks(MOD_SYSTEMINFO)
                .iter()
                .all(|p| p.checksum.is_none())
        );
        assert_eq!(pure_paks("\\sv_paks\\-5 7 \\sv_pure\\1"), Some(vec![-5, 7]));
        assert_eq!(pure_paks("\\sv_paks\\\\sv_pure\\0"), None);
        // A wrong-checksum file in the way gets retail's hex suffix.
        let pak = ReferencedPak {
            name: "main/foo".into(),
            checksum: Some(-1),
        };
        assert_eq!(
            lacking(&pak, &["main"], &|_| false, &|_| true),
            Some(PathBuf::from("main/foo.ffffffff.pk3"))
        );
    }

    #[test]
    fn candidates_match_on_shared_name_tokens() {
        // Seen live: map `dufresne_final` ships in `main/n_dufresne`.
        let info = "\\sv_referencedPakNames\\main/zzz_zfunmod main/n_degaulle main/n_dufresne";
        let got: Vec<String> =
            candidates_for_map(info, "dufresne_final", &["main"], |_| false, |_| false)
                .into_iter()
                .map(|(n, _)| n)
                .collect();
        assert_eq!(
            got,
            vec!["main/n_dufresne", "main/zzz_zfunmod", "main/n_degaulle"]
        );
    }

    #[test]
    fn candidates_match_on_names_without_separators() {
        // Seen live on 167.235.192.175:23120.
        let info = "\\sv_referencedPakNames\\main/zzz_zfunmod main/farm main/BunkerCourt2AvsG";
        let got = candidates_for_map(info, "BunkerCourt2_A_vs_G", &["main"], |_| false, |_| false);
        assert_eq!(got[0].0, "main/BunkerCourt2AvsG");
        let got = candidates_for_map(info, "mp_farm", &["main"], |_| false, |_| false);
        assert_eq!(got[0].0, "main/farm");
    }

    #[test]
    fn download_spools_and_renames() {
        let dir = std::env::temp_dir().join(format!("vcod-dl-spool-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dest = dir.join("main/foo.pk3");

        let zip = test_zip_bytes();
        let mut dl = Download::create("main/foo.pk3", &dest).unwrap();
        dl.accept_block(&zip[..10]).unwrap();
        dl.accept_block(&zip[10..]).unwrap();
        assert_eq!(dl.next_block, 2);
        assert_eq!(dl.received, zip.len() as u64);
        assert!(!dest.exists(), "must not appear before the EOF block");
        dl.finish().unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), zip);

        assert!(Download::create("main/foo.pk3", &dest).is_err());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn finish_rejects_a_file_that_is_not_a_zip() {
        let dir = std::env::temp_dir().join(format!("vcod-dl-garbage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dest = dir.join("main/bad.pk3");

        let mut dl = Download::create("main/bad.pk3", &dest).unwrap();
        dl.accept_block(b"<html>not a pk3</html>").unwrap();
        let err = dl.finish().unwrap_err();
        assert!(err.to_string().contains("not a valid pk3"), "{err:#}");
        assert!(!dest.exists());
        assert!(
            std::fs::read_dir(dir.join("main"))
                .unwrap()
                .next()
                .is_none()
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn abort_removes_the_partial_file() {
        let dir = std::env::temp_dir().join(format!("vcod-dl-abort-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dest = dir.join("main/bar.pk3");

        let mut dl = Download::create("main/bar.pk3", &dest).unwrap();
        dl.accept_block(b"partial").unwrap();
        dl.abort();
        assert!(
            std::fs::read_dir(dir.join("main"))
                .unwrap()
                .next()
                .is_none()
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
