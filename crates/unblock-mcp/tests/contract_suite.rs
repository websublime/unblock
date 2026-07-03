//! FR-12 golden conformance + the ONE hash-coupled `CONTRACT_VERSION` drift gate (D22 widened by D25)
//! + the taxonomy-conformance golden closing PRD §12.2.
//!
//! Guarantees:
//! 1. **Snapshots** of the pure `capabilities()` + `schema_bundle()` builders (blessed HERE) — any
//!    tool/resource/prompt descriptor, error-map, or input/output-schema change shows up, and the two
//!    goldens localize WHICH document moved.
//! 2. **The ONE hash-coupled drift gate (non-vacuous both ways):** both builders stamp
//!    `CONTRACT_VERSION`, AND a SHA-256 over the ORDERED two-document tuple
//!    `(capabilities(), schema_bundle())` equals the pinned `CONTRACT_HASH`. So a content edit in
//!    EITHER document FORCES a `CONTRACT_VERSION` bump + a hash re-pin (not a silent golden re-bless),
//!    and a version bump without a document change is also caught (both documents embed
//!    `contract_version`).
//! 3. **Taxonomy conformance (PRD §12.2):** over a LIVE `serve_duplex_for_test` duplex the
//!    `list_tools` (≤ 8 — exactly 7, `create_bulk` is a discriminator NOT a tool) / resource-templates
//!    (5) / prompts (3) sets EQUAL the pure `capabilities()` builder set (builder-vs-router parity —
//!    the builder cannot silently drift from what the server actually serves).

mod common;

use common::{connect, session};
use sha2::{Digest, Sha256};
use unblock_mcp::{CONTRACT_HASH, CONTRACT_VERSION, capabilities, schema_bundle};

/// The canonical SHA-256 hex digest of the ordered two-document tuple `(capabilities(),
/// schema_bundle())` — the gate's computed half (D22 widened by D25).
fn contract_hash() -> String {
    use std::fmt::Write as _;
    let bytes =
        serde_json::to_vec(&(capabilities(), schema_bundle())).expect("serialize contract tuple");
    let digest = Sha256::digest(&bytes);
    digest.iter().fold(String::with_capacity(64), |mut acc, b| {
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

#[test]
fn contract_version_is_stamped_on_both_builders() {
    assert_eq!(capabilities().contract_version, CONTRACT_VERSION);
    assert_eq!(schema_bundle().contract_version, CONTRACT_VERSION);
}

/// The ONE hash-coupled gate: the pinned `CONTRACT_HASH` matches the live two-document digest. A
/// content edit in either discovery document changes the digest → this fails until BOTH
/// `CONTRACT_VERSION` and `CONTRACT_HASH` are bumped (non-vacuous: the digest is over the real
/// documents, and `contract_version` is a field of both, so a version bump alone also changes the
/// digest — the gate is non-vacuous in both directions).
#[test]
fn contract_hash_matches_the_pinned_gate() {
    assert_eq!(
        contract_hash(),
        CONTRACT_HASH,
        "the contract surface drifted: bump CONTRACT_VERSION + re-pin CONTRACT_HASH (+ re-bless the \
         capabilities/schema_bundle goldens — they identify WHICH document moved). The \
         FR-12/D22/D25 gate fires by design.",
    );
}

/// impl-plan AC(5), capabilities side, MECHANIZED: mutating ONE descriptor-copy string moves the
/// two-document digest. Self-contained (compares against a FRESH unmutated digest, not only the pin),
/// so it stays meaningful while the pin is stale during a re-pin window.
#[test]
fn capabilities_content_mutation_moves_the_gate() {
    let unmutated = contract_hash();

    let mut caps = capabilities();
    assert!(!caps.tools.is_empty(), "there is a descriptor to mutate");
    caps.tools[0].description.push('!');
    let mutated = {
        use std::fmt::Write as _;
        let bytes =
            serde_json::to_vec(&(caps, schema_bundle())).expect("serialize mutated contract tuple");
        let digest = Sha256::digest(&bytes);
        digest.iter().fold(String::with_capacity(64), |mut acc, b| {
            let _ = write!(acc, "{b:02x}");
            acc
        })
    };
    assert_ne!(
        mutated, unmutated,
        "a capabilities-document content mutation MUST move the two-document digest"
    );
}

#[test]
fn capabilities_golden() {
    insta::assert_json_snapshot!("capabilities", capabilities());
}

#[test]
fn schema_bundle_golden() {
    insta::assert_json_snapshot!("schema_bundle", schema_bundle());
}

// --------------------------------------------------------------------------------------------------
// Taxonomy conformance (PRD §12.2) — builder-vs-router parity over a LIVE duplex.
// --------------------------------------------------------------------------------------------------

/// The LIVE server advertises exactly the 7 tools the pure `capabilities()` builder lists, in the same
/// set (builder-vs-router parity). The count is ≤ 8; `create_bulk` is an `issue`-tool discriminator,
/// NOT an 8th tool.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_list_tools_equals_the_builder_seven() {
    let session = session().await;
    let (client, server, _cancel) = connect(session).await;

    let tools = client.list_all_tools().await.expect("list_tools");
    assert_eq!(
        tools.len(),
        7,
        "exactly 7 tools (≤ 8; create_bulk is a discriminator)"
    );

    let mut live: Vec<String> = tools.into_iter().map(|t| t.name.to_string()).collect();
    live.sort();
    let mut built: Vec<String> = capabilities().tools.into_iter().map(|t| t.name).collect();
    built.sort();
    assert_eq!(
        live, built,
        "live router tool set == the pure capabilities() builder set"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

/// The LIVE server advertises exactly the 5 resource templates + 3 prompts the builder lists.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_resource_templates_and_prompts_match_the_builder() {
    let session = session().await;
    let (client, server, _cancel) = connect(session).await;

    let templates = client
        .list_all_resource_templates()
        .await
        .expect("list_resource_templates");
    assert_eq!(templates.len(), 5, "exactly 5 resource templates");
    let mut live_uris: Vec<String> = templates.into_iter().map(|t| t.raw.uri_template).collect();
    live_uris.sort();
    let mut built_uris: Vec<String> = capabilities()
        .resources
        .into_iter()
        .map(|r| r.uri)
        .collect();
    built_uris.sort();
    assert_eq!(
        live_uris, built_uris,
        "live resource-template set == the builder set"
    );

    let prompts = client.list_all_prompts().await.expect("list_prompts");
    assert_eq!(prompts.len(), 3, "exactly 3 prompts");
    let mut live_prompts: Vec<String> = prompts.into_iter().map(|p| p.name).collect();
    live_prompts.sort();
    let mut built_prompts: Vec<String> =
        capabilities().prompts.into_iter().map(|p| p.name).collect();
    built_prompts.sort();
    assert_eq!(
        live_prompts, built_prompts,
        "live prompt set == the builder set"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}
