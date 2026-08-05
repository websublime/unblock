//! The D47 UN-DECODABLE-ENVELOPE-`id` predicate — a `serde::de::DeserializeSeed` over the ROOT
//! object that recovers the envelope `id` from a frame's RAW BYTES.
//!
//! # Why the bytes are the only place the information exists
//!
//! rmcp decodes a JSON-RPC frame through a serde UNTAGGED union tried Request-first (rmcp
//! `src/model.rs:575-588`), and `JsonRpcRequest` requires `id: RequestId` (`:431-436`). So ANY
//! frame whose `id` fails to decode falls through to the Notification variant — where
//! `JsonRpcNotification` has no `id` field at all (`:486-490`) and `CustomNotification` carries only
//! `method`/`params`/`Extensions` (`:721-729`). The surplus `id` is DROPPED, not retained. By the
//! time the transport holds a message, the only surviving copy of the id is the raw line.
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
//! verdict gates EXECUTION. Here a tokenizer failure yields [`EnvelopeId::Absent`], i.e. silence.
//! A reader will assume symmetry, so: this verdict gates a REPLY to a frame on which nothing
//! executes either way, and over-firing would answer a genuine notification, violating JSON-RPC
//! 2.0's "The Server MUST NOT reply to a Notification" in a way every conforming client sees. The
//! arm is also unreachable in practice — [`scan`] only ever runs on bytes `serde_json::from_slice`
//! has ALREADY accepted.

use rmcp::model::RequestId;
use serde::Deserialize;
use serde::de::{self, DeserializeSeed, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use std::fmt;

/// The result of scanning a frame's RAW BYTES for top-level `id` members.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EnvelopeId {
    /// No top-level `id` member (or a non-object root).
    ///
    /// **D47 OUT OF SCOPE.** This is a legitimate JSON-RPC Notification — "a Request object without
    /// an id member" — and it MUST keep behaving exactly as it did before D47.
    Absent,
    /// One or more `id` members, ALL mutually equal, whose common value `RequestId` accepts.
    ///
    /// The "unambiguously recovered" case: the reply rides this id, which is the only spelling that
    /// resolves a waiting rmcp peer (its client DROPS an error carrying no id).
    Recovered(RequestId),
    /// An `id` exists but cannot be answered on: two or more occurrences with DIFFERENT values, or
    /// a uniform value `RequestId` rejects (`null`, object, array, bool, non-integer, out of i64).
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
/// `null`/object/array/bool ("Expect number or string"), non-integers ("Expected an integer") and
/// values outside `i64` ("Number too large for i64").
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

    // `deserializer.end()` is deliberately NOT called. Trailing-character rejection is already
    // guaranteed, because `scan` only ever runs on bytes `serde_json::from_slice` has ALREADY
    // accepted; a second, differently-configured strictness oracle here could only introduce
    // divergence. (`dup_key` needs `end()` because it runs BEFORE the parse; this runs after.)

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
    use crate::envelope_id_corpus::divergence_corpus;
    use rmcp::model::{NumberOrString, RequestId};
    use serde::Deserialize as _;

    /// Fetch one divergence-corpus frame by entry id.
    fn frame(entry: &str) -> Vec<u8> {
        divergence_corpus()
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

    /// **A1** — a frame with NO root `id` is `Absent`, which is D47's explicit carve-out.
    ///
    /// Mutant: deleting the `Absent => {}` arm at the transport (so a genuine notification falls
    /// into the `Unusable` reply arm).
    #[test]
    fn scan_is_absent_without_a_root_id() {
        assert_eq!(
            scan(br#"{"jsonrpc":"2.0","method":"notifications/foo","params":{}}"#),
            EnvelopeId::Absent
        );
    }

    /// **A2** — a LONE valid id is recovered.
    ///
    /// **Honesty note, and it is not a hedge word: this arm is PROVABLY UNREACHABLE end-to-end with
    /// rmcp 1.7.** `CustomRequest` and `CustomNotification` have byte-identical `Deserialize`
    /// bodies (rmcp `src/model/serde_impl.rs:338-360` versus `:391-413`) and the untagged union
    /// tries `Request` FIRST, so any body the Notification variant would accept is also accepted as
    /// a `CustomRequest` — and with a valid `id` present, `JsonRpcRequest` simply succeeds. A single
    /// well-formed id therefore ALWAYS yields a Request. Consistent with live evidence: an
    /// id-carrying `notifications/initialized|cancelled|progress|message|foo` is answered `-32601`
    /// today.
    ///
    /// The cell and its mutant are KEPT anyway, deliberately, as an rmcp-version-independence
    /// hedge: they are what will notice if a future rmcp makes the arm reachable.
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
    }

    /// **A6** — a number outside what `RequestId` accepts is unusable, and the neighbours that ARE
    /// accepted are asserted alongside so this pins a BOUNDARY rather than a blanket rejection.
    ///
    /// Mutants: accepting an f64 whose `fract() == 0.0` by truncating (`M10`, killed by D10/D11);
    /// widening the integer test to `i128` (`M11`, killed by D12/D13).
    #[test]
    fn scan_is_unusable_on_a_non_i64_number() {
        for entry in ["D10", "D11", "D12", "D13"] {
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
    /// direction is the right one: the id is omitted rather than guessed.
    ///
    /// Mutation: the equality test rewritten to DROP occurrences whose `RequestId::deserialize`
    /// FAILS and compare only the survivors — that spelling recovers `1` and turns this cell RED.
    ///
    /// **Do NOT spell the mutant "compare the two decoded `RequestId`s": it SURVIVES.** `serde_json`
    /// stores `1.0` as an f64, so `as_i64`/`as_u64` both return `None` and rmcp takes its "Expected
    /// an integer" branch — `1.0` never becomes `Number(1)`, the second occurrence simply fails to
    /// decode, the pair is unequal, and the verdict stays `Unusable` with the cell GREEN.
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

    /// **A12** — `scan` agrees with `RequestId::deserialize` for ANY value spliced as the id.
    ///
    /// **A12 splices exactly ONE `id` member, so it never exercises the COMPARISON at all** — it
    /// grades the recoverability decision only. Equality is graded by A3/A4/A10/A13. Saying so is
    /// the point: read as equality coverage it would be a false claim.
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
}
