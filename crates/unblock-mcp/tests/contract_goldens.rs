//! Golden snapshots of the pure discovery builders (FR-12): the capabilities document and the schema
//! bundle. These are the `contract_version` drift detectors — any tool/resource/prompt schema change
//! must re-bless these goldens AND bump `CONTRACT_VERSION` (the full FR-12 drift gate lands at T2.3;
//! T2.2 lands the builders + goldens). Pure (no `Session`).

use unblock_mcp::{CONTRACT_VERSION, capabilities, schema_bundle};

#[test]
fn capabilities_golden() {
    let caps = capabilities();
    assert_eq!(caps.contract_version, CONTRACT_VERSION);
    insta::assert_json_snapshot!("capabilities", caps);
}

#[test]
fn schema_bundle_golden() {
    let bundle = schema_bundle();
    assert_eq!(bundle.contract_version, CONTRACT_VERSION);
    insta::assert_json_snapshot!("schema_bundle", bundle);
}
