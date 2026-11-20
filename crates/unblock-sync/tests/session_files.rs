//! PRD §4 D5 amendment (v1.1, `ub-c15`): a sync target is never the session database or one of its
//! `-wal`/`-shm`/`-journal` sidecars, under any name, `dry_run` included.
//!
//! Matrix: action ∈ {`import_jsonl`, `import_jsonl {dry_run}`, `import_bd`, `export_jsonl`} × target ∈
//! {db, `-wal`, `-shm`, `-journal`} × shape ∈ {the db itself carries the JSONL name, a hard link
//! `alias.jsonl`, a symlink `alias.jsonl`, a hard link to a symlinked database's target files}. The
//! own-name and hard-link shapes are refused as `SessionDatabase`; the symlink leaf is refused earlier
//! by `validate_sync_path` step 6 (`NonRegularFile`); a sidecar's own name is refused by the allowlist
//! (`DisallowedExtension`). Every refusal writes nothing and reads nothing: the target's bytes and
//! identity are unchanged, no temp is left in `.unblock/`, the storage saw zero writes and zero reads
//! (`list_issues` and `get_issue`), and — the fd witness — no descriptor is left on the target.
//! The session files are plain files created by the test (`SessionFiles::of_database` only stats
//! them).
//!
//! The same-name cells are unconditional; link and identity cells are `#[cfg(unix)]`.

mod common;
mod fake;

use std::fs;
use std::path::{Path, PathBuf};

use fake::{FakeStorage, sample_issue};
use unblock_error::ErrorCode;
use unblock_sync::{
    ExportOptions, ImportOptions, PathReject, SIDECAR_SUFFIXES, SessionFiles, SyncError,
    export_jsonl, import_bd, import_jsonl, serialize_issue_line, sidecar,
};

/// The four sync actions of the MCP `sync` tool (`import` with and without `dry_run`).
#[derive(Debug, Clone, Copy)]
enum Action {
    Import,
    ImportDry,
    ImportBd,
    Export,
}

const ACTIONS: [Action; 4] = [
    Action::Import,
    Action::ImportDry,
    Action::ImportBd,
    Action::Export,
];

/// A tempdir holding `.unblock/` with a database file named `db_name` whose bytes are one valid JSONL
/// record (so an import that is NOT refused would write it).
struct Ws {
    _dir: tempfile::TempDir,
    unblock: PathBuf,
    db: PathBuf,
}

fn line() -> String {
    serialize_issue_line(&sample_issue("ub-c15a")).expect("serialize")
}

fn workspace(db_name: &str) -> Ws {
    let dir = tempfile::tempdir().expect("tempdir");
    let unblock = dir.path().join(".unblock");
    fs::create_dir(&unblock).expect("mkdir .unblock");
    let unblock = fs::canonicalize(&unblock).expect("canonicalize");
    let db = unblock.join(db_name);
    fs::write(&db, format!("{}\n", line())).expect("write db");
    Ws {
        _dir: dir,
        unblock,
        db,
    }
}

/// Create the db's sidecars as plain files carrying the same JSONL bytes.
fn sidecars(db: &Path) -> Vec<PathBuf> {
    SIDECAR_SUFFIXES
        .iter()
        .map(|suffix| {
            let path = sidecar(db, suffix);
            fs::write(&path, format!("{}\n", line())).expect("write sidecar");
            path
        })
        .collect()
}

/// Run `action` with the default (confined) options.
async fn run(
    action: Action,
    storage: &FakeStorage,
    path: &Path,
    root: &Path,
    session: &SessionFiles,
) -> Result<(), SyncError> {
    run_with(action, storage, path, root, session, None).await
}

/// Run `action`; `external` is the written reason of an `allow_external` override.
async fn run_with(
    action: Action,
    storage: &FakeStorage,
    path: &Path,
    root: &Path,
    session: &SessionFiles,
    external: Option<&str>,
) -> Result<(), SyncError> {
    let import = |dry_run| ImportOptions {
        dry_run,
        allow_external: external.is_some(),
        external_reason: external.map(str::to_owned),
        ..ImportOptions::default()
    };
    match action {
        Action::Import => import_jsonl(storage, path, root, "tester", &import(false), session)
            .await
            .map(|_| ()),
        Action::ImportDry => import_jsonl(storage, path, root, "tester", &import(true), session)
            .await
            .map(|_| ()),
        Action::ImportBd => import_bd(storage, path, root, "tester", session)
            .await
            .map(|_| ()),
        Action::Export => export_jsonl(
            storage,
            path,
            root,
            &ExportOptions {
                allow_external: external.is_some(),
                external_reason: external.map(str::to_owned),
            },
            session.clone(),
        )
        .await
        .map(|_| ()),
    }
}

/// A snapshot of a file's bytes and (on unix) identity.
#[derive(Debug, PartialEq, Eq)]
struct Snap {
    bytes: Vec<u8>,
    #[cfg(unix)]
    id: (u64, u64),
}

fn snap(p: &Path) -> Snap {
    #[cfg(unix)]
    let id = {
        use std::os::unix::fs::MetadataExt as _;
        let m = fs::metadata(p).expect("stat");
        (m.dev(), m.ino())
    };
    Snap {
        bytes: fs::read(p).expect("read"),
        #[cfg(unix)]
        id,
    }
}

fn no_temp_left(dir: &Path) {
    for entry in fs::read_dir(dir).expect("read_dir") {
        let name = entry
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .into_owned();
        assert!(
            !Path::new(&name)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("tmp")),
            "a temp was left behind: {name}"
        );
    }
}

fn reason(err: &SyncError) -> Option<PathReject> {
    match err {
        SyncError::PathTraversal { reason, .. } => Some(*reason),
        _ => None,
    }
}

/// Run `action` on `target_path` and assert it is refused with `expected`, wrote nothing, read
/// nothing from storage, and left no descriptor on `watched` (the session file behind the target),
/// which keeps its bytes and identity.
async fn assert_refused(
    action: Action,
    ws: &Ws,
    target_path: &Path,
    watched: &Path,
    expected: PathReject,
) {
    assert_refused_with(action, ws, target_path, watched, expected, None).await;
}

async fn assert_refused_with(
    action: Action,
    ws: &Ws,
    target_path: &Path,
    watched: &Path,
    expected: PathReject,
    external: Option<&str>,
) {
    let storage = FakeStorage::with_issues(vec![sample_issue("ub-existing")]);
    let session = SessionFiles::of_database(&ws.db);
    let before = snap(watched);
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let fds = {
        common::fd_witness::assert_witness_sees(watched);
        common::fd_witness::descriptors_on(watched)
    };
    let err = run_with(
        action,
        &storage,
        target_path,
        &ws.unblock,
        &session,
        external,
    )
    .await
    .expect_err(&format!(
        "{action:?} on {} must be refused",
        target_path.display()
    ));
    assert_eq!(
        reason(&err),
        Some(expected),
        "{action:?} {}: {err:?}",
        target_path.display()
    );
    assert_eq!(err.code(), ErrorCode::PathTraversal);
    assert_eq!(err.code().exit_code(), 6);
    assert_eq!(
        snap(watched),
        before,
        "{action:?} changed {}",
        watched.display()
    );
    assert_eq!(
        storage.create_issues_calls(),
        0,
        "{action:?} wrote to storage"
    );
    assert_eq!(
        storage.create_issue_calls(),
        0,
        "{action:?} wrote to storage"
    );
    assert_eq!(
        storage.list_issues_calls(),
        0,
        "{action:?} read the corpus before refusing"
    );
    assert_eq!(
        storage.get_issue_calls(),
        0,
        "{action:?} read storage before refusing"
    );
    no_temp_left(&ws.unblock);
    if let Some(parent) = target_path.parent() {
        no_temp_left(parent);
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    assert_eq!(
        common::fd_witness::descriptors_on(watched),
        fds,
        "{action:?} left a descriptor on {} (a stat-refused target is never opened)",
        watched.display()
    );
}

/// (S1) Same name: the db itself is named `issues.jsonl` (the `--db .unblock/issues.jsonl` shape),
/// and every action targets that path.
#[tokio::test]
async fn same_name_database_is_refused_for_every_action() {
    for action in ACTIONS {
        let ws = workspace("issues.jsonl");
        let db = ws.db.clone();
        assert_refused(action, &ws, &db, &db, PathReject::SessionDatabase).await;
    }
}

/// (S2) A sidecar's own name (`issues.jsonl-wal`, …) is already refused by the allowlist; that
/// precedence is pinned.
#[tokio::test]
async fn sidecar_own_name_is_refused_by_the_allowlist_first() {
    for action in ACTIONS {
        let ws = workspace("issues.jsonl");
        for sidecar in sidecars(&ws.db) {
            assert_refused(
                action,
                &ws,
                &sidecar,
                &sidecar,
                PathReject::DisallowedExtension,
            )
            .await;
        }
    }
}

/// (S3) A hard link `alias.jsonl` → the db or a sidecar.
#[cfg(unix)]
#[tokio::test]
async fn hard_link_to_a_session_file_is_refused_for_every_action() {
    for action in ACTIONS {
        let ws = workspace("unblock.db");
        let mut targets = vec![ws.db.clone()];
        targets.extend(sidecars(&ws.db));
        for target in targets {
            let alias = ws.unblock.join("alias.jsonl");
            let _ = fs::remove_file(&alias);
            fs::hard_link(&target, &alias).expect("hard link");
            assert_refused(action, &ws, &alias, &target, PathReject::SessionDatabase).await;
            fs::remove_file(&alias).expect("remove alias");
        }
    }
}

/// (S4) A symlink `alias.jsonl` → the db or a sidecar: refused earlier by `validate_sync_path` step 6
/// (a precedence pin).
#[cfg(unix)]
#[tokio::test]
async fn symlink_to_a_session_file_is_refused_as_non_regular_first() {
    for action in ACTIONS {
        let ws = workspace("unblock.db");
        let mut targets = vec![ws.db.clone()];
        targets.extend(sidecars(&ws.db));
        for target in targets {
            let alias = ws.unblock.join("alias.jsonl");
            let _ = fs::remove_file(&alias);
            std::os::unix::fs::symlink(&target, &alias).expect("symlink");
            assert_refused(action, &ws, &alias, &target, PathReject::NonRegularFile).await;
            fs::remove_file(&alias).expect("remove alias");
        }
    }
}

/// (S5) A symlinked database keeps its sidecars next to the symlink's TARGET (`SQLite` resolves the
/// link): a JSONL-named hard link to the target db or one of its sidecars is a session file too, for
/// every action (`of_database` also stats the canonical base).
#[cfg(unix)]
#[tokio::test]
async fn symlinked_database_target_sidecars_are_session_files() {
    for action in ACTIONS {
        let dir = tempfile::tempdir().expect("tempdir");
        let unblock = fs::canonicalize(dir.path())
            .expect("canonicalize")
            .join(".unblock");
        let store = unblock.join("store");
        fs::create_dir_all(&store).expect("mkdir store");
        let real = store.join("unblock.db");
        fs::write(&real, format!("{}\n", line())).expect("write db");
        let targets = sidecars(&real);
        let link = unblock.join("unblock.db");
        std::os::unix::fs::symlink(&real, &link).expect("symlink db");
        let ws = Ws {
            _dir: dir,
            unblock: unblock.clone(),
            db: link,
        };
        let mut all = vec![real.clone()];
        all.extend(targets);
        for target in all {
            let alias = unblock.join("alias.jsonl");
            let _ = fs::remove_file(&alias);
            fs::hard_link(&target, &alias).expect("hard link");
            assert_refused(action, &ws, &alias, &target, PathReject::SessionDatabase).await;
            fs::remove_file(&alias).expect("remove alias");
        }
    }
}

/// (S6) CONTROL: a distinct `issues.jsonl` with the same bytes is not a session file. Import writes
/// one `create_issues`, the dry run writes nothing but reports, export writes the file: a cell that
/// refused everything would go red here.
#[tokio::test]
async fn a_distinct_file_is_not_refused() {
    let ws = workspace("unblock.db");
    let other = ws.unblock.join("issues.jsonl");
    fs::write(&other, format!("{}\n", line())).expect("write");
    let session = SessionFiles::of_database(&ws.db);

    let storage = FakeStorage::with_issues(vec![]);
    run(Action::Import, &storage, &other, &ws.unblock, &session)
        .await
        .expect("a distinct file imports");
    assert_eq!(storage.create_issues_calls(), 1);

    let storage = FakeStorage::with_issues(vec![]);
    let report = import_jsonl(
        &storage,
        &other,
        &ws.unblock,
        "tester",
        &ImportOptions {
            dry_run: true,
            ..ImportOptions::default()
        },
        &session,
    )
    .await
    .expect("a distinct file dry-runs");
    assert_eq!(
        report.imported + report.skipped,
        1,
        "the dry run reports the one record"
    );
    assert_eq!(storage.create_issues_calls(), 0, "a dry run writes nothing");

    let storage = FakeStorage::with_issues(vec![sample_issue("ub-export")]);
    fs::remove_file(&other).expect("remove");
    run(Action::Export, &storage, &other, &ws.unblock, &session)
        .await
        .expect("a distinct file exports");
    assert!(
        fs::read_to_string(&other)
            .expect("read the export")
            .contains("ub-export"),
        "the export wrote the file"
    );
}

/// (S7) The open + fstat half: a hard link to the db (what a rename between the stat pre-check and
/// the open produces) is recognized on the OPENED descriptor → `Ok(None)`, and that descriptor is
/// kept open (fd witness +1). A distinct file is handed back.
#[cfg(unix)]
#[test]
fn open_verified_detects_a_session_file_on_the_opened_descriptor() {
    let ws = workspace("unblock.db");
    let alias = ws.unblock.join("alias.jsonl");
    fs::hard_link(&ws.db, &alias).expect("hard link");
    let session = SessionFiles::of_database(&ws.db);
    assert!(session.contains(&alias));
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let fds = {
        common::fd_witness::assert_witness_sees(&ws.db);
        common::fd_witness::descriptors_on(&ws.db)
    };
    assert!(
        session.open_verified(&alias).expect("open").is_none(),
        "a session file must not be handed back"
    );
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    assert_eq!(
        common::fd_witness::descriptors_on(&ws.db),
        fds + 1,
        "the session file's descriptor is kept open, never closed"
    );
    let other = ws.unblock.join("other.jsonl");
    fs::write(&other, b"x\n").expect("write");
    assert!(session.open_verified(&other).expect("open").is_some());
}

/// (S8) `SessionFiles::none()` matches nothing: the session-aware scanners scan a normal file (and
/// even the database file itself) when no session is given.
#[test]
fn none_matches_nothing() {
    let ws = workspace("issues.jsonl");
    let none = SessionFiles::none();
    assert!(!none.contains(&ws.db));
    none.reject(&ws.db).expect("none rejects nothing");
    assert!(none.open_verified(&ws.db).expect("open").is_some());
    assert!(
        unblock_sync::scan_conflict_markers(&ws.db, &none)
            .expect("scan")
            .is_empty()
    );
    unblock_sync::ensure_no_conflict_markers(&ws.db, &none).expect("no markers");
    // `of_database` on a missing db contains nothing that exists.
    let missing = SessionFiles::of_database(&ws.unblock.join("nope.db"));
    assert!(!missing.contains(&ws.db));
}

/// (S9) The reason-gated `allow_external` override relaxes only containment, never the session
/// check: a database named `issues.jsonl` OUTSIDE `.unblock/`, targeted with the override and a
/// written reason, is still refused (`import_bd` has no override).
#[tokio::test]
async fn an_external_override_does_not_bypass_the_session_check() {
    for action in [Action::Import, Action::ImportDry, Action::Export] {
        let ws = workspace("unblock.db");
        let outside = ws
            .unblock
            .parent()
            .expect("the workspace root")
            .join("issues.jsonl");
        fs::write(&outside, format!("{}\n", line())).expect("write the external db");
        let external = Ws {
            _dir: tempfile::tempdir().expect("tempdir"),
            unblock: ws.unblock.clone(),
            db: outside.clone(),
        };
        assert_refused_with(
            action,
            &external,
            &outside,
            &outside,
            PathReject::SessionDatabase,
            Some("operator-approved external target"),
        )
        .await;
    }
}
