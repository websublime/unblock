//! D42 — the `issue create_bulk` rejections, driven over the REAL tool path with a spy `Storage`.
//!
//! The pure-parser cells live in `bulk_markdown.rs`'s unit module. These are the tool-level cells:
//! they assert the wire shape (`isError`, `code`, `context`) **and** `spy.mutation_count() == 0`,
//! which the parser tests structurally cannot.
//!
//! All three rejections close the same harm as the L7 seam's unknown-field rejection — a silent
//! drop returning `isError:false` — on a path `#[serde(deny_unknown_fields)]` cannot reach, because
//! the `markdown` body is an opaque `String` as far as serde is concerned.

mod common;

use common::{call_tool, connect, session_recording};
use serde_json::json;
use unblock_mcp::Quotas;

use crate::common::connect_with_quotas;

/// Drive `issue create_bulk` against a spy-backed session; return `(is_error, payload, mutations)`.
async fn create_bulk(markdown: &str) -> (bool, serde_json::Value, usize) {
    let (session, spy) = session_recording().await;
    let (client, server, _cancel) = connect_with_quotas(session, Quotas::default(), None).await;
    let (is_error, payload) = call_tool(
        &client,
        "issue",
        json!({ "action": "create_bulk", "markdown": markdown }),
    )
    .await;
    let mutations = spy.mutation_count();
    let _ = client.cancel().await;
    let _ = server.cancel().await;
    (is_error, payload, mutations)
}

// --- R4(i): an invalid `### Priority` is REJECTED, not silently defaulted to P2 -------------------

/// The core case. Before D42 `.and_then(|p| p.parse::<Priority>().ok())` collapsed `"URGENT"` to
/// `None`, and `write.rs`'s `unwrap_or_default()` turned that into `MEDIUM` (P2): the user asked for
/// a priority, got P2, and got NO error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_priority_is_rejected_with_zero_mutations() {
    let (is_error, payload, mutations) = create_bulk("## T\n### Priority\nURGENT\n").await;
    assert!(is_error, "an invalid `### Priority` is an in-band error");
    assert_eq!(payload["code"], "VALIDATION_FAILED");
    assert_eq!(payload["retryable"], true);
    assert_eq!(payload["context"]["kind"], "section_value");
    assert_eq!(payload["context"]["section"], "Priority");
    assert_eq!(payload["context"]["value"], "URGENT");
    assert_eq!(
        mutations, 0,
        "rejected strictly before Session::create_bulk"
    );
}

/// OUT-OF-RANGE, not merely non-numeric. `Priority::from_str` rejects `7` via its `Ok(p) if
/// !(0..=4)` arm — a `str::parse::<i32>`-only guard would accept it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn out_of_range_priority_is_rejected() {
    let (is_error, payload, mutations) = create_bulk("## T\n### Priority\n7\n").await;
    assert!(is_error, "priority 7 is out of the 0..=4 range");
    assert_eq!(payload["context"]["kind"], "section_value");
    assert_eq!(payload["context"]["value"], "7");
    assert_eq!(mutations, 0);
}

/// The error LOCALIZES the offending record. A 50-record document whose error says only "invalid
/// priority" is unactionable — the same usability failure as the silent drop it replaces.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn priority_rejection_names_the_offending_record() {
    let markdown =
        "## First\n### Type\ntask\n\n## Second\n### Priority\nURGENT\n\n## Third\n### Type\nbug\n";
    let (is_error, payload, _) = create_bulk(markdown).await;
    assert!(is_error);
    assert_eq!(payload["context"]["index"], 1, "0-based record index");
    assert_eq!(payload["context"]["title"], "Second");
}

/// NON-VACUITY for the three above: an ABSENT `### Priority` is still perfectly legal and still
/// yields the P2 default. `.transpose()` is what keeps "absent" distinct from "invalid" — without it
/// the rejection would fire on every record that simply omits the section.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn absent_priority_is_still_accepted_and_defaults() {
    let (is_error, payload, mutations) = create_bulk("## T\n### Type\ntask\n").await;
    assert!(!is_error, "an absent `### Priority` is legal: {payload}");
    let issues = payload["issues"].as_array().expect("issues array");
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0]["priority"], 2, "the model default, MEDIUM/P2");
    assert!(mutations > 0, "the happy path really did reach storage");
}

/// An unknown `### Type` is **PRESERVED**, not rejected: `IssueType::from_str` is infallible by
/// construction and yields `IssueType::Custom(s)`. This is what proves the `issue_type` half of the
/// shared helper is BEHAVIOUR-NEUTRAL hygiene — without it the fix could silently become a breaking
/// rejection of every custom type.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_issue_type_is_preserved_not_rejected() {
    let (is_error, payload, _) = create_bulk("## T\n### Type\nspike\n").await;
    assert!(
        !is_error,
        "an unknown `### Type` must NOT reject: {payload}"
    );
    assert_eq!(payload["issues"][0]["issue_type"], "spike");
}

// --- R4(ii) + R1-i: the structural / unknown-section rejections at the TOOL level -----------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn section_before_first_issue_is_rejected_with_zero_mutations() {
    let (is_error, payload, mutations) =
        create_bulk("### ID\nstand-in-1\n### Priority\n0\n## Real Title\n### Type\ntask\n").await;
    assert!(
        is_error,
        "before D42 this returned ONE issue and isError:false"
    );
    assert_eq!(payload["code"], "VALIDATION_FAILED");
    assert_eq!(payload["context"]["kind"], "section_before_issue");
    assert_eq!(payload["context"]["section"], "ID");
    assert_eq!(payload["context"]["line"], 1);
    assert_eq!(mutations, 0);
}

/// THE CONSISTENCY CELL. The same tool now rejects a misspelled ARGUMENT (`descriptionn` on
/// `create`) and a misspelled MARKDOWN SECTION (`### Descriptoin` on `create_bulk`) through the same
/// shape: in-band, `VALIDATION_FAILED`, retryable, a non-empty hint, `context.field == "markdown"`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_section_is_rejected_like_an_unknown_argument() {
    let (is_error, payload, mutations) = create_bulk("## T\n### Descriptoin\nX\n").await;
    assert!(is_error);
    assert_eq!(payload["code"], "VALIDATION_FAILED");
    assert_eq!(payload["retryable"], true);
    assert_eq!(payload["context"]["field"], "markdown");
    assert_eq!(payload["context"]["kind"], "unknown_section");
    let hint = payload["hint"].as_str().expect("an enumerating hint");
    assert!(
        hint.contains("description"),
        "the hint must enumerate the closed section set — it is published NOWHERE else on the \
         wire, and on a flattening MCP client it is one of only two signals that survive: {hint}"
    );
    assert_eq!(mutations, 0);
}

// --- MF-1: fenced code blocks, over the WIRE ------------------------------------------------------

/// **MF-1 arm (a) at the wire.** The regression the first D42 cut introduced: a document whose
/// section body embeds a markdown code sample containing a `### ` line was hard-rejected with
/// `isError:true`, `context.kind == "unknown_section"`, `line: 8`, and ZERO writes — while `main`
/// ACCEPTED it. The emitted hint ("use `#### `") was unactionable: the author controls their own
/// headings but not the bytes of a code example. This repo's own docs are full of `### ` in fences.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_document_embedding_a_fenced_h3_is_accepted_with_its_content_intact() {
    let markdown = "## Document the grammar\n\
                    ### Type\ntask\n\
                    ### Design\n\
                    The importer accepts:\n\
                    \n\
                    ```markdown\n\
                    ## Issue Title\n\
                    ### Bogus Section\n\
                    body\n\
                    ```\n\
                    \n\
                    …and rejects anything else.\n";
    let (is_error, payload, mutations) = create_bulk(markdown).await;
    assert!(
        !is_error,
        "a fenced `### ` is CONTENT — rejecting it is a false positive on shipped GA behaviour: \
         {payload}"
    );
    let issues = payload["issues"].as_array().expect("issues");
    assert_eq!(
        issues.len(),
        1,
        "the fenced `## ` must not split the record"
    );
    let design = issues[0]["design"].as_str().expect("design");
    assert!(design.contains("### Bogus Section"), "{design:?}");
    assert!(design.contains("## Issue Title"), "{design:?}");
    assert!(
        design.ends_with("…and rejects anything else."),
        "{design:?}"
    );
    assert!(mutations > 0, "the document really was written");
}

/// **MF-1 arm (b) at the wire — the SILENT one.** A *known* section name inside a fence used to tear
/// the fence in half and relocate the sample's bytes into another field with `isError:false`:
/// `design` ended `"example:\n```"` and `description` became `"INSIDE-FENCE\n```"`. Both fields must
/// now be one intact body, and `description` must come from the record's own prose only.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fenced_known_section_no_longer_relocates_content_across_fields() {
    let markdown = "## T\n### Design\nexample:\n```\n### Description\nINSIDE-FENCE\n```\n";
    let (is_error, payload, _) = create_bulk(markdown).await;
    assert!(!is_error, "{payload}");
    let issue = &payload["issues"][0];
    assert_eq!(
        issue["design"].as_str(),
        Some("example:\n```\n### Description\nINSIDE-FENCE\n```"),
        "the fence body must stay ONE intact field"
    );
    assert!(
        issue["description"].is_null(),
        "no content may be relocated into `description`: {}",
        issue["description"]
    );
}

/// The unterminated-fence rejection reaches the wire in-band with ZERO writes, naming the OPENING
/// line. NON-VACUITY for the two cells above: fence tracking did not degrade into "accept anything".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unterminated_fence_is_rejected_in_band_with_zero_mutations() {
    let markdown = "## T\n### Design\n```\ncode\n## Another\n### Type\ntask\n";
    let (is_error, payload, mutations) = create_bulk(markdown).await;
    assert!(is_error, "{payload}");
    assert_eq!(payload["code"], "VALIDATION_FAILED");
    assert_eq!(payload["retryable"], true);
    assert_eq!(payload["context"]["field"], "markdown");
    assert_eq!(payload["context"]["kind"], "unterminated_code_fence");
    assert_eq!(payload["context"]["line"], 3, "the OPENING line");
    assert_eq!(mutations, 0);
}

/// NON-VACUITY across the whole file: a well-formed multi-record document still creates issues, so
/// none of the rejections above is passing because `create_bulk` is broken outright.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_well_formed_document_still_creates_every_record() {
    let markdown = "## Alpha\n### Type\ntask\n### Priority\n1\n\n## Beta\n### Type\nfeature\n";
    let (is_error, payload, mutations) = create_bulk(markdown).await;
    assert!(!is_error, "{payload}");
    let issues = payload["issues"].as_array().expect("issues");
    assert_eq!(issues.len(), 2);
    assert_eq!(issues[0]["priority"], 1);
    assert!(mutations > 0);
}

/// Keep the D22 batch-cap verdict: the batch quota still fires BEFORE the fallible map, so an
/// over-cap document containing an invalid priority reports `kind:"batch"`, not `"section_value"`.
/// If this flips, the fix is to restore the step order — never to relax the assertion.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_batch_cap_still_wins_over_the_section_value_rejection() {
    let quotas = Quotas {
        max_batch: 1,
        ..Quotas::default()
    };
    let (session, spy) = session_recording().await;
    let (client, server, _cancel) = connect_with_quotas(session, quotas, None).await;
    let markdown = "## One\n### Priority\nURGENT\n\n## Two\n### Type\ntask\n";
    let (is_error, payload) = call_tool(
        &client,
        "issue",
        json!({ "action": "create_bulk", "markdown": markdown }),
    )
    .await;
    assert!(is_error);
    assert_eq!(
        payload["context"]["kind"], "batch",
        "step order is parse -> batch quota -> map; hoisting the map would reorder this verdict"
    );
    assert_eq!(spy.mutation_count(), 0);
    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

/// Scope guard: `connect` is exercised so the unused-import lint stays honest if the helpers move.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_bulk_over_a_plain_session_round_trips() {
    let session = common::session().await;
    let (client, server, _cancel) = connect(session).await;
    let (is_error, payload) = call_tool(
        &client,
        "issue",
        json!({ "action": "create_bulk", "markdown": "## Solo\n### Type\ntask\n" }),
    )
    .await;
    assert!(!is_error, "{payload}");
    let _ = client.cancel().await;
    let _ = server.cancel().await;
}
