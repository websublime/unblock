//! The fresh OBSERVER process of the PRD §4 D29 clause F6 lock-retention cells (`ub-kpe`), and the
//! parser of what it printed.
//!
//! The observer is the test binary itself, re-executed at its `kpe_observer_entry` test with
//! `UBKPE_OBSERVE_DB=<db>` set. It opens ONE raw libsql connection, sets `busy_timeout = 0`, runs
//! `PRAGMA locking_mode=EXCLUSIVE`, `BEGIN EXCLUSIVE` and `COMMIT`, and prints
//! `\nUBKPE-OBSERVED LOCKED`, `\nUBKPE-OBSERVED BUSY <code>` or `\nUBKPE-OBSERVED FAILED <error>`.
//! `BUSY` means some other process still guards the database; `LOCKED` means nobody does.
//!
//! libtest's pretty formatter prints `test kpe_observer_entry ... ` without a newline before the body
//! runs, so the token never starts a line: the spawner passes `-q`, the observer prints a leading
//! `\n`, and [`Observed::parse`] searches the token ANYWHERE in stdout.
//!
//! Shared with `unblock-cli`'s lock cells by `#[path]`, so this file uses no `super::`/`crate::` path
//! and spawns no process (a spawner here would sit outside the tree the cli's `spawn_lock_gate.rs`
//! scans).

use std::io::Write as _;
use std::time::Duration;

/// The env var that turns `kpe_observer_entry` into the observer.
pub const OBSERVE_ENV: &str = "UBKPE_OBSERVE_DB";

/// The token the observer prints before its verdict.
pub const OBSERVED_TOKEN: &str = "UBKPE-OBSERVED ";

/// The libtest arguments that run exactly `entry`, ignored or not, quietly, on one thread.
#[must_use]
pub fn entry_args(entry: &str) -> [&str; 6] {
    [
        entry,
        "--exact",
        "--include-ignored",
        "--nocapture",
        "--test-threads=1",
        "-q",
    ]
}

/// The observer body; a no-op unless [`OBSERVE_ENV`] is set.
///
/// # Panics
///
/// When the runtime cannot be built or the database cannot be opened.
pub fn observer_main() {
    let Some(db) = std::env::var_os(OBSERVE_ENV) else {
        return;
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("observer runtime");
    let verdict = runtime.block_on(async move {
        let database = libsql::Builder::new_local(std::path::PathBuf::from(db))
            .build()
            .await
            .expect("observer: open the database");
        let conn = database.connect().expect("observer: connect");
        conn.busy_timeout(Duration::ZERO)
            .expect("observer: busy_timeout");
        let result = async {
            let mut rows = conn.query("PRAGMA locking_mode=EXCLUSIVE", ()).await?;
            while rows.next().await?.is_some() {}
            drop(rows);
            conn.execute("BEGIN EXCLUSIVE", ()).await?;
            conn.execute("COMMIT", ()).await?;
            Ok::<(), libsql::Error>(())
        }
        .await;
        let verdict = match result {
            Ok(()) => "LOCKED".to_owned(),
            Err(libsql::Error::SqliteFailure(code, message)) => {
                if code & 0xff == 5 {
                    format!("BUSY {code}")
                } else {
                    format!("FAILED {code} {message}")
                }
            }
            Err(other) => format!("FAILED {other}"),
        };
        // One connection only (the ub-q1u double-close precedent): drop it before returning.
        drop(conn);
        drop(database);
        verdict
    });
    print!("\n{OBSERVED_TOKEN}{verdict}\n");
    std::io::stdout().flush().expect("flush the observation");
}

/// What one observer process printed.
#[derive(Debug, Clone)]
pub struct Observed {
    /// The verdict after the token (`LOCKED`, `BUSY <code>`, `FAILED …`), or the whole stdout when no
    /// token was found.
    pub line: String,
    /// The observer's stderr.
    pub stderr: String,
}

impl Observed {
    /// Parse the observer's output: the token is searched anywhere in stdout, the verdict runs to the
    /// next newline.
    #[must_use]
    pub fn parse(stdout: &str, stderr: &str) -> Self {
        let line = stdout.split_once(OBSERVED_TOKEN).map_or_else(
            || format!("<no {OBSERVED_TOKEN:?} token in {stdout:?}>"),
            |(_, rest)| {
                rest.split('\n')
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_owned()
            },
        );
        Self {
            line,
            stderr: stderr.to_owned(),
        }
    }

    /// `BUSY <code>` with `code & 0xff == SQLITE_BUSY`.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.line
            .strip_prefix("BUSY ")
            .and_then(|code| code.parse::<i32>().ok())
            .is_some_and(|code| code & 0xff == 5)
    }

    /// `LOCKED`: nobody guards the database.
    #[must_use]
    pub fn is_locked(&self) -> bool {
        self.line == "LOCKED"
    }
}
