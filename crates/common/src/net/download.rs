//! pk3 downloads over the Q3/RTCW UDP download protocol (docs/protocol-1.1.md,
//! svc_download). Pak bookkeeping and the file spool; the wire handling is on
//! `NetClient`.

use anyhow::{bail, Context};
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

/// Validate a server-supplied pak name into `<mod_dir>/<file>.pk3`. The name
/// gets joined onto a local directory, so traversal and odd characters are
/// rejected, and the directory must be the client's active mod dir so a server
/// cannot drop files anywhere else under the install.
pub fn safe_rel_path(name: &str, mod_dir: &str) -> Option<PathBuf> {
    let (dir, file) = name.split_once('/')?;
    if dir != mod_dir {
        return None;
    }
    let ok = !file.is_empty()
        && !file.starts_with('.')
        && file
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.');
    if !ok {
        return None;
    }
    Some(PathBuf::from(dir).join(format!("{file}.pk3")))
}

/// Referenced paks worth downloading for `map`, most likely first: stock,
/// unsafe and present (per `exists`, given the relative path) paks are dropped;
/// paks sharing a name token with the map sort ahead of the rest.
pub fn candidates_for_map(
    systeminfo: &str,
    map: &str,
    mod_dir: &str,
    exists: impl Fn(&Path) -> bool,
) -> Vec<String> {
    // Pak names rarely match the map exactly (`main/n_dufresne` holds
    // `dufresne_final`), so match on a shared `_`-separated token.
    let map = map.to_lowercase();
    let map_tokens: Vec<&str> = map.split('_').filter(|t| t.len() >= 4).collect();
    let mut like_map = Vec::new();
    let mut rest = Vec::new();
    for name in referenced_pak_names(systeminfo) {
        if is_stock_pak(&name) {
            continue;
        }
        let Some(rel) = safe_rel_path(&name, mod_dir) else {
            continue;
        };
        if exists(&rel) {
            continue;
        }
        let base = name.rsplit('/').next().unwrap_or(&name).to_lowercase();
        if base
            .split('_')
            .any(|t| t.len() >= 4 && map_tokens.contains(&t))
        {
            like_map.push(name);
        } else {
            rest.push(name);
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
            safe_rel_path("main/bellicourt_v1_1", "main"),
            Some(PathBuf::from("main/bellicourt_v1_1.pk3"))
        );
        assert_eq!(
            safe_rel_path("main/z1.2map", "main"),
            Some(PathBuf::from("main/z1.2map.pk3"))
        );
        assert_eq!(
            safe_rel_path("uo/foo", "uo"),
            Some(PathBuf::from("uo/foo.pk3"))
        );
    }

    #[test]
    fn safe_rel_path_only_writes_into_the_active_mod_dir() {
        assert_eq!(safe_rel_path("uo/foo", "main"), None);
        assert_eq!(safe_rel_path("Main/foo", "main"), None);
        assert_eq!(safe_rel_path("mainx/foo", "main"), None);
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
                safe_rel_path(bad, "main"),
                None,
                "{bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn candidates_prefer_the_map_pak_and_skip_stock_and_present() {
        let got = candidates_for_map(SYSTEMINFO, "mp_bellicourt_v1_1", "main", |rel| {
            rel == Path::new("main/farm.pk3")
        });
        assert_eq!(
            got,
            vec!["main/bellicourt_v1_1", "main/zzz_zfunmod", "main/shipment"]
        );
    }

    #[test]
    fn candidates_match_on_shared_name_tokens() {
        // Seen live: map `dufresne_final` ships in `main/n_dufresne`.
        let info = "\\sv_referencedPakNames\\main/zzz_zfunmod main/n_degaulle main/n_dufresne";
        let got = candidates_for_map(info, "dufresne_final", "main", |_| false);
        assert_eq!(
            got,
            vec!["main/n_dufresne", "main/zzz_zfunmod", "main/n_degaulle"]
        );
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
        assert!(std::fs::read_dir(dir.join("main"))
            .unwrap()
            .next()
            .is_none());

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
        assert!(std::fs::read_dir(dir.join("main"))
            .unwrap()
            .next()
            .is_none());

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
