use std::process::Command;

// Bakes a build id into the binary so `nobrew --version` can report exactly
// which build is installed. Preview/release CI sets NOBREW_BUILD_ID; local
// builds fall back to the short git sha.
fn main() {
    let build_id = std::env::var("NOBREW_BUILD_ID").ok().or_else(|| {
        Command::new("git")
            .args(["rev-parse", "--short=12", "HEAD"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    });
    let build_id = build_id.unwrap_or_else(|| "dev".to_string());
    println!("cargo:rustc-env=NOBREW_BUILD_ID={build_id}");
    println!("cargo:rerun-if-env-changed=NOBREW_BUILD_ID");
}
