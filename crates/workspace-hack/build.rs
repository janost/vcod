// Cargo resolves build-dependencies only for a package with a build script.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
}
