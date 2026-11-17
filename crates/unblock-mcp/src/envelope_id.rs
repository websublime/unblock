//! The ENVELOPE-`id` scan — a `serde::de::DeserializeSeed` over the ROOT object that recovers the
//! envelope `id` from a frame's RAW BYTES. First minted for D47; since D59 it is D54's leg (c): the
//! id source for the reply [`crate::wire`] writes to a line that fails the typed parse, where it runs
//! only on lines the strict-JSON gate accepted.
//!
//! # Why the bytes are the only place the information exists
//!
//! A line that fails rmcp's typed parse never becomes a message at all, so by the time the transport
//! holds the failure the only surviving copy of the id is the raw line. rmcp 3.10 decodes by key
//! PRESENCE (`rmcp-3.10.0/src/model.rs:907`), so a `method`+`id` frame is a Request or nothing; the
//! rmcp 1.7 path on which an id-carrying frame fell through to the Notification variant, D47's
//! class, is gone (PRD §4 D59 clause (3)).
//!
//! # Why this is a separate module from [`crate::wire`]
//!
//! `wire`'s module doc is entirely about FORKING rmcp's private framing helpers, and it is pinned
//! as such. This scan forks nothing — it is our own addition — so homing it there would make that
//! doc false.
//!
//! # Three normative properties
//!
//! 1. **Keys are compared DECODED, never as raw spans.** [`unblock_error::dup_key`] states this
//!    normatively for the sibling scanner and the reason is identical here: the member name
//!    `"\u0069d"` — the key `id` whose first character is written as a `\u` escape — decodes to `id`
//!    and IS a genuine envelope id member. A byte prefilter for the literal bytes `"id"` is
//!    FORBIDDEN — unsound (it misses that spelling) and measured SLOWER than the seed it would
//!    avoid.
//! 2. **ROOT LEVEL ONLY.** Every non-`id` member is consumed with `IgnoredAny` without descending,
//!    so `params.id`, `params.arguments.id` and `params[0].id` are invisible. This is the OPPOSITE
//!    discipline from `dup_key`, which descends at any depth, and it is deliberate: the JSON-RPC
//!    envelope id is a root member by definition.
//! 3. **No short-circuit.** The walk always reaches the end of the root object. That is exactly the
//!    property a collector fused into `dup_key`'s root-key loop would lose — that scanner unwinds
//!    with an `Err` on the first duplicate it finds, so a frame carrying a duplicate inside `params`
//!    and a TRAILING `id` would never reach the `id`.
//!
//! # The fail-open direction is the OPPOSITE of D43's, on purpose
//!
//! `dup_key`'s normative rule is that `Indeterminate` is never `Clean` — fail-CLOSED, because that
//! verdict gates EXECUTION. Here a tokenizer failure yields [`EnvelopeId::Absent`], i.e. no id. This
//! verdict gates only which id a REPLY carries, to a frame on which nothing executes either way. The
//! arm is also unreachable in practice, because the caller runs [`scan`] only on a line whose typed
//! parse failed but whose `Value` parse succeeded (the strict-JSON gate). A seed failure there would
//! yield `Absent`, i.e. rule (γ)'s id-less reply — still the fail-safe direction.

use rmcp::model::RequestId;
use serde::Deserialize;
use serde::de::{self, DeserializeSeed, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use std::fmt;

/// The result of scanning a frame's RAW BYTES for top-level `id` members.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EnvelopeId {
    /// No top-level `id` member (or a non-object root).
    ///
    /// On the parse-error arm, `Absent` means "no id to recover": the reply is rule (γ)'s id-less
    /// `-32600` (D59 clause (2)).
    Absent,
    /// One or more `id` members, ALL mutually equal, whose common value `RequestId` accepts.
    ///
    /// The "unambiguously recovered" case: the reply rides this id, which is the only spelling that
    /// resolves a waiting rmcp peer (its client DROPS an error carrying no id).
    Recovered(RequestId),
    /// An `id` exists but cannot be answered on: two or more occurrences with DIFFERENT values, or
    /// a uniform value `RequestId` rejects (`null`, object, array, bool, a non-integer number, an
    /// integer outside i64, or a float-form integer at or past 2^53).
    Unusable,
}

/// Collect every top-level `id` member's value. Bounded, monotone, no short-circuit.
struct IdCollector {
    /// The FIRST top-level `id` value, verbatim. At most ONE value is ever retained, so memory is
    /// O(one id value) rather than O(occurrences x size).
    first: Option<serde_json::Value>,
    /// Set once a later occurrence differs from `first`. **Monotone: never cleared** — a late
    /// disagreement must still poison an already-equal run.
    ambiguous: bool,
}

/// Scan `line` for top-level `id` members and decide the trichotomy.
///
/// Recoverability delegates to rmcp's OWN `RequestId` deserializer rather than to a hand-written
/// type table, so agreement with the real decoder is structural. It is that impl which rejects
/// `null`/object/array/bool ("Expect number or string"), non-integers and float-form integers at or
/// past 2^53 ("Expected an integer"), and values outside `i64` ("Number too large for i64"); since
/// rmcp 3.10 it ACCEPTS a float-form integer below 2^53 (`1.0`, `1e2`) as that integer
/// (`rmcp-3.10.0/src/model.rs:378-397`).
///
/// One caller: the transport's parse-error arm, which runs it only behind the strict-JSON gate,
/// because this scan is NOT a JSON validator — see the `end()` note in the body.
pub(crate) fn scan(line: &[u8]) -> EnvelopeId {
    // Strip exactly ONE prefix BOM, reusing the transport's constant. Scanner and parser must see
    // the same document; two copies of that constant is precisely how they drift apart.
    let body = line
        .strip_prefix(crate::wire::UTF8_BOM.as_slice())
        .unwrap_or(line);

    let mut collector = IdCollector {
        first: None,
        ambiguous: false,
    };
    let mut deserializer = serde_json::Deserializer::from_slice(body);
    let seeded = IdSeed {
        collector: &mut collector,
    }
    .deserialize(&mut deserializer);

    // `deserializer.end()` is deliberately NOT called. The caller guarantees the input was ALREADY
    // accepted by `serde_json`: it runs on a line its strict `Value` parse accepted
    // (`wire::ParseFailure::NotAMessage`). That is also why this
    // scan must never BE the strictness oracle: `IgnoredAny` checks no nesting depth (serde_json's
    // `ignore_value` keeps its own stack), validates neither UTF-8 nor surrogate escapes inside the
    // strings it skips, and range-checks no number, so on an unvalidated line it recovers ids from
    // text that is not JSON. (`dup_key` needs `end()` because it runs BEFORE any parse.)

    if seeded.is_err() {
        return EnvelopeId::Absent;
    }
    let Some(first) = collector.first else {
        return EnvelopeId::Absent;
    };
    if collector.ambiguous {
        return EnvelopeId::Unusable;
    }
    match RequestId::deserialize(first) {
        Ok(id) => EnvelopeId::Recovered(id),
        Err(_) => EnvelopeId::Unusable,
    }
}

/// Drives [`IdVisitor`] over the document root.
struct IdSeed<'a> {
    collector: &'a mut IdCollector,
}

impl<'de> DeserializeSeed<'de> for IdSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<(), D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(IdVisitor {
            collector: self.collector,
        })
    }
}

/// Collects root-level `id` members; every other shape is drained without inspection.
struct IdVisitor<'a> {
    collector: &'a mut IdCollector,
}

impl<'de> Visitor<'de> for IdVisitor<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("any JSON value")
    }

    // A non-object root has no `id` member to find. Every scalar is simply accepted.
    fn visit_bool<E: de::Error>(self, _v: bool) -> Result<(), E> {
        Ok(())
    }
    fn visit_i64<E: de::Error>(self, _v: i64) -> Result<(), E> {
        Ok(())
    }
    fn visit_u64<E: de::Error>(self, _v: u64) -> Result<(), E> {
        Ok(())
    }
    fn visit_f64<E: de::Error>(self, _v: f64) -> Result<(), E> {
        Ok(())
    }
    fn visit_str<E: de::Error>(self, _v: &str) -> Result<(), E> {
        Ok(())
    }
    fn visit_unit<E: de::Error>(self) -> Result<(), E> {
        Ok(())
    }
    fn visit_none<E: de::Error>(self) -> Result<(), E> {
        Ok(())
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<(), A::Error>
    where
        A: SeqAccess<'de>,
    {
        // A root ARRAY has no envelope id, but it must still be DRAINED rather than returned from
        // early, or the parser reports trailing characters.
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(())
    }

    fn visit_map<A>(self, mut map: A) -> Result<(), A::Error>
    where
        A: MapAccess<'de>,
    {
        let collector = self.collector;
        // `next_key::<String>()` yields the key AFTER escape resolution, which is the whole of
        // property 1: the member name `"\u0069d"` arrives here as the key `id`.
        while let Some(key) = map.next_key::<String>()? {
            if key != "id" {
                // Property 2: consumed WITHOUT descending, so nested `id` members stay invisible.
                map.next_value::<IgnoredAny>()?;
                continue;
            }
            if collector.first.is_none() {
                collector.first = Some(map.next_value::<serde_json::Value>()?);
            } else if collector.ambiguous {
                // The verdict can no longer change, so the value need not be materialised.
                map.next_value::<IgnoredAny>()?;
            } else {
                let next = map.next_value::<serde_json::Value>()?;
                // Equality is decided on the DECODED value, never on the raw spans. A raw-span
                // comparator would call `"a"` and its `\u`-escaped spelling two different ids,
                // report ambiguity, omit the id, and hang the client the recovery rule exists to
                // release. It is invisible to every other corpus entry, because every other equal
                // pair is byte-identical.
                if collector.first.as_ref() != Some(&next) {
                    collector.ambiguous = true;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{EnvelopeId, scan};
    use crate::envelope_id_corpus::id_corpus;
    use proptest::prelude::*;
    use rmcp::model::{NumberOrString, RequestId};
    use serde::Deserialize as _;

    /// Fetch one envelope-`id` corpus frame by entry id.
    fn frame(entry: &str) -> Vec<u8> {
        id_corpus()
            .into_iter()
            .find(|f| f.id == entry)
            .unwrap_or_else(|| panic!("corpus entry {entry} is missing"))
            .frame
    }

    fn num(n: i64) -> EnvelopeId {
        EnvelopeId::Recovered(RequestId::Number(n))
    }

    fn text(s: &str) -> EnvelopeId {
        EnvelopeId::Recovered(NumberOrString::String(s.into()))
    }

    /// **A1** — a frame with NO root `id` is `Absent`.
    ///
    /// Mutant: reporting `Unusable` for a frame that names no id at all.
    #[test]
    fn scan_is_absent_without_a_root_id() {
        assert_eq!(
            scan(br#"{"jsonrpc":"2.0","method":"notifications/foo","params":{}}"#),
            EnvelopeId::Absent
        );
    }

    /// **A2** — a LONE valid id is recovered.
    ///
    /// On the wire this is D54's headline: a strict-JSON line with one readable id that fails the
    /// typed parse for another reason (a duplicated `method`, a scalar `params`) is answered on it.
    ///
    /// Mutant: requiring `occurrences >= 2` before recovering (`M22`) — killed by this cell ONLY.
    #[test]
    fn scan_recovers_a_lone_valid_id() {
        assert_eq!(scan(br#"{"jsonrpc":"2.0","id":1,"method":"x"}"#), num(1));
    }

    /// **A3** — occurrences that are ALL EQUAL are unambiguous, however many there are.
    ///
    /// Mutant: replacing the all-equal test with `occurrences == 1` (`M8`).
    #[test]
    fn scan_recovers_equal_duplicates() {
        assert_eq!(scan(&frame("D01")), num(90001));
        assert_eq!(scan(&frame("D02")), text("e2e-abc"));
        assert_eq!(scan(&frame("D03")), num(90003));
        assert_eq!(scan(&frame("D15")), num(90015));
    }

    /// **A4** — DIFFERING occurrences are unusable, and ambiguity is monotone.
    ///
    /// Mutant: replacing the all-equal test with `true`, i.e. taking the first occurrence (`M7`).
    #[test]
    fn scan_is_unusable_on_differing_duplicates() {
        assert_eq!(scan(&frame("D04")), EnvelopeId::Unusable);
        assert_eq!(scan(&frame("D05")), EnvelopeId::Unusable);
    }

    /// **A5** — a wrongly-TYPED id is unusable.
    ///
    /// Mutant: replacing `RequestId::deserialize` with "stringify any value" (`M9`).
    #[test]
    fn scan_is_unusable_on_a_wrong_typed_id() {
        for entry in ["D06", "D07", "D08", "D09"] {
            assert_eq!(
                scan(&frame(entry)),
                EnvelopeId::Unusable,
                "{entry} must be Unusable"
            );
        }
        // The EMPTY array, which the corpus does not carry: D09 spells the array case `[1]`, a
        // different byte shape reaching the same `RequestId` rejection. Asserted here rather than
        // added to the shared corpus because it grades nothing at the wire — the two frames are
        // answered with identical bytes — so a corpus entry would cost every duplex and CLI tier a
        // round trip to re-prove one library decision.
        assert_eq!(
            scan(br#"{"jsonrpc":"2.0","id":[],"method":"ping"}"#),
            EnvelopeId::Unusable,
            "an EMPTY array id is no representable RequestId either"
        );
    }

    /// **A6** — a number outside what `RequestId` accepts is unusable, and the neighbours that ARE
    /// accepted are asserted alongside so this pins a BOUNDARY rather than a blanket rejection.
    ///
    /// Since D59 (rmcp 3.10, `rmcp-3.10.0/src/model.rs:378-397`) a float-form integer below 2^53 IS
    /// accepted as that integer (D10, D11, D26), while a non-integer (D24) and 2^53 itself (D25)
    /// stay unusable.
    ///
    /// Mutants: accepting any f64 by truncating (killed by D24/D25); rejecting every float form
    /// (killed by D10/D11/D26); widening the integer test to `i128` (`M11`, killed by D12/D13).
    #[test]
    fn scan_is_unusable_on_a_non_i64_number() {
        assert_eq!(
            scan(&frame("D10")),
            num(100),
            "1e2 is the integer 100 on rmcp 3.10"
        );
        assert_eq!(
            scan(&frame("D11")),
            num(100),
            "100.0 is the integer 100 on rmcp 3.10"
        );
        assert_eq!(
            scan(&frame("D26")),
            num(1),
            "1.0 is the integer 1 on rmcp 3.10"
        );
        for entry in ["D12", "D13", "D24", "D25"] {
            assert_eq!(
                scan(&frame(entry)),
                EnvelopeId::Unusable,
                "{entry} must be Unusable"
            );
        }
        // The in-range neighbours, which must still be RECOVERED.
        assert_eq!(
            scan(br#"{"jsonrpc":"2.0","id":100,"method":"ping","params":{}}"#),
            num(100),
            "the same number as D10 written without an exponent must be recoverable"
        );
        let max = format!(r#"{{"jsonrpc":"2.0","id":{},"method":"ping"}}"#, i64::MAX);
        assert!(
            matches!(scan(max.as_bytes()), EnvelopeId::Recovered(_)),
            "i64::MAX must be recoverable — D12 is one PAST it"
        );
        let min = format!(r#"{{"jsonrpc":"2.0","id":{},"method":"ping"}}"#, i64::MIN);
        assert!(
            matches!(scan(min.as_bytes()), EnvelopeId::Recovered(_)),
            "i64::MIN must be recoverable — D13 is one BELOW it"
        );
    }

    /// **A7** — KEYS are compared DECODED: the spelling `"\u0069d"` IS a root `id` member.
    ///
    /// Mutant: `map.next_key::<String>()` replaced by a raw-span `== b"id"` compare (`M5`), or a
    /// four-byte-window prefilter trusted in the negative direction (`M5b`). Under either, D15's
    /// BOTH-escaped occurrences are invisible, the scan reports `Absent`, and the frame goes silent.
    #[test]
    fn scan_decodes_keys() {
        assert_eq!(scan(&frame("D14")), EnvelopeId::Unusable);
        assert_eq!(scan(&frame("D15")), num(90015));
    }

    /// **A8** — a NESTED `id` is not an envelope id, at any depth and inside arrays too.
    ///
    /// Mutant: recursing into nested objects instead of `IgnoredAny` (`M6`).
    #[test]
    fn scan_ignores_nested_id_members() {
        assert_eq!(
            scan(
                br#"{"jsonrpc":"2.0","method":"notifications/foo","params":{"id":1,"a":{"id":2}}}"#
            ),
            EnvelopeId::Absent
        );
        assert_eq!(
            scan(br#"{"jsonrpc":"2.0","method":"notifications/foo","params":[{"id":3}]}"#),
            EnvelopeId::Absent
        );
    }

    /// **A14** — the ROOT SHAPES that are not an object: every scalar root, an array root, and an
    /// `id` sitting inside a ROOT-LEVEL array element are ALL `Absent`.
    ///
    /// A root-level array element is NOT what A8 drives: that cell nests its array under `params`,
    /// so the root is still an object and [`IdVisitor::visit_map`] still runs. These lines are the
    /// only ones in the module that reach the visitor's scalar arms and [`IdVisitor::visit_seq`] at
    /// all.
    ///
    /// **What this cell does NOT do, stated because "it exercises them" reads as "it grades them":
    /// it does not grade those arms.** Delete every scalar arm and `visit_seq` and the verdict is
    /// UNCHANGED — `deserialize_any` then fails with an `invalid_type` error and [`scan`] maps a
    /// failed seed to `Absent` too (the `seeded.is_err()` guard), so arm-deletion is an EQUIVALENT
    /// mutant by construction, not a hole a byte corpus could close. What the cell does pin is that
    /// a non-object root NEVER yields an id, so its reply is always rule (γ)'s id-less one.
    ///
    /// One arm is unreachable even here, and saying so is cheaper than leaving a reader to
    /// rediscover it: `visit_none` is never called by `serde_json`'s `deserialize_any`, which
    /// renders JSON `null` through `visit_unit`. `visit_none` belongs to the `Option` protocol and
    /// no `Option` is ever deserialized on this path.
    #[test]
    fn scan_is_absent_on_a_non_object_root() {
        for line in [
            &b"null"[..],
            &b"true"[..],
            &b"42"[..],
            &b"-1"[..],
            &b"1.5"[..],
            &b"18446744073709551615"[..],
            // A STRING root that spells the key itself: the scan is structural, never a substring
            // hunt for the bytes `id`.
            &br#""id""#[..],
            &b"[]"[..],
            &b"[1,2,3]"[..],
            // The plan's root-level array ELEMENT: `id` at depth 1 of an ARRAY root, which is the
            // one root shape neither A1 nor A8 reaches.
            &br#"[{"id":3}]"#[..],
        ] {
            assert_eq!(
                scan(line),
                EnvelopeId::Absent,
                "a non-object root has no envelope id: {}",
                String::from_utf8_lossy(line)
            );
        }
    }

    /// **A9** — exactly ONE prefix BOM is stripped, mirroring the parser.
    ///
    /// A DOUBLE BOM is `Absent`: the second is not stripped, so the document never parses — which
    /// is the same thing the real parse does with it, and the point of sharing one constant.
    ///
    /// Mutant: deleting the BOM strip (`M4`).
    #[test]
    fn scan_strips_exactly_one_bom() {
        assert_eq!(scan(&frame("D19")), num(90019));

        let mut double = b"\xEF\xBB\xBF\xEF\xBB\xBF".to_vec();
        double.extend_from_slice(br#"{"jsonrpc":"2.0","id":90019,"id":90019,"method":"ping"}"#);
        assert_eq!(
            scan(&double),
            EnvelopeId::Absent,
            "a SECOND BOM is not stripped, so the document does not parse"
        );
    }

    /// **A11** — the collector never short-circuits and is never prefix-bounded.
    ///
    /// Mutation (concrete, at the collector, but deliberately carrying no catalogue number because
    /// it is a shape rather than a temptation): any early return once one occurrence is seen, or
    /// any scan bounded to a prefix of the line. D20's second occurrence sits 100 KiB past the
    /// first.
    #[test]
    fn scan_never_short_circuits() {
        assert_eq!(scan(&frame("D20")), num(90020));
    }

    /// **A10** — `1` and `1.0` are UNEQUAL as `serde_json::Value`, so the pair is ambiguous.
    ///
    /// This is the one place a reviewer's intuition disagrees with the code, and the conservative
    /// direction is the right one: the id is omitted rather than guessed. The comparison is over the
    /// decoded `Value`s, never over the decoded `RequestId`s: since rmcp 3.10 both spellings decode
    /// to `Number(1)`, so the mutant "compare the two decoded `RequestId`s" recovers `1` and turns
    /// this cell RED.
    #[test]
    fn scan_is_unusable_on_int_vs_float_duplicates() {
        assert_eq!(
            scan(br#"{"jsonrpc":"2.0","id":1,"id":1.0,"method":"ping","params":{}}"#),
            EnvelopeId::Unusable
        );
    }

    /// **A13** — VALUES are compared DECODED, exactly as A7 pins for the KEYS.
    ///
    /// D23's two occurrences are the same one-character string spelled plainly and as a `\u`
    /// escape: their raw spans DIFFER, their decoded values are EQUAL, so the pair is ONE id and
    /// must be RECOVERED.
    ///
    /// Mutant: comparing the occurrences' RAW SPANS instead of their decoded values — e.g.
    /// `map.next_value::<Box<serde_json::value::RawValue>>()` plus a string compare (`M24`). This
    /// is the ONLY unit cell that can fail under it: every other equal pair in the corpus is
    /// byte-identical.
    #[test]
    fn scan_compares_values_decoded_not_by_raw_span() {
        let bytes = frame("D23");
        // Non-vacuity: if someone "tidies" the escape away, this cell degenerates into a copy of
        // D02 and stops grading anything.
        let rendered = String::from_utf8(bytes.clone()).expect("D23 is UTF-8");
        assert!(
            rendered.contains(r#""id":"a""#) && !rendered.contains(r#""id":"a","id":"a""#),
            "D23's two occurrences must NOT be byte-identical, or it stops killing the raw-span \
             value comparator: {rendered}"
        );
        assert_eq!(scan(&bytes), text("a"));
    }

    /// **A12** — `scan` agrees with `RequestId::deserialize` on a FIXED TABLE of the boundary
    /// values that decide recoverability. The "for ANY value" half of that property is A15, the
    /// proptest below; this cell is its named-boundary regression corpus, kept because a shrunk
    /// proptest counterexample is not a guarantee that `i64::MIN` is retried on every run.
    ///
    /// **This cell splices exactly ONE `id` member, so it never exercises the COMPARISON at all**
    /// — it grades the recoverability decision only. Equality is graded by A3/A4/A10/A13. Saying so
    /// is the point: read as equality coverage it would be a false claim.
    ///
    /// Mutation: `RequestId::deserialize` replaced by a hand-written type table at the collector.
    #[test]
    fn recovery_agrees_with_rmcp() {
        let values = [
            serde_json::json!(1),
            serde_json::json!(-1),
            serde_json::json!(0),
            serde_json::json!(i64::MAX),
            serde_json::json!(i64::MIN),
            serde_json::json!("abc"),
            serde_json::json!(""),
            serde_json::json!(null),
            serde_json::json!(true),
            serde_json::json!(1.5),
            serde_json::json!(100.0),
            serde_json::json!({}),
            serde_json::json!([1]),
            serde_json::json!(u64::MAX),
        ];
        for value in values {
            let frame = serde_json::json!({
                "jsonrpc": "2.0",
                "id": value,
                "method": "ping",
            });
            let bytes = serde_json::to_vec(&frame).expect("frame serialises");
            let expected = match RequestId::deserialize(value.clone()) {
                Ok(id) => EnvelopeId::Recovered(id),
                Err(_) => EnvelopeId::Unusable,
            };
            assert_eq!(
                scan(&bytes),
                expected,
                "scan disagreed with RequestId::deserialize on {value}"
            );
        }
    }

    /// Every JSON shape, bounded in depth and width, as an envelope-`id` candidate.
    ///
    /// Bounded deliberately: the property below is about the value's TYPE, and `RequestId` rejects
    /// every container outright, so unbounded nesting would trade retries of the interesting
    /// scalars — the i64 boundaries, the float/integer split, the empty string — for deep trees
    /// that all decide the same way. `f64` is generated over its FULL range on purpose, NaN and the
    /// infinities included: `serde_json` has no JSON spelling for those, so they arrive as `Null`
    /// and cost nothing, while every finite `f64` round-trips exactly through `ryu`.
    fn any_json_value() -> impl Strategy<Value = serde_json::Value> {
        let leaf = prop_oneof![
            Just(serde_json::Value::Null),
            any::<bool>().prop_map(serde_json::Value::from),
            any::<i64>().prop_map(serde_json::Value::from),
            any::<u64>().prop_map(serde_json::Value::from),
            any::<f64>().prop_map(serde_json::Value::from),
            ".*".prop_map(serde_json::Value::from),
        ];
        leaf.prop_recursive(3, 16, 3, |inner| {
            prop_oneof![
                proptest::collection::vec(inner.clone(), 0..3).prop_map(serde_json::Value::Array),
                proptest::collection::vec((".*", inner), 0..3)
                    .prop_map(|members| serde_json::Value::Object(members.into_iter().collect())),
            ]
        })
    }

    // **A15** — the PROPERTY the crate plan names: `scan` agrees with `RequestId::deserialize` for
    // ANY value spliced as the envelope `id`. A12 is the same claim over 14 hand-picked values;
    // this is the claim itself, and the two are kept apart so neither is read as the other.
    //
    // The agreement is meant to be STRUCTURAL — `scan` calls rmcp's own deserializer rather than a
    // type table — so the only way this property can fail is if the SPLICE loses information: a
    // value whose JSON encoding does not parse back to itself would make the two sides disagree
    // about what they were even asked. That is exactly the failure a fixed table cannot look for,
    // because whoever writes the table picks values that round-trip.
    //
    // Mutation: `RequestId::deserialize` replaced by a hand-written type table at the collector
    // (the same mutant A12 names) — the table gets the i64/u64 boundary or the float-vs-integer
    // split wrong on some generated value and the property shrinks straight to it.
    proptest::proptest! {
        #[test]
        fn recovery_agrees_with_rmcp_for_any_spliced_value(value in any_json_value()) {
            let frame = serde_json::json!({
                "jsonrpc": "2.0",
                "id": value,
                "method": "ping",
            });
            let bytes = serde_json::to_vec(&frame).expect("frame serialises");
            let expected = match RequestId::deserialize(value.clone()) {
                Ok(id) => EnvelopeId::Recovered(id),
                Err(_) => EnvelopeId::Unusable,
            };
            prop_assert_eq!(
                scan(&bytes),
                expected,
                "scan disagreed with RequestId::deserialize on {}",
                value
            );
        }
    }
}
