use std::process::Command;

// Bakes a build id into the binary so `xbrew --version` can report exactly
// which build is installed. Preview/release CI sets XBREW_BUILD_ID; local
// builds fall back to the short git sha.
fn main() {
    // `recipe.rs` embeds the whole recipes/ directory with `include_dir!`, which
    // can only register files that already exist — a *newly added* recipe is
    // invisible to the crate's fingerprint. Emitting any `rerun-if-*` directive
    // also switches off cargo's "rerun on any package change" default, so
    // without this line adding recipes/foo.toml recompiles nothing: the tests
    // pass green against a binary that does not contain the new recipe.
    // Cargo scans a declared directory recursively, so this covers add, edit,
    // and delete.
    println!("cargo:rerun-if-changed=recipes");
    let build_id = std::env::var("XBREW_BUILD_ID").ok().or_else(|| {
        Command::new("git")
            .args(["rev-parse", "--short=12", "HEAD"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    });
    let build_id = build_id.unwrap_or_else(|| "dev".to_string());
    println!("cargo:rustc-env=XBREW_BUILD_ID={build_id}");
    println!("cargo:rerun-if-env-changed=XBREW_BUILD_ID");
}
