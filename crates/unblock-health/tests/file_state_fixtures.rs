//! v1-lite file-state classification across a matrix of on-disk workspace states (a port of the
//! original `e2e_doctor_fixture_suite`, scoped to the v1 subset). One case per [`FileAnomaly`] plus
//! combinations, the healthy/empty baselines, and the `jsonl: None` path — materialized on disk via
//! `tempfile`. Plus proptests: `classify_file_state` never panics on arbitrary bytes and the
//! conflict-marker scan is a single pass over arbitrary content.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use proptest::prelude::*;
use tempfile::TempDir;
use unblock_health::{
    FileAnomaly, HealthLevel, classify_file_state, is_orphaned_lock_file,
    jsonl_has_conflict_markers, sidecar,
};

/// A temp workspace with `db`/`jsonl` paths (nothing materialized yet).
struct Fixture {
    _dir: TempDir,
    db: PathBuf,
    jsonl: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("unblock.db");
        let jsonl = dir.path().join("issues.jsonl");
        Self {
            _dir: dir,
            db,
            jsonl,
        }
    }

    /// Materialize a valid (magic-header) `SQLite` database file.
    fn write_valid_db(&self) {
        let mut f = std::fs::File::create(&self.db).unwrap();
        f.write_all(b"SQLite format 3\0").unwrap();
        f.write_all(&[0_u8; 100]).unwrap();
    }

    /// Materialize a clean single-line JSONL export.
    fn write_clean_jsonl(&self) {
        std::fs::write(&self.jsonl, "{\"id\":\"ub-1\"}\n").unwrap();
    }

    fn classify(&self) -> Vec<FileAnomaly> {
        classify_file_state(&self.db, Some(&self.jsonl))
    }
}

fn has_code(anomalies: &[FileAnomaly], code: &str) -> bool {
    anomalies.iter().any(|a| a.code() == code)
}

#[test]
fn case_healthy_is_empty() {
    let fx = Fixture::new();
    fx.write_valid_db();
    fx.write_clean_jsonl();
    assert!(fx.classify().is_empty());
}

#[test]
fn case_empty_workspace_is_empty() {
    // No db, no jsonl: DatabaseMissing needs a jsonl to recover from, so nothing fires.
    let fx = Fixture::new();
    assert!(fx.classify().is_empty());
}

#[test]
fn case_jsonl_none_never_fires_jsonl_anomalies() {
    let fx = Fixture::new();
    // db missing + jsonl None → no DatabaseMissing (needs jsonl present) and no conflict scan.
    assert!(classify_file_state(&fx.db, None).is_empty());
}

#[test]
fn case_database_missing() {
    let fx = Fixture::new();
    fx.write_clean_jsonl(); // db absent, jsonl present
    let anomalies = fx.classify();
    assert!(has_code(&anomalies, "database_missing"));
    assert_eq!(anomalies[0].severity(), HealthLevel::Recoverable);
}

#[test]
fn case_database_not_sqlite() {
    let fx = Fixture::new();
    std::fs::write(&fx.db, "this is not a sqlite file").unwrap();
    fx.write_clean_jsonl();
    assert!(has_code(&fx.classify(), "database_not_sqlite"));
}

#[test]
fn case_tiny_db_below_magic_is_not_sqlite() {
    let fx = Fixture::new();
    std::fs::write(&fx.db, b"short").unwrap(); // < 16 bytes → read_exact fails
    fx.write_clean_jsonl();
    assert!(has_code(&fx.classify(), "database_not_sqlite"));
}

#[test]
fn case_shm_without_wal_is_sidecar_mismatch() {
    let fx = Fixture::new();
    fx.write_valid_db();
    fx.write_clean_jsonl();
    std::fs::write(sidecar(&fx.db, "-shm"), [0_u8; 64]).unwrap();
    let anomalies = fx.classify();
    assert!(anomalies.iter().any(|a| matches!(
        a,
        FileAnomaly::SidecarMismatch {
            has_wal: false,
            has_shm: true
        }
    )));
}

#[test]
fn case_truncated_wal() {
    let fx = Fixture::new();
    fx.write_valid_db();
    fx.write_clean_jsonl();
    std::fs::write(sidecar(&fx.db, "-wal"), b"short wal").unwrap(); // < 32 bytes
    assert!(has_code(&fx.classify(), "truncated_wal"));
}

#[test]
fn case_empty_wal_is_not_truncated() {
    // A 0-byte `-wal` is a VALID live/checkpointed state (SQLite creates it on open before writing any
    // frame) — it must NOT be flagged as truncated. This preserves beads' observable "healthy → clean"
    // behavior under unblock's persistent-open Session (which classifies the live-open DB).
    let fx = Fixture::new();
    fx.write_valid_db();
    fx.write_clean_jsonl();
    std::fs::write(sidecar(&fx.db, "-wal"), []).unwrap(); // 0-byte WAL
    std::fs::write(sidecar(&fx.db, "-shm"), [0_u8; 64]).unwrap(); // both present → no SidecarMismatch
    assert!(
        fx.classify().is_empty(),
        "an empty live WAL (with its -shm) is a healthy state, not truncated"
    );
}

#[test]
fn case_journal_sidecar_present() {
    let fx = Fixture::new();
    fx.write_valid_db();
    fx.write_clean_jsonl();
    std::fs::write(sidecar(&fx.db, "-journal"), b"journal data").unwrap();
    assert!(has_code(&fx.classify(), "journal_sidecar_present"));
}

#[test]
fn case_fresh_lock_is_not_orphaned_but_injected_clock_flags_it() {
    let fx = Fixture::new();
    fx.write_valid_db();
    fx.write_clean_jsonl();
    let lock = fx.db.parent().unwrap().join(".unblock.lock");
    std::fs::write(&lock, "pid:12345").unwrap();

    // Through classify (real `now`) a just-created lock is NOT orphaned.
    assert!(!has_code(&fx.classify(), "orphaned_lock_file"));

    // With an injected clock 1h in the future the same lock IS stale/orphaned (positive path).
    let future = SystemTime::now() + Duration::from_hours(1);
    assert!(is_orphaned_lock_file(&lock, future));
    // And not stale against the real present.
    assert!(!is_orphaned_lock_file(&lock, SystemTime::now()));
}

#[test]
fn case_jsonl_conflict_markers_each_sigil() {
    for marker in ["<<<<<<<", ">>>>>>>", "=======", "|||||||"] {
        let fx = Fixture::new();
        fx.write_valid_db();
        std::fs::write(&fx.jsonl, format!("{marker} something\n{{\"id\":\"a\"}}\n")).unwrap();
        let anomalies = fx.classify();
        assert!(
            has_code(&anomalies, "jsonl_conflict_markers"),
            "sigil {marker:?} must be detected"
        );
        assert_eq!(
            anomalies
                .iter()
                .find(|a| a.code() == "jsonl_conflict_markers")
                .unwrap()
                .severity(),
            HealthLevel::Unsafe
        );
    }
}

#[test]
fn case_diff3_style_markers_are_detected() {
    let fx = Fixture::new();
    fx.write_valid_db();
    std::fs::write(
        &fx.jsonl,
        "<<<<<<< HEAD\n{\"id\":\"a\"}\n||||||| base\n{\"id\":\"base\"}\n=======\n{\"id\":\"b\"}\n>>>>>>> branch\n",
    )
    .unwrap();
    assert!(has_code(&fx.classify(), "jsonl_conflict_markers"));
}

#[test]
fn case_combined_anomalies_keep_deterministic_order_and_worst_is_max() {
    // Conflict markers (Unsafe, pushed 5th) + a journal sidecar (Recoverable, pushed 6th): the push
    // order is variant-declaration order — the faithful beads order, so JsonlConflictMarkers comes
    // BEFORE JournalSidecarPresent — and the composite worst is the `max` (Unsafe).
    let fx = Fixture::new();
    fx.write_valid_db();
    std::fs::write(sidecar(&fx.db, "-journal"), b"journal data").unwrap();
    std::fs::write(&fx.jsonl, "<<<<<<< HEAD\n{\"id\":\"a\"}\n").unwrap();

    let anomalies = fx.classify();
    let codes: Vec<&str> = anomalies.iter().map(FileAnomaly::code).collect();
    assert_eq!(codes, ["jsonl_conflict_markers", "journal_sidecar_present"]);

    let worst = anomalies
        .iter()
        .map(FileAnomaly::severity)
        .max()
        .unwrap_or(HealthLevel::Healthy);
    assert_eq!(worst, HealthLevel::Unsafe);
}

#[test]
fn case_truncated_wal_boundary_values() {
    // SF1 — boundary-value analysis around the 32-byte WAL-header threshold and the ratified D29
    // lower bound: a non-empty `-wal` of 1..=31 bytes IS truncated; 0 bytes (a valid live/checkpointed
    // WAL) and >= 32 bytes are NOT. Guards the off-by-one on the deviating `(1..MIN_WAL_LEN)` line.
    for (len, fires) in [(0_usize, false), (1, true), (31, true), (32, false)] {
        let fx = Fixture::new();
        fx.write_valid_db();
        fx.write_clean_jsonl();
        std::fs::write(sidecar(&fx.db, "-wal"), vec![0_u8; len]).unwrap();
        // A `-shm` alongside the `-wal` keeps SidecarMismatch (shm-without-wal) quiet for the 0-byte
        // case, so `truncated_wal` is the sole variable under test.
        std::fs::write(sidecar(&fx.db, "-shm"), [0_u8; 64]).unwrap();
        assert_eq!(
            has_code(&fx.classify(), "truncated_wal"),
            fires,
            "a {len}-byte -wal: truncated_wal should fire == {fires}"
        );
    }
}

#[test]
fn case_orphaned_lock_emitted_by_classify_via_backdated_mtime() {
    // SF2 — drive the REAL-clock `classify_file_state` path (not just the injected-clock helper) so it
    // genuinely EMITS OrphanedLockFile: create `.unblock.lock` next to the db and BACKDATE its mtime
    // an hour past the 30-min stale threshold via `File::set_modified` (stable since Rust 1.75 — no
    // `filetime` dependency).
    let fx = Fixture::new();
    fx.write_valid_db();
    fx.write_clean_jsonl();
    let lock = fx.db.parent().unwrap().join(".unblock.lock");
    std::fs::write(&lock, b"pid:12345").unwrap();
    let backdated = SystemTime::now()
        .checked_sub(Duration::from_hours(1))
        .expect("now - 1h is representable");
    // Reopen (no truncate) purely to set the mtime; the write above is already flushed + closed.
    let handle = std::fs::File::options().write(true).open(&lock).unwrap();
    handle.set_modified(backdated).unwrap();
    drop(handle);

    assert!(
        has_code(&fx.classify(), "orphaned_lock_file"),
        "a lock backdated 1h past the 30-min stale threshold must be emitted by classify_file_state"
    );
}

#[test]
fn case_db_and_jsonl_as_directories_degrade_without_panic() {
    // SF6 — adversarial fs-shape: the db path and jsonl path are DIRECTORIES-in-place. classify must
    // not panic and must not misfire DatabaseNotSqlite (a directory is not a readable SQLite file);
    // the conflict scan over a directory conservatively returns false (a dir cannot be read as bytes).
    let fx = Fixture::new();
    std::fs::create_dir(&fx.db).unwrap();
    std::fs::create_dir(&fx.jsonl).unwrap();
    let anomalies = fx.classify(); // must not panic
    assert!(
        !has_code(&anomalies, "database_not_sqlite"),
        "a directory-in-place db is not flagged as corrupt SQLite (db.is_file() is false): {anomalies:?}"
    );
    assert!(
        !jsonl_has_conflict_markers(&fx.jsonl),
        "a directory cannot be read as bytes — the conflict scan degrades to false"
    );
}

#[cfg(unix)]
#[test]
fn case_broken_symlink_paths_degrade_without_panic() {
    // SF6 — the db + jsonl paths are BROKEN symlinks (target absent). `is_file()` follows the link and
    // reports false, so no anomaly fires and neither the classifier nor the conflict scan panics.
    use std::os::unix::fs::symlink;
    let fx = Fixture::new();
    let missing = fx.db.parent().unwrap().join("nope-does-not-exist");
    symlink(&missing, &fx.db).unwrap();
    symlink(&missing, &fx.jsonl).unwrap();
    let anomalies = fx.classify(); // must not panic
    assert!(
        anomalies.is_empty(),
        "broken symlinks resolve to nothing on disk: {anomalies:?}"
    );
    assert!(!jsonl_has_conflict_markers(&fx.jsonl));
}

#[cfg(unix)]
#[test]
fn case_permission_denied_jsonl_degrades_without_panic() {
    // SF6 — a jsonl carrying conflict markers but chmod 000 (unreadable). The scan must NOT panic;
    // when the process cannot open it (non-root) it degrades to `false` (absence of evidence). The
    // exact boolean depends on privilege (root bypasses the mode bit), so the invariant under test is
    // graceful, panic-free degradation.
    use std::os::unix::fs::PermissionsExt;
    let fx = Fixture::new();
    fx.write_valid_db();
    std::fs::write(&fx.jsonl, "<<<<<<< HEAD\n{\"id\":\"a\"}\n").unwrap();
    std::fs::set_permissions(&fx.jsonl, std::fs::Permissions::from_mode(0o000)).unwrap();

    let _ = jsonl_has_conflict_markers(&fx.jsonl); // must not panic
    let _ = fx.classify(); // classify over the same workspace must also not panic

    // Restore permissions so the tempdir cleanup is unencumbered.
    std::fs::set_permissions(&fx.jsonl, std::fs::Permissions::from_mode(0o644)).unwrap();
}

/// Materialize `bytes` at `path`.
fn write_bytes(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap();
}

proptest! {
    /// `classify_file_state` never panics for arbitrary db/jsonl file contents.
    #[test]
    fn classify_never_panics_on_arbitrary_bytes(db_bytes in proptest::collection::vec(any::<u8>(), 0..512),
                                                jsonl_bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("unblock.db");
        let jsonl = dir.path().join("issues.jsonl");
        write_bytes(&db, &db_bytes);
        write_bytes(&jsonl, &jsonl_bytes);
        // The only assertion is that this returns (does not panic) — determinism/severity are covered
        // by the fixture cases above.
        let _ = classify_file_state(&db, Some(&jsonl));
    }

    /// The conflict-marker scan terminates (single pass) and never panics on arbitrary content, and a
    /// buffer that never begins a line with a sigil is reported clean.
    #[test]
    fn conflict_scan_never_panics_and_is_clean_without_a_leading_sigil(bytes in proptest::collection::vec(any::<u8>(), 0..1024)) {
        let dir = TempDir::new().unwrap();
        let jsonl = dir.path().join("issues.jsonl");
        write_bytes(&jsonl, &bytes);
        let flagged = jsonl_has_conflict_markers(&jsonl);
        // If flagged, SOME line must start with a 7-byte sigil — re-verify independently (a plain
        // line-split) that a sigil exists at a line start, proving no false positive from mid-line
        // bytes. `split` yields every line including the first (content before the first `\n`).
        if flagged {
            let has_sigil_at_line_start = bytes.split(|&b| b == b'\n').any(line_starts_with_sigil);
            prop_assert!(has_sigil_at_line_start, "a flagged file has a sigil at a line start");
        }
    }
}

/// Whether `line` starts with one of the four 7-byte conflict sigils.
fn line_starts_with_sigil(line: &[u8]) -> bool {
    const SIGILS: [&[u8; 7]; 4] = [b"<<<<<<<", b">>>>>>>", b"=======", b"|||||||"];
    SIGILS.iter().any(|sigil| line.starts_with(*sigil))
}
