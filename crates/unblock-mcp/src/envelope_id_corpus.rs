//! The D47 UN-DECODABLE-ENVELOPE-`id` frame corpus — declared ONCE, consumed by this crate's own
//! in-module byte cells, its duplex suite, and `unblock-cli`'s raw-stdio suite.
//!
//! It also declares the D54 parse-error corpus ([`parse_error_corpus`]): hand-written lines that
//! FAIL the typed parse, with their EXACT reply bytes, under the same text-is-the-test rule.
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
    #[must_use]
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
#[must_use]
pub fn expected_bytes(expect: &Expect) -> Vec<u8> {
    let body = match expect {
        Expect::RecoveredNum(n) => {
            format!(
                r#"{{"jsonrpc":"2.0","id":{n},"error":{{"code":-32600,"message":"{MESSAGE}"}}}}"#
            )
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
///
/// The length is a consequence of the corpus being DATA: each entry is one frame plus the one line
/// saying what it pins that nothing else does. Splitting it into arbitrary halves would hide the
/// enumeration this cell exists to make readable.
#[must_use]
#[allow(clippy::too_many_lines)]
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
        br#"{"jsonrpc":"2.0","\u0069d":90015,"\u0069d":90015,"method":"ping","params":{}}"#
            .to_vec(),
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
    push(
        "D19",
        {
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
    push(
        "D20",
        {
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

// =================================================================================================
// [v1.0.1/D54] THE PARSE-ERROR CORPUS — lines that FAIL the typed parse
// =================================================================================================

/// What the transport must reply to one line that fails the typed parse (D54).
///
/// As for [`Expect`], the `id` half is the point, so every cell asserts EXACT bytes.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ParseExpect {
    /// In class: strict JSON whose numeric id is readable — `-32700` ON that id.
    RecoveredNum(i64),
    /// In class: strict JSON whose string id is readable — `-32700` ON that id.
    RecoveredStr(&'static str),
    /// Out of class: rmcp's own id-less `-32700`, byte for byte.
    IdLess,
    /// Out of class: nothing at all — the compatibility filter drops the line, as rmcp does.
    Silent,
}

/// The four KINDS a [`ParseExpect`] can take, as a set-comparable value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ParseExpectKind {
    /// [`ParseExpect::RecoveredNum`].
    RecoveredNum,
    /// [`ParseExpect::RecoveredStr`].
    RecoveredStr,
    /// [`ParseExpect::IdLess`].
    IdLess,
    /// [`ParseExpect::Silent`].
    Silent,
}

impl ParseExpect {
    /// The kind of this expectation, for set-coverage assertions.
    #[must_use]
    pub fn kind(&self) -> ParseExpectKind {
        match self {
            Self::RecoveredNum(_) => ParseExpectKind::RecoveredNum,
            Self::RecoveredStr(_) => ParseExpectKind::RecoveredStr,
            Self::IdLess => ParseExpectKind::IdLess,
            Self::Silent => ParseExpectKind::Silent,
        }
    }
}

/// Which half of D54's strict-JSON gate an out-of-class entry witnesses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum GateHalf {
    /// The line is not UTF-8 after the one BOM strip.
    Utf8,
    /// The line is UTF-8, but `serde_json`'s `Value` parse rejects it.
    Json,
}

/// What an entry exists to grade. The non-vacuity cell checks it, so an entry "tidied" into one
/// that grades nothing goes red instead of passing silently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseRole {
    /// The typed parse fails, the line is strict JSON, and `envelope_id::scan` recovers EXACTLY the
    /// declared id.
    InClass,
    /// The gate rejects the line, while `envelope_id::scan` ALONE would recover an id from it.
    GateWitness(GateHalf),
    /// The line is strict JSON and fails the typed parse, but `scan` recovers no id.
    ScanWitness,
    /// The compatibility filter drops the line; it never reaches the parse-error arm.
    CompatDropped,
    /// The gate rejects the line and `scan` recovers nothing either: it pins only that the reply
    /// stays rmcp's, and grades no gate.
    ParityOnly,
}

/// One parse-error corpus entry.
pub struct ParseErrorFrame {
    /// The stable entry id (`E01`..`E24` in class, `X01`..`X13` out of class).
    pub id: &'static str,
    /// The RAW line bytes, exactly as they go on the wire (no terminator).
    pub frame: Vec<u8>,
    /// The reply the transport must write.
    pub expect: ParseExpect,
    /// What the entry grades.
    pub role: ParseRole,
    /// One line: what this entry pins that no other entry does.
    pub why: &'static str,
}

/// The EXACT bytes the transport must write for `expect`, terminator included.
///
/// Hand-written, never encoded through the transport, for the reason the module doc gives.
/// `-32700` and `"Parse error"` are rmcp's own (`rmcp-1.7.0/src/transport/async_rw.rs:145-153`),
/// and the member order is rmcp's `JsonRpcError` field order: `jsonrpc`, `id`, `error`
/// (`src/model.rs:462-470`), then `code`, `message`, `data`, the last skipped because it is `None`.
#[must_use]
pub fn parse_error_bytes(expect: &ParseExpect) -> Vec<u8> {
    let body = match expect {
        ParseExpect::RecoveredNum(n) => format!(
            r#"{{"jsonrpc":"2.0","id":{n},"error":{{"code":-32700,"message":"Parse error"}}}}"#
        ),
        ParseExpect::RecoveredStr(s) => format!(
            r#"{{"jsonrpc":"2.0","id":"{s}","error":{{"code":-32700,"message":"Parse error"}}}}"#
        ),
        ParseExpect::IdLess => {
            r#"{"jsonrpc":"2.0","error":{"code":-32700,"message":"Parse error"}}"#.to_string()
        }
        ParseExpect::Silent => return Vec::new(),
    };
    let mut out = body.into_bytes();
    out.push(b'\n');
    out
}

/// A `tools/call` whose `params` is `arrays` nested arrays, so the whole line nests `arrays + 1`
/// containers. `serde_json`'s `Value` parse accepts 127 and rejects 128 (`remaining_depth: 128`,
/// `serde_json-1.0.150/src/de.rs:63`).
fn nested_params(id: i64, arrays: usize) -> Vec<u8> {
    let mut line = format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":"#);
    line.push_str(&"[".repeat(arrays));
    line.push_str(&"]".repeat(arrays));
    line.push('}');
    line.into_bytes()
}

/// The whole D54 parse-error corpus, in entry order.
///
/// Every entry was measured against the shipped binary before the fix: every E entry and every X
/// entry but X11 and X13 drew the id-less `-32700`, and X11 and X13 drew nothing. An E entry whose
/// "before" was already answered on its id would prove nothing.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn parse_error_corpus() -> Vec<ParseErrorFrame> {
    let mut corpus = Vec::new();

    let mut push = |id, frame: Vec<u8>, expect, role, why| {
        corpus.push(ParseErrorFrame {
            id,
            frame,
            expect,
            role,
            why,
        });
    };
    let mut bom_framed = BOM.to_vec();
    bom_framed
        .extend_from_slice(br#"{"jsonrpc":"2.0","id":54017,"method":"ping","method":"ping"}"#);

    push(
        "E01",
        br#"{"jsonrpc":"2.0","id":54001,"method":"ping","method":"ping"}"#.to_vec(),
        ParseExpect::RecoveredNum(54001),
        ParseRole::InClass,
        "the headline case: a duplicated `method` with EQUAL values fails the typed parse",
    );
    push(
        "E02",
        br#"{"jsonrpc":"2.0","id":54002,"method":"ping","method":"tools/list"}"#.to_vec(),
        ParseExpect::RecoveredNum(54002),
        ParseRole::InClass,
        "a duplicated `method` with DIFFERING values",
    );
    push(
        "E03",
        br#"{"jsonrpc":"2.0","jsonrpc":"2.0","id":54003,"method":"ping"}"#.to_vec(),
        ParseExpect::RecoveredNum(54003),
        ParseRole::InClass,
        "a duplicated `jsonrpc`, numeric id",
    );
    push(
        "E04",
        br#"{"jsonrpc":"2.0","jsonrpc":"2.0","id":"ub788-s","method":"ping"}"#.to_vec(),
        ParseExpect::RecoveredStr("ub788-s"),
        ParseRole::InClass,
        "a duplicated `jsonrpc`, STRING id",
    );
    push(
        "E05",
        br#"{"jsonrpc":"1.0","id":54005,"method":"ping"}"#.to_vec(),
        ParseExpect::RecoveredNum(54005),
        ParseRole::InClass,
        "`jsonrpc` set to \"1.0\"",
    );
    push(
        "E06",
        br#"{"id":54006,"method":"ping"}"#.to_vec(),
        ParseExpect::RecoveredNum(54006),
        ParseRole::InClass,
        "`jsonrpc` missing",
    );
    push(
        "E07",
        br#"{"jsonrpc":2.0,"id":54007,"method":"ping"}"#.to_vec(),
        ParseExpect::RecoveredNum(54007),
        ParseRole::InClass,
        "`jsonrpc` numeric",
    );
    push(
        "E08",
        br#"{"jsonrpc":"2.0","id":54008}"#.to_vec(),
        ParseExpect::RecoveredNum(54008),
        ParseRole::InClass,
        "`method` missing",
    );
    push(
        "E09",
        br#"{"jsonrpc":"2.0","id":54009,"method":5}"#.to_vec(),
        ParseExpect::RecoveredNum(54009),
        ParseRole::InClass,
        "`method` numeric",
    );
    push(
        "E10",
        br#"{"jsonrpc":"2.0","id":54010,"method":"tools/call","params":5}"#.to_vec(),
        ParseExpect::RecoveredNum(54010),
        ParseRole::InClass,
        "a scalar `params` on `tools/call`",
    );
    push(
        "E11",
        br#"{"jsonrpc":"2.0","id":12,"method":"tools/call","params":{"name":"issue"},"params":{"name":"claim"}}"#
            .to_vec(),
        ParseExpect::RecoveredNum(12),
        ParseRole::InClass,
        "formerly CD-7 parity entry NS2: a duplicated envelope `params` KEY on a `tools/call`. The \
         outcome is METHOD-DEPENDENT and this entry pins only the `tools/call` half: the same \
         duplication on `ping` — a request with no `params` at all — is a plain SUCCESS (PRD \
         section 4 D47, spine section 5.6)",
    );
    push(
        "E12",
        br#"{"jsonrpc":"2.0","id":54012,"id":54012,"method":"ping","method":"ping"}"#.to_vec(),
        ParseExpect::RecoveredNum(54012),
        ParseRole::InClass,
        "boundary partner of D01: two EQUAL ids, but a duplicated `method` fails the typed parse, \
         so D54 answers it `-32700` where D01 draws D47's `-32600`",
    );
    push(
        "E13",
        br#"{"jsonrpc":"2.0","method":"ping","method":"ping","id":54013}"#.to_vec(),
        ParseExpect::RecoveredNum(54013),
        ParseRole::InClass,
        "the id written LAST, after the member that breaks the typed parse",
    );
    push(
        "E14",
        br#"{"jsonrpc":"2.0","id":"ub788-str","method":"ping","method":"ping"}"#.to_vec(),
        ParseExpect::RecoveredStr("ub788-str"),
        ParseRole::InClass,
        "a STRING id on a duplicated `method`",
    );
    push(
        "E15",
        br#"{"jsonrpc":"2.0","id":"a","id":"\u0061","method":"ping","method":"ping"}"#.to_vec(),
        ParseExpect::RecoveredStr("a"),
        ParseRole::InClass,
        "two ids EQUAL as decoded values but different as bytes — the decoded comparison on this arm",
    );
    push(
        "E16",
        br#"{"jsonrpc":"2.0","\u0069d":54016,"method":"ping","method":"ping"}"#.to_vec(),
        ParseExpect::RecoveredNum(54016),
        ParseRole::InClass,
        "the ESCAPED key: the bytes hold no `\"id\"` window, so a byte prefilter on that literal \
         would miss a genuine id",
    );
    push(
        "E17",
        bom_framed,
        ParseExpect::RecoveredNum(54017),
        ParseRole::InClass,
        "a BOM-prefixed line: the scan strips the same ONE BOM the parse does, on this arm too",
    );
    push(
        "E18",
        br#"{"jsonrpc":"2.0","id":54018,"method":"ping","method":"ping"}   "#.to_vec(),
        ParseExpect::RecoveredNum(54018),
        ParseRole::InClass,
        "trailing WHITESPACE is still strict JSON — the boundary partner of X01",
    );
    push(
        "E19",
        br#"{"jsonrpc":"2.0","id":54019,"method":"tools/call","method":"tools/call","params":{"name":"issue","arguments":{"action":"delete","ids":["{ID}"]}}}"#
            .to_vec(),
        ParseExpect::RecoveredNum(54019),
        ParseRole::InClass,
        "the store-effect entry: a destructive `tools/call` that must be answered and must EXECUTE \
         NOTHING; the duplex suite substitutes a live id for `{ID}`",
    );
    push(
        "E20",
        br#"{"jsonrpc":"2.0","id":20,"method":"tools/call","method":"ping","params":{}}"#.to_vec(),
        ParseExpect::RecoveredNum(20),
        ParseRole::InClass,
        "formerly CD-7 parity entry F20, which pinned that D47 left this open; D54 answers it \
         `-32700` ON id 20",
    );
    push(
        "E21",
        br#"{"jsonrpc":"2.0","id":6,"method":"ping","method":"ping","params":{}}"#.to_vec(),
        ParseExpect::RecoveredNum(6),
        ParseRole::InClass,
        "the frame of the deleted W-R1 cell, which pinned the residual as id-less",
    );
    push(
        "E22",
        br#"{"jsonrpc":"2.0","id":1,"method":"nope/nope","params":5}"#.to_vec(),
        ParseExpect::RecoveredNum(1),
        ParseRole::InClass,
        "the compatibility-filter cell's own frame: an unknown non-notification method with \
         untypeable `params` is answered, now on its id",
    );
    push(
        "E23",
        br#"{"jsonrpc":"2.0","id":5,"method":"initialize","params":5}"#.to_vec(),
        ParseExpect::RecoveredNum(5),
        ParseRole::InClass,
        "the pre-handshake shape: an `initialize` whose scalar `params` fails every variant",
    );
    push(
        "E24",
        nested_params(54124, 126),
        ParseExpect::RecoveredNum(54124),
        ParseRole::InClass,
        "DEPTH BOUNDARY, accepted side: 127 nesting levels is strict JSON and gets its id — the \
         partner of X04",
    );
    push(
        "X01",
        br#"{"jsonrpc":"2.0","id":54101,"method":"ping","method":"ping"} xyz"#.to_vec(),
        ParseExpect::IdLess,
        ParseRole::GateWitness(GateHalf::Json),
        "TRAILING bytes after a complete object: `scan` alone recovers 54101 because it never calls \
         `end()`; the strict-JSON gate keeps the reply id-less",
    );
    push(
        "X02",
        br#"{"jsonrpc":"2.0","id":54102,"method":"pi"#.to_vec(),
        ParseExpect::IdLess,
        ParseRole::ParityOnly,
        "TRUNCATED after the id: the gate rejects it and `scan` returns Absent anyway, so it grades no \
         gate and pins only rmcp parity",
    );
    push(
        "X03",
        b"{\"jsonrpc\":\"2.0\",\"id\":54103,\"method\":\"pi\xffng\"}".to_vec(),
        ParseExpect::IdLess,
        ParseRole::GateWitness(GateHalf::Utf8),
        "a NON-UTF-8 byte inside a string `scan` skips: `scan` alone recovers 54103; the only entry \
         that sees a non-UTF-8 line classed as JSON (F10's bad bytes sit in a KEY, so `scan` finds \
         no id there)",
    );
    push(
        "X04",
        nested_params(54104, 127),
        ParseExpect::IdLess,
        ParseRole::GateWitness(GateHalf::Json),
        "DEPTH BOUNDARY, rejected side: 128 nesting levels; `scan` alone recovers 54104 because \
         `IgnoredAny` checks no depth — the partner of E24",
    );
    push(
        "X05",
        br#"{"jsonrpc":"2.0","id":54105,"method":"ping","method":"ping","params":{"s":"\ud800"}}"#
            .to_vec(),
        ParseExpect::IdLess,
        ParseRole::GateWitness(GateHalf::Json),
        "a LONE SURROGATE escape in a skipped string: RFC 8259 grammar admits it, `serde_json`'s \
         `Value` parse does not, and `scan` alone recovers 54105",
    );
    push(
        "X06",
        br#"[{"jsonrpc":"2.0","id":54106,"method":"ping","method":"ping"}]"#.to_vec(),
        ParseExpect::IdLess,
        ParseRole::ScanWitness,
        "a BATCH root: no root members, so no id, though an element carries one",
    );
    push(
        "X07",
        br#""{\"jsonrpc\":\"2.0\",\"id\":54107}""#.to_vec(),
        ParseExpect::IdLess,
        ParseRole::ScanWitness,
        "a bare STRING root that spells an envelope inside it",
    );
    push(
        "X08",
        br#"{"jsonrpc":"2.0","method":"ping","method":"ping"}"#.to_vec(),
        ParseExpect::IdLess,
        ParseRole::ScanWitness,
        "NO id at all: the reply stays id-less, unchanged",
    );
    push(
        "X09",
        br#"{"jsonrpc":"2.0","id":true,"method":"ping","method":"ping"}"#.to_vec(),
        ParseExpect::IdLess,
        ParseRole::ScanWitness,
        "a wrongly TYPED id: no `RequestId`, so no id",
    );
    push(
        "X10",
        br#"{"jsonrpc":"2.0","id":54110,"id":54111,"method":"ping","method":"ping"}"#.to_vec(),
        ParseExpect::IdLess,
        ParseRole::ScanWitness,
        "two DIFFERING ids: ambiguous, so no id. It also kills reading the id from the compatibility \
         filter's `Value`, which keeps the LAST duplicate",
    );
    push(
        "X11",
        br#"{"jsonrpc":"2.0","id":17,"method":"notifications/foo","params":5}"#.to_vec(),
        ParseExpect::Silent,
        ParseRole::CompatDropped,
        "an id-carrying `notifications/*` line with untypeable `params` (CD-7 F17's shape): the \
         compatibility filter drops it before the parse-error arm, so it stays SILENT, as in rmcp",
    );
    push(
        "X12",
        br#"{"jsonrpc":"2.0","id":54112,"method":"ping","method":"ping","x":1e400}"#.to_vec(),
        ParseExpect::IdLess,
        ParseRole::GateWitness(GateHalf::Json),
        "a number outside `f64` in a skipped member: legal grammar that `serde_json`'s `Value` parse \
         rejects, while `scan` alone recovers 54112",
    );
    push(
        "X13",
        br#"{"jsonrpc":"2.0","id":54113,"method":"ping","method":"notifications/foo"}"#.to_vec(),
        ParseExpect::Silent,
        ParseRole::CompatDropped,
        "a REQUEST-shaped line with a readable id whose LAST `method` is a non-standard \
         `notifications/*`: the compatibility filter reads the last-wins `Value`, drops it before the \
         parse-error arm, and it stays SILENT, as in rmcp (D54 clause (7))",
    );

    corpus
}
