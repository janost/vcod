//! `cargo run -p vcod-common --example stock_tree -- <install> <out>`: a
//! tree of symlinks to the install's stock paks alone, `main/` and (on a
//! United Offensive install) `uo/`, what `--vanilla` keeps. Point `COD_DIR`
//! at `<out>` for the 1.5 and UO suites and retail servers, which a
//! third-party pak in the install would move (docs/dev/testing.md).
//! Run again, it relinks the same tree.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use vcod_common::pk3::Pk3Fs;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [install, out] = args.as_slice() else {
        bail!("usage: stock_tree <install> <out>");
    };
    let (install, out) = (Path::new(install), Path::new(out));
    // `uo/` searches over `main/` beside it, so one open lists both.
    let base = ["uo", "main"]
        .iter()
        .map(|d| install.join(d))
        .find(|d| d.is_dir())
        .with_context(|| format!("{} has no main/", install.display()))?;
    let fs = Pk3Fs::open_search(&base, None, None, true)?;
    let mut linked = 0;
    for pak in fs.paks() {
        let target = pak.path.canonicalize()?;
        let name = target.file_name().context("a pak with no file name")?;
        let dir = out.join(&pak.game);
        std::fs::create_dir_all(&dir)?;
        link(&target, &dir.join(name))?;
        linked += 1;
    }
    println!("{linked} stock paks linked under {}", out.display());
    Ok(())
}

#[cfg(unix)]
fn link(target: &Path, at: &PathBuf) -> Result<()> {
    if at.symlink_metadata().is_ok() {
        std::fs::remove_file(at)?;
    }
    std::os::unix::fs::symlink(target, at).with_context(|| format!("linking {}", at.display()))
}

#[cfg(not(unix))]
fn link(_: &Path, _: &PathBuf) -> Result<()> {
    bail!("stock_tree makes symlinks, which this platform's runners do not use")
}
