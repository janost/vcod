//! Names from the console, a menu, a server or a pak joined onto a local
//! directory. `Path::join` keeps `..` and drops the base for an absolute
//! name, so every such name passes through here first.

use std::path::{Path, PathBuf};

/// `name` as a relative path: `None` when it is empty, rooted, holds a `..`
/// component, a `:` (a drive or a stream) or a NUL. `/` and `\` both
/// separate; empty and `.` components drop out.
pub fn relative(name: &str) -> Option<PathBuf> {
    if name.starts_with(['/', '\\']) || name.contains([':', '\0']) {
        return None;
    }
    let mut out = PathBuf::new();
    for part in name.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => return None,
            p => out.push(p),
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// `dir/name` when `name` passes [`relative`].
pub fn under(dir: &Path, name: &str) -> Option<PathBuf> {
    Some(dir.join(relative(name)?))
}

/// Whether `name` is one plain path component: a directory or file name
/// with no separator, not `.` or `..`, and no `:`, NUL or other control
/// character.
pub fn is_component(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', ':'])
        && !name.chars().any(char::is_control)
}

/// `dir/name` when `name` is one plain component ([`is_component`]).
pub fn under_one(dir: &Path, name: &str) -> Option<PathBuf> {
    is_component(name).then(|| dir.join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_names_and_subdirectories_stay_under_the_directory() {
        let d = Path::new("base");
        assert_eq!(under(d, "x.cfg"), Some(d.join("x.cfg")));
        assert_eq!(under(d, "a/b.cfg"), Some(d.join("a").join("b.cfg")));
        assert_eq!(under(d, "a\\b.cfg"), Some(d.join("a").join("b.cfg")));
        assert_eq!(under(d, "./a//b"), Some(d.join("a").join("b")));
        // `..` inside a name is not a component
        assert_eq!(under(d, "a..b"), Some(d.join("a..b")));
        assert_eq!(under_one(d, "Able"), Some(d.join("Able")));
    }

    #[test]
    fn climbing_rooted_and_drive_names_are_refused() {
        let d = Path::new("base");
        for bad in [
            "",
            ".",
            "../x",
            "a/../../x",
            "a\\..\\x",
            "/abs/x",
            "\\abs\\x",
            "C:x",
            "C:\\x",
            "x:stream",
            "a\0b",
        ] {
            assert_eq!(under(d, bad), None, "{bad:?}");
            assert_eq!(under_one(d, bad), None, "{bad:?}");
        }
        for bad in ["a/b", "a\\b", "..", "a\nb"] {
            assert_eq!(under_one(d, bad), None, "{bad:?}");
        }
    }
}
