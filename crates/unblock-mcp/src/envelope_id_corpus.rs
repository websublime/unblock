//! The D47 UN-DECODABLE-ENVELOPE-`id` frame corpus — declared ONCE, consumed by this crate's own
//! in-module byte cells, its duplex suite, and `unblock-cli`'s raw-stdio suite.
//!
//! # The text IS the test
//!
//! Every frame here is a HAND-WRITTEN whole envelope, never built by serializing a structure. That
//! is the same rule [`crate::duplicate_key_corpus`] states for D43 arguments, applied one level
//! out: for D43 the raw text is `arguments`, for D47 it is the ENVELOPE. It is not stylistic —
//! `serde_json` cannot emit a duplicated key, a `null` id or an out-of-`i64` number in the first
//! place, so a serialized frame structurally cannot express a single entry below. For the same
//! reason NO cell may reach for `duplicate_key_corpus::raw_tools_call`: its signature interpolates
//! one well-formed `i64` id.
//!
//! # Why the expected reply bytes are spelled out here
//!
//! [`expected_bytes`] hard-codes the `-32600` message text instead of importing
//! `crate::wire::INVALID_REQUEST_ID_MESSAGE`. Deriving it from the constant would make every
//! byte-exact assertion agree with whatever the constant currently says, so a mutant that rewrites
//! the constant would pass the entire suite. The duplication is the pin.

/// What the transport must reply to one corpus frame.
///
/// The `id` half is the whole point: "differs from rmcp" is an assertion a mutant writing garbage
/// also satisfies, so every cell asserts the EXACT bytes, recovered-versus-omitted id included.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Expect {
    /// The bytes yielded ONE unambiguous numeric id: answer on it.
    RecoveredNum(i64),
    /// The bytes yielded ONE unambiguous string id: answer on it.
    RecoveredStr(&'static str),
    /// The bytes are ambiguous (two DIFFERENT ids) or the value is no representable `RequestId`:
    /// answer with the `id` member OMITTED.
    ///
    /// Omitted and NOT a literal `"id":null`: `rmcp::model::JsonRpcError.id` is `Option<RequestId>`
    /// under `skip_serializing_if = "Option::is_none"` (rmcp `src/model.rs:462-470`), so no value
    /// of that field serializes to a null. Both spellings decode to `id: None` for the peer anyway.
    Omitted,
}

/// The three KINDS an [`Expect`] can take, as a set-comparable value.
///
/// Used by the corpus-coverage cell, which asserts a SET rather than a count — a count rots, a set
/// cannot be off-by-one against itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExpectKind {
    /// [`Expect::RecoveredNum`].
    RecoveredNum,
    /// [`Expect::RecoveredStr`].
    RecoveredStr,
    /// [`Expect::Omitted`].
    Omitted,
}

impl Expect {
    /// The kind of this expectation, for set-coverage assertions.
    pub fn kind(&self) -> ExpectKind {
        match self {
            Self::RecoveredNum(_) => ExpectKind::RecoveredNum,
            Self::RecoveredStr(_) => ExpectKind::RecoveredStr,
            Self::Omitted => ExpectKind::Omitted,
        }
    }
}

/// One corpus entry.
pub struct Frame {
    /// The stable entry id (`D01`..`D23`), used as the attributable name in set-equality guards.
    pub id: &'static str,
    /// The RAW frame bytes, exactly as they go on the wire (no terminator).
    pub frame: Vec<u8>,
    /// The reply the transport must write.
    pub expect: Expect,
    /// One line: what this entry pins that no other entry does.
    pub why: &'static str,
}

/// The `-32600` message, spelled out rather than imported. See the module doc.
const MESSAGE: &str =
    "Invalid Request: the id member is duplicated or is not a valid JSON-RPC request id";

/// The EXACT bytes the transport must write for `expect`, terminator included.
///
/// One helper so that a future decision to respell the fallback is one edit rather than
/// twenty-three. The member order is rmcp's struct field order (`jsonrpc`, `id`, `error`, then
/// `code`, `message`, `data` — the last skipped because `data` is `None`).
pub fn expected_bytes(expect: &Expect) -> Vec<u8> {
    let body = match expect {
        Expect::RecoveredNum(n) => {
            format!(r#"{{"jsonrpc":"2.0","id":{n},"error":{{"code":-32600,"message":"{MESSAGE}"}}}}"#)
        }
        Expect::RecoveredStr(s) => format!(
            r#"{{"jsonrpc":"2.0","id":"{s}","error":{{"code":-32600,"message":"{MESSAGE}"}}}}"#
        ),
        Expect::Omitted => {
            format!(r#"{{"jsonrpc":"2.0","error":{{"code":-32600,"message":"{MESSAGE}"}}}}"#)
        }
    };
    let mut out = body.into_bytes();
    out.push(b'\n');
    out
}

/// The UTF-8 byte order mark, for the one entry that carries it.
const BOM: &[u8; 3] = b"\xEF\xBB\xBF";

/// The `{ID}` placeholder the store-effect entry (`D16`) carries where a live issue id goes.
///
/// The duplex cell substitutes a freshly minted id; the in-module byte cells use the frame as-is,
/// which is sound because the reply bytes do not depend on `arguments` at all — the fault is about
/// the ENVELOPE and is decided before any method is known.
pub const ISSUE_ID_PLACEHOLDER: &str = "{ID}";

/// The whole divergence corpus, in entry order.
///
/// Every entry was confirmed SILENT against the shipped binary before the fix landed — a cell whose
/// "before" was already answered proves nothing.
pub fn divergence_corpus() -> Vec<Frame> {
    let mut corpus = Vec::new();

    let mut push = |id, frame: Vec<u8>, expect, why| {
        corpus.push(Frame {
            id,
            frame,
            expect,
            why,
        });
    };

    push(
        "D01",
        br#"{"jsonrpc":"2.0","id":90001,"id":90001,"method":"ping","params":{}}"#.to_vec(),
        Expect::RecoveredNum(90001),
        "the headline case: two EQUAL numeric ids are unambiguous, so the answer rides that id",
    );
    push(
        "D02",
        br#"{"jsonrpc":"2.0","id":"e2e-abc","id":"e2e-abc","method":"ping","params":{}}"#.to_vec(),
        Expect::RecoveredStr("e2e-abc"),
        "a STRING id is recoverable too — `RequestId` is a number-OR-string",
    );
    push(
        "D03",
        br#"{"jsonrpc":"2.0","id":90003,"id":90003,"id":90003,"method":"ping","params":{}}"#
            .to_vec(),
        Expect::RecoveredNum(90003),
        "THREE equal occurrences are still unambiguous — the rule is equality, not a count of one",
    );
    push(
        "D04",
        br#"{"jsonrpc":"2.0","id":90004,"id":90005,"method":"ping","params":{}}"#.to_vec(),
        Expect::Omitted,
        "the fallback's headline: two DIFFERENT ids cannot be answered on, so the id is omitted",
    );
    push(
        "D05",
        br#"{"jsonrpc":"2.0","id":90006,"id":90006,"id":90007,"method":"ping","params":{}}"#
            .to_vec(),
        Expect::Omitted,
        "ambiguity is MONOTONE — a late disagreement still poisons an already-equal run",
    );
    push(
        "D06",
        br#"{"jsonrpc":"2.0","id":null,"method":"ping","params":{}}"#.to_vec(),
        Expect::Omitted,
        "JSON null is no representable `RequestId` (rmcp: `Expect number or string`)",
    );
    push(
        "D07",
        br#"{"jsonrpc":"2.0","id":{},"method":"ping","params":{}}"#.to_vec(),
        Expect::Omitted,
        "an OBJECT id, same rejection arm",
    );
    push(
        "D08",
        br#"{"jsonrpc":"2.0","id":true,"method":"ping","params":{}}"#.to_vec(),
        Expect::Omitted,
        "a BOOLEAN id, same rejection arm",
    );
    push(
        "D09",
        br#"{"jsonrpc":"2.0","id":[1],"method":"ping","params":{}}"#.to_vec(),
        Expect::Omitted,
        "an ARRAY id, same rejection arm",
    );
    push(
        "D10",
        br#"{"jsonrpc":"2.0","id":1e2,"method":"ping","params":{}}"#.to_vec(),
        Expect::Omitted,
        "the EXPONENT spelling of an integer: `serde_json` stores 1e2 as an f64, so rmcp takes its \
         `Expected an integer` branch — while the same number written `100` is answered normally, \
         which is what makes this a boundary rather than a blanket",
    );
    push(
        "D11",
        br#"{"jsonrpc":"2.0","id":100.0,"method":"ping","params":{}}"#.to_vec(),
        Expect::Omitted,
        "the same f64 trap written with a decimal point",
    );
    push(
        "D12",
        br#"{"jsonrpc":"2.0","id":9223372036854775808,"method":"ping","params":{}}"#.to_vec(),
        Expect::Omitted,
        "2^63 — one past `i64::MAX`, which IS answered, so this pins a boundary not a blanket",
    );
    push(
        "D13",
        br#"{"jsonrpc":"2.0","id":-9223372036854775809,"method":"ping","params":{}}"#.to_vec(),
        Expect::Omitted,
        "one below `i64::MIN` — the negative half of the same boundary",
    );
    push(
        "D14",
        br#"{"jsonrpc":"2.0","\u0069d":{},"method":"ping","params":{}}"#.to_vec(),
        Expect::Omitted,
        "DECODED keys, unusable direction: the escaped spelling IS a genuine root `id` member, so a \
         byte prefilter for the literal bytes misses it entirely",
    );
    push(
        "D15",
        br#"{"jsonrpc":"2.0","\u0069d":90015,"\u0069d":90015,"method":"ping","params":{}}"#.to_vec(),
        Expect::RecoveredNum(90015),
        "DECODED keys, RECOVERY direction, with BOTH occurrences escaped on purpose: a MIXED \
         escaped/plain pair would leave a raw-span key comparator undetected, since it would still \
         see one plain occurrence and recover the same id",
    );
    push(
        "D16",
        br#"{"jsonrpc":"2.0","id":90016,"id":90016,"method":"tools/call","params":{"name":"issue","arguments":{"action":"delete","ids":["{ID}"]}}}"#
            .to_vec(),
        Expect::RecoveredNum(90016),
        "the STORE-EFFECT frame: a class frame naming a destructive action must be answered AND \
         must execute nothing — the one shape that kills a `rebuild it as a Request and deliver it` \
         implementation, which passes every channel-only assertion",
    );
    push(
        "D17",
        br#"{"jsonrpc":"2.0","id":90017,"id":90017,"method":"notifications/cancelled","params":{"requestId":7,"reason":null}}"#
            .to_vec(),
        Expect::RecoveredNum(90017),
        "the ONE shape whose inner value is a genuinely typed notification — and the one whose \
         DELIVERY has a real effect today (rmcp's serve loop cancels the matching in-flight request \
         before any handler runs), an effect D47 removes deliberately",
    );
    push(
        "D18",
        br#"{"jsonrpc":"2.0","id":90018,"id":90018,"method":"notifications/initialized"}"#.to_vec(),
        Expect::RecoveredNum(90018),
        "the other standard notification carrier; unlike D17 it executes nothing either way",
    );
    push("D19", {
        let mut bytes = BOM.to_vec();
        bytes.extend_from_slice(
            br#"{"jsonrpc":"2.0","id":90019,"id":90019,"method":"ping","params":{}}"#,
        );
        bytes
    },
        Expect::RecoveredNum(90019),
        "the scan strips exactly the ONE prefix BOM the parser strips — scanner and parser must see \
         the same document",
    );
    push("D20", {
        let pad = "x".repeat(100 * 1024);
        format!(
            r#"{{"jsonrpc":"2.0","id":90020,"pad":"{pad}","id":90020,"method":"ping","params":{{}}}}"#
        )
        .into_bytes()
    },
        Expect::RecoveredNum(90020),
        "NO short-circuit and no prefix-bounded scan: the second occurrence sits 100 KiB past the \
         first, so an early-returning collector reports one occurrence and a truncated one reports \
         none",
    );
    push(
        "D21",
        br#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"issue","arguments":{"a":1,"a":2}},"id":{}}"#
            .to_vec(),
        Expect::Omitted,
        "the fused-scanner counterexample made EXECUTABLE: `dup_key::scan` unwinds on the FIRST \
         duplicate it finds and descends `params` in document order, so a collector fused into its \
         root loop never reaches this TRAILING `id` and the defect survives for an attacker-chosen \
         member order",
    );
    push(
        "D22",
        br#"{"jsonrpc":"2.0","id":90022,"id":90022,"method":"tools/call","params":{"name":"issue","arguments":{"action":"create","action":"delete"}}}"#
            .to_vec(),
        Expect::RecoveredNum(90022),
        "D43/D47 PRECEDENCE: a class frame that ALSO carries a D43 duplicate under the scan root is \
         still answered and still NOT delivered — the D47 arm must not defer to the D43 verdict",
    );
    push(
        "D23",
        br#"{"jsonrpc":"2.0","id":"a","id":"\u0061","method":"ping","params":{}}"#.to_vec(),
        Expect::RecoveredStr("a"),
        "the VALUE half of the equality rule, which NO other entry reaches: the two occurrences are \
         the SAME one-character string spelled two ways, so their raw spans DIFFER while their \
         decoded values are EQUAL. Every other equal pair in this corpus is byte-identical, so a \
         raw-span VALUE comparator passes all of them while calling this one ambiguous, omitting \
         the id, and hanging exactly the client the recovery rule exists to release",
    );

    corpus
}
