use std::fs;
use std::path::PathBuf;

const PREVIEW_TAG: &str = "preview-2026-07-20-095048-29731634437-1-5f887198112c";
const RELEASE_BASE: &str = "https://github.com/2lab-ai/dbotter/releases/download";

fn recipe() -> toml::Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("recipes/dbotter.toml");
    let source = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} must exist: {error}", path.display()));
    toml::from_str(&source)
        .unwrap_or_else(|error| panic!("{} must be valid TOML: {error}", path.display()))
}

#[test]
fn dbotter_pins_the_approved_preview_for_both_architectures() {
    let recipe = recipe();
    assert_eq!(recipe["name"].as_str(), Some("dbotter"));

    let binary = &recipe["arch"]["binary"];
    assert_eq!(binary["name"].as_str(), Some("dbotter"));

    let cases = [
        (
            "aarch64",
            "07f139a7d68f60159ed6a5c82807944aef69af6be6fb4248316bb67b581081a3",
        ),
        (
            "x86_64",
            "e397963b0b7be6df0016f526d4f29db1662b948091503fdc6837d382d6101ed6",
        ),
    ];

    for (arch, sha256) in cases {
        let asset = format!("dbotter-preview-linux-{arch}");
        assert_eq!(
            binary[arch]["url"].as_str(),
            Some(format!("{RELEASE_BASE}/{PREVIEW_TAG}/{asset}").as_str())
        );
        assert_eq!(binary[arch]["sha256"].as_str(), Some(sha256));
    }

    assert!(
        recipe.get("script").is_none(),
        "dbotter must use xbrew's checksum verifier, not a shell installer"
    );
}
