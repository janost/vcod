//! A fatal error or panic also goes to `vcod-error.log` beside the
//! executable: a double-clicked vcod whose console closes with it, or a
//! launcher entry with none, still leaves the message to read.

use std::path::{Path, PathBuf};

/// The log's name, beside the executable.
const NAME: &str = "vcod-error.log";

/// Writes `msg` under `title` (the version line) to the log in `dir`, and
/// returns the path written.
fn write_in(dir: &Path, title: &str, msg: &str) -> Option<PathBuf> {
    let path = dir.join(NAME);
    std::fs::write(&path, format!("{title}\n{msg}\n")).ok()?;
    Some(path)
}

/// Records a fatal `msg` beside the executable and says where on stderr.
/// A directory it cannot write to (an install under Program Files) leaves
/// stderr alone.
pub fn record(title: &str, msg: &str) {
    let dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    if let Some(path) = dir.and_then(|d| write_in(&d, title, msg)) {
        eprintln!("(also written to {})", path.display());
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_log_holds_the_version_and_the_message() {
        let dir = std::env::temp_dir().join(format!("vcod-fatal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = super::write_in(&dir, "vcod 0.1.0", "Error: no GPU adapter found").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, "vcod 0.1.0\nError: no GPU adapter found\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
