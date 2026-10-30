//! D57 hostile-input and warning cells (Verify round 1): every echo of a hand-written database row or
//! an attacker-supplied key/value is CLIPPED and control-escaped; a non-`TEXT` cell is a visible
//! `invalid_value` fault; a TOML parse error never echoes the source line; `EnvOverrides`' `Debug`
//! never prints the auth token; and the criterion (e2) "warned" half — an unknown user-file key and
//! an unknown DB row each emit ONE `unblock.config` warning naming the key.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::LookupSpan;
use unblock_config::{ConfigError, EnvOverrides, EnvSource, decode_db_rows, load_user_config};
use unblock_error::{CodedError, ErrorCode, MAX_ECHOED_BYTES, StructuredError};
use unblock_model::ConfigFaultKind;

struct MapEnv(HashMap<String, String>);

impl MapEnv {
    fn new(pairs: &[(&str, &str)]) -> Self {
        Self(
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        )
    }
}

impl EnvSource for MapEnv {
    fn get(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
}

/// A hostile row value: an ESC/OSC-52 clipboard write, a newline that forges a second `error[` line,
/// a BEL — and enough bytes to need clipping.
fn hostile_value() -> String {
    format!(
        "x\n error[FORGED]: pwned \u{1b}]52;c;aGk=\u{7}{}",
        "A".repeat(4 * MAX_ECHOED_BYTES)
    )
}

/// Assert `text` is ONE printable line of bounded size — below the hostile value's own raw size
/// (more than `4 * MAX_ECHOED_BYTES`), with room for a message that echoes two clipped rows.
fn assert_one_printable_line(text: &str, what: &str) {
    assert!(
        !text.chars().any(char::is_control),
        "{what}: no raw control character survives: {text:?}"
    );
    assert!(
        text.len() < 4 * MAX_ECHOED_BYTES,
        "{what}: the echo is clipped ({} bytes)",
        text.len()
    );
}

/// D57 clause (2a) + spine §2.4: a hand-written `search_cap` row carrying ESC, a newline, `error[`
/// and megabytes of text decodes to an `invalid_value` fault whose REASON — and the `FaultyDbConfig`
/// message and hint built from it — is one clipped, printable line.
#[test]
fn a_hostile_db_row_is_clipped_and_escaped_in_every_echo() {
    let marked = format!("{}blob:6B", unblock_storage::NON_TEXT_CELL_MARKER);
    let rows = vec![
        ("search_cap".to_string(), hostile_value()),
        (marked, "1".to_string()),
    ];
    let faults = decode_db_rows(&rows);
    assert_eq!(faults.len(), 2, "both rows are faults");
    assert_eq!(faults[0].fault.kind, ConfigFaultKind::InvalidValue);
    assert_eq!(
        faults[0].value, rows[0].1,
        "the fault keeps the row value verbatim"
    );
    assert_one_printable_line(&faults[0].fault.reason, "the fault reason");
    assert!(
        faults[0].fault.reason.contains("\\n") && faults[0].fault.reason.contains("\\u{1b}"),
        "the control characters are ESCAPED, not dropped: {}",
        faults[0].fault.reason
    );

    let err = ConfigError::FaultyDbConfig { faults };
    assert_one_printable_line(&err.to_string(), "the FaultyDbConfig message");
    let hint = err.hint().expect("hint");
    assert!(
        !hint.chars().any(|c| c.is_control() && c != '\n'),
        "{hint:?}"
    );
    assert!(
        hint.contains(r#""key":"\u0001blob:6B""#),
        "the marked key's repair call is JSON-escaped, not raw: {hint}"
    );
    let structured = StructuredError::from(&err);
    assert_eq!(structured.code, ErrorCode::ConfigError);
    assert_one_printable_line(&structured.message, "the structured message");
}

/// MCP-supplied keys and values (up to 64 KiB each) are clipped in every refusal's message and in
/// `context.key`.
#[test]
fn an_oversized_key_or_value_is_clipped_in_refusals() {
    let huge = format!("k\u{1b}{}", "k".repeat(4 * MAX_ECHOED_BYTES));
    let unknown = unblock_config::classify_runtime_key(&huge).expect_err("unknown key");
    assert_one_printable_line(&unknown.to_string(), "UnknownKey");
    let context_key = unknown.context()["key"].as_str().unwrap().to_string();
    assert!(
        context_key.len() < 2 * MAX_ECHOED_BYTES,
        "context.key is clipped"
    );

    let rk = unblock_config::classify_runtime_key("actor").unwrap();
    let invalid = unblock_config::canonicalize_runtime_value(rk, &format!("\u{7}{huge}"))
        .expect_err("a control character is not a valid actor");
    assert_one_printable_line(&invalid.to_string(), "InvalidValue");
}

/// D57 clause (2a), "never a silent skip": a cell `Storage::read_config` returned in its non-`TEXT`
/// spelling — key or value — is an `invalid_value` (BLOCKING) fault naming the storage class.
#[test]
fn a_non_text_cell_is_an_invalid_value_fault() {
    let marker = unblock_storage::NON_TEXT_CELL_MARKER;
    let rows = vec![
        (format!("{marker}blob:6B"), "1".to_string()),
        ("actor".to_string(), format!("{marker}blob:00FF")),
    ];
    let faults = decode_db_rows(&rows);
    assert_eq!(faults.len(), 2);
    for fault in &faults {
        assert_eq!(fault.fault.kind, ConfigFaultKind::InvalidValue, "{fault:?}");
        assert!(fault.fault.kind.is_blocking());
        assert!(
            fault.fault.reason.contains("stored as blob, not TEXT"),
            "{fault:?}"
        );
        assert_one_printable_line(&fault.fault.reason, "the non-TEXT reason");
    }
    // A TEXT key that began with the marker reads back as `\u{1}text:<HEX>` (the injective
    // spelling) and is a blocking fault naming the reserved marker.
    let faults = decode_db_rows(&[(format!("{marker}text:016B"), "1".to_string())]);
    assert_eq!(faults[0].fault.kind, ConfigFaultKind::InvalidValue);
    assert!(
        faults[0].fault.reason.contains("reserved U+0001 marker"),
        "{faults:?}"
    );
}

/// A TOML parse failure never echoes the offending SOURCE LINE (a credential typed unquoted fails
/// the parse before the NFR-18 deny could redact it): the message names the file, the parser's
/// message and a byte offset only.
#[test]
fn a_parse_error_never_echoes_the_source_line() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("unblock");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.toml"), "auth_token = sk-live-SECRET123\n").unwrap();
    let env = MapEnv::new(&[("XDG_CONFIG_HOME", &tmp.path().to_string_lossy())]);
    let err = load_user_config(&env).expect_err("malformed");
    assert_eq!(err.code(), ErrorCode::ConfigParseError);
    let message = err.to_string();
    assert!(
        !message.contains("SECRET123"),
        "the source line is not echoed: {message}"
    );
    assert!(message.contains("at byte"), "{message}");
    assert_one_printable_line(&message, "the parse message");
}

/// NFR-18: `EnvOverrides` (captured by the session's reloader) never debug-prints the auth token.
#[test]
fn env_overrides_debug_redacts_the_auth_token() {
    let env = EnvOverrides::from_source(&MapEnv::new(&[
        ("UNBLOCK_AUTH_TOKEN", "tok-SECRET456"),
        ("UNBLOCK_ACTOR", "ann"),
    ]))
    .unwrap();
    let debug = format!("{env:?}");
    assert!(!debug.contains("SECRET456"), "{debug}");
    assert!(
        debug.contains("<redacted>") && debug.contains("ann"),
        "{debug}"
    );
}

/// One captured event: its level, target and field VALUES.
#[derive(Debug, Clone)]
struct Captured {
    level: Level,
    target: String,
    fields: HashMap<String, String>,
}

struct FieldGrab(HashMap<String, String>);

impl Visit for FieldGrab {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0
            .insert(field.name().to_string(), format!("{value:?}"));
    }
}

/// A scoped capture layer (no level filter) collecting every event.
struct CaptureLayer(Arc<Mutex<Vec<Captured>>>);

impl<S> Layer<S> for CaptureLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut grab = FieldGrab(HashMap::new());
        event.record(&mut grab);
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(Captured {
                level: *event.metadata().level(),
                target: event.metadata().target().to_string(),
                fields: grab.0,
            });
    }
}

/// Run `f` under a scoped capture subscriber and return the `unblock.config` WARN events.
fn config_warnings(f: impl FnOnce()) -> Vec<Captured> {
    let events = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::registry().with(CaptureLayer(Arc::clone(&events)));
    tracing::subscriber::with_default(subscriber, f);
    let events = events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    events
        .into_iter()
        .filter(|e| e.target == "unblock.config" && e.level == Level::WARN)
        .collect()
}

/// **D57 criterion (e2) — "warned", the user-file half**: an unknown user-file key is IGNORED (the
/// load succeeds) and WARNED exactly once on `unblock.config`, naming the key.
///
/// MUTANT KILLED: deleting `warn_unknown_keys()` from the user loader.
#[test]
fn an_unknown_user_file_key_is_warned_once() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("unblock");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.toml"), "future_key = 1\nactor = \"ann\"\n").unwrap();
    let env = MapEnv::new(&[("XDG_CONFIG_HOME", &tmp.path().to_string_lossy())]);
    let warnings = config_warnings(|| {
        let user = load_user_config(&env).expect("an unknown key is ignored");
        assert_eq!(user.actor.as_deref(), Some("ann"));
    });
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(
        warnings[0].fields.get("key").map(String::as_str),
        Some("future_key")
    );
}

/// **D57 criterion (e2) — "warned", the DB half**: an unknown DB row is WARNED exactly once on
/// `unblock.config`, naming the key, and its fault does not block.
///
/// MUTANT KILLED: deleting the `unknown_key` warn from the tolerant DB decode.
#[test]
fn an_unknown_db_row_is_warned_once() {
    let rows = vec![("zz_future".to_string(), "1".to_string())];
    let mut faults = Vec::new();
    let warnings = config_warnings(|| faults = decode_db_rows(&rows));
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(
        warnings[0].fields.get("key").map(String::as_str),
        Some("zz_future")
    );
    assert_eq!(faults[0].fault.kind, ConfigFaultKind::UnknownKey);
    assert!(!faults[0].fault.kind.is_blocking());
}
