//! NFR-15 guard: `unblock-error` is the deepest leaf and must have **zero** internal `unblock-*`
//! dependencies. This is belt-and-suspenders to the workspace-level `cargo xtask check-layering`.

#[test]
fn manifest_declares_no_internal_unblock_dependency() {
    let manifest = include_str!("../Cargo.toml");

    // Scan every non-comment line that references another workspace crate. The only legitimate
    // `unblock-` token is this crate's own package name on the `name = "unblock-error"` line.
    for line in manifest.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            continue;
        }
        if trimmed.starts_with("name =") {
            continue;
        }
        assert!(
            !line.contains("unblock-"),
            "unblock-error must not depend on any internal crate, found: {line}"
        );
    }
}
