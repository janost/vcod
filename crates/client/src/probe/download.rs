//! `--probe-download`: the GUI client's download path, headless.

use super::*;

/// `--probe-download`: the download path a GUI client takes at a gamestate,
/// headless. The referenced paks no local pak matches by checksum go into
/// `dir` (`FS_ComparePaks`' rule and file names), then `donedl`; each pak's
/// checksum is printed beside the one `sv_referencedPaks` gave for it.
/// Under `--vanilla` it fetches nothing and searches only stock paks, as the
/// client's `start_loading` does.
pub(super) struct DownloadProbe {
    dir: std::path::PathBuf,
    /// `--vanilla`, read off the install's search path.
    vanilla: bool,
    /// Checksums of the install's paks.
    installed: Vec<i32>,
    /// The install's search path, which `searched` puts the scratch
    /// directory over.
    install_dirs: Vec<std::path::PathBuf>,
    /// The install and the scratch directory, as the client's search path
    /// stands once a pak lands: what `cp` reads after the downloads.
    searched: Option<vcod_common::pk3::Pk3Fs>,
    /// The scratch directories the server's paks land in.
    scratch_dirs: Vec<std::path::PathBuf>,
    loader: Option<crate::loading::MapLoader>,
    /// remote name -> (local path, the server's checksum).
    expected: std::collections::HashMap<String, (std::path::PathBuf, Option<i32>)>,
    done: bool,
}

impl DownloadProbe {
    pub(super) fn new(dir: std::path::PathBuf, fs: Option<&vcod_common::pk3::Pk3Fs>) -> Self {
        DownloadProbe {
            dir,
            vanilla: fs.is_some_and(vcod_common::pk3::Pk3Fs::vanilla),
            installed: fs.map_or_else(Vec::new, |fs| fs.paks().map(|p| p.checksum).collect()),
            install_dirs: fs.map_or_else(Vec::new, |fs| fs.dirs().to_vec()),
            searched: None,
            scratch_dirs: Vec::new(),
            loader: None,
            expected: std::collections::HashMap::new(),
            done: false,
        }
    }

    /// From the gamestate until the downloads are over and the next
    /// gamestate is in.
    pub(super) fn holds_cmds(&self) -> bool {
        self.loader.is_some() && !self.done
    }

    /// The search path with the scratch directory in it, once one opened.
    pub(super) fn fs(&self) -> Option<&vcod_common::pk3::Pk3Fs> {
        self.searched.as_ref()
    }

    /// Reopens [`Self::searched`] over the install and the scratch
    /// directories, as the client's `FS_Restart` does after a download.
    fn reopen(&mut self) {
        let dirs: Vec<std::path::PathBuf> = self
            .install_dirs
            .iter()
            .chain(&self.scratch_dirs)
            .cloned()
            .collect();
        match vcod_common::pk3::Pk3Fs::open_dirs(dirs, None, self.vanilla) {
            Ok(fs) => self.searched = Some(fs),
            Err(e) => println!("DOWNLOAD: cannot reopen the search path: {e:#}"),
        }
    }

    pub(super) fn step(
        &mut self,
        client: &mut NetClient<UdpTransport>,
        events: &[NetEvent],
        now: Instant,
    ) -> anyhow::Result<()> {
        use crate::loading::Action;
        if self.done {
            return Ok(());
        }
        if self.loader.is_none() && events.contains(&NetEvent::GamestateReady) {
            let info = client.configstring(1).to_string();
            let game = net::download::fs_game(&info, "main");
            let dirs: Vec<&str> = std::iter::once("main").chain(game.as_deref()).collect();
            let dir_paths: Vec<std::path::PathBuf> =
                dirs.iter().map(|d| self.dir.join(d)).collect();
            self.scratch_dirs = dir_paths.clone();
            // A pak an earlier run left there counts for `cp` too.
            self.reopen();
            let mut have = self.installed.clone();
            if let Some(fs) = &self.searched {
                have.extend(fs.paks().map(|p| p.checksum));
            }
            let referenced = net::download::referenced_paks(&info);
            let candidates: Vec<(String, std::path::PathBuf)> = if self.vanilla {
                println!("DOWNLOAD: --vanilla fetches nothing");
                Vec::new()
            } else {
                net::download::candidates_for_map(
                    &info,
                    client.protocol(),
                    "",
                    &dirs,
                    |c| have.contains(&c),
                    |rel| self.dir.join(rel).exists(),
                )
                .into_iter()
                .map(|(name, rel)| {
                    let sum = referenced
                        .iter()
                        .find(|p| p.name == name)
                        .and_then(|p| p.checksum);
                    let remote = format!("{name}.pk3");
                    let dest = self.dir.join(rel);
                    self.expected.insert(remote.clone(), (dest.clone(), sum));
                    (remote, dest)
                })
                .collect()
            };
            println!(
                "DOWNLOAD: {} referenced, {} to fetch into {}",
                referenced.len(),
                candidates.len(),
                self.dir.display()
            );
            self.loader = Some(crate::loading::MapLoader::new(String::new(), candidates));
        }
        let Some(loader) = &mut self.loader else {
            return Ok(());
        };
        for e in events {
            if let NetEvent::DownloadComplete(remote) = e
                && let Some((path, want)) = self.expected.get(remote)
            {
                let got = vcod_common::pak_checksum::pak_checksums(path, 0)?.checksum;
                let verdict = match want {
                    Some(w) if *w == got => "match",
                    Some(_) => "MISMATCH",
                    None => "unpaired",
                };
                println!(
                    "DOWNLOAD: {remote} -> {} {} bytes, checksum {got}, server {want:?}: {verdict}",
                    path.display(),
                    std::fs::metadata(path).map_or(0, |m| m.len()),
                );
            }
        }
        match loader.step(events, client.download_progress(), true, now) {
            Action::BeginDownload { remote, dest } => {
                println!("DOWNLOAD: requesting {remote}");
                client.begin_download(&remote, &dest)?;
            }
            Action::FinishDownloads => {
                println!("DOWNLOAD: donedl");
                client.finish_downloads();
            }
            Action::Ready => {
                println!("DOWNLOAD: done, gamestate in hand");
                self.done = true;
            }
            Action::Failed(why) => {
                println!("DOWNLOAD: failed: {why}");
                self.done = true;
            }
            Action::Reopen => self.reopen(),
            Action::Wait(_) => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `--vanilla` holds across the download reopen: a pak an earlier run
    /// left in the scratch directory stays off the search path `cp` reads.
    #[test]
    fn a_vanilla_download_reopen_keeps_to_the_stock_paks() {
        use std::io::Write;
        let pk3 = |dir: &std::path::Path, file: &str| {
            std::fs::create_dir_all(dir).unwrap();
            let mut z = zip::ZipWriter::new(std::fs::File::create(dir.join(file)).unwrap());
            z.start_file("readme.txt", zip::write::SimpleFileOptions::default())
                .unwrap();
            z.write_all(file.as_bytes()).unwrap();
            z.finish().unwrap();
        };
        let root = tempfile::tempdir().unwrap();
        let install = root.path().join("install/main");
        let scratch = root.path().join("scratch");
        pk3(&install, "pak0.pk3");
        pk3(&scratch.join("main"), "mp_custom.pk3");
        for vanilla in [false, true] {
            let fs = vcod_common::pk3::Pk3Fs::open_search(&install, None, None, vanilla).unwrap();
            let mut dl = DownloadProbe::new(scratch.clone(), Some(&fs));
            dl.scratch_dirs = vec![scratch.join("main")];
            dl.reopen();
            let names: Vec<String> = dl
                .fs()
                .unwrap()
                .paks()
                .map(vcod_common::pk3::PakInfo::qualified_name)
                .collect();
            assert_eq!(
                names.contains(&"main/mp_custom".to_string()),
                !vanilla,
                "{names:?}"
            );
            assert_eq!(dl.fs().unwrap().vanilla(), vanilla);
        }
    }
}
