//! FR-13 full through the engine (PRD §4 D57, `ub-lp9.2`): the four `config` operations, the live
//! reload, the clause (2a) fail-closed gate and its repair — over REAL libsql storage.
//!
//! Workspace cells open through `unblock_config::open_with_storage_with_env` over an INJECTED
//! environment (NFR-16), so no developer's `$HOME`, `XDG_CONFIG_HOME` or `UNBLOCK_*` reaches a cell.
//! Each `config_…` cell names the D57 clause (12) criterion it covers.

mod common;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use common::parked::ParkedStorage;
use common::{dep, session_over, session_over_in_dir};
use unblock_config::{CliOverrides, ConfigError, EnvSource, open_with_storage_with_env};
use unblock_engine::{
    DeleteMode, DeletePlan, EngineError, ImportOptions, IssuePatch, NewIssue, Session,
    SessionConfig,
};
use unblock_error::{CodedError, ErrorCode, StructuredError};
use unblock_model::{
    ConfigEntry, ConfigFaultKind, ConfigSource, DependencyType, DiagnosticKind, Issue, ListFilters,
};
use unblock_storage::{LibsqlStorage, Storage};

// ------------------------------------------------------------------------------------------------
// Harness
// ------------------------------------------------------------------------------------------------

/// An injected environment (NFR-16): only the listed variables exist.
#[derive(Default)]
struct MapEnv(HashMap<String, String>);

impl MapEnv {
    fn with(pairs: &[(&str, &str)]) -> Self {
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

/// A fresh `.unblock/` workspace under a tempdir (the guard owns it).
fn workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(dir.path().join(".unblock")).expect("mkdir .unblock");
    dir
}

/// Open a live `Session` over `dir` through the storage-bearing facade, with `env` injected.
async fn open(dir: &Path, env: &MapEnv) -> Session {
    let ctx = open_with_storage_with_env(Some(dir), &CliOverrides::default(), env)
        .await
        .expect("storage-bearing open");
    Session::open(ctx, SessionConfig::default())
        .await
        .expect("Session::open")
}

/// Write `rows` straight into the workspace's `config` table (storage stores opaque strings — it
/// never classifies a key), the way another build or a hand edit would, then close.
async fn seed_rows(dir: &Path, rows: &[(&str, &str)]) {
    let ctx = open_with_storage_with_env(Some(dir), &CliOverrides::default(), &MapEnv::default())
        .await
        .expect("seed open");
    for (key, value) in rows {
        ctx.storage.set_config(key, value).await.expect("seed row");
    }
}

/// The workspace's `config` rows, read through a SEPARATE storage handle on the same file.
async fn table(dir: &Path) -> Vec<(String, String)> {
    let db = dir.join(".unblock").join("unblock.db");
    let storage = LibsqlStorage::open_local(&db, 1_000)
        .await
        .expect("open db");
    storage.read_config().await.expect("read_config")
}

/// A real in-memory storage, migrated, plus a session over it (actor `tester` at the `$USER` tier).
async fn memory_session() -> (Session, Arc<dyn Storage>) {
    let storage = LibsqlStorage::open_in_memory().await.expect("in-memory");
    storage.migrate().await.expect("migrate");
    let storage: Arc<dyn Storage> = Arc::new(storage);
    let session = session_over(Arc::clone(&storage), SessionConfig::default()).await;
    (session, storage)
}

/// Assert `err` is the clause (2a) fail-closed gate naming exactly `keys`, with a hint that names
/// each key's recovery call.
fn assert_gated(err: &EngineError, keys: &[&str], what: &str) {
    assert!(
        matches!(
            err,
            EngineError::Config {
                source: ConfigError::FaultyDbConfig { .. }
            }
        ),
        "{what}: expected FaultyDbConfig, got {err:?}"
    );
    let structured: StructuredError = err.into();
    assert_eq!(structured.code, ErrorCode::ConfigError, "{what}");
    assert_eq!(structured.code.exit_code(), 7, "{what}");
    assert_eq!(
        structured.context["keys"],
        serde_json::json!(keys),
        "{what}: context.keys"
    );
    let hint = structured.hint.unwrap_or_default();
    for key in keys {
        assert!(
            hint.contains(&format!(r#""key":"{key}""#)),
            "{what}: the hint names {key}'s recovery call: {hint}"
        );
    }
}

fn new_issue(title: &str) -> NewIssue {
    NewIssue {
        title: title.to_string(),
        ..NewIssue::default()
    }
}

/// Drive EVERY gated `Session` method once and assert each is refused by the gate (D57 clause
/// (2a)): every read, every mutation, interchange, `diagnostics`, `integrity_check`, `migrate`,
/// `doctor` and `recover`.
async fn assert_every_gated_method_refuses(session: &Session, dir: &Path, keys: &[&str]) {
    let filters = ListFilters::default();
    let jsonl = dir.join(".unblock").join("issues.jsonl");
    let issue = Issue {
        id: "ub-gate".to_string(),
        title: "gate".to_string(),
        ..Issue::default()
    };
    macro_rules! gated {
        ($call:expr) => {{
            let err = $call.await.expect_err(stringify!($call));
            assert_gated(&err, keys, stringify!($call));
        }};
    }
    gated!(session.get("ub-x"));
    gated!(session.list(&filters));
    gated!(session.ready(&filters));
    gated!(session.blocked(&filters));
    gated!(session.search("x", &filters));
    gated!(session.count(&filters, None));
    gated!(session.stale(Utc::now(), &filters));
    gated!(session.list_dependencies("ub-x"));
    gated!(session.list_comments("ub-x"));
    gated!(session.dependency_tree("ub-x"));
    gated!(session.dependency_graph(&[]));
    gated!(session.detect_cycles(true));
    gated!(session.diagnostics(DiagnosticKind::Stats, None));
    gated!(session.integrity_check());
    gated!(session.list_all_labels());
    gated!(session.epic_statuses(None, false));
    gated!(session.export_jsonl(&jsonl));
    gated!(session.import_jsonl(&jsonl, ImportOptions::default()));
    gated!(session.import_bd(&jsonl));
    gated!(session.migrate());
    gated!(session.doctor());
    gated!(session.recover());
    gated!(session.create(&issue));
    gated!(session.create_issue(new_issue("gated")));
    gated!(session.create_bulk(vec![new_issue("gated")]));
    gated!(session.update("ub-x", &IssuePatch::default()));
    gated!(session.delete(&DeletePlan {
        mode: DeleteMode::DryRun,
        targets: vec!["ub-x".to_string()],
        cascade_children: Vec::new(),
    }));
    gated!(session.restore("ub-x"));
    gated!(session.claim("ub-x", "someone"));
    gated!(session.defer("ub-x", Utc::now()));
    gated!(session.undefer("ub-x"));
    gated!(session.add_dep(&dep("ub-x", "ub-y", DependencyType::Blocks)));
    gated!(session.remove_dep("ub-x", "ub-y", &DependencyType::Blocks));
    gated!(session.add_comment("ub-x", "body"));
    gated!(session.update_comment(1, "body"));
    gated!(session.delete_comment(1));
    gated!(session.close_with_suggestions("ub-x", None));
    gated!(session.rename_label("a", "b"));
    gated!(session.rename_label("a", "a"));
    gated!(session.close_eligible(true));
}

// ------------------------------------------------------------------------------------------------
// (d) DB FAULT GATE, (d2) GATE EXEMPTIONS
// ------------------------------------------------------------------------------------------------

/// (d)+(d2): a STARTUP-key row fail-closes every gated method with `FaultyDbConfig` (context.keys +
/// the `unset` hint) and writes nothing, while `open`, the four `config` ops and `shutdown` run;
/// `config list` reports the row with its fault, and `config unset` of it re-opens the SAME session.
#[tokio::test]
async fn config_startup_key_row_fail_closes_and_unset_repairs_the_same_session() {
    let ws = workspace();
    seed_rows(ws.path(), &[("db_filename", "elsewhere.db")]).await;
    // (d2) `Session::open` succeeds over the faulty table.
    let session = open(ws.path(), &MapEnv::default()).await;

    assert_every_gated_method_refuses(&session, ws.path(), &["db_filename"]).await;
    let structured: StructuredError = (&session.get("ub-x").await.expect_err("gated")).into();
    assert!(
        structured
            .hint
            .as_deref()
            .unwrap_or_default()
            .contains(r#"issue {"action":"config","op":"unset","key":"db_filename"}"#),
        "the hint names the exact recovery call"
    );
    assert_eq!(table(ws.path()).await.len(), 1, "the gate wrote nothing");

    // (d2) all four config ops run; list reports the row with its fault.
    let list = session
        .config_list()
        .await
        .expect("list runs under the gate");
    let keys: Vec<&str> = list.iter().map(|e| e.key.as_str()).collect();
    assert_eq!(
        keys,
        ["actor", "db_filename", "output_format", "search_cap"]
    );
    let row = &list[1];
    assert_eq!(row.value.as_deref(), Some("elsewhere.db"));
    assert_eq!(row.effective, None);
    assert_eq!(row.source, None);
    assert_eq!(
        row.fault.as_ref().map(|f| f.kind),
        Some(ConfigFaultKind::StartupKey)
    );
    let got = session.config_get("db_filename").await.expect("get runs");
    assert_eq!(got, *row);
    session
        .config_set("search_cap", "12")
        .await
        .expect("set runs under the gate");
    // A set of ANOTHER key leaves the startup-key fault in place: still gated.
    assert_gated(
        &session
            .list(&ListFilters::default())
            .await
            .expect_err("still gated"),
        &["db_filename"],
        "after an unrelated set",
    );

    let unset = session
        .config_unset("db_filename")
        .await
        .expect("unset repairs");
    assert_eq!(unset.key, "db_filename");
    assert_eq!(
        (unset.value, unset.effective, unset.source, unset.fault),
        (None, None, None, None),
        "a non-layer key's unset entry is all-null"
    );
    assert_eq!(
        table(ws.path()).await,
        [("search_cap".to_string(), "12".to_string())]
    );
    // The very next non-config call on the SAME session succeeds.
    session
        .list(&ListFilters::default())
        .await
        .expect("un-gated on the same session");
    assert_eq!(session.runtime().search_cap, 12);

    // (d2) shutdown closes cleanly.
    session.shutdown().await.expect("shutdown");
}

/// (d)+(d2): an UNDECODABLE layer-key row (`search_cap = 0`) is a blocking `invalid_value` fault
/// whose hint reads the row first; `config set` with a valid value REPLACES it and re-opens the
/// session, and `shutdown` closes a gated session cleanly.
#[tokio::test]
async fn config_undecodable_layer_row_fail_closes_and_set_replaces_it() {
    let ws = workspace();
    seed_rows(ws.path(), &[("search_cap", "0")]).await;
    let session = open(ws.path(), &MapEnv::default()).await;

    assert_every_gated_method_refuses(&session, ws.path(), &["search_cap"]).await;
    let structured: StructuredError = (&session
        .ready(&ListFilters::default())
        .await
        .expect_err("gated"))
        .into();
    let hint = structured.hint.unwrap_or_default();
    assert!(
        hint.contains(r#"issue {"action":"config","op":"get","key":"search_cap"}"#),
        "a LAYER key's hint reads the row first: {hint}"
    );

    let got = session.config_get("search_cap").await.expect("get");
    assert_eq!(got.value.as_deref(), Some("0"));
    assert_eq!(
        got.fault.as_ref().map(|f| f.kind),
        Some(ConfigFaultKind::InvalidValue)
    );
    // The faulty row contributes nothing: the effective value is the default's.
    assert_eq!(got.effective.as_deref(), Some("50"));
    assert_eq!(got.source, Some(ConfigSource::Default));

    let set = session
        .config_set("search_cap", "7")
        .await
        .expect("set replaces");
    assert_eq!(set.value.as_deref(), Some("7"));
    assert_eq!(set.effective.as_deref(), Some("7"));
    assert_eq!(set.source, Some(ConfigSource::Db));
    assert_eq!(set.fault, None);
    session
        .search("x", &ListFilters::default())
        .await
        .expect("un-gated after the replace");

    // A gated session still shuts down cleanly.
    seed_rows(ws.path(), &[("id_prefix", "zz")]).await;
    let gated = open(ws.path(), &MapEnv::default()).await;
    assert!(gated.list(&ListFilters::default()).await.is_err());
    gated
        .shutdown()
        .await
        .expect("a gated session closes cleanly");
}

// ------------------------------------------------------------------------------------------------
// (d3) CROSS-PROCESS REPAIR
// ------------------------------------------------------------------------------------------------

/// (d3): two sessions over one gated workspace. A deletes the startup-key row; B, still gated,
/// runs the hinted `unset` — no row holds the key and B's snapshot lists it as a fault — which
/// SUCCEEDS with no write, re-resolves, and un-gates B's very next call.
#[tokio::test]
async fn config_cross_process_unset_of_a_stale_fault_succeeds_without_a_write() {
    let ws = workspace();
    seed_rows(ws.path(), &[("backend", "libsql")]).await;
    let a = open(ws.path(), &MapEnv::default()).await;
    let b = open(ws.path(), &MapEnv::default()).await;

    a.config_unset("backend").await.expect("A repairs");
    a.list(&ListFilters::default()).await.expect("A un-gated");
    assert_gated(
        &b.list(&ListFilters::default())
            .await
            .expect_err("B is stale"),
        &["backend"],
        "B before its own config call",
    );
    // `get` has NO stale-fault exception: a startup key no row holds is refused.
    let refused = b.config_get("backend").await.expect_err("get refuses");
    assert!(matches!(
        refused,
        EngineError::Config {
            source: ConfigError::StartupKeyInRuntimeLayer {
                layer: ConfigSource::Db,
                ..
            }
        }
    ));

    let entry = b
        .config_unset("backend")
        .await
        .expect("B's hinted unset succeeds");
    assert_eq!(
        (entry.value, entry.effective, entry.source, entry.fault),
        (None, None, None, None)
    );
    assert!(table(ws.path()).await.is_empty(), "B wrote nothing");
    b.list(&ListFilters::default()).await.expect("B un-gated");
}

/// (d3): after A's repair, a `config set` of ANY layer key on B re-resolves from the rows read
/// under its guard and clears B's stale gate.
#[tokio::test]
async fn config_cross_process_set_of_any_layer_key_clears_a_stale_gate() {
    let ws = workspace();
    seed_rows(ws.path(), &[("jsonl_export", "true")]).await;
    let a = open(ws.path(), &MapEnv::default()).await;
    let b = open(ws.path(), &MapEnv::default()).await;

    a.config_unset("jsonl_export").await.expect("A repairs");
    assert!(b.list(&ListFilters::default()).await.is_err(), "B is stale");
    b.config_set("output_format", "markdown")
        .await
        .expect("B sets");
    b.list(&ListFilters::default()).await.expect("B un-gated");
    // And `get`/`list` report a row ANOTHER session wrote, with its real value.
    let seen = a
        .config_get("output_format")
        .await
        .expect("A reads B's row");
    assert_eq!(seen.value.as_deref(), Some("markdown"));
}

// ------------------------------------------------------------------------------------------------
// (e) NON-LAYER jsonl_export, (e2) UNKNOWN KEYS, (e3) STARTUP KEYS IN THE ACTION
// ------------------------------------------------------------------------------------------------

/// (e): `jsonl_export` is not a layer key — with no row, set/get/unset are `CONFIG_ERROR` naming
/// `ub-p1h` with zero writes and `list` omits it; a row of it is a BLOCKING `invalid_value` fault
/// that `list` reports and `unset` removes.
#[tokio::test]
async fn config_jsonl_export_is_refused_without_a_row_and_removable_with_one() {
    let ws = workspace();
    let session = open(ws.path(), &MapEnv::default()).await;
    for err in [
        session
            .config_set("jsonl_export", "true")
            .await
            .expect_err("set"),
        session.config_get("jsonl_export").await.expect_err("get"),
        session
            .config_unset("jsonl_export")
            .await
            .expect_err("unset"),
    ] {
        assert!(
            matches!(
                err,
                EngineError::Config {
                    source: ConfigError::InvalidValue { .. }
                }
            ),
            "{err:?}"
        );
        assert_eq!(err.code(), ErrorCode::ConfigError);
        assert!(err.to_string().contains("ub-p1h"), "{err}");
    }
    assert!(table(ws.path()).await.is_empty(), "zero writes");
    let keys: Vec<String> = session
        .config_list()
        .await
        .expect("list")
        .into_iter()
        .map(|e| e.key)
        .collect();
    assert_eq!(keys, ["actor", "output_format", "search_cap"]);
    drop(session);

    seed_rows(ws.path(), &[("jsonl_export", "true")]).await;
    let session = open(ws.path(), &MapEnv::default()).await;
    assert_gated(
        &session
            .list(&ListFilters::default())
            .await
            .expect_err("gated"),
        &["jsonl_export"],
        "jsonl_export row",
    );
    let listed = session.config_list().await.expect("list");
    let row = listed
        .iter()
        .find(|e| e.key == "jsonl_export")
        .expect("listed with its fault");
    assert_eq!(
        row.fault.as_ref().map(|f| f.kind),
        Some(ConfigFaultKind::InvalidValue)
    );
    session
        .config_unset("jsonl_export")
        .await
        .expect("unset removes it");
    assert!(table(ws.path()).await.is_empty());
    session
        .list(&ListFilters::default())
        .await
        .expect("un-gated");
}

/// (e2): an UNKNOWN DB row is listed with `unknown_key`, blocks nothing and is removable; `set` of
/// an unknown key, and `get`/`unset` of one no row holds, are `UnknownKey` with zero writes.
#[tokio::test]
async fn config_unknown_keys_warn_list_and_never_block() {
    let ws = workspace();
    seed_rows(ws.path(), &[("from_the_future", "1")]).await;
    let session = open(ws.path(), &MapEnv::default()).await;
    session
        .list(&ListFilters::default())
        .await
        .expect("an unknown row blocks nothing");
    let row = session
        .config_get("from_the_future")
        .await
        .expect("get reaches it");
    assert_eq!(row.value.as_deref(), Some("1"));
    assert_eq!(
        row.fault.as_ref().map(|f| f.kind),
        Some(ConfigFaultKind::UnknownKey)
    );
    assert!(
        session
            .config_list()
            .await
            .expect("list")
            .iter()
            .any(|e| e.key == "from_the_future")
    );
    session
        .config_unset("from_the_future")
        .await
        .expect("removable");
    assert!(table(ws.path()).await.is_empty());

    for err in [
        session.config_set("nope", "1").await.expect_err("set"),
        session.config_get("nope").await.expect_err("get"),
        session.config_unset("nope").await.expect_err("unset"),
    ] {
        assert!(
            matches!(
                &err,
                EngineError::Config {
                    source: ConfigError::UnknownKey { key }
                } if key == "nope"
            ),
            "{err:?}"
        );
        assert_eq!(err.code(), ErrorCode::ConfigError);
    }
    assert!(table(ws.path()).await.is_empty(), "zero writes");
}

/// (e3): `set db_filename`, and `get`/`unset db_filename` with no row and no snapshot fault, are
/// `StartupKeyInRuntimeLayer` with `context.layer: "db"` and zero writes.
#[tokio::test]
async fn config_startup_keys_are_refused_by_the_action() {
    let (session, storage) = memory_session().await;
    for err in [
        session
            .config_set("db_filename", "x.db")
            .await
            .expect_err("set"),
        session
            .config_set("jsonl_export_filename", "x.jsonl")
            .await
            .expect_err("set alias"),
        session.config_get("db_filename").await.expect_err("get"),
        session
            .config_unset("db_filename")
            .await
            .expect_err("unset"),
    ] {
        assert!(
            matches!(
                err,
                EngineError::Config {
                    source: ConfigError::StartupKeyInRuntimeLayer {
                        layer: ConfigSource::Db,
                        ..
                    }
                }
            ),
            "{err:?}"
        );
        let structured: StructuredError = (&err).into();
        assert_eq!(structured.code, ErrorCode::ConfigError);
        assert_eq!(structured.context["layer"], "db");
    }
    assert!(storage.read_config().await.expect("rows").is_empty());
}

// ------------------------------------------------------------------------------------------------
// (g) ACTOR CHAIN, (h) SET AND RELOAD
// ------------------------------------------------------------------------------------------------

/// (g): `config set actor "  bob  "` stores the TRIMMED `bob`; an all-blank actor is `CONFIG_ERROR`.
#[tokio::test]
async fn config_set_actor_trims_and_refuses_blank() {
    let (session, storage) = memory_session().await;
    let entry = session.config_set("actor", "  bob  ").await.expect("set");
    assert_eq!(entry.value.as_deref(), Some("bob"));
    assert_eq!(entry.effective.as_deref(), Some("bob"));
    assert_eq!(entry.source, Some(ConfigSource::Db));
    let err = session.config_set("actor", "   ").await.expect_err("blank");
    assert_eq!(err.code(), ErrorCode::ConfigError);
    assert_eq!(
        storage.read_config().await.expect("rows"),
        [("actor".to_string(), "bob".to_string())]
    );
}

/// (h): `config set search_cap` is visible to the very next search on the same live session.
#[tokio::test]
async fn config_set_search_cap_reaches_the_next_search_without_a_reopen() {
    let (session, _storage) = memory_session().await;
    for i in 0..5 {
        session
            .create_issue(new_issue(&format!("needle {i}")))
            .await
            .expect("seed");
    }
    let all = session
        .search("needle", &ListFilters::default())
        .await
        .expect("search");
    assert_eq!(all.len(), 5);
    session.config_set("search_cap", "2").await.expect("set");
    let capped = session
        .search("needle", &ListFilters::default())
        .await
        .expect("search");
    assert_eq!(
        capped.len(),
        2,
        "the reloaded cap applies to the very next search"
    );
}

/// (h): `config set actor` makes the next mutation's events AND a `dep add` edge's `created_by`
/// carry the new actor (the caller's `created_by` is overwritten); `config unset` restores the next
/// layer's value (`$USER`, source `os_user`).
#[tokio::test]
async fn config_set_actor_reaches_events_and_dep_created_by_and_unset_restores() {
    let (session, storage) = memory_session().await;
    let a = session.create_issue(new_issue("a")).await.expect("a");
    let b = session.create_issue(new_issue("b")).await.expect("b");

    session.config_set("actor", "bob").await.expect("set actor");
    assert_eq!(session.actor(), "bob");
    session
        .update(
            &a.id,
            &IssuePatch {
                title: Some("a2".to_string()),
                ..IssuePatch::default()
            },
        )
        .await
        .expect("update");
    let events = storage.list_events(&a.id).await.expect("events");
    let last = events.last().expect("an update event");
    assert_eq!(last.actor, "bob");

    let mut edge = dep(&a.id, &b.id, DependencyType::Blocks);
    edge.created_by = Some("someone-else".to_string());
    session.add_dep(&edge).await.expect("dep add");
    let edges = session.list_dependencies(&a.id).await.expect("deps");
    assert_eq!(edges[0].created_by.as_deref(), Some("bob"));

    let restored = session.config_unset("actor").await.expect("unset");
    assert_eq!(restored.value, None);
    assert_eq!(restored.effective.as_deref(), Some("tester"));
    assert_eq!(restored.source, Some(ConfigSource::OsUser));
    assert_eq!(session.actor(), "tester");
}

// ------------------------------------------------------------------------------------------------
// (i) VALIDATION BEFORE COMMIT
// ------------------------------------------------------------------------------------------------

/// (i): an invalid value (`search_cap` `0`, `-1`, `9223372036854775808`, junk), a startup key and a
/// missing value each leave the table byte-identical and the snapshot unchanged.
#[tokio::test]
async fn config_invalid_values_leave_table_and_snapshot_unchanged() {
    let (session, storage) = memory_session().await;
    session
        .config_set("search_cap", "9")
        .await
        .expect("baseline");
    let rows_before = storage.read_config().await.expect("rows");
    let snapshot_before = session.runtime();
    for (key, value) in [
        ("search_cap", "0"),
        ("search_cap", "-1"),
        ("search_cap", "9223372036854775808"),
        ("search_cap", "lots"),
        ("output_format", "yaml"),
        ("actor", ""),
        ("id_prefix", "zz"),
    ] {
        let err = session.config_set(key, value).await.expect_err(value);
        assert_eq!(err.code(), ErrorCode::ConfigError, "{key}={value}");
        assert_eq!(storage.read_config().await.expect("rows"), rows_before);
        assert_eq!(*session.runtime(), *snapshot_before, "{key}={value}");
    }
}

/// (i): a reload failure caused by a malformed USER file fails `set` and `unset` with zero writes
/// and the snapshot unchanged.
#[tokio::test]
async fn config_reload_failure_from_a_malformed_user_file_writes_nothing() {
    let ws = workspace();
    let xdg = tempfile::tempdir().expect("xdg");
    let user_file = xdg.path().join("unblock").join("config.toml");
    std::fs::create_dir_all(user_file.parent().expect("parent")).expect("mkdir");
    std::fs::write(&user_file, "search_cap = 9\n").expect("user file");
    let env = MapEnv::with(&[("XDG_CONFIG_HOME", xdg.path().to_str().expect("utf8"))]);
    let session = open(ws.path(), &env).await;
    assert_eq!(session.runtime().search_cap, 9);
    assert_eq!(session.runtime().sources.search_cap, ConfigSource::User);
    seed_rows(ws.path(), &[("output_format", "csv")]).await;
    let rows_before = table(ws.path()).await;
    let snapshot_before = session.runtime();

    std::fs::write(&user_file, "search_cap = = broken\n").expect("break the user file");
    let set = session
        .config_set("search_cap", "3")
        .await
        .expect_err("reload fails");
    assert_eq!(set.code(), ErrorCode::ConfigParseError);
    let unset = session
        .config_unset("output_format")
        .await
        .expect_err("reload fails");
    assert_eq!(unset.code(), ErrorCode::ConfigParseError);
    assert_eq!(table(ws.path()).await, rows_before, "zero writes");
    assert_eq!(*session.runtime(), *snapshot_before, "snapshot unchanged");
}

// ------------------------------------------------------------------------------------------------
// (j) SHADOW AND SHAPES
// ------------------------------------------------------------------------------------------------

/// (j): a DB value under an env value reports `value` = the row, `effective` = the env value and
/// `source: "env"` — the shadow is visible in the same response.
#[tokio::test]
async fn config_db_value_shadowed_by_env_reports_both() {
    let ws = workspace();
    let env = MapEnv::with(&[("UNBLOCK_OUTPUT_FORMAT", "markdown")]);
    let session = open(ws.path(), &env).await;
    let entry = session
        .config_set("output_format", "csv")
        .await
        .expect("set");
    assert_eq!(entry.value.as_deref(), Some("csv"));
    assert_eq!(entry.effective.as_deref(), Some("markdown"));
    assert_eq!(entry.source, Some(ConfigSource::Env));
    assert_eq!(
        session.config_get("output_format").await.expect("get"),
        entry
    );
}

/// (j): `list` on an empty table is exactly the three layer keys in key order, each `value: null`,
/// `fault: null`, with its resolved source; `get` returns one entry of that shape; `unset` of a
/// LAYER key with no row returns the entry with no error and no write.
#[tokio::test]
async fn config_empty_table_shapes() {
    let (session, storage) = memory_session().await;
    let list = session.config_list().await.expect("list");
    let expect = |key: &str, effective: &str, source: ConfigSource| ConfigEntry {
        key: key.to_string(),
        value: None,
        effective: Some(effective.to_string()),
        source: Some(source),
        fault: None,
    };
    assert_eq!(
        list,
        [
            expect("actor", "tester", ConfigSource::OsUser),
            expect("output_format", "json", ConfigSource::Default),
            expect("search_cap", "50", ConfigSource::Default),
        ]
    );
    assert_eq!(
        session.config_get("search_cap").await.expect("get"),
        list[2]
    );
    assert_eq!(
        session.config_unset("search_cap").await.expect("unset"),
        list[2]
    );
    assert!(
        storage.read_config().await.expect("rows").is_empty(),
        "no write"
    );
}

// ------------------------------------------------------------------------------------------------
// (k) LOCKING AND CANCELLATION
// ------------------------------------------------------------------------------------------------

/// (k): `set` holds the write permit AND the `.write.lock` across its commit (a concurrent writer
/// and a second process's lock attempt both wait), while `get` and `list` take neither.
#[tokio::test]
async fn config_set_holds_permit_and_write_lock_while_get_and_list_do_not() {
    let ws = workspace();
    let unblock_dir = ws.path().join(".unblock");
    let db = unblock_dir.join("unblock.db");
    let inner = LibsqlStorage::open_local(&db, 5_000).await.expect("open");
    inner.migrate().await.expect("migrate");
    let parked = ParkedStorage::new_gated_config(Arc::new(inner));
    let storage: Arc<dyn Storage> = Arc::clone(&parked) as Arc<dyn Storage>;
    let session = Arc::new(
        session_over_in_dir(
            storage,
            SessionConfig::default(),
            ws.path().to_path_buf(),
            unblock_dir,
        )
        .await,
    );

    let setter = {
        let session = Arc::clone(&session);
        tokio::spawn(async move { session.config_set("search_cap", "4").await })
    };
    parked.wait_until_parked().await;

    // get and list complete while the guard is held.
    let got = tokio::time::timeout(Duration::from_secs(5), session.config_get("search_cap"))
        .await
        .expect("get takes no guard")
        .expect("get");
    assert_eq!(got.value.as_deref(), Some("4"), "the row is committed");
    tokio::time::timeout(Duration::from_secs(5), session.config_list())
        .await
        .expect("list takes no guard")
        .expect("list");

    // A concurrent in-process writer waits for the permit.
    let writer = {
        let session = Arc::clone(&session);
        tokio::spawn(async move { session.create_issue(new_issue("waits")).await })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !writer.is_finished(),
        "the in-process writer waits for the permit"
    );

    // Another process's `.write.lock` attempt times out while the set holds it.
    let other = LibsqlStorage::open_local(&db, 100)
        .await
        .expect("second handle");
    let locked = other
        .acquire_write_lock()
        .await
        .expect_err("the lock is held");
    assert_eq!(locked.code(), ErrorCode::DatabaseLocked);

    parked.release();
    setter.await.expect("join").expect("set");
    writer.await.expect("join").expect("the writer proceeds");
    assert_eq!(session.runtime().search_cap, 4);
}

/// (k): a `config set` future DROPPED after its commit still leaves the snapshot equal to the
/// committed table — the commit and the swap run in one task that owns the guard.
#[tokio::test]
async fn config_set_future_dropped_after_commit_still_swaps() {
    let inner = LibsqlStorage::open_in_memory().await.expect("in-memory");
    inner.migrate().await.expect("migrate");
    let parked = ParkedStorage::new_gated_config(Arc::new(inner));
    let storage: Arc<dyn Storage> = Arc::clone(&parked) as Arc<dyn Storage>;
    let session = Arc::new(session_over(Arc::clone(&storage), SessionConfig::default()).await);

    let caller = {
        let session = Arc::clone(&session);
        tokio::spawn(async move { session.config_set("search_cap", "3").await })
    };
    parked.wait_until_parked().await;
    caller.abort();
    assert!(
        caller
            .await
            .expect_err("the caller future was dropped")
            .is_cancelled(),
        "the caller was cancelled after the commit"
    );
    assert_eq!(
        session.runtime().search_cap,
        50,
        "not swapped yet: still parked"
    );

    parked.release();
    // A mutation waits for the guard the commit+swap task owns, so once it runs the swap is done.
    session
        .create_issue(new_issue("barrier"))
        .await
        .expect("barrier");
    assert_eq!(
        storage.read_config().await.expect("rows"),
        [("search_cap".to_string(), "3".to_string())]
    );
    assert_eq!(
        session.runtime().search_cap,
        3,
        "the snapshot equals the committed table"
    );
}
