//! Cross-module contract suite (plan §2 `tests/contract_versions.rs`; FR-12).
//!
//! Asserts that every contract schema string is present, unique, and listed by
//! [`contract_versions`], and pins the active set + the [`ContractEnvelope`] schema-first shape with
//! `insta`. In v1 the policy crate ships no per-contract output type yet (the
//! scheduler/coordination/gate contracts land in v1.1), so the active set is exactly the
//! [`POLICY_CONTRACT_VERSION`] family root; later releases extend it **additively** (a string is
//! never removed — this suite is where that guarantee is enforced).

use std::collections::BTreeSet;

use chrono::{TimeZone, Utc};

use unblock_policy::{Contract, ContractEnvelope, POLICY_CONTRACT_VERSION, contract_versions};

// A representative contract impl to prove the `Contract` trait + `for_contract` wiring; in v1 the
// real contract outputs are not yet defined (v1.1), so this stands in for the SCHEMA-const shape.
struct DemoContract;
impl Contract for DemoContract {
    const SCHEMA: &'static str = "unblock.demo.v1";
}

#[test]
fn versions_are_present_unique_and_sorted() {
    let versions = contract_versions();
    assert!(
        versions.contains(&POLICY_CONTRACT_VERSION),
        "the family root must be listed"
    );

    // Unique.
    let unique: BTreeSet<&str> = versions.iter().copied().collect();
    assert_eq!(unique.len(), versions.len(), "no duplicate schema strings");

    // Sorted/stable.
    let mut sorted = versions.clone();
    sorted.sort_unstable();
    assert_eq!(versions, sorted, "versions must be returned sorted");
}

#[test]
fn schema_const_is_reachable_via_trait() {
    assert_eq!(DemoContract::SCHEMA, "unblock.demo.v1");
}

#[test]
fn golden_contract_versions() {
    insta::assert_json_snapshot!(contract_versions());
}

#[test]
fn golden_envelope_schema_first_round_trip() {
    let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    let env = ContractEnvelope::for_contract::<DemoContract>(
        now,
        vec!["alpha".to_string(), "beta".to_string()],
    );

    // `schema` must be the leading JSON field (self-describing, FR-12).
    let json = serde_json::to_string(&env).expect("serializes");
    assert!(
        json.starts_with("{\"schema\":\"unblock.demo.v1\""),
        "schema must serialize first: {json}"
    );

    // Round-trips losslessly.
    let back: ContractEnvelope<Vec<String>> = serde_json::from_str(&json).expect("round-trips");
    assert_eq!(env, back);

    insta::assert_json_snapshot!(env);
}
