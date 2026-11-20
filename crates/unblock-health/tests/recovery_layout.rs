//! The `.unblock/.recovery/` evidence layout (D58 clause (9); criterion (i), layout half).
//!
//! Name format, collision retry, the 16-attempt cap, confinement, the symlinked-root refusal, the
//! export copy and its refusals, the 0700/0600 modes, manifest round-trip, the temp-then-rename
//! manifest write and the stale scan.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tempfile::TempDir;
use unblock_error::ErrorCode;
use unblock_health::recovery::{
    Manifest, ManifestAction, ManifestOriginal, Outcome, Phase, RECOVERY_FORMAT, RunDir,
    manifest_file, run_dir_name, scan_recovery_root, sha256_bytes, sha256_file,
};
use unblock_health::{
    Advisory, AdvisoryFacts, DbFacts, DbFileFact, ExportFacts, ExportScan, HealthError,
    HealthLevel, PreOpenFacts, classify,
};

const DAY: Duration = Duration::from_hours(24);
const PID: u32 = 4242;

/// 2026-11-13T12:34:56.789Z
fn now() -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(1_794_573_296_789)
}

fn unblock_dir(dir: &TempDir) -> PathBuf {
    dir.path().join(".unblock")
}

fn root(dir: &TempDir) -> PathBuf {
    unblock_dir(dir).join(".recovery")
}

/// Write `body` to `.unblock/issues.jsonl` and return its path.
fn export_src(dir: &TempDir, body: &[u8]) -> PathBuf {
    std::fs::create_dir_all(unblock_dir(dir)).unwrap();
    let src = unblock_dir(dir).join("issues.jsonl");
    std::fs::write(&src, body).unwrap();
    src
}

fn assert_refused(result: Result<PathBuf, HealthError>, what: &str) {
    match result {
        Err(err @ HealthError::RecoveryEvidenceWrite { .. }) => {
            assert_eq!(err.code(), ErrorCode::IoError, "{what}");
        }
        other => panic!("{what}: expected RecoveryEvidenceWrite, got {other:?}"),
    }
}

fn assert_name_format(name: &str) {
    let (stamp, run_id) = name.split_once('-').expect("stamp-runid");
    assert_eq!(stamp.len(), 16, "{name}");
    assert_eq!(&stamp[8..9], "T");
    assert_eq!(&stamp[15..], "Z");
    assert!(stamp[..8].bytes().all(|b| b.is_ascii_digit()));
    assert!(stamp[9..15].bytes().all(|b| b.is_ascii_digit()));
    assert_eq!(run_id.len(), 8, "{name}");
    assert!(
        run_id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "{name}"
    );
}

#[test]
fn run_dir_name_is_utc_stamp_then_eight_lowercase_hex() {
    let name = run_dir_name(now(), PID, 0);
    assert_name_format(&name);
    assert!(name.starts_with("20261113T123456Z-"), "{name}");
    assert_eq!(name, run_dir_name(now(), PID, 0), "deterministic");
    assert_ne!(name, run_dir_name(now(), PID, 1), "attempt is hashed");
    assert_ne!(name, run_dir_name(now(), PID + 1, 0), "pid is hashed");
    assert_ne!(
        name,
        run_dir_name(now() + Duration::from_nanos(1), PID, 0),
        "nanoseconds are hashed"
    );
}

#[test]
fn create_makes_the_root_and_one_leaf_with_its_accessors() {
    let dir = TempDir::new().unwrap();
    let run = RunDir::create(&root(&dir), now(), PID).unwrap();
    assert_eq!(run.name(), run_dir_name(now(), PID, 0));
    assert_eq!(run.path(), root(&dir).join(run.name()));
    assert!(run.path().is_dir());
    assert_eq!(run.run_id(), &run.name()[17..]);
    assert_eq!(run.started_utc(), "2026-11-13T12:34:56Z");
    assert_eq!(std::fs::read_dir(run.path()).unwrap().count(), 0);
    assert!(!dir.path().join(".gitignore").exists());
    assert!(!root(&dir).join(".gitignore").exists());
}

#[test]
fn a_name_collision_retries_with_the_next_attempt() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(root(&dir).join(run_dir_name(now(), PID, 0))).unwrap();
    let run = RunDir::create(&root(&dir), now(), PID).unwrap();
    assert_eq!(run.name(), run_dir_name(now(), PID, 1));
}

#[test]
fn sixteen_collisions_fail_with_recovery_evidence_write() {
    let dir = TempDir::new().unwrap();
    for attempt in 0..16 {
        std::fs::create_dir_all(root(&dir).join(run_dir_name(now(), PID, attempt))).unwrap();
    }
    let err = RunDir::create(&root(&dir), now(), PID).unwrap_err();
    assert!(
        matches!(err, HealthError::RecoveryEvidenceWrite { .. }),
        "{err:?}"
    );
    assert_eq!(err.code(), ErrorCode::IoError);
    assert_eq!(err.code().exit_code(), 8);
    assert_eq!(std::fs::read_dir(root(&dir)).unwrap().count(), 16);
}

#[test]
fn a_root_that_cannot_be_created_fails_with_recovery_evidence_write() {
    let dir = TempDir::new().unwrap();
    let blocker = dir.path().join("file");
    std::fs::write(&blocker, b"x").unwrap();
    let err = RunDir::create(&blocker.join(".recovery"), now(), PID).unwrap_err();
    assert!(matches!(err, HealthError::RecoveryEvidenceWrite { .. }));
}

#[test]
fn a_root_that_is_a_file_is_refused() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(unblock_dir(&dir)).unwrap();
    std::fs::write(root(&dir), b"x").unwrap();
    let err = RunDir::create(&root(&dir), now(), PID).unwrap_err();
    assert!(matches!(err, HealthError::RecoveryEvidenceWrite { .. }));
    assert_eq!(std::fs::read(root(&dir)).unwrap(), b"x");
}

#[cfg(unix)]
#[test]
fn a_symlinked_recovery_root_is_refused_and_nothing_is_written_through_it() {
    let dir = TempDir::new().unwrap();
    let outside = dir.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::create_dir_all(unblock_dir(&dir)).unwrap();
    std::os::unix::fs::symlink(&outside, root(&dir)).unwrap();
    let err = RunDir::create(&root(&dir), now(), PID).unwrap_err();
    assert!(
        matches!(err, HealthError::RecoveryEvidenceWrite { .. }),
        "{err:?}"
    );
    assert_eq!(err.code(), ErrorCode::IoError);
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn evidence_directories_are_0700_and_evidence_files_0600() {
    use std::os::unix::fs::PermissionsExt as _;

    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    let dir = TempDir::new().unwrap();
    let src = export_src(&dir, b"{}\n");
    std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o644)).unwrap();
    let run = RunDir::create(&root(&dir), now(), PID).unwrap();
    let copy = run
        .copy_export(
            &src,
            &unblock_dir(&dir),
            &unblock_sync::SessionFiles::none(),
        )
        .unwrap();
    run.write_manifest(&sample_manifest(&run)).unwrap();
    assert_eq!(mode(&root(&dir)), 0o700, "a created recovery root");
    assert_eq!(mode(run.path()), 0o700, "the run directory");
    assert_eq!(mode(&run.path().join("export")), 0o700, "export/");
    assert_eq!(mode(&copy), 0o600, "the export copy");
    let original = run.write_original("unblock.db-shm", b"shm").unwrap();
    assert_eq!(mode(&run.path().join("original")), 0o700, "original/");
    assert_eq!(mode(&original), 0o600, "a capture");
    assert_eq!(
        mode(&run.path().join("manifest.json")),
        0o600,
        "the manifest"
    );
}

#[test]
fn confined_refuses_escapes_and_accepts_plain_relative_paths() {
    let dir = TempDir::new().unwrap();
    let run = RunDir::create(&root(&dir), now(), PID).unwrap();
    for bad in ["../escape", "raw/../../escape", "", "/etc/passwd", "./raw"] {
        let err = run.confined(Path::new(bad)).unwrap_err();
        assert!(
            matches!(err, HealthError::RecoveryEvidenceWrite { .. }),
            "{bad:?}: {err:?}"
        );
    }
    assert_eq!(
        run.confined(Path::new("raw/unblock.db")).unwrap(),
        run.path().join("raw").join("unblock.db")
    );
}

#[test]
fn copy_export_writes_export_issues_jsonl_and_never_overwrites() {
    let dir = TempDir::new().unwrap();
    let src = export_src(&dir, b"<<<<<<< HEAD\n{}\n=======\n{}\n>>>>>>> theirs\n");
    let confine = unblock_dir(&dir);
    let run = RunDir::create(&root(&dir), now(), PID).unwrap();
    let copy = run
        .copy_export(&src, &confine, &unblock_sync::SessionFiles::none())
        .unwrap();
    assert_eq!(copy, run.path().join("export").join("issues.jsonl"));
    assert_eq!(std::fs::read(&copy).unwrap(), std::fs::read(&src).unwrap());

    let err = run
        .copy_export(&src, &confine, &unblock_sync::SessionFiles::none())
        .unwrap_err();
    assert!(matches!(err, HealthError::RecoveryEvidenceWrite { .. }));
    assert_eq!(std::fs::read(&copy).unwrap(), std::fs::read(&src).unwrap());

    assert_refused(
        run.copy_export(
            &confine.join("absent.jsonl"),
            &confine,
            &unblock_sync::SessionFiles::none(),
        ),
        "a missing export",
    );
}

/// A run directory whose `export/` must stay absent after a refused copy.
fn assert_no_export_copy(run: &RunDir) {
    assert!(!run.path().join("export").exists(), "nothing was copied");
}

#[test]
fn copy_export_refuses_a_path_outside_the_confine_root() {
    let dir = TempDir::new().unwrap();
    let outside = dir.path().join("issues.jsonl");
    std::fs::write(&outside, b"{}\n").unwrap();
    let run = RunDir::create(&root(&dir), now(), PID).unwrap();
    assert_refused(
        run.copy_export(
            &outside,
            &unblock_dir(&dir),
            &unblock_sync::SessionFiles::none(),
        ),
        "an export outside .unblock/",
    );
    assert_refused(
        run.copy_export(
            &unblock_dir(&dir).join("..").join("issues.jsonl"),
            &unblock_dir(&dir),
            &unblock_sync::SessionFiles::none(),
        ),
        "a .. escape",
    );
    assert_no_export_copy(&run);
}

#[cfg(unix)]
#[test]
fn copy_export_refuses_a_symlink() {
    let dir = TempDir::new().unwrap();
    let real = export_src(&dir, b"{}\n");
    let link = unblock_dir(&dir).join("linked.jsonl");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let outside = dir.path().join("secret.jsonl");
    std::fs::write(&outside, b"secret\n").unwrap();
    let escaping = unblock_dir(&dir).join("escaping.jsonl");
    std::os::unix::fs::symlink(&outside, &escaping).unwrap();
    let run = RunDir::create(&root(&dir), now(), PID).unwrap();
    assert_refused(
        run.copy_export(
            &link,
            &unblock_dir(&dir),
            &unblock_sync::SessionFiles::none(),
        ),
        "a symlink inside .unblock/",
    );
    assert_refused(
        run.copy_export(
            &escaping,
            &unblock_dir(&dir),
            &unblock_sync::SessionFiles::none(),
        ),
        "a symlink leading outside .unblock/",
    );
    assert_no_export_copy(&run);
}

#[cfg(unix)]
#[test]
fn copy_export_refuses_a_fifo_without_blocking() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(unblock_dir(&dir)).unwrap();
    let fifo = unblock_dir(&dir).join("issues.jsonl");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(status.success());
    let run = RunDir::create(&root(&dir), now(), PID).unwrap();
    assert_refused(
        run.copy_export(
            &fifo,
            &unblock_dir(&dir),
            &unblock_sync::SessionFiles::none(),
        ),
        "a FIFO",
    );
    assert_no_export_copy(&run);
}

#[test]
fn copy_export_refuses_a_directory() {
    let dir = TempDir::new().unwrap();
    let src = unblock_dir(&dir).join("issues.jsonl");
    std::fs::create_dir_all(&src).unwrap();
    let run = RunDir::create(&root(&dir), now(), PID).unwrap();
    assert_refused(
        run.copy_export(
            &src,
            &unblock_dir(&dir),
            &unblock_sync::SessionFiles::none(),
        ),
        "a directory",
    );
    assert_no_export_copy(&run);
}

#[test]
fn copy_export_refuses_a_file_over_the_import_cap() {
    let dir = TempDir::new().unwrap();
    let src = export_src(&dir, b"");
    // Sparse: no bytes are written, only the length is set.
    std::fs::File::options()
        .write(true)
        .open(&src)
        .unwrap()
        .set_len(unblock_sync::MAX_IMPORT_FILE_BYTES + 1)
        .unwrap();
    let run = RunDir::create(&root(&dir), now(), PID).unwrap();
    let err = run
        .copy_export(
            &src,
            &unblock_dir(&dir),
            &unblock_sync::SessionFiles::none(),
        )
        .unwrap_err();
    match &err {
        HealthError::RecoveryEvidenceWrite { source, .. } => {
            assert_eq!(source.kind(), std::io::ErrorKind::FileTooLarge, "{err:?}");
        }
        other => panic!("expected RecoveryEvidenceWrite, got {other:?}"),
    }
    assert_no_export_copy(&run);
}

#[test]
fn write_original_writes_under_original_and_never_overwrites() {
    let dir = TempDir::new().unwrap();
    let run = RunDir::create(&root(&dir), now(), PID).unwrap();
    let wal = run.write_original("unblock.db-wal", b"12345").unwrap();
    let shm = run.write_original("unblock.db-shm", b"shm").unwrap();
    assert_eq!(wal, run.path().join("original").join("unblock.db-wal"));
    assert_eq!(std::fs::read(&wal).unwrap(), b"12345");
    assert_eq!(std::fs::read(&shm).unwrap(), b"shm");
    let err = run.write_original("unblock.db-wal", b"other").unwrap_err();
    assert!(matches!(err, HealthError::RecoveryEvidenceWrite { .. }));
    assert_eq!(std::fs::read(&wal).unwrap(), b"12345");
}

#[test]
fn write_original_refuses_a_name_that_is_not_one_plain_file_name() {
    let dir = TempDir::new().unwrap();
    let run = RunDir::create(&root(&dir), now(), PID).unwrap();
    for bad in ["", ".", "..", "../escape", "a/b", "a\\b", "/etc/passwd"] {
        let err = run.write_original(bad, b"x").unwrap_err();
        assert!(
            matches!(err, HealthError::RecoveryEvidenceWrite { .. }),
            "{bad:?}: {err:?}"
        );
    }
    assert!(!run.path().join("original").exists(), "nothing was written");
    assert!(!root(&dir).join("escape").exists());
}

#[test]
fn sha256_file_hashes_the_bytes() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("abc");
    std::fs::write(&path, b"abc").unwrap();
    assert_eq!(
        sha256_file(&path).unwrap(),
        (
            3,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_string()
        )
    );
    assert!(matches!(
        sha256_file(&dir.path().join("absent")),
        Err(HealthError::RecoveryEvidenceWrite { .. })
    ));
}

#[test]
fn sha256_bytes_matches_sha256_file() {
    assert_eq!(
        sha256_bytes(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        sha256_bytes(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn manifest_file_hashes_the_copy_inside_the_run_dir() {
    let dir = TempDir::new().unwrap();
    let src = export_src(&dir, b"abc");
    let run = RunDir::create(&root(&dir), now(), PID).unwrap();
    let copy = run
        .copy_export(
            &src,
            &unblock_dir(&dir),
            &unblock_sync::SessionFiles::none(),
        )
        .unwrap();

    let entry = manifest_file(run.path(), &copy).unwrap();
    assert_eq!(entry.path, "export/issues.jsonl");
    assert_eq!(entry.bytes, 3);
    assert_eq!(
        entry.sha256,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        manifest_file(run.path(), Path::new("export/issues.jsonl")).unwrap(),
        entry
    );

    // The live file is outside the run directory, so it is refused.
    assert!(matches!(
        manifest_file(run.path(), &src),
        Err(HealthError::RecoveryEvidenceWrite { .. })
    ));
    assert!(matches!(
        manifest_file(run.path(), Path::new("../issues.jsonl")),
        Err(HealthError::RecoveryEvidenceWrite { .. })
    ));
}

fn sample_manifest(run: &RunDir) -> Manifest {
    let mut manifest = Manifest::new(
        run,
        Phase::Preopen,
        "1.1.0",
        Some(4),
        HealthLevel::Recoverable,
    );
    manifest.actions.push(ManifestAction {
        action: "quarantine_sidecar".to_string(),
        target: None,
        outcome: Outcome::Refused,
        reason: Some("write_lock_held".to_string()),
    });
    manifest.actions.push(ManifestAction {
        action: "reindex".to_string(),
        target: Some("idx_issues_status".to_string()),
        outcome: Outcome::Applied,
        reason: None,
    });
    manifest
}

#[test]
fn manifest_round_trips_with_exactly_the_clause_nine_keys() {
    let dir = TempDir::new().unwrap();
    let src = export_src(&dir, b"{}\n");
    let run = RunDir::create(&root(&dir), now(), PID).unwrap();
    let copy = run
        .copy_export(
            &src,
            &unblock_dir(&dir),
            &unblock_sync::SessionFiles::none(),
        )
        .unwrap();
    let mut manifest = sample_manifest(&run);
    manifest
        .files
        .push(manifest_file(run.path(), &copy).unwrap());
    let original = run.write_original("unblock.db-shm", b"abc").unwrap();
    assert_eq!(original, run.path().join("original").join("unblock.db-shm"));
    let hashed = manifest_file(run.path(), &original).unwrap();
    manifest.originals.push(ManifestOriginal {
        path: hashed.path,
        bytes: hashed.bytes,
        sha256: hashed.sha256,
        note: Some("rebuilt by SQLite's read-only open".to_string()),
    });
    run.write_manifest(&manifest).unwrap();

    let bytes = std::fs::read(run.path().join("manifest.json")).unwrap();
    let back: Manifest = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(back, manifest);

    let value: Value = serde_json::from_slice(&bytes).unwrap();
    let keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let mut expected = vec![
        "format",
        "run_id",
        "started_utc",
        "phase",
        "binary_version",
        "schema_version",
        "level_before",
        "files",
        "originals",
        "actions",
    ];
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    expected.sort_unstable();
    assert_eq!(sorted, expected);
    assert_eq!(value["format"], RECOVERY_FORMAT);
    assert_eq!(value["format"], "unblock.recovery.v1");
    assert_eq!(value["run_id"], run.run_id());
    assert_eq!(value["started_utc"], "2026-11-13T12:34:56Z");
    assert_eq!(value["phase"], "preopen");
    assert_eq!(value["level_before"], "recoverable");
    assert_eq!(value["schema_version"], 4);
    let text = std::str::from_utf8(&bytes).unwrap();
    let at = |key: &str| text.find(&format!("\"{key}\":")).unwrap();
    assert!(
        at("files") < at("originals") && at("originals") < at("actions"),
        "originals is serialized after files"
    );
    let original = value["originals"][0].as_object().unwrap();
    let mut original_keys: Vec<&str> = original.keys().map(String::as_str).collect();
    original_keys.sort_unstable();
    assert_eq!(original_keys, vec!["bytes", "note", "path", "sha256"]);
    assert_eq!(value["originals"][0]["path"], "original/unblock.db-shm");
    assert_eq!(value["originals"][0]["bytes"], 3);
    assert_eq!(
        value["originals"][0]["sha256"],
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    let file = value["files"][0].as_object().unwrap();
    let mut file_keys: Vec<&str> = file.keys().map(String::as_str).collect();
    file_keys.sort_unstable();
    assert_eq!(file_keys, vec!["bytes", "path", "sha256"]);
    let action = value["actions"][0].as_object().unwrap();
    let mut action_keys: Vec<&str> = action.keys().map(String::as_str).collect();
    action_keys.sort_unstable();
    assert_eq!(action_keys, vec!["action", "outcome", "reason", "target"]);
    assert_eq!(value["actions"][0]["outcome"], "refused");
    assert_eq!(value["actions"][1]["outcome"], "applied");
    assert_eq!(
        serde_json::to_value(Outcome::Failed).unwrap(),
        Value::from("failed")
    );
    assert_eq!(
        serde_json::to_value(Phase::Online).unwrap(),
        Value::from("online")
    );
}

#[test]
fn manifest_rewrite_is_temp_then_rename_and_never_half_written() {
    let dir = TempDir::new().unwrap();
    let run = RunDir::create(&root(&dir), now(), PID).unwrap();
    let first = sample_manifest(&run);
    run.write_manifest(&first).unwrap();
    let entries: Vec<String> = std::fs::read_dir(run.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        entries,
        vec!["manifest.json".to_string()],
        "no temp file left"
    );

    // Rewrite at the phase's end.
    let mut second = first.clone();
    second.actions[0].outcome = Outcome::Failed;
    run.write_manifest(&second).unwrap();
    let back: Manifest =
        serde_json::from_slice(&std::fs::read(run.path().join("manifest.json")).unwrap()).unwrap();
    assert_eq!(back, second);

    // Inject a failure: the temp path is a directory, so the temp write fails before any rename.
    std::fs::create_dir(run.path().join(".manifest.json.tmp")).unwrap();
    let mut third = second.clone();
    third.actions.clear();
    let err = run.write_manifest(&third).unwrap_err();
    assert!(matches!(err, HealthError::RecoveryEvidenceWrite { .. }));
    let back: Manifest =
        serde_json::from_slice(&std::fs::read(run.path().join("manifest.json")).unwrap()).unwrap();
    assert_eq!(back, second, "the previous manifest survives whole");
}

#[test]
fn scan_recovery_root_parses_stamps_and_feeds_the_stale_advisory() {
    let dir = TempDir::new().unwrap();
    let recovery = root(&dir);
    let old = run_dir_name(now() - DAY * 31, PID, 0);
    let recent = run_dir_name(now() - DAY * 29, PID, 0);
    for name in [&old, &recent, &"moved-by-hand".to_string()] {
        std::fs::create_dir_all(recovery.join(name)).unwrap();
    }
    std::fs::write(recovery.join("stray-file"), b"x").unwrap();

    let entries = scan_recovery_root(&recovery);
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    let mut expected = vec![old.as_str(), recent.as_str(), "moved-by-hand"];
    expected.sort_unstable();
    assert_eq!(names, expected);
    let stamp_of = |name: &str| entries.iter().find(|e| e.name == name).unwrap().stamp_utc;
    let whole_seconds = |t: SystemTime| {
        UNIX_EPOCH + Duration::from_secs(t.duration_since(UNIX_EPOCH).unwrap().as_secs())
    };
    assert_eq!(stamp_of(&old), Some(whole_seconds(now() - DAY * 31)));
    assert_eq!(stamp_of(&recent), Some(whole_seconds(now() - DAY * 29)));
    assert_eq!(stamp_of("moved-by-hand"), None);

    let pre = PreOpenFacts {
        db: DbFileFact::SqliteHeader,
        lost: false,
        wal_len: None,
        shm_present: false,
        journal_present: false,
        write_lock_held: false,
        recovery_entries: entries,
    };
    let export = ExportFacts {
        path: None,
        scan: ExportScan::Absent,
    };
    let class = classify(
        &pre,
        Some(&DbFacts::default()),
        &export,
        &AdvisoryFacts { now_utc: now() },
    );
    assert_eq!(class.level, HealthLevel::Healthy);
    assert_eq!(
        class.advisories,
        vec![Advisory::StaleRecoveryArtifacts {
            count: 2,
            oldest_days: Some(31)
        }]
    );
}

#[test]
fn scan_recovery_root_of_an_absent_root_is_empty_and_creates_nothing() {
    let dir = TempDir::new().unwrap();
    assert!(scan_recovery_root(&root(&dir)).is_empty());
    assert!(!dir.path().join(".unblock").exists());
}
