//! The model + error fuzz cores (`run_*_case`).
//!
//! These are the **stable, libFuzzer-free** logic for five targets; the nested `fuzz/fuzz_targets/`
//! wrappers are 5-line `fuzz_target!` shims over them, and `tests/regression.rs` replays the
//! committed corpus through them on stable. They mirror (and extend) the invariants in
//! `unblock-model`'s `tests/proptest_panic_safety.rs`.

// The cores assert their invariants (a breach = the bug libFuzzer reports). The `# Panics` section is
// therefore noise; the lint is scoped off for this module.
#![allow(clippy::missing_panics_doc)]

use serde_json::Value;

use unblock_error::{find_similar_ids, sanitize_message};
use unblock_model::{
    DependencyType, EventType, Issue, IssueType, Status, is_valid_id_format, parse_id,
};

use crate::FuzzError;
use crate::arbitraries::{arbitrary_issue, normalize_issue};
use crate::cursor::ByteCursor;
use crate::invariants::{assert_hash_well_formed, assert_issue_surface_well_formed};

/// **`content_hash`** — `compute_content_hash` is total, deterministic, transport-independent, and
/// field-scoped (spine §1.8).
///
/// We build a normalized (validatable) issue from the bytes, then prove the hash is recomputed
/// identically after the issue is serialized + re-parsed in **four transport forms** (compact,
/// pretty, key-reordered, CRLF-newlined). This is a real round-trip through
/// `serde_json::from_str::<Issue>(form).compute_content_hash()` — NOT a tautology over the in-memory
/// value — so it proves "recompute-on-load is transport-independent" (do **not** simplify it into a
/// `hash == hash` self-comparison). It also checks the included/excluded field scope.
///
/// # Errors
///
/// Returns [`FuzzError`] if an internal serialize/parse step fails (an environment problem, not an
/// input bug); a malformed *input* is handled gracefully and yields `Ok`.
pub fn run_content_hash_case(data: &[u8]) -> Result<(), FuzzError> {
    let mut cursor = ByteCursor::new(data);
    let issue = normalize_issue(arbitrary_issue(&mut cursor));

    let base = issue.compute_content_hash();
    assert_hash_well_formed(&base);
    // Determinism.
    assert_eq!(
        base,
        issue.compute_content_hash(),
        "hash must be deterministic"
    );

    // Transport-independence: serialize, re-shape the wire form four ways, re-parse, recompute.
    // `content_hash` is `#[serde(skip)]`, so the re-parsed issue's hash is freshly recomputed — the
    // round-trip proves the recompute does not depend on the transport encoding.
    let compact = serde_json::to_string(&issue)?;
    let pretty = serde_json::to_string_pretty(&issue)?;
    let reordered = reorder_top_level_keys(&compact)?;
    let crlf = compact.replace('\n', "\r\n");

    for form in [&compact, &pretty, &reordered, &crlf] {
        let reparsed: Issue = serde_json::from_str(form)?;
        assert_eq!(
            reparsed.compute_content_hash(),
            base,
            "recompute-on-load must be transport-independent ({form:?})"
        );
    }

    // Field-scope: an EXCLUDED field (estimated_minutes) never changes the hash; an INCLUDED field
    // (title) does (when it actually differs).
    let mut excluded = issue.clone();
    excluded.estimated_minutes = Some(
        excluded
            .estimated_minutes
            .unwrap_or(0)
            .wrapping_add(7)
            .abs(),
    );
    assert_eq!(
        excluded.compute_content_hash(),
        base,
        "estimated_minutes is excluded from the hash"
    );

    let mut included = issue.clone();
    included.title = format!("{}-changed", included.title);
    assert_ne!(
        included.compute_content_hash(),
        base,
        "a real title change must change the hash"
    );

    Ok(())
}

/// **`issue_ingest`** — `serde_json::from_slice::<Issue>` over arbitrary bytes never panics; a
/// surviving issue then survives the full read-side surface (validate / hash / `sync_equals` /
/// tombstone TTL).
///
/// # Errors
///
/// Never returns `Err` (a parse failure is the expected case for most inputs) — the signature is
/// uniform with the other cores.
pub fn run_issue_ingest_case(data: &[u8]) -> Result<(), FuzzError> {
    // Raw-bytes path: arbitrary bytes through the deserializer (Ok or Err, never a panic).
    if let Ok(issue) = serde_json::from_slice::<Issue>(data) {
        assert_issue_surface_well_formed(&issue);
        // validate is total (Ok or Err — both fine).
        let _ = unblock_model::IssueValidator::validate(&issue);
    }

    // Typed path: a normalized issue is always valid and round-trips through JSON.
    let mut cursor = ByteCursor::new(data);
    let normalized = normalize_issue(arbitrary_issue(&mut cursor));
    assert!(
        unblock_model::IssueValidator::validate(&normalized).is_ok(),
        "normalize_issue must always validate"
    );
    assert_issue_surface_well_formed(&normalized);
    Ok(())
}

/// **`parse_id`** — `parse_id` / `is_valid_id_format` over arbitrary UTF-8 never panic, and the two
/// agree (`is_valid_id_format(s) == parse_id(s).is_ok()`).
///
/// # Errors
///
/// Never returns `Err`; the signature is uniform with the other cores.
pub fn run_parse_id_case(data: &[u8]) -> Result<(), FuzzError> {
    let s = String::from_utf8_lossy(data);
    let parsed_ok = parse_id(&s).is_ok();
    assert_eq!(
        is_valid_id_format(&s),
        parsed_ok,
        "is_valid_id_format must agree with parse_id().is_ok()"
    );
    // A successfully parsed id round-trips to itself.
    if let Ok(parsed) = parse_id(&s) {
        assert_eq!(
            parse_id(&parsed.to_id_string()).as_ref(),
            Ok(&parsed),
            "a parsed id must re-parse to itself"
        );
    }
    Ok(())
}

/// **`enum_deserialize`** — the hand-rolled open-enum `Deserialize` over arbitrary strings (a) never
/// panics and (b) round-trips its wire form: `from_value(to_value(parsed)) == parsed` for all four
/// open enums (Status/IssueType/DependencyType/EventType).
///
/// We deliberately do **not** assert `as_str() == input`: the known-match is case-insensitive for
/// Status/IssueType/DependencyType (and `DependencyType` lowercases its `Custom` tail) but
/// case-sensitive for `EventType`, so such an assertion would false-fail. Mirrors
/// `unblock-model`'s `enum_deserialize_never_panics` proptest.
///
/// # Errors
///
/// Never returns `Err`; the signature is uniform with the other cores.
pub fn run_enum_deserialize_case(data: &[u8]) -> Result<(), FuzzError> {
    let s = String::from_utf8_lossy(data).into_owned();
    let value = Value::String(s);

    // The open-enum Deserialize is infallible for a JSON string (unknown → Custom). Each enum
    // round-trips its wire form: `from_value(to_value(parsed)) == parsed`. (No `as_str() == input`
    // assertion — see the doc above for why it would false-fail.)
    if let Ok(status) = serde_json::from_value::<Status>(value.clone()) {
        let again: Status = serde_json::from_value(serde_json::to_value(&status)?)?;
        assert_eq!(status, again, "Status wire form must round-trip");
    }
    if let Ok(issue_type) = serde_json::from_value::<IssueType>(value.clone()) {
        let again: IssueType = serde_json::from_value(serde_json::to_value(&issue_type)?)?;
        assert_eq!(issue_type, again, "IssueType wire form must round-trip");
    }
    if let Ok(dep_type) = serde_json::from_value::<DependencyType>(value.clone()) {
        let again: DependencyType = serde_json::from_value(serde_json::to_value(&dep_type)?)?;
        assert_eq!(dep_type, again, "DependencyType wire form must round-trip");
    }
    if let Ok(event_type) = serde_json::from_value::<EventType>(value) {
        let again: EventType = serde_json::from_value(serde_json::to_value(&event_type)?)?;
        assert_eq!(event_type, again, "EventType wire form must round-trip");
    }
    Ok(())
}

/// **`sanitize`** — `unblock_error::sanitize_message` over arbitrary text is total, leaks no raw
/// terminal-control byte (only `\n`/`\t` survive), and is **idempotent** on its own output. Also
/// exercises the bounded `find_similar_ids` help path (NFR-14 / the T0.4 deferred target).
///
/// # Errors
///
/// Never returns `Err`; the signature is uniform with the other cores.
pub fn run_sanitize_case(data: &[u8]) -> Result<(), FuzzError> {
    let text = String::from_utf8_lossy(data);

    let once = sanitize_message(&text).into_owned();
    // No raw control byte survives (the C1 block, ESC, BEL, CR, …) — only \n / \t.
    crate::invariants::assert_no_raw_control(&once);
    // Idempotent on already-sanitized text.
    let twice = sanitize_message(&once).into_owned();
    assert_eq!(once, twice, "sanitize_message must be idempotent");

    // The hint help path: a raw not-found id reaches `find_similar_ids` before validation. It must be
    // total (no panic) and bounded for any input.
    let candidates = vec![
        "ub-abc123".to_string(),
        "ub-abc124".to_string(),
        text.chars().take(40).collect::<String>(),
    ];
    let hits = find_similar_ids(&text, &candidates, 5);
    assert!(hits.len() <= 5, "find_similar_ids respects the max cap");

    Ok(())
}

/// Reparse `compact` JSON, sort its top-level object keys, and re-serialize — a key-reordered (but
/// semantically identical) transport form for the content-hash round-trip.
fn reorder_top_level_keys(compact: &str) -> Result<String, FuzzError> {
    let mut value: Value = serde_json::from_str(compact)?;
    if let Value::Object(map) = &mut value {
        // serde_json::Map preserves insertion order with the default features; rebuild it sorted.
        let mut sorted: serde_json::Map<String, Value> = serde_json::Map::new();
        let mut keys: Vec<String> = map.keys().cloned().collect();
        keys.sort();
        for key in keys {
            if let Some(v) = map.remove(&key) {
                sorted.insert(key, v);
            }
        }
        value = Value::Object(sorted);
    }
    Ok(serde_json::to_string(&value)?)
}

#[cfg(test)]
mod tests {
    use super::{
        run_content_hash_case, run_enum_deserialize_case, run_issue_ingest_case, run_parse_id_case,
        run_sanitize_case,
    };

    #[test]
    fn cores_never_panic_on_empty_and_garbage() {
        let inputs: &[&[u8]] = &[b"", b"\0", b"{", b"not json", &[0xffu8; 64]];
        for input in inputs {
            run_content_hash_case(input).expect("content_hash core ok");
            run_issue_ingest_case(input).expect("issue_ingest core ok");
            run_parse_id_case(input).expect("parse_id core ok");
            run_enum_deserialize_case(input).expect("enum_deserialize core ok");
            run_sanitize_case(input).expect("sanitize core ok");
        }
    }
}
