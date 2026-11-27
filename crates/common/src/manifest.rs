//! A crate's directory at run time. `env!("CARGO_MANIFEST_DIR")` is frozen at
//! compile time, and a worktree seeded with another checkout's `target/` keeps
//! workspace units that name the seed's path, so tests read the seed's docs
//! and fixtures. `cargo test` and `cargo run` set `CARGO_MANIFEST_DIR` in the
//! process environment; these macros read it first and fall back to the
//! compiled-in value for a binary run straight from `target/`.
//!
//! `crates/common/tests/manifest_dir.rs` fails on a bare `env!` use elsewhere.

use std::ops::Deref;
use std::path::Path;
use std::sync::OnceLock;

/// The directory of the crate that compiled in `compiled`. The run-time value
/// names whichever package cargo is running, so only its parent is taken and
/// the compiled-in crate's own directory name is kept (code of one crate can
/// run inside another's test binary).
pub fn resolve(compiled: &str) -> String {
    let runtime = std::env::var_os("CARGO_MANIFEST_DIR").map(std::path::PathBuf::from);
    let name = Path::new(compiled).file_name();
    if let (Some(runtime), Some(name)) = (runtime, name)
        && let Some(parent) = runtime.parent()
    {
        let dir = parent.join(name);
        if dir.is_dir() {
            return dir.to_string_lossy().into_owned();
        }
    }
    compiled.to_owned()
}

/// A path under a crate's directory for a `static`, resolved on first use.
/// Build it with [`manifest_const!`](crate::manifest_const).
pub struct ManifestPath {
    compiled: &'static str,
    rest: &'static str,
    cell: OnceLock<String>,
}

impl ManifestPath {
    pub const fn new(compiled: &'static str, rest: &'static str) -> Self {
        Self {
            compiled,
            rest,
            cell: OnceLock::new(),
        }
    }
}

impl Deref for ManifestPath {
    type Target = str;
    fn deref(&self) -> &str {
        self.cell
            .get_or_init(|| format!("{}{}", resolve(self.compiled), self.rest))
    }
}

impl std::fmt::Display for ManifestPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self)
    }
}

impl AsRef<str> for ManifestPath {
    fn as_ref(&self) -> &str {
        self
    }
}

impl AsRef<Path> for ManifestPath {
    fn as_ref(&self) -> &Path {
        Path::new(&**self)
    }
}

impl AsRef<std::ffi::OsStr> for ManifestPath {
    fn as_ref(&self) -> &std::ffi::OsStr {
        std::ffi::OsStr::new(&**self)
    }
}

/// The calling crate's directory, as a `&'static str` (replaces
/// `env!("CARGO_MANIFEST_DIR")`).
#[macro_export]
macro_rules! manifest_dir {
    () => {{
        static DIR: ::std::sync::OnceLock<::std::string::String> = ::std::sync::OnceLock::new();
        DIR.get_or_init(|| $crate::manifest::resolve(env!("CARGO_MANIFEST_DIR")))
            .as_str()
    }};
}

/// The calling crate's directory followed by `concat!` of the arguments, as a
/// `&'static str` (replaces `concat!(env!("CARGO_MANIFEST_DIR"), ...)`).
#[macro_export]
macro_rules! manifest_path {
    ($($rest:tt)*) => {{
        static PATH: ::std::sync::OnceLock<::std::string::String> = ::std::sync::OnceLock::new();
        PATH.get_or_init(|| {
            ::std::format!(
                "{}{}",
                $crate::manifest::resolve(env!("CARGO_MANIFEST_DIR")),
                concat!($($rest)*)
            )
        })
        .as_str()
    }};
}

/// A [`ManifestPath`] for a `static` item: `static F: ManifestPath =
/// manifest_const!("/tests/fixtures");`.
#[macro_export]
macro_rules! manifest_const {
    ($($rest:tt)*) => {
        $crate::manifest::ManifestPath::new(env!("CARGO_MANIFEST_DIR"), concat!($($rest)*))
    };
}
