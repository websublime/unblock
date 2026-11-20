//! PRD §4 D5 amendment and D58 amendment (`ub-c15`) test-only helper `[[bin]]`: ONE process holding a
//! real `Session` — and so `SQLite`'s SHARED lock and the `-shm` DMS read lock — while the parent test
//! observes it from outside (`F_GETLK`, `/proc/locks` on Linux, and a fresh `locking_mode=EXCLUSIVE`
//! observer).
//!
//! Usage: `session_lock_holder <unblock_dir> [--db <db_path>] [--mode sync|doctor|recover]`.
//! Startup, per mode (the engine cannot depend on the cli, so the doctor and recover orders mirror
//! `unblock-cli/src/commands/doctor.rs`; the cli end-to-end cells pin the shipped path):
//! - `sync` (default; the MCP server's order): `open_with_storage_with_cli` → `Session::open` → one
//!   read → `READY`;
//! - `doctor` (`health`): resolve-only `open_with_cli(no_db)` → `gather_preopen` → read-only
//!   `open_with_cli` → `Session::open` → one read → `READY`;
//! - `recover` (`health`): non-migrating `open_with_cli` → `Session::open` → one read → `READY`.
//!
//! Without the `health` feature the `doctor` and `recover` modes print `UNSUPPORTED health` and exit.
//!
//! One reply line per stdin line (stdout flushed per reply):
//! - `NOOP` → `OK`; `READ` → `READ <n>` / `READ-FAILED <err>`; `QUIT` / EOF → `BYE`, exit 0
//! - `PROBE-DB-CLOSE` → `File::open(<db>)` + drop (the close-drop hazard itself) → `DB-CLOSED`
//! - `PROBE-SHM-CLOSE` → `File::open(<db>-shm)` + drop → `SHM-CLOSED` (`sync`)
//! - `OPEN-VERIFIED <path>` → `SessionFiles::of_database(db).open_verified(path)` →
//!   `VERIFIED <none|some>` (`sync`)
//! - `IMPORT <path>` / `IMPORT-DRY <path>` / `IMPORT-BD <path>` / `EXPORT <path>` → `SYNC-OK` /
//!   `SYNC-REFUSED <code> <msg>` (`sync`)
//! - `DOCTOR` (`doctor` mode) → `DOCTORED <labels>` / `DOCTOR-FAILED <err>`; a second →
//!   `DOCTOR-SPENT` (`health`)
//! - `RECOVER` (`recover` mode) → `RECOVERED <label>=<detail>;…` / `RECOVER-FAILED <code> <msg>`
//!   (`health`)
//!
//! A `sync`-gated command without `sync` replies `UNSUPPORTED sync`; a `health`-gated one without
//! `health` replies `UNSUPPORTED health`. Every failure prints a reply instead of panicking
//! mid-protocol; the parent asserts on replies.

#![forbid(unsafe_code)]

use std::io::{BufRead as _, Write as _};
use std::path::{Path, PathBuf};

use unblock_config::CliOverrides;
#[cfg(feature = "health")]
use unblock_config::{OpenedContext, StorageOpenMode, open_with_cli};
use unblock_engine::{Session, SessionConfig};

fn reply(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

fn session_config() -> SessionConfig {
    SessionConfig {
        import_on_open: false,
        ..SessionConfig::default()
    }
}

/// The holder's startup mode.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Sync,
    Doctor,
    Recover,
}

/// The parsed argv.
struct Args {
    dir: PathBuf,
    db: Option<PathBuf>,
    mode: Mode,
}

fn parse_args() -> Option<Args> {
    let mut argv = std::env::args().skip(1);
    let dir = PathBuf::from(argv.next()?);
    let mut db = None;
    let mut mode = Mode::Sync;
    while let Some(flag) = argv.next() {
        let value = argv.next()?;
        match flag.as_str() {
            "--db" => db = Some(PathBuf::from(value)),
            "--mode" => {
                mode = match value.as_str() {
                    "sync" => Mode::Sync,
                    "doctor" => Mode::Doctor,
                    "recover" => Mode::Recover,
                    _ => return None,
                };
            }
            _ => return None,
        }
    }
    Some(Args { dir, db, mode })
}

/// What a `doctor`-mode startup carries into the `DOCTOR` command: the pre-open facts, consumed
/// once.
#[cfg(feature = "health")]
type Preopen = Option<unblock_engine::PreOpenFacts>;
#[cfg(not(feature = "health"))]
type Preopen = Option<std::convert::Infallible>;

/// What a successful startup opened: the session, the pre-open facts (`doctor` mode) and the
/// database path the session opened.
type Opened = (Session, Preopen, PathBuf);

/// Open the session in `mode`; `Err` carries the reply line.
async fn open(overrides: &CliOverrides, mode: Mode) -> Result<Opened, String> {
    match mode {
        Mode::Sync => {
            let ctx = unblock_config::open_with_storage_with_cli(overrides)
                .await
                .map_err(|err| format!("OPEN-FAILED {err}"))?;
            let db = ctx.paths.db_path.clone();
            let session = Session::open(ctx, session_config())
                .await
                .map_err(|err| format!("OPEN-FAILED {err}"))?;
            Ok((session, None, db))
        }
        Mode::Doctor | Mode::Recover => open_health(overrides, mode).await,
    }
}

#[cfg(feature = "health")]
async fn open_health(overrides: &CliOverrides, mode: Mode) -> Result<Opened, String> {
    let (storage_mode, facts) = if mode == Mode::Doctor {
        // The cli's doctor order: resolve only, gather the pre-open facts, then open read-only.
        let resolved = open_with_cli(&overrides.clone().with_no_db(true))
            .await
            .map_err(|err| format!("OPEN-FAILED {err}"))?;
        let paths = match resolved {
            OpenedContext::Resolved(ctx) => ctx.paths,
            OpenedContext::Storage(ctx) => ctx.paths.clone(),
        };
        let facts = unblock_engine::preopen::gather_preopen(&paths)
            .map_err(|err| format!("OPEN-FAILED {err}"))?;
        (StorageOpenMode::ReadOnly, Some(facts))
    } else {
        (StorageOpenMode::NoMigrate, None)
    };
    let cli = overrides
        .clone()
        .with_no_db(false)
        .with_storage_open(storage_mode);
    let ctx = match open_with_cli(&cli).await {
        Ok(OpenedContext::Storage(ctx)) => ctx,
        Ok(OpenedContext::Resolved(_)) => return Err("OPEN-FAILED resolve-only".to_owned()),
        Err(err) => return Err(format!("OPEN-FAILED {err}")),
    };
    let db = ctx.paths.db_path.clone();
    let session = Session::open(ctx, session_config())
        .await
        .map_err(|err| format!("OPEN-FAILED {err}"))?;
    Ok((session, facts, db))
}

#[cfg(not(feature = "health"))]
#[allow(clippy::unused_async)] // the `health` body awaits
async fn open_health(_overrides: &CliOverrides, _mode: Mode) -> Result<Opened, String> {
    Err("UNSUPPORTED health".to_owned())
}

#[cfg(feature = "sync")]
fn sync_reply<T>(result: unblock_engine::Result<T>) -> String {
    use unblock_error::CodedError as _;
    match result {
        Ok(_) => "SYNC-OK".to_owned(),
        Err(err) => format!("SYNC-REFUSED {} {err}", err.code().as_str()),
    }
}

/// The `sync`-gated commands; `None` when `command` is not one of them.
#[cfg(feature = "sync")]
async fn sync_command(session: &Session, db: &Path, command: &str, path: &Path) -> Option<String> {
    use unblock_engine::ImportOptions;
    Some(match command {
        "PROBE-SHM-CLOSE" => {
            match std::fs::File::open(unblock_sync::sidecar(db, unblock_sync::SHM_SUFFIX)) {
                Ok(file) => {
                    drop(file);
                    "SHM-CLOSED".to_owned()
                }
                Err(err) => format!("SHM-FAILED {err}"),
            }
        }
        "OPEN-VERIFIED" => match unblock_sync::SessionFiles::of_database(db).open_verified(path) {
            Ok(None) => "VERIFIED none".to_owned(),
            Ok(Some(_file)) => "VERIFIED some".to_owned(),
            Err(err) => format!("VERIFY-FAILED {err}"),
        },
        "IMPORT" => sync_reply(
            session
                .import_jsonl(path, ImportOptions { dry_run: false })
                .await,
        ),
        "IMPORT-DRY" => sync_reply(
            session
                .import_jsonl(path, ImportOptions { dry_run: true })
                .await,
        ),
        "IMPORT-BD" => sync_reply(session.import_bd(path).await),
        "EXPORT" => sync_reply(session.export_jsonl(path).await),
        _ => return None,
    })
}

#[cfg(not(feature = "sync"))]
#[allow(clippy::unused_async)] // the `sync` body awaits
async fn sync_command(
    _session: &Session,
    _db: &Path,
    command: &str,
    _path: &Path,
) -> Option<String> {
    matches!(
        command,
        "PROBE-SHM-CLOSE" | "OPEN-VERIFIED" | "IMPORT" | "IMPORT-DRY" | "IMPORT-BD" | "EXPORT"
    )
    .then(|| "UNSUPPORTED sync".to_owned())
}

/// The `health`-gated commands; `None` when `command` is not one of them.
#[cfg(feature = "health")]
async fn health_command(session: &Session, preopen: &mut Preopen, command: &str) -> Option<String> {
    use unblock_error::CodedError as _;
    Some(match command {
        "DOCTOR" => match preopen.take() {
            None => "DOCTOR-SPENT".to_owned(),
            Some(facts) => match session.doctor(&facts).await {
                Ok(health) => format!(
                    "DOCTORED {}",
                    health
                        .report
                        .findings
                        .iter()
                        .map(|f| f.label.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                ),
                Err(err) => format!("DOCTOR-FAILED {err}"),
            },
        },
        "RECOVER" => match session.recover().await {
            Ok(report) => format!(
                "RECOVERED {}",
                report
                    .findings
                    .iter()
                    .map(|f| format!("{}={}", f.label, f.detail))
                    .collect::<Vec<_>>()
                    .join(";")
            ),
            Err(err) => format!("RECOVER-FAILED {} {err}", err.code().as_str()),
        },
        _ => return None,
    })
}

#[cfg(not(feature = "health"))]
#[allow(clippy::unused_async)] // the `health` body awaits
async fn health_command(
    _session: &Session,
    _preopen: &mut Preopen,
    command: &str,
) -> Option<String> {
    matches!(command, "DOCTOR" | "RECOVER").then(|| "UNSUPPORTED health".to_owned())
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let Some(args) = parse_args() else {
        eprintln!(
            "usage: session_lock_holder <unblock_dir> [--db <db_path>] \
             [--mode sync|doctor|recover]"
        );
        std::process::exit(2);
    };
    let mut overrides = CliOverrides::new().with_dir(args.dir);
    if let Some(db) = args.db {
        overrides = overrides.with_db(db);
    }

    let (session, mut preopen, db) = match open(&overrides, args.mode).await {
        Ok(opened) => opened,
        Err(line) => return reply(&line),
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
            "READ" => match session.integrity_check().await {
                Ok(rows) => format!("READ {}", rows.len()),
                Err(err) => format!("READ-FAILED {err}"),
            },
            "PROBE-DB-CLOSE" => match std::fs::File::open(&db) {
                Ok(file) => {
                    drop(file);
                    "DB-CLOSED".to_owned()
                }
                Err(err) => format!("DB-FAILED {err}"),
            },
            "QUIT" => break,
            other => {
                if let Some(answer) = sync_command(&session, &db, other, path).await {
                    answer
                } else if let Some(answer) = health_command(&session, &mut preopen, other).await {
                    answer
                } else {
                    format!("UNKNOWN {other}")
                }
            }
        };
        reply(&answer);
    }
    reply("BYE");
    drop(session);
}
