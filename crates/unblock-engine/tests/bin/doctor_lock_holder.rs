//! PRD §4 D29 clause F6 (`ub-kpe`) and D5 amendment (`ub-w1j`) test-only helper `[[bin]]`: ONE
//! process holding a real `Session` — and so `SQLite`'s SHARED lock — while the parent test observes
//! it from outside (`F_GETLK`, `/proc/locks` on Linux, and a fresh `locking_mode=EXCLUSIVE` observer).
//!
//! Usage: `doctor_lock_holder <unblock_dir> [--db <db_path>]`. Startup copies the cli's doctor open
//! order (`commands/doctor.rs`; the engine cannot depend on the cli, so the production order itself is
//! pinned by the cli cell): `open_workspace_with_cli` → `DoctorPreflight::capture` →
//! `open_resolved_with_storage` → `Session::open` → one read (so the connection holds SHARED) →
//! `READY`.
//!
//! One reply line per stdin line (stdout flushed per reply):
//! - `NOOP` → `OK`
//! - `DOCTOR` → `DOCTORED <labels>` / `DOCTOR-FAILED <err>`; a second one → `DOCTOR-SPENT`
//! - `READ` → `READ <n>` / `READ-FAILED <err>`
//! - `PROBE-V102` → runs the DELETED v1.0.2 header probe verbatim → `PROBED <bool>`
//! - `PROBE-SHM-CLOSE` → `File::open(<db>-shm)` + drop → `SHM-CLOSED`
//! - `OPEN-VERIFIED <path>` → `SessionFiles::of_database(db).open_verified(path)` → `VERIFIED
//!   <none|some>` (`sync` feature; else `UNSUPPORTED sync`)
//! - `IMPORT <path>` / `IMPORT-BD <path>` / `EXPORT <path>` → `SYNC-OK` / `SYNC-REFUSED <code> <msg>`
//! - `QUIT` / EOF → `BYE`, drop the session, exit 0
//!
//! Every failure prints a reply instead of panicking mid-protocol; the parent asserts on replies.

#![forbid(unsafe_code)]

use std::io::{BufRead as _, Write as _};
use std::path::{Path, PathBuf};

use unblock_config::{CliOverrides, open_resolved_with_storage, open_workspace_with_cli};
use unblock_engine::{DoctorPreflight, ImportOptions, Session, SessionConfig};
use unblock_error::CodedError as _;

/// The v1.0.2 header probe, copied VERBATIM from `v1.0.2:crates/unblock-health/src/file_state.rs`
/// (`db_header_mismatches_sqlite`): the in-tree positive control (`kpe1m`) that the instrument can
/// see the lock loss this descriptor close causes.
fn db_header_mismatches_sqlite(db: &Path) -> bool {
    use std::io::Read as _;

    let Ok(mut file) = std::fs::File::open(db) else {
        return false;
    };
    let mut header = [0_u8; 16];
    file.read_exact(&mut header).is_err() || header != *b"SQLite format 3\0"
}

fn reply(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// `<db><suffix>`.
fn sidecar(db: &Path, suffix: &str) -> PathBuf {
    let mut name = db.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn sync_reply<T>(result: unblock_engine::Result<T>) -> String {
    match result {
        Ok(_) => "SYNC-OK".to_owned(),
        Err(err) => format!("SYNC-REFUSED {} {err}", err.code().as_str()),
    }
}

#[cfg(feature = "sync")]
fn open_verified(db: &Path, path: &Path) -> String {
    match unblock_sync::SessionFiles::of_database(db).open_verified(path) {
        Ok(None) => "VERIFIED none".to_owned(),
        Ok(Some(_file)) => "VERIFIED some".to_owned(),
        Err(err) => format!("VERIFY-FAILED {err}"),
    }
}

#[cfg(not(feature = "sync"))]
fn open_verified(_db: &Path, _path: &Path) -> String {
    "UNSUPPORTED sync".to_owned()
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (dir, db_override) = match args.as_slice() {
        [_, dir] => (PathBuf::from(dir), None),
        [_, dir, flag, db] if flag == "--db" => (PathBuf::from(dir), Some(PathBuf::from(db))),
        _ => {
            eprintln!("usage: doctor_lock_holder <unblock_dir> [--db <db_path>]");
            std::process::exit(2);
        }
    };
    let mut overrides = CliOverrides::new().with_dir(dir);
    if let Some(db) = db_override {
        overrides = overrides.with_db(db);
    }

    // The cli's doctor open order (D29 clause F6).
    let resolved = match open_workspace_with_cli(&overrides).await {
        Ok(resolved) => resolved,
        Err(err) => return reply(&format!("OPEN-FAILED {err}")),
    };
    let db = resolved.paths.db_path.clone();
    let mut preflight = Some(DoctorPreflight::capture(&db));
    let ctx = match open_resolved_with_storage(resolved).await {
        Ok(ctx) => ctx,
        Err(err) => return reply(&format!("OPEN-FAILED {err}")),
    };
    let session = match Session::open(
        ctx,
        SessionConfig {
            import_on_open: false,
            ..SessionConfig::default()
        },
    )
    .await
    {
        Ok(session) => session,
        Err(err) => return reply(&format!("OPEN-FAILED {err}")),
    };
    if let Err(err) = session.integrity_check().await {
        return reply(&format!("OPEN-FAILED {err}"));
    }
    reply("READY");

    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    while let Some(Ok(line)) = lines.next() {
        let line = line.trim();
        let (command, arg) = line.split_once(' ').unwrap_or((line, ""));
        let path = Path::new(arg);
        let answer = match command {
            "NOOP" => "OK".to_owned(),
            "DOCTOR" => match preflight.take() {
                None => "DOCTOR-SPENT".to_owned(),
                Some(preflight) => match session.doctor(preflight).await {
                    Ok(report) => format!(
                        "DOCTORED {}",
                        report
                            .findings
                            .iter()
                            .map(|f| f.label.as_str())
                            .collect::<Vec<_>>()
                            .join(",")
                    ),
                    Err(err) => format!("DOCTOR-FAILED {err}"),
                },
            },
            "READ" => match session.integrity_check().await {
                Ok(rows) => format!("READ {}", rows.len()),
                Err(err) => format!("READ-FAILED {err}"),
            },
            "PROBE-V102" => format!("PROBED {}", db_header_mismatches_sqlite(&db)),
            "PROBE-SHM-CLOSE" => match std::fs::File::open(sidecar(&db, "-shm")) {
                Ok(file) => {
                    drop(file);
                    "SHM-CLOSED".to_owned()
                }
                Err(err) => format!("SHM-FAILED {err}"),
            },
            "OPEN-VERIFIED" => open_verified(&db, path),
            "IMPORT" => sync_reply(
                session
                    .import_jsonl(path, ImportOptions { dry_run: false })
                    .await,
            ),
            "IMPORT-BD" => sync_reply(session.import_bd(path).await),
            "EXPORT" => sync_reply(session.export_jsonl(path).await),
            "QUIT" => break,
            other => format!("UNKNOWN {other}"),
        };
        reply(&answer);
    }
    reply("BYE");
    drop(session);
}
