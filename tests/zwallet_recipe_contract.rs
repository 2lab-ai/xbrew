//! Contract for the zwallet recipe: the pinned stable release binary on Arch,
//! 2lab-ai's tap formula on macOS, and no Debian/RHEL backend (zwallet ships
//! no packages for them).

use std::fs;
use std::path::PathBuf;
use std::process::Command;

const RELEASE_TAG: &str = "v0.1.0";
const RELEASE_BASE: &str = "https://github.com/2lab-ai/zwallet/releases/download";
const FORMULA: &str = "2lab-ai/tap/zwallet";

fn recipe() -> toml::Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("recipes/zwallet.toml");
    let source = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} must exist: {error}", path.display()));
    toml::from_str(&source)
        .unwrap_or_else(|error| panic!("{} must be valid TOML: {error}", path.display()))
}

#[test]
fn zwallet_pins_the_stable_release_for_both_architectures() {
    let recipe = recipe();
    assert_eq!(recipe["name"].as_str(), Some("zwallet"));

    let binary = &recipe["arch"]["binary"];
    assert_eq!(binary["name"].as_str(), Some("zwallet"));

    // Copied from the v0.1.0 release's SHA256SUMS.
    let cases = [
        (
            "aarch64",
            "8bc365232fb102ba3b737d51d8ee5a2afa672237a245bc324b392ad41e6c4681",
        ),
        (
            "x86_64",
            "131109e6b98572058f484ad9209d1001efe35d8aea77f73c1dc2c637bac0cfc3",
        ),
    ];

    for (arch, sha256) in cases {
        let asset = format!("zwallet-linux-{arch}");
        assert_eq!(
            binary[arch]["url"].as_str(),
            Some(format!("{RELEASE_BASE}/{RELEASE_TAG}/{asset}").as_str())
        );
        assert_eq!(binary[arch]["sha256"].as_str(), Some(sha256));
    }
}

#[test]
fn zwallet_installs_the_tap_formula_on_macos_and_nothing_on_debian_or_rhel() {
    let recipe = recipe();
    assert_eq!(recipe["macos"]["formula"].as_str(), Some(FORMULA));
    assert!(
        recipe["macos"].get("cask").is_none() && recipe["macos"].get("dmg").is_none(),
        "the tap formula is the only macOS backend"
    );
    for section in ["debian", "rhel"] {
        assert!(
            recipe.get(section).is_none(),
            "zwallet ships no {section} package — do not imply support"
        );
    }
    assert!(
        recipe.get("script").is_none(),
        "zwallet must use xbrew's checksum verifier or brew, not a shell installer"
    );

    // `xbrew info` prints the description on every platform, and Arch does not
    // install from the tap, so it must not name one backend.
    let description = recipe["description"]
        .as_str()
        .expect("description must be a string")
        .to_lowercase();
    assert!(description.contains("wallet"), "{description}");
    for term in ["tap", "brew"] {
        assert!(
            !description.contains(term),
            "description must not name a single backend ({term:?}): {description}"
        );
    }
}

/// The crate is binary-only, so the embedded registry is reached the way a user
/// reaches it: the real `xbrew info`. `registry()` silently drops a recipe that
/// does not parse as `Recipe`, so "recipe:  yes" is the receipt that the file
/// was embedded and parsed (`Recipe` ignores unknown keys; the raw-TOML tests
/// above catch a misspelled one). HOME is a fresh directory so a developer's
/// own `~/.xbrew/recipes/zwallet.toml` cannot answer instead of the built-in.
#[test]
fn zwallet_is_a_built_in_recipe_of_the_xbrew_binary() {
    let home = std::env::temp_dir().join(format!("xbrew-zwallet-info-{}", std::process::id()));
    let _ = fs::remove_dir_all(&home);
    fs::create_dir_all(&home).expect("create isolated HOME");

    let output = Command::new(env!("CARGO_BIN_EXE_xbrew"))
        .args(["info", "zwallet"])
        .env_clear()
        .env("HOME", &home)
        .env("PATH", "/usr/bin:/bin")
        .output()
        .expect("launch xbrew");
    let _ = fs::remove_dir_all(&home);

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "xbrew info failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.lines().any(|line| line == "recipe:  yes"),
        "zwallet is not a built-in recipe: {stdout}"
    );

    let description = recipe()["description"]
        .as_str()
        .expect("description must be a string")
        .to_string();
    assert!(
        stdout
            .lines()
            .any(|line| line == format!("about:   {description}")),
        "the embedded recipe is not recipes/zwallet.toml: {stdout}"
    );
    // Only macOS prints a backend line deterministically: Linux CI is Ubuntu,
    // where zwallet declares nothing, and the Arch line depends on the host.
    if cfg!(target_os = "macos") {
        assert!(
            stdout
                .lines()
                .any(|line| line == format!("  macos → formula: {FORMULA}")),
            "{stdout}"
        );
    }
}
