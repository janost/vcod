// Build script shared by the `vcod` and `vcod-server` packages: sets
// `VCOD_VERSION` to `<package version>-<git short sha> <os>-<arch>`, named
// as the nightly archives are, with ` debug` on a debug build. The sha is
// `VCOD_GIT_SHA`, which the nightly workflow sets, else `unknown`: a script
// that read git's HEAD reran on every commit and checkout and rebuilt both
// crates and all their tests.

fn main() {
    println!("cargo:rerun-if-env-changed=VCOD_GIT_SHA");
    let sha = std::env::var("VCOD_GIT_SHA")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into());
    let version = env!("CARGO_PKG_VERSION");
    let var = |k: &str| std::env::var(k).unwrap_or_default();
    let os = var("CARGO_CFG_TARGET_OS");
    let arch = match var("CARGO_CFG_TARGET_ARCH") {
        a if a == "x86_64" => "amd64".to_string(),
        a if a == "aarch64" => "arm64".to_string(),
        a => a,
    };
    let debug = if var("PROFILE") == "release" {
        ""
    } else {
        " debug"
    };
    println!("cargo:rustc-env=VCOD_VERSION={version}-{sha} {os}-{arch}{debug}");
}
