//! v1.0.3 (`ub-w1j`, PRD §4 D5 amendment): a sync target is never the session database or one of its
//! `-wal`/`-shm`/`-journal` sidecars, under any name.
//!
//! Matrix: action ∈ {`import_jsonl`, `import_bd`, `export_jsonl`} × target ∈ {db, `-wal`, `-shm`,
//! `-journal`} × shape ∈ {(a) the db itself carries the JSONL name, (b) a hard link `alias.jsonl`,
//! (c) a symlink `alias.jsonl`}. (a) and (b) are refused as `SessionDatabase`; (c) is refused earlier by
//! `validate_sync_path` step 6 (`NonRegularFile`); a sidecar's own name is refused by the allowlist
//! (`DisallowedExtension`). Every refusal writes nothing: the target's bytes and identity are unchanged,
//! no temp is left in `.unblock/`, and the storage saw zero writes. The session files are plain files
//! created by the test (`SessionFiles::of_database` only stats them).
//!
//! The same-name cells are unconditional (they compile on the Windows check); link and identity cells
//! are `#[cfg(unix)]`.

mod fake;

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use fake::{FakeStorage, sample_issue};
use unblock_error::ErrorCode;
use unblock_sync::{
    ExportOptions, ImportOptions, PathReject, SessionFiles, SyncError, export_jsonl,
    has_stream_colon, import_bd, import_jsonl, leaf_name_reject, serialize_issue_line,
    unportable_name_reject,
};

/// The three sync actions of the MCP `sync` tool.
#[derive(Debug, Clone, Copy)]
enum Action {
    Import,
    ImportBd,
    Export,
}

const ACTIONS: [Action; 3] = [Action::Import, Action::ImportBd, Action::Export];
const SIDECARS: [&str; 3] = ["-wal", "-shm", "-journal"];

/// A tempdir holding `.unblock/` with a database file named `db_name` whose bytes are one valid JSONL
/// record (so an import that is NOT refused would write it).
struct Ws {
    _dir: tempfile::TempDir,
    unblock: PathBuf,
    db: PathBuf,
}

fn line() -> String {
    serialize_issue_line(&sample_issue("ub-w1j1")).expect("serialize")
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
#[cfg(unix)]
fn sidecars(db: &Path) -> Vec<PathBuf> {
    SIDECARS
        .iter()
        .map(|suffix| {
            let mut name = db.as_os_str().to_os_string();
            name.push(suffix);
            let path = PathBuf::from(name);
            fs::write(&path, format!("{}\n", line())).expect("write sidecar");
            path
        })
        .collect()
}

async fn run(
    action: Action,
    storage: &FakeStorage,
    path: &Path,
    root: &Path,
    session: &SessionFiles,
) -> Result<(), SyncError> {
    match action {
        Action::Import => import_jsonl(
            storage,
            path,
            root,
            "tester",
            &ImportOptions::default(),
            session,
        )
        .await
        .map(|_| ()),
        Action::ImportBd => import_bd(storage, path, root, "tester", session)
            .await
            .map(|_| ()),
        Action::Export => export_jsonl(storage, path, root, &ExportOptions::default(), session)
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

fn no_temp_left(unblock: &Path) {
    for entry in fs::read_dir(unblock).expect("read_dir") {
        let name = entry
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .into_owned();
        assert!(
            !std::path::Path::new(&name)
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

/// Run `action` on `target_path` and assert it is refused with `expected` and wrote nothing (the
/// `watched` file — the session file behind the target — keeps its bytes and identity).
async fn assert_refused(
    action: Action,
    ws: &Ws,
    target_path: &Path,
    watched: &Path,
    expected: PathReject,
) {
    let storage = FakeStorage::with_issues(vec![sample_issue("ub-existing")]);
    let session = SessionFiles::of_database(&ws.db);
    let before = snap(watched);
    let err = run(action, &storage, target_path, &ws.unblock, &session)
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
    no_temp_left(&ws.unblock);
}

/// (a) same name: the db itself is named `issues.jsonl` (the `--db .unblock/issues.jsonl` shape), and
/// every action targets that path.
#[tokio::test]
async fn same_name_database_is_refused_for_every_action() {
    for action in ACTIONS {
        let ws = workspace("issues.jsonl");
        assert_refused(
            action,
            &ws,
            &ws.db.clone(),
            &ws.db.clone(),
            PathReject::SessionDatabase,
        )
        .await;
    }
}

/// (a) for a sidecar: its own name (`issues.jsonl-wal`, …) is already refused by the allowlist, and that
/// precedence is pinned.
#[tokio::test]
async fn sidecar_own_name_is_refused_by_the_allowlist_first() {
    for action in ACTIONS {
        let ws = workspace("issues.jsonl");
        for suffix in SIDECARS {
            let mut name = ws.db.as_os_str().to_os_string();
            name.push(suffix);
            let sidecar = PathBuf::from(name);
            fs::write(&sidecar, format!("{}\n", line())).expect("write sidecar");
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

/// (b) hard link `alias.jsonl` → the db or a sidecar.
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

/// (c) symlink `alias.jsonl` → the db or a sidecar: refused earlier by `validate_sync_path` step 6.
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

/// A distinct file with the same bytes is NOT a session file: the import runs (control).
#[tokio::test]
async fn a_distinct_file_is_not_refused() {
    let ws = workspace("unblock.db");
    let other = ws.unblock.join("issues.jsonl");
    fs::write(&other, format!("{}\n", line())).expect("write");
    let storage = FakeStorage::with_issues(vec![]);
    let session = SessionFiles::of_database(&ws.db);
    run(Action::Import, &storage, &other, &ws.unblock, &session)
        .await
        .expect("a distinct file imports");
    assert_eq!(storage.create_issues_calls(), 1);
}

/// The open + fstat half: a hard link to the db (what a rename between the stat pre-check and the open
/// produces) is recognized on the OPENED descriptor → `Ok(None)` (the descriptor is kept open).
#[cfg(unix)]
#[test]
fn open_verified_detects_a_session_file_on_the_opened_descriptor() {
    use std::os::unix::fs::MetadataExt as _;

    let ws = workspace("unblock.db");
    let alias = ws.unblock.join("alias.jsonl");
    fs::hard_link(&ws.db, &alias).expect("hard link");
    let session = SessionFiles::of_database(&ws.db);
    assert!(session.contains(&alias));
    assert!(
        session.open_verified(&alias).expect("open").is_none(),
        "a session file must not be handed back"
    );
    let err = session
        .open_import_source(&alias)
        .expect_err("refused at the reader");
    assert_eq!(reason(&err), Some(PathReject::SessionDatabase));
    // A distinct file is handed back.
    let other = ws.unblock.join("other.jsonl");
    fs::write(&other, b"x\n").expect("write");
    assert!(session.open_verified(&other).expect("open").is_some());
    // `with_inode` adds an extra identity (doctor's probe inode).
    let m = fs::metadata(&other).expect("stat");
    let extended = SessionFiles::none().with_inode(m.dev(), m.ino());
    assert!(extended.contains(&other));
    assert!(extended.open_verified(&other).expect("open").is_none());
}

/// `SessionFiles::none()` matches nothing: the delegating `pub` helpers behave exactly as in v1.0.2
/// (a scan of the database file itself is allowed when no session is given).
#[test]
fn none_matches_nothing() {
    let ws = workspace("issues.jsonl");
    let none = SessionFiles::none();
    assert!(!none.contains(&ws.db));
    none.reject(&ws.db).expect("none rejects nothing");
    assert!(none.open_verified(&ws.db).expect("open").is_some());
    assert!(
        unblock_sync::scan_conflict_markers(&ws.db)
            .expect("scan")
            .is_empty()
    );
    unblock_sync::ensure_no_conflict_markers(&ws.db).expect("no markers");
    // `of_database` on a missing db contains nothing that exists.
    let missing = SessionFiles::of_database(&ws.unblock.join("nope.db"));
    assert!(!missing.contains(&ws.db));
}

/// A symlinked database keeps its sidecars next to the symlink's TARGET (`SQLite` resolves the link):
/// a hard link with a JSONL name to the target's `-shm` (or the target db) is a session file too, for
/// every action (verify1 finding: the set is also derived from the canonical db path).
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

/// v1.0.4 (`ub-b9z`, NFR-7): a target whose name holds `:` (an NTFS stream of the db on Windows) or
/// is a Windows reserved device name is refused by `validate_sync_path` step (1a) for every action,
/// before any open. The lexical checks are public (`ub-gp0` reuses them on `main`).
#[tokio::test]
async fn colon_and_device_name_targets_are_refused_for_every_action() {
    assert_eq!(
        leaf_name_reject(OsStr::new("NUL.jsonl")),
        Some(PathReject::ReservedDeviceName)
    );
    assert_eq!(
        leaf_name_reject(OsStr::new("ab:c.jsonl")),
        Some(PathReject::ColonInName)
    );
    assert!(has_stream_colon(OsStr::new("xy:z")));
    assert!(!has_stream_colon(OsStr::new("issues.jsonl")));
    assert_eq!(
        unportable_name_reject(Path::new("/ws/xy:z/issues.jsonl")),
        cfg!(windows).then_some(PathReject::ColonInName)
    );
    assert_eq!(
        unportable_name_reject(Path::new("/ws/.unblock/NUL.jsonl")),
        Some(PathReject::ReservedDeviceName)
    );
    for action in ACTIONS {
        let ws = workspace("unblock.db");
        let stream = ws.unblock.join("unblock.db:x.jsonl");
        assert_refused(action, &ws, &stream, &ws.db, PathReject::ColonInName).await;
        let device = ws.unblock.join("NUL.jsonl");
        assert_refused(action, &ws, &device, &ws.db, PathReject::ReservedDeviceName).await;
    }
}
