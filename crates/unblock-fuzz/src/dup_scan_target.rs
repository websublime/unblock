//! The **`dup_scan`** fuzz core (D43) — a DIFFERENTIAL target over
//! `unblock_error::dup_key::scan`.
//!
//! # Why differential, and against what
//!
//! The scanner exists to answer "does this document carry a duplicate object key at any depth?" on
//! bytes that are about to be handed to `serde_json`. Two things can go wrong, and they need
//! different oracles:
//!
//! * **Under-rejection** — the scanner says `Clean` on a document that DOES carry a duplicate. That
//!   is the security failure. Oracle: an INDEPENDENT second implementation ([`RawJson`], below) that
//!   preserves duplicate members in a `Vec` instead of collapsing them into a `Map`, and therefore
//!   sees the duplicate directly.
//! * **Over-rejection** — the scanner says `Indeterminate` on bytes the SERVER WOULD HAVE HAPPILY
//!   PARSED, turning a legitimate request into an error. Oracle: rmcp's own
//!   `RxJsonRpcMessage<RoleServer>` parse of the SAME bytes must also fail. `serde_json` failing is
//!   necessary but NOT sufficient here — the scanner has to be judged against the parser the frame
//!   is actually routed through, which is why this core carries the `rmcp` dependency.
//!
//! Divergence is the failure mode that matters; neither oracle re-implements the scanner's
//! algorithm, so agreement is evidence rather than tautology.

use std::collections::HashSet;
use std::fmt;

use rmcp::RoleServer;
use rmcp::service::RxJsonRpcMessage;
use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use unblock_error::dup_key::{DupScan, scan};

use crate::FuzzError;

/// A JSON document that **preserves duplicate object members**.
///
/// `serde_json::Value` cannot express this input at all: its `Map` holds one entry per key, which is
/// precisely where the collapse under test happens. Keeping members in a `Vec` makes "this object
/// had two members with the same decoded name" directly observable — a genuinely different data
/// structure from the scanner's pooled `HashSet`, with no short-circuit and no path tracking.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum RawJson {
    /// `null`.
    Null,
    /// `true` / `false`.
    Bool(bool),
    /// Any number, kept as its decoded text-free f64 (the value is irrelevant here).
    Number(f64),
    /// A decoded string.
    String(String),
    /// An array.
    Array(Vec<RawJson>),
    /// An object, members in source order, DUPLICATES KEPT.
    Object(Vec<(String, RawJson)>),
}

impl RawJson {
    /// Every value stored under `name` at the ROOT of this document, in source order.
    ///
    /// Plural because duplicates survive here: a document with two `params` members yields both, and
    /// the scanner scans both (the fail-closed direction), so the oracle must consider both too.
    #[must_use]
    pub fn members_named(&self, name: &str) -> Vec<&RawJson> {
        match self {
            Self::Object(members) => members
                .iter()
                .filter(|(key, _)| key == name)
                .map(|(_, value)| value)
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Does any object in this document carry two members with the same decoded name?
    #[must_use]
    pub fn has_duplicate_member(&self) -> bool {
        match self {
            Self::Object(members) => {
                let mut seen: HashSet<&str> = HashSet::new();
                for (key, _) in members {
                    if !seen.insert(key.as_str()) {
                        return true;
                    }
                }
                members
                    .iter()
                    .any(|(_, value)| value.has_duplicate_member())
            }
            Self::Array(items) => items.iter().any(RawJson::has_duplicate_member),
            _ => false,
        }
    }
}

impl<'de> Deserialize<'de> for RawJson {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(RawJsonVisitor)
    }
}

struct RawJsonVisitor;

impl<'de> Visitor<'de> for RawJsonVisitor {
    type Value = RawJson;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("any JSON value")
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<RawJson, E> {
        Ok(RawJson::Bool(v))
    }
    #[allow(
        clippy::cast_precision_loss,
        reason = "the numeric VALUE is irrelevant to the duplicate-key property"
    )]
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<RawJson, E> {
        Ok(RawJson::Number(v as f64))
    }
    #[allow(
        clippy::cast_precision_loss,
        reason = "the numeric VALUE is irrelevant to the duplicate-key property"
    )]
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<RawJson, E> {
        Ok(RawJson::Number(v as f64))
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<RawJson, E> {
        Ok(RawJson::Number(v))
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<RawJson, E> {
        Ok(RawJson::String(v.to_string()))
    }
    fn visit_unit<E: de::Error>(self) -> Result<RawJson, E> {
        Ok(RawJson::Null)
    }
    fn visit_none<E: de::Error>(self) -> Result<RawJson, E> {
        Ok(RawJson::Null)
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<RawJson, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(RawJson::Array(items))
    }

    fn visit_map<A>(self, mut map: A) -> Result<RawJson, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut members = Vec::new();
        while let Some(key) = map.next_key::<String>()? {
            let value = map.next_value()?;
            members.push((key, value));
        }
        Ok(RawJson::Object(members))
    }
}

/// **`dup_scan`** — the D43 duplicate-key scanner is total, never over-rejects, and never
/// under-rejects.
///
/// Properties asserted on every input:
/// 1. the scan is TOTAL (it returns; libFuzzer reports a panic or a stack overflow as a crash);
/// 2. if the document parses, the verdict matches the independent [`RawJson`] walker EXACTLY —
///    `Clean` iff no duplicate exists, `Duplicate` iff one does, and never `Indeterminate`;
/// 3. if the document does NOT parse, the verdict is never `Clean` (fail-closed);
/// 4. `Indeterminate` implies rmcp's OWN `RxJsonRpcMessage<RoleServer>` parse of the same bytes also
///    fails — the scanner is never stricter than the parser the frame is routed through;
/// 5. the MCP scan root (`params`) is total, and — when the document parses — its verdict matches
///    the walker **on that subtree**. It is deliberately NOT coupled to the whole-document verdict:
///    see the comment at the assertion, and the counter-example this target found.
///
/// # Errors
///
/// Never returns `Err`; the signature is uniform with the other cores.
///
/// # Panics
///
/// **Deliberately** — on any divergence between the scanner and either oracle. A fuzz core reports
/// an invariant breach by panicking, which is what libFuzzer records as a crash; an `Err` here would
/// mean "this input did not reach the deep path", not "the code under test is wrong".
pub fn run_dup_scan_case(data: &[u8]) -> Result<(), FuzzError> {
    // (1) totality — over the WHOLE document (the `bd` scan root).
    let verdict = scan(data, &[]);

    // Strip the BOM the same way the scanner does, so the oracles see the same document.
    let body = data
        .strip_prefix(b"\xEF\xBB\xBF".as_slice())
        .unwrap_or(data);

    match serde_json::from_slice::<RawJson>(body) {
        Ok(document) => {
            // (2) exact agreement with the independent walker.
            let expected_duplicate = document.has_duplicate_member();
            match &verdict {
                DupScan::Duplicate { .. } => assert!(
                    expected_duplicate,
                    "OVER-REJECTION: the scanner reported a duplicate the independent walker does \
                     not see"
                ),
                DupScan::Clean => assert!(
                    !expected_duplicate,
                    "UNDER-REJECTION: the independent walker sees a duplicate the scanner reported \
                     CLEAN — this is the security failure the target exists to find"
                ),
                DupScan::Indeterminate => panic!(
                    "the document parses cleanly, so the verdict must be decidable, not \
                     INDETERMINATE"
                ),
            }
        }
        Err(_) => {
            // (3) fail-closed: an unparseable document is never CLEAN.
            assert!(
                !matches!(verdict, DupScan::Clean),
                "FAIL-OPEN: unparseable bytes must never be reported CLEAN"
            );
        }
    }

    // (4) the false-rejection guard, against the parser the frame is ACTUALLY routed through.
    if matches!(verdict, DupScan::Indeterminate) {
        assert!(
            serde_json::from_slice::<RxJsonRpcMessage<RoleServer>>(body).is_err(),
            "FALSE REJECTION: the scanner is INDETERMINATE on bytes rmcp itself parses happily — \
             the scanner must never be stricter than the parser the frame is routed through"
        );
    }

    // (5) the MCP scan root, judged against the walker on the SAME subtree.
    //
    // ⚠️ It is judged against the WALKER, not against the whole-document verdict, and that is not a
    // detail. The obvious-looking invariant "a duplicate inside `params` implies a duplicate of the
    // whole document" is **FALSE**, and this target found the counter-example within 60 seconds: the
    // two roots do not examine the same bytes. The whole-document scan must DECODE every string it
    // walks, so a frame carrying invalid UTF-8 in, say, `method` makes it INDETERMINATE — while the
    // `params` scan skips that member with `IgnoredAny` (which does not validate UTF-8) and reaches
    // a real duplicate inside `params`. Neither verdict implies the other; only the walker can
    // adjudicate, one subtree at a time.
    //
    // The divergence is harmless in production for the reason the whole design turns on: such a
    // frame fails rmcp's own parse too, so it is answered `-32700` and never reaches `call_tool`.
    let params_verdict = scan(data, &["params"]);
    if let Ok(document) = serde_json::from_slice::<RawJson>(body) {
        // Every occurrence, because the scanner scans every occurrence of a repeated root key.
        let subtrees = document.members_named("params");
        let expected = subtrees
            .iter()
            .any(|subtree| subtree.has_duplicate_member());
        match &params_verdict {
            DupScan::Duplicate { .. } => assert!(
                expected,
                "OVER-REJECTION at the `params` root: the scanner reported a duplicate the \
                 independent walker does not see in that subtree"
            ),
            DupScan::Clean => assert!(
                !expected,
                "UNDER-REJECTION at the `params` root: the walker sees a duplicate inside `params` \
                 that the scanner reported CLEAN — this is the MCP security failure"
            ),
            DupScan::Indeterminate => panic!(
                "the document parses cleanly, so the `params` verdict must be decidable, not \
                 INDETERMINATE"
            ),
        }
    }
    // When the document does NOT parse, no sound claim couples the two roots: the `params` scan may
    // legitimately be decisive on a subtree whose siblings are garbage. Totality is the only
    // property left, and reaching this line is what asserts it. Fail-open is not a risk there —
    // a document the walker cannot parse is one rmcp cannot parse either, so it never executes.

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{RawJson, run_dup_scan_case};
    use unblock_error::dup_key::{DupScan, scan};

    #[test]
    fn the_independent_walker_really_sees_duplicates() {
        // Non-vacuity for the ORACLE itself: if `RawJson` collapsed duplicates like `Value` does,
        // property (2) above would be a tautology and the whole target would prove nothing.
        let document: RawJson =
            serde_json::from_str(r#"{"a":1,"a":2}"#).expect("the raw walker parses");
        assert!(document.has_duplicate_member());
        let clean: RawJson = serde_json::from_str(r#"{"a":1,"b":2}"#).expect("parses");
        assert!(!clean.has_duplicate_member());
        let nested: RawJson = serde_json::from_str(r#"{"a":[{"b":1,"b":2}]}"#).expect("parses");
        assert!(nested.has_duplicate_member(), "at any depth");
    }

    #[test]
    fn the_core_is_total_over_a_hand_picked_corpus() {
        for input in [
            &b""[..],
            &b"{"[..],
            &br#"{"a":1,"a":2}"#[..],
            &br#"{"params":{"a":1,"a":2}}"#[..],
            &b"\xEF\xBB\xBF{\"a\":1}"[..],
            &b"[1,2,3]"[..],
            &b"\xff\xfe"[..],
        ] {
            run_dup_scan_case(input).expect("the core never errors");
        }
    }

    /// **The MINIMUM-DUPLICATE FLOOR (non-vacuity of the GENERATOR, not of the code).**
    ///
    /// Without an asserted floor, a generator that (by a distribution bug) never actually emits a
    /// duplicated key still reports the differential property as "passing" on every document it
    /// produced — while never once exercising the `Duplicate` arm. This drives a FIXED-SEED
    /// generator and asserts the arm is genuinely reached.
    #[test]
    fn a_seeded_run_reaches_the_duplicate_arm_often_enough() {
        // A tiny deterministic LCG: fixed seed, no `rand` dependency, byte-identical every run.
        let mut state: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) as u32
        };

        let keys = ["a", "b", "c", "id", "action"];
        let mut duplicates = 0usize;
        let mut cleans = 0usize;
        let iterations = 500usize;

        for _ in 0..iterations {
            let members = 2 + (next() % 4) as usize;
            let force_duplicate = next() % 2 == 0;
            let mut parts: Vec<String> = Vec::new();
            for index in 0..members {
                let key = if force_duplicate && index > 0 {
                    keys[0]
                } else {
                    keys[(next() as usize) % keys.len()]
                };
                parts.push(format!("\"{key}\":{index}"));
            }
            let document = format!("{{{}}}", parts.join(","));
            run_dup_scan_case(document.as_bytes()).expect("total");
            match scan(document.as_bytes(), &[]) {
                DupScan::Duplicate { .. } => duplicates += 1,
                DupScan::Clean => cleans += 1,
                DupScan::Indeterminate => panic!("generated documents are always well-formed"),
            }
        }

        assert!(
            duplicates >= iterations / 4,
            "the generator must actually reach the Duplicate arm — got {duplicates} of \
             {iterations}; a generator that never emits a duplicate passes the differential \
             property VACUOUSLY"
        );
        assert!(
            cleans >= iterations / 10,
            "and it must also produce clean documents — got {cleans} of {iterations}"
        );
    }
}
