//! Guards for the deployable bundle in `dist/`.
//!
//! These are text/artifact checks, not runtime tests: they keep the deployment
//! guide's version in step with the crate and make sure a live credential file
//! is never shipped.

use std::path::Path;

#[test]
fn install_guide_matches_the_package_version() {
    let guide = include_str!("../dist/INSTALL.md");
    let expected = format!("**Version:** {}", env!("CARGO_PKG_VERSION"));
    assert!(
        guide.contains(&expected),
        "dist/INSTALL.md must state {expected}"
    );
}

#[test]
fn install_guide_covers_every_supported_client() {
    let guide = include_str!("../dist/INSTALL.md");
    for client in ["Claude Code", "OpenCode", "DeepSeek Harness"] {
        assert!(
            guide.contains(client),
            "dist/INSTALL.md must document {client}"
        );
    }
}

#[test]
fn bundle_ships_a_template_not_a_live_credential_file() {
    let dist = Path::new(env!("CARGO_MANIFEST_DIR")).join("dist");
    assert!(
        !dist.join(".credentials").exists(),
        "dist/ must never contain a live .credentials file"
    );

    let template = include_str!("../dist/credentials.example");
    assert!(
        template.contains("replace-with-your-key"),
        "dist/credentials.example must stay a placeholder template"
    );
}
