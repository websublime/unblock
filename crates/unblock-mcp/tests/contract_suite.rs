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
//! 3. **Taxonomy conformance (PRD §12.2):** over a LIVE `mcp_server_duplex_for_test` duplex the
//!    `list_tools` (≤ 8 — exactly 7, `create_bulk` is a discriminator NOT a tool) / resources (the
//!    CD-3 split: 4 concrete via `resources/list` + 1 `{id}` template via `resources/templates/list`)
//!    / prompts (3) sets EQUAL the pure `capabilities()` builder set (builder-vs-router parity — the
//!    builder cannot silently drift from what the server actually serves).

mod common;

use common::{call_tool, connect, session};
use rmcp::model::GetPromptRequestParams;
use serde_json::json;
use sha2::{Digest, Sha256};
use unblock_error::ErrorCode;
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

/// The LIVE server advertises exactly the 8 tools the pure `capabilities()` builder lists, in the same
/// set (builder-vs-router parity). The RK-3 budget is now FULL at 8 ≤ 8 (`comment` is the 8th tool,
/// D37; `create_bulk` remains an `issue`-tool discriminator, not a tool).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_list_tools_equals_the_builder_eight() {
    let session = session().await;
    let (client, server, _cancel) = connect(session).await;

    let tools = client.list_all_tools().await.expect("list_tools");
    assert_eq!(
        tools.len(),
        8,
        "exactly 8 tools (RK-3 budget FULL at 8 ≤ 8; create_bulk is a discriminator)"
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

/// CD-3 (MCP spec): the LIVE server splits its five resources by shape — the FOUR concrete
/// (non-parameterized) URIs ride `resources/list`, the ONE genuine `{id}` template rides
/// `resources/templates/list` — and the UNION still equals the pure `capabilities()` builder's five
/// resource URIs; plus the 3 prompts the builder lists (builder-vs-router parity across the split).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_resource_templates_and_prompts_match_the_builder() {
    let session = session().await;
    let (client, server, _cancel) = connect(session).await;

    // resources/templates/list holds ONLY the one genuine RFC-6570 template (CD-3).
    let templates = client
        .list_all_resource_templates()
        .await
        .expect("list_resource_templates");
    assert_eq!(templates.len(), 1, "exactly 1 resource template (CD-3)");
    let template_uris: Vec<String> = templates
        .iter()
        .map(|t| t.raw.uri_template.clone())
        .collect();
    assert_eq!(
        template_uris,
        vec!["unblock://issues/{id}".to_string()],
        "the only template is unblock://issues/{{id}} (CD-3)"
    );
    // Invariant: every advertised template carries a `{param}` placeholder (fails today for the four
    // concrete URIs that were mis-registered as templates before CD-3).
    for template in &templates {
        assert!(
            template.raw.uri_template.contains('{'),
            "a resources/templates/list entry MUST be parameterized, got `{}` (CD-3)",
            template.raw.uri_template
        );
    }

    // resources/list holds EXACTLY the four concrete (non-parameterized) URIs (CD-3).
    let resources = client.list_all_resources().await.expect("list_resources");
    assert_eq!(resources.len(), 4, "exactly 4 concrete resources (CD-3)");
    let mut concrete_uris: Vec<String> = resources.iter().map(|r| r.raw.uri.clone()).collect();
    concrete_uris.sort();
    assert_eq!(
        concrete_uris,
        vec![
            "unblock://capabilities".to_string(),
            "unblock://issues/blocked".to_string(),
            "unblock://issues/ready".to_string(),
            "unblock://schema".to_string(),
        ],
        "resources/list == the four concrete URIs (CD-3)"
    );
    // No concrete URI may carry a `{` (they are fully resolved, not templates).
    for resource in &resources {
        assert!(
            !resource.raw.uri.contains('{'),
            "a resources/list entry MUST be concrete, got `{}` (CD-3)",
            resource.raw.uri
        );
    }

    // The UNION of the split (4 concrete ∪ 1 template) equals the builder's five resource URIs.
    let mut live_union: Vec<String> = concrete_uris;
    live_union.extend(template_uris);
    live_union.sort();
    let mut built_uris: Vec<String> = capabilities()
        .resources
        .into_iter()
        .map(|r| r.uri)
        .collect();
    built_uris.sort();
    assert_eq!(
        live_union, built_uris,
        "resources/list ∪ resources/templates/list == the builder's five resource URIs (CD-3)"
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

// --------------------------------------------------------------------------------------------------
// MCP-conformance drift gates (CD-1 input root type / CD-2 structuredContent object) over a LIVE duplex.
// --------------------------------------------------------------------------------------------------

/// CD-1 (spine §5.2a, NORMATIVE): every LIVE `tools/list` entry's `inputSchema` root MUST carry
/// `"type": "object"`. rmcp 1.7 guards the tool *output* schema root (`schema_for_output`) but NOT the
/// input, so this invariant is unblock-owned: the six tagged-enum inputs earn it via
/// `#[schemars(extend("type" = "object"))]`; `ClaimInput` already conforms. A strict TS-SDK client
/// rejects the WHOLE `tools/list` if any single element omits the root `type`, so this asserts it for
/// ALL 8 tools over the real router (not just the pure bundle builder).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_tools_input_schema_root_is_object() {
    let session = session().await;
    let (client, server, _cancel) = connect(session).await;

    let tools = client.list_all_tools().await.expect("list_tools");
    assert_eq!(tools.len(), 8, "exactly 8 tools");
    for tool in &tools {
        assert_eq!(
            tool.input_schema.get("type"),
            Some(&json!("object")),
            "tool `{}` inputSchema root MUST be `type: object` (CD-1) — a strict MCP client rejects \
             the whole tools/list otherwise",
            tool.name
        );
    }

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

/// CD-2 (spine §5.3, NORMATIVE): a tool's structured success payload rides the rmcp
/// `CallToolResult.structuredContent`, whose MCP type is an OBJECT. The list-shaped arms
/// (`query`/`dep`/`issue`) MUST NOT serialize as a bare top-level array: they are object-wrapped
/// (`{"issues":[…]}` / `{"counts":[…]}` / `{"deps":[…]}` / `{"cycles":[…]}`). This drives the real
/// `query`-tool list path over the live duplex and asserts the wire value is a JSON object keyed
/// `issues`, never a bare array.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_query_list_structured_content_is_an_object() {
    let session = session().await;
    let (client, server, _cancel) = connect(session).await;

    // `query{list}` returns the (possibly empty) issue set — a list arm. Its structuredContent MUST be
    // the CD-2 object-wrap, not a bare array.
    let (is_error, out) = call_tool(&client, "query", json!({ "kind": "list" })).await;
    assert!(!is_error, "query list must succeed");
    assert!(
        out.is_object(),
        "structuredContent MUST be a JSON object (CD-2), got {out:?}"
    );
    assert!(
        out.get("issues")
            .and_then(serde_json::Value::as_array)
            .is_some(),
        "the object-wrapped list arm carries an `issues` array (CD-2), got {out:?}"
    );
    assert!(
        !out.is_array(),
        "structuredContent must never be a bare array (CD-2)"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

/// CD-2 for the `dep` tool's OWN wire path (its list arms are not exercised elsewhere): `dep{list}` and
/// `dep{cycles}` MUST each ride an object-wrapped `structuredContent` (`{"deps":[…]}` / `{"cycles":[…]}`),
/// never a bare array. Empty result sets still prove the wrap (the object shape is structural).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_dep_list_and_cycles_structured_content_are_objects() {
    let session = session().await;
    let (client, server, _cancel) = connect(session).await;

    // Seed one issue so `dep{list}` addresses a real id (an empty `deps` still object-wraps).
    let (is_error, created) = call_tool(
        &client,
        "issue",
        json!({ "action": "create", "title": "seed", "quick": true }),
    )
    .await;
    assert!(!is_error, "seed create must succeed");
    let id = created["id"].as_str().expect("minted id").to_string();

    let (is_error, deps) = call_tool(&client, "dep", json!({ "action": "list", "id": id })).await;
    assert!(!is_error, "dep list must succeed");
    assert!(
        deps.get("deps")
            .and_then(serde_json::Value::as_array)
            .is_some(),
        "dep list structuredContent MUST be object-wrapped `{{\"deps\":[…]}}` (CD-2), got {deps:?}"
    );
    assert!(
        !deps.is_array(),
        "dep list must never be a bare array (CD-2)"
    );

    let (is_error, cycles) = call_tool(&client, "dep", json!({ "action": "cycles" })).await;
    assert!(!is_error, "dep cycles must succeed");
    assert!(
        cycles
            .get("cycles")
            .and_then(serde_json::Value::as_array)
            .is_some(),
        "dep cycles structuredContent MUST be object-wrapped `{{\"cycles\":[…]}}` (CD-2), got {cycles:?}"
    );
    assert!(
        !cycles.is_array(),
        "dep cycles must never be a bare array (CD-2)"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

// --------------------------------------------------------------------------------------------------
// The 8th tool — `comment` (FR-6/D37) over the LIVE router.
// --------------------------------------------------------------------------------------------------

/// The full `comment` lifecycle over the live wire: add -> list -> update -> delete(redact),
/// pinning the CD-2 object-wrap and the D-E redact wire form.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_comment_add_list_update_delete_round_trip() {
    let session = session().await;
    let (client, server, _cancel) = connect(session).await;

    let (is_error, created) = call_tool(
        &client,
        "issue",
        json!({ "action": "create", "title": "seed", "quick": true }),
    )
    .await;
    assert!(!is_error, "seed create must succeed");
    let id = created["id"].as_str().expect("minted id").to_string();

    // add -> the single affected Comment (the scalar arm, not a wrap).
    let (is_error, added) = call_tool(
        &client,
        "comment",
        json!({ "action": "add", "issue_id": id, "body": "first" }),
    )
    .await;
    assert!(!is_error, "comment add must succeed: {added:?}");
    assert_eq!(added["text"], json!("first"), "the body rides under `text`");
    assert_eq!(added["issue_id"], json!(id));
    // MUST-1: add leaves updated_at NULL, and it skips-when-None on the wire.
    assert!(added.get("updated_at").is_none());
    assert!(added.get("redacted_at").is_none());
    let comment_id = added["id"].as_i64().expect("minted comment id");

    // list -> CD-2 object-wrapped `{"comments":[…]}`, NEVER a bare array.
    let (is_error, listed) = call_tool(
        &client,
        "comment",
        json!({ "action": "list", "issue_id": id }),
    )
    .await;
    assert!(!is_error, "comment list must succeed");
    assert!(
        listed
            .get("comments")
            .and_then(serde_json::Value::as_array)
            .is_some(),
        "comment list structuredContent MUST be object-wrapped `{{\"comments\":[…]}}` (CD-2), got \
         {listed:?}"
    );
    assert!(
        !listed.is_array(),
        "comment list must never be a bare array (CD-2)"
    );
    assert_eq!(listed["comments"].as_array().expect("array").len(), 1);

    // update -> provenance-preserving (D-D): the body changes AND updated_at appears.
    let (is_error, edited) = call_tool(
        &client,
        "comment",
        json!({ "action": "update", "comment_id": comment_id, "body": "revised" }),
    )
    .await;
    assert!(!is_error, "comment update must succeed: {edited:?}");
    assert_eq!(edited["text"], json!("revised"));
    assert!(
        edited.get("updated_at").is_some(),
        "the update MUST surface updated_at — the bump IS the provenance (D-D)"
    );

    // delete -> the D-E redact wire form: redacted_at present + "text":"" and NO extra bool.
    let (is_error, redacted) = call_tool(
        &client,
        "comment",
        json!({ "action": "delete", "comment_id": comment_id }),
    )
    .await;
    assert!(!is_error, "comment delete must succeed: {redacted:?}");
    assert_eq!(redacted["text"], json!(""), "the body is masked to \"\"");
    assert!(
        redacted.get("redacted_at").is_some(),
        "the PRESENCE of redacted_at is the flag"
    );
    assert!(
        redacted.get("redacted").is_none(),
        "there is NO extra top-level `redacted` bool (spine §5.3)"
    );

    // The row is KEPT — a soft-redact, never a hard delete.
    let (_, after) = call_tool(
        &client,
        "comment",
        json!({ "action": "list", "issue_id": id }),
    )
    .await;
    assert_eq!(
        after["comments"].as_array().expect("array").len(),
        1,
        "soft-redact KEEPS the row"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

/// A `comment add` on a missing issue surfaces the in-band FR-11 error with the REUSED
/// `ISSUE_NOT_FOUND` code (FORK-E1 — the `ErrorCode` taxonomy did not grow for D37).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_comment_add_missing_issue_is_in_band_issue_not_found() {
    let session = session().await;
    let (client, server, _cancel) = connect(session).await;

    let (is_error, payload) = call_tool(
        &client,
        "comment",
        json!({ "action": "add", "issue_id": "ub-nope", "body": "hi" }),
    )
    .await;
    assert!(is_error, "a missing issue must be an in-band domain error");
    assert_eq!(payload["code"], json!("ISSUE_NOT_FOUND"), "{payload:?}");

    // An empty body is the engine's ValidationFailed aggregate, in-band.
    let (is_error, payload) = call_tool(
        &client,
        "comment",
        json!({ "action": "add", "issue_id": "ub-nope", "body": "   " }),
    )
    .await;
    assert!(is_error);
    assert_eq!(payload["code"], json!("VALIDATION_FAILED"), "{payload:?}");

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

// --------------------------------------------------------------------------------------------------
// The F-5 non-vacuous ErrorCode cross-check (the code->{exit_code,retryable,hint_shape} parity pin).
// --------------------------------------------------------------------------------------------------

/// Every `capabilities().error_codes` entry round-trips its `code` STRING through serde BACK into
/// `unblock_error::ErrorCode` and matches the const-fn views (`exit_code`/`is_retryable`/`hint_shape`).
/// The serde wire round-trip is what makes it non-vacuous — it catches an `as_str`↔serde mismatch, a
/// wrong descriptor field, ordering loss, or a duplicate (NOT an in-crate `x == x` compare). 36
/// entries, unique, in `ErrorCode::ALL` declaration order.
#[test]
fn capabilities_error_map_round_trips_error_code_all() {
    let caps = capabilities();
    assert_eq!(caps.error_codes.len(), 36, "exactly 36 error-code entries");

    let mut seen = std::collections::HashSet::new();
    for (i, descriptor) in caps.error_codes.iter().enumerate() {
        // Parse the descriptor's `code` STRING back into an `ErrorCode` through serde (the wire path).
        let code: ErrorCode = serde_json::from_value(json!(descriptor.code)).unwrap_or_else(|_| {
            panic!(
                "descriptor code {:?} parses back to ErrorCode",
                descriptor.code
            )
        });

        assert!(seen.insert(code), "duplicate code {:?}", descriptor.code);
        assert_eq!(
            code,
            ErrorCode::ALL[i],
            "entries are in ErrorCode::ALL order"
        );
        assert_eq!(descriptor.exit_code, code.exit_code(), "exit_code parity");
        assert_eq!(
            descriptor.retryable,
            code.is_retryable(),
            "retryable parity"
        );
        assert_eq!(
            descriptor.hint_shape,
            code.hint_shape(),
            "hint_shape parity"
        );
    }
    assert_eq!(seen.len(), 36, "all 36 codes present, unique");
}

// --------------------------------------------------------------------------------------------------
// F-10 — the diagnostics{kind:version} finding pins mcp_contract_version.
// --------------------------------------------------------------------------------------------------

/// The live `diagnostics {"kind":"version"}` tool output carries `mcp_contract_version ==
/// CONTRACT_VERSION` as a finding (pins the `with_contract_version` wiring, `diagnostics.rs`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn diagnostics_version_reports_mcp_contract_version() {
    let session = session().await;
    let (client, server, _cancel) = connect(session).await;

    let (is_error, out) = call_tool(&client, "diagnostics", json!({ "kind": "version" })).await;
    assert!(!is_error, "diagnostics version must succeed");
    let findings = out["findings"].as_array().expect("findings array");
    let stamped = findings.iter().any(|f| {
        f["label"].as_str() == Some("mcp_contract_version")
            && f["detail"].as_str() == Some(CONTRACT_VERSION)
    });
    assert!(
        stamped,
        "a finding must carry mcp_contract_version == {CONTRACT_VERSION}, got {findings:?}"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

// --------------------------------------------------------------------------------------------------
// F-6 — the 3 prompt rendered-message goldens (GOLDEN-ONLY — deliberately NOT version-coupled:
// prompt message content is re-blessable without a CONTRACT_VERSION bump; the prompt SET/NAMES are
// parity-checked and hash-pinned; description strings only via their document copies). Going through
// `get_prompt` (not the pure `messages()` fns) pins the real wire surface incl. the #[prompt_router].
// --------------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompt_triage_golden() {
    let session = session().await;
    let (client, server, _cancel) = connect(session).await;
    let result = client
        .get_prompt(GetPromptRequestParams::new("triage"))
        .await
        .expect("get_prompt triage");
    insta::assert_json_snapshot!("prompt_triage", result.messages);
    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompt_plan_next_work_golden() {
    let session = session().await;
    let (client, server, _cancel) = connect(session).await;
    let result = client
        .get_prompt(GetPromptRequestParams::new("plan_next_work"))
        .await
        .expect("get_prompt plan_next_work");
    insta::assert_json_snapshot!("prompt_plan_next_work", result.messages);
    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompt_close_with_suggestions_golden() {
    let session = session().await;
    let (client, server, _cancel) = connect(session).await;
    let result = client
        .get_prompt(GetPromptRequestParams::new("close_with_suggestions"))
        .await
        .expect("get_prompt close_with_suggestions");
    insta::assert_json_snapshot!("prompt_close_with_suggestions", result.messages);
    let _ = client.cancel().await;
    let _ = server.cancel().await;
}
