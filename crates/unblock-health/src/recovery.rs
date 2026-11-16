//! The `.unblock/.recovery/` evidence layout (D58 clause (9)).
//!
//! One run directory per mutating phase, named `YYYYMMDDTHHMMSSZ-<run-id>`. This module names and
//! creates it, copies the export into it, writes the pre-gate sidecar captures under `original/`,
//! hashes the copies and writes `manifest.json` temp-file-then-rename. Every write stays inside the run directory. Directories are created 0700
//! and files 0600 on unix, because the evidence holds the whole database. It never opens the
//! database or a sidecar, never deletes anything and never writes a `.gitignore`. Its file I/O
//! blocks, so async callers run it on a blocking thread.

use std::fs::{DirBuilder, File, Metadata, OpenOptions};
use std::io::{self, ErrorKind, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use snafu::ResultExt;

use crate::error::{HealthError, RecoveryEvidenceWriteSnafu, SerializeSnafu};
use crate::facts::RecoveryEntry;
use crate::level::HealthLevel;

/// The manifest format tag.
pub const RECOVERY_FORMAT: &str = "unblock.recovery.v1";

/// How many run-directory names [`RunDir::create`] tries before it fails.
const MAX_ATTEMPTS: u32 = 16;

/// The manifest file name inside a run directory.
const MANIFEST_NAME: &str = "manifest.json";

/// The temp name the manifest is written under before its rename.
const MANIFEST_TEMP_NAME: &str = ".manifest.json.tmp";

/// The export copy's path inside a run directory.
const EXPORT_COPY: &str = "export/issues.jsonl";

/// The directory inside a run directory that holds the pre-gate sidecar captures.
const ORIGINAL_DIR: &str = "original";

/// Length of the `YYYYMMDDTHHMMSSZ` stamp.
const STAMP_LEN: usize = 16;

/// Length of the hex run id.
const RUN_ID_LEN: usize = 8;

/// Seconds in one day.
const SECONDS_PER_DAY: u64 = 86_400;

/// The run-directory name for a phase that starts at `now_utc` in process `pid`.
///
/// `YYYYMMDDTHHMMSSZ-` followed by the first 8 lowercase hex digits of the SHA-256 over the UTC
/// start in nanoseconds (16 bytes), `pid` (4 bytes) and `attempt` (4 bytes), each little-endian.
#[must_use]
pub fn run_dir_name(now_utc: SystemTime, pid: u32, attempt: u32) -> String {
    let since_epoch = since_epoch(now_utc);
    let mut hasher = Sha256::new();
    hasher.update(since_epoch.as_nanos().to_le_bytes());
    hasher.update(pid.to_le_bytes());
    hasher.update(attempt.to_le_bytes());
    let digest = hasher.finalize();
    let mut name = compact_stamp(since_epoch.as_secs());
    name.push('-');
    name.push_str(&hex(digest.get(..RUN_ID_LEN / 2).unwrap_or_default()));
    name
}

/// One created run directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunDir {
    path: PathBuf,
    name: String,
    started: SystemTime,
}

impl RunDir {
    /// Create a run directory under `recovery_root` for a phase starting at `now_utc`.
    ///
    /// Refuses a `recovery_root` that is a symlink or not a directory, so evidence never lands
    /// outside the workspace. Creates a missing root, the leaf and later `export/` with mode 0700
    /// on unix. A leaf name that already exists is retried with the next attempt, 16 attempts in
    /// all.
    ///
    /// # Errors
    ///
    /// [`HealthError::RecoveryEvidenceWrite`] when `recovery_root` is a symlink or not a
    /// directory, a directory cannot be created, or all 16 names exist.
    pub fn create(
        recovery_root: &Path,
        now_utc: SystemTime,
        pid: u32,
    ) -> Result<Self, HealthError> {
        ensure_recovery_root(recovery_root)?;
        for attempt in 0..MAX_ATTEMPTS {
            let name = run_dir_name(now_utc, pid, attempt);
            let path = recovery_root.join(&name);
            match private_dir_builder(false).create(&path) {
                Ok(()) => {
                    return Ok(Self {
                        path,
                        name,
                        started: now_utc,
                    });
                }
                Err(err) if err.kind() == ErrorKind::AlreadyExists => {}
                Err(source) => return Err(HealthError::RecoveryEvidenceWrite { source, path }),
            }
        }
        Err(HealthError::RecoveryEvidenceWrite {
            source: io::Error::new(
                ErrorKind::AlreadyExists,
                format!("all {MAX_ATTEMPTS} run directory names already exist"),
            ),
            path: recovery_root.to_path_buf(),
        })
    }

    /// The run directory's path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The run directory's name, `YYYYMMDDTHHMMSSZ-<run-id>`.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The 8-hex-digit run id.
    #[must_use]
    pub fn run_id(&self) -> &str {
        self.name.get(STAMP_LEN + 1..).unwrap_or_default()
    }

    /// The phase's UTC start in RFC 3339 form (`YYYY-MM-DDTHH:MM:SSZ`).
    #[must_use]
    pub fn started_utc(&self) -> String {
        rfc3339_stamp(since_epoch(self.started).as_secs())
    }

    /// The absolute path of `rel` inside the run directory.
    ///
    /// # Errors
    ///
    /// [`HealthError::RecoveryEvidenceWrite`] when `rel` is empty, absolute, or holds any
    /// component other than a plain name (`..` included).
    pub fn confined(&self, rel: &Path) -> Result<PathBuf, HealthError> {
        let plain = rel
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
        if rel.as_os_str().is_empty() || !plain {
            return Err(HealthError::RecoveryEvidenceWrite {
                source: io::Error::new(
                    ErrorKind::InvalidInput,
                    "the path escapes the recovery run directory",
                ),
                path: rel.to_path_buf(),
            });
        }
        Ok(self.path.join(rel))
    }

    /// Copy the export at `src` to `export/issues.jsonl` and fsync the copy.
    ///
    /// `src` must pass `unblock-sync`'s path preflight under `confine_root` (the `.unblock/`
    /// directory) and be a regular file, never a symlink, FIFO, directory or device. The opened
    /// handle must be that same regular file (same device and inode on unix), and at most
    /// [`unblock_sync::MAX_IMPORT_FILE_BYTES`] bytes are copied. The copy is created new with mode
    /// 0600 on unix, so existing evidence is never overwritten.
    ///
    /// # Errors
    ///
    /// [`HealthError::RecoveryEvidenceWrite`] when `src` fails the preflight, is not a regular
    /// file, changes between its check and its open, exceeds the size cap, cannot be read, or
    /// the copy cannot be written.
    pub fn copy_export(&self, src: &Path, confine_root: &Path) -> Result<PathBuf, HealthError> {
        let cap = unblock_sync::MAX_IMPORT_FILE_BYTES;
        let checked = unblock_sync::validate_sync_path(src, confine_root, false)
            .map_err(|err| refusal(src, ErrorKind::InvalidInput, err.to_string()))?;
        let before = std::fs::symlink_metadata(&checked)
            .context(RecoveryEvidenceWriteSnafu { path: src })?;
        if !before.file_type().is_file() {
            return Err(refusal(
                src,
                ErrorKind::InvalidInput,
                "the export is not a regular file".to_string(),
            ));
        }
        let input = File::open(&checked).context(RecoveryEvidenceWriteSnafu { path: src })?;
        let opened = input
            .metadata()
            .context(RecoveryEvidenceWriteSnafu { path: src })?;
        if !opened.is_file() || !same_file(&before, &opened) {
            return Err(refusal(
                src,
                ErrorKind::InvalidInput,
                "the export changed between its check and its open".to_string(),
            ));
        }
        if opened.len() > cap {
            return Err(too_large(src, cap));
        }
        let dst = self.confined(Path::new(EXPORT_COPY))?;
        if let Some(parent) = dst.parent() {
            private_dir_builder(false)
                .create(parent)
                .context(RecoveryEvidenceWriteSnafu { path: parent })?;
        }
        let mut output = private_file_options()
            .create_new(true)
            .open(&dst)
            .context(RecoveryEvidenceWriteSnafu { path: &dst })?;
        let copied = io::copy(&mut input.take(cap.saturating_add(1)), &mut output)
            .context(RecoveryEvidenceWriteSnafu { path: &dst })?;
        if copied > cap {
            return Err(too_large(src, cap));
        }
        output
            .sync_all()
            .context(RecoveryEvidenceWriteSnafu { path: &dst })?;
        Ok(dst)
    }

    /// Write a pre-gate capture to `original/<name>` and fsync it.
    ///
    /// `name` is one plain file name. The file is created new with mode 0600 and `original/` with
    /// mode 0700 on unix, so an earlier capture is never overwritten.
    ///
    /// # Errors
    ///
    /// [`HealthError::RecoveryEvidenceWrite`] when `name` is empty, `.`, `..` or holds a path
    /// separator, when `original/` is not a directory, or when the file cannot be written.
    pub fn write_original(&self, name: &str, bytes: &[u8]) -> Result<PathBuf, HealthError> {
        if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
            return Err(refusal(
                Path::new(name),
                ErrorKind::InvalidInput,
                "an original's name must be one plain file name".to_string(),
            ));
        }
        let dir = self.confined(Path::new(ORIGINAL_DIR))?;
        match private_dir_builder(false).create(&dir) {
            Ok(()) => {}
            Err(err) if err.kind() == ErrorKind::AlreadyExists => {
                let meta = std::fs::symlink_metadata(&dir)
                    .context(RecoveryEvidenceWriteSnafu { path: &dir })?;
                if meta.file_type().is_symlink() || !meta.is_dir() {
                    return Err(refusal(
                        &dir,
                        ErrorKind::InvalidInput,
                        "original/ is a symlink or not a directory".to_string(),
                    ));
                }
            }
            Err(source) => return Err(HealthError::RecoveryEvidenceWrite { source, path: dir }),
        }
        let dst = self.confined(&Path::new(ORIGINAL_DIR).join(name))?;
        let mut file = private_file_options()
            .create_new(true)
            .open(&dst)
            .context(RecoveryEvidenceWriteSnafu { path: &dst })?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .context(RecoveryEvidenceWriteSnafu { path: &dst })?;
        Ok(dst)
    }

    /// Write `manifest.json`: a temp file in the run directory, fsynced, then renamed over the
    /// manifest, so a reader sees the old manifest or the new one and never half of one.
    ///
    /// # Errors
    ///
    /// [`HealthError::Serialize`] when the manifest cannot be encoded;
    /// [`HealthError::RecoveryEvidenceWrite`] when the temp file cannot be written or renamed.
    pub fn write_manifest(&self, manifest: &Manifest) -> Result<(), HealthError> {
        let mut bytes = serde_json::to_vec_pretty(manifest).context(SerializeSnafu)?;
        bytes.push(b'\n');
        let temp = self.confined(Path::new(MANIFEST_TEMP_NAME))?;
        let target = self.confined(Path::new(MANIFEST_NAME))?;
        let mut file = private_file_options()
            .create(true)
            .truncate(true)
            .open(&temp)
            .context(RecoveryEvidenceWriteSnafu { path: &temp })?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .context(RecoveryEvidenceWriteSnafu { path: &temp })?;
        drop(file);
        std::fs::rename(&temp, &target).context(RecoveryEvidenceWriteSnafu { path: &target })?;
        sync_directory(&self.path)
    }
}

/// Refuse a recovery root that is a symlink or not a directory; create a missing one 0700.
///
/// The root is checked again after its creation, so a symlink planted in between is refused too.
fn ensure_recovery_root(root: &Path) -> Result<(), HealthError> {
    match std::fs::symlink_metadata(root) {
        Ok(meta) => check_recovery_root(root, &meta),
        Err(err) if err.kind() == ErrorKind::NotFound => {
            private_dir_builder(true)
                .create(root)
                .context(RecoveryEvidenceWriteSnafu { path: root })?;
            let meta = std::fs::symlink_metadata(root)
                .context(RecoveryEvidenceWriteSnafu { path: root })?;
            check_recovery_root(root, &meta)
        }
        Err(source) => Err(HealthError::RecoveryEvidenceWrite {
            source,
            path: root.to_path_buf(),
        }),
    }
}

/// The recovery root's own metadata must be a real directory, never a symlink.
fn check_recovery_root(root: &Path, meta: &Metadata) -> Result<(), HealthError> {
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(refusal(
            root,
            ErrorKind::InvalidInput,
            "the recovery root is a symlink or not a directory".to_string(),
        ));
    }
    Ok(())
}

/// A [`HealthError::RecoveryEvidenceWrite`] refusal at `path`.
fn refusal(path: &Path, kind: ErrorKind, message: String) -> HealthError {
    HealthError::RecoveryEvidenceWrite {
        source: io::Error::new(kind, message),
        path: path.to_path_buf(),
    }
}

/// The refusal of an export larger than `cap` bytes.
fn too_large(path: &Path, cap: u64) -> HealthError {
    refusal(
        path,
        ErrorKind::FileTooLarge,
        format!("the export exceeds the {cap}-byte import cap"),
    )
}

/// A directory builder that creates with mode 0700 on unix.
fn private_dir_builder(recursive: bool) -> DirBuilder {
    let mut builder = DirBuilder::new();
    builder.recursive(recursive);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
}

/// Write options that create with mode 0600 on unix.
fn private_file_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

/// Whether two metadata reads name the same file (device and inode).
#[cfg(unix)]
fn same_file(a: &Metadata, b: &Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}

/// Other targets have no stable file identity in std, so the size and modification time stand in,
/// as in the engine capture.
#[cfg(not(unix))]
fn same_file(a: &Metadata, b: &Metadata) -> bool {
    a.len() == b.len() && a.modified().ok() == b.modified().ok()
}

/// Fsync a directory so a rename inside it survives a crash.
#[cfg(unix)]
fn sync_directory(dir: &Path) -> Result<(), HealthError> {
    File::open(dir)
        .and_then(|handle| handle.sync_all())
        .context(RecoveryEvidenceWriteSnafu { path: dir })
}

/// Other targets have no portable directory fsync; the rename itself is still atomic.
#[cfg(not(unix))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the signature matches the unix body"
)]
fn sync_directory(_dir: &Path) -> Result<(), HealthError> {
    Ok(())
}

/// The kind of mutating phase a run directory records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// The pre-open exclusive window.
    Preopen,
    /// One `Session::recover()` call.
    Online,
}

/// What happened to one action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The action ran.
    Applied,
    /// The action was refused before it ran.
    Refused,
    /// The action ran and failed.
    Failed,
}

/// `manifest.json` (D58 clause (9)).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Always [`RECOVERY_FORMAT`].
    pub format: String,
    /// The run id (the 8 hex digits of the directory name).
    pub run_id: String,
    /// The phase's UTC start, RFC 3339.
    pub started_utc: String,
    /// The phase kind.
    pub phase: Phase,
    /// The unblock binary version.
    pub binary_version: String,
    /// The schema stamp before the phase, when the phase read one.
    pub schema_version: Option<i64>,
    /// The level classified before the phase.
    pub level_before: HealthLevel,
    /// Every evidence file, hashed over its copy in the run directory.
    pub files: Vec<ManifestFile>,
    /// The sidecars captured before the gate, kept apart from the window's copies in `files`.
    pub originals: Vec<ManifestOriginal>,
    /// Every action with its outcome.
    pub actions: Vec<ManifestAction>,
}

impl Manifest {
    /// A manifest for `run_dir` with no files and no actions yet.
    #[must_use]
    pub fn new(
        run_dir: &RunDir,
        phase: Phase,
        binary_version: &str,
        schema_version: Option<i64>,
        level_before: HealthLevel,
    ) -> Self {
        Self {
            format: RECOVERY_FORMAT.to_string(),
            run_id: run_dir.run_id().to_string(),
            started_utc: run_dir.started_utc(),
            phase,
            binary_version: binary_version.to_string(),
            schema_version,
            level_before,
            files: Vec::new(),
            originals: Vec::new(),
            actions: Vec::new(),
        }
    }
}

/// One evidence file in the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestFile {
    /// The path relative to the run directory, `/`-separated.
    pub path: String,
    /// The copy's size in bytes.
    pub bytes: u64,
    /// The lowercase hex SHA-256 of the copy.
    pub sha256: String,
}

/// One pre-gate sidecar capture in the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestOriginal {
    /// The path relative to the run directory, `original/<name>`.
    pub path: String,
    /// The capture's size in bytes.
    pub bytes: u64,
    /// The lowercase hex SHA-256 of the capture.
    pub sha256: String,
    /// What happened to the live file after its capture, when worth saying.
    pub note: Option<String>,
}

/// One action in the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestAction {
    /// The action code.
    pub action: String,
    /// The object the action targets, when it names one.
    pub target: Option<String>,
    /// What happened.
    pub outcome: Outcome,
    /// Why it was refused or failed.
    pub reason: Option<String>,
}

/// The size and lowercase hex SHA-256 of the file at `path`.
///
/// Blocking: async callers run it on a blocking thread.
///
/// # Errors
///
/// [`HealthError::RecoveryEvidenceWrite`] when the file cannot be read.
pub fn sha256_file(path: &Path) -> Result<(u64, String), HealthError> {
    let mut file = File::open(path).context(RecoveryEvidenceWriteSnafu { path })?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut bytes = 0u64;
    loop {
        let read = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            Err(source) => {
                return Err(HealthError::RecoveryEvidenceWrite {
                    source,
                    path: path.to_path_buf(),
                });
            }
        };
        hasher.update(buffer.get(..read).unwrap_or_default());
        bytes = bytes.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
    }
    Ok((bytes, hex(&hasher.finalize())))
}

/// The lowercase hex SHA-256 of `bytes`, the same spelling [`sha256_file`] and the manifest use.
#[must_use]
pub fn sha256_bytes(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// The manifest entry for the copy at `file` inside `run_dir`.
///
/// `file` is absolute under `run_dir` or relative to it. The hash is taken over that copy, never
/// over a live workspace file.
///
/// # Errors
///
/// [`HealthError::RecoveryEvidenceWrite`] when `file` lies outside `run_dir` or cannot be read.
pub fn manifest_file(run_dir: &Path, file: &Path) -> Result<ManifestFile, HealthError> {
    let rel = if file.is_absolute() {
        file.strip_prefix(run_dir).unwrap_or(file)
    } else {
        file
    };
    let mut parts = Vec::new();
    for component in rel.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            _ => {
                return Err(HealthError::RecoveryEvidenceWrite {
                    source: io::Error::new(
                        ErrorKind::InvalidInput,
                        "the evidence file lies outside the recovery run directory",
                    ),
                    path: file.to_path_buf(),
                });
            }
        }
    }
    if parts.is_empty() {
        return Err(HealthError::RecoveryEvidenceWrite {
            source: io::Error::new(ErrorKind::InvalidInput, "no evidence file named"),
            path: file.to_path_buf(),
        });
    }
    let (bytes, sha256) = sha256_file(&run_dir.join(rel))?;
    Ok(ManifestFile {
        path: parts.join("/"),
        bytes,
        sha256,
    })
}

/// The run directories under `recovery_root`, sorted by name, each with the UTC instant its name
/// encodes.
///
/// An absent or unreadable root yields no entries. Only directories count. Never writes.
#[must_use]
pub fn scan_recovery_root(recovery_root: &Path) -> Vec<RecoveryEntry> {
    let Ok(read_dir) = std::fs::read_dir(recovery_root) else {
        return Vec::new();
    };
    let mut entries: Vec<RecoveryEntry> = read_dir
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let stamp_utc = parse_run_dir_name(&name);
            RecoveryEntry { name, stamp_utc }
        })
        .collect();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

/// The UTC instant a `YYYYMMDDTHHMMSSZ-<8 lowercase hex>` name encodes.
fn parse_run_dir_name(name: &str) -> Option<SystemTime> {
    let bytes = name.as_bytes();
    if bytes.len() != STAMP_LEN + 1 + RUN_ID_LEN
        || bytes.get(8) != Some(&b'T')
        || bytes.get(15) != Some(&b'Z')
        || bytes.get(16) != Some(&b'-')
    {
        return None;
    }
    let run_id = bytes.get(STAMP_LEN + 1..)?;
    if !run_id
        .iter()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return None;
    }
    let number = |range: std::ops::Range<usize>| -> Option<u64> {
        let digits = bytes.get(range)?;
        digits.iter().try_fold(0u64, |acc, byte| {
            byte.is_ascii_digit()
                .then(|| acc * 10 + u64::from(byte - b'0'))
        })
    };
    let year = i64::try_from(number(0..4)?).ok()?;
    let month = number(4..6)?;
    let day = number(6..8)?;
    let hour = number(9..11)?;
    let minute = number(11..13)?;
    let second = number(13..15)?;
    if !(1..=12).contains(&month)
        || day == 0
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let days = u64::try_from(days_from_civil(year, month, day)).ok()?;
    let secs = days * SECONDS_PER_DAY + hour * 3600 + minute * 60 + second;
    UNIX_EPOCH.checked_add(Duration::from_secs(secs))
}

/// The time since the Unix epoch; an instant before it counts as the epoch.
fn since_epoch(time: SystemTime) -> Duration {
    time.duration_since(UNIX_EPOCH).unwrap_or_default()
}

/// `YYYYMMDDTHHMMSSZ` for `secs` since the epoch.
fn compact_stamp(secs: u64) -> String {
    let (year, month, day, hour, minute, second) = civil_time(secs);
    format!("{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}Z")
}

/// `YYYY-MM-DDTHH:MM:SSZ` for `secs` since the epoch.
fn rfc3339_stamp(secs: u64) -> String {
    let (year, month, day, hour, minute, second) = civil_time(secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// The UTC civil date and time of `secs` since the epoch.
fn civil_time(secs: u64) -> (i64, u64, u64, u64, u64, u64) {
    let days = i64::try_from(secs / SECONDS_PER_DAY).unwrap_or(i64::MAX);
    let rem = secs % SECONDS_PER_DAY;
    let (year, month, day) = civil_from_days(days);
    (year, month, day, rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Days since 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u64, u64) {
    let z = days.saturating_add(719_468);
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (
        year,
        u64::try_from(month).unwrap_or_default(),
        u64::try_from(day).unwrap_or_default(),
    )
}

/// A proleptic Gregorian date to days since 1970-01-01 (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: u64, day: u64) -> i64 {
    let month = i64::try_from(month).unwrap_or_default();
    let day = i64::try_from(day).unwrap_or_default();
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400);
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The number of days in `month` of `year`.
fn days_in_month(year: i64, month: u64) -> u64 {
    match month {
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Lowercase hex of `bytes`.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

#[cfg(test)]
mod tests {
    use super::{civil_from_days, compact_stamp, days_from_civil, parse_run_dir_name};
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn civil_conversion_round_trips_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(days_from_civil(2000, 2, 29), 11_016);
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(compact_stamp(1_794_573_296), "20261113T123456Z");
    }

    #[test]
    fn parse_rejects_impossible_dates() {
        assert!(parse_run_dir_name("20260230T000000Z-0123abcd").is_none());
        assert!(parse_run_dir_name("20261113T240000Z-0123abcd").is_none());
        assert!(parse_run_dir_name("20261113T000000Z-0123ABCD").is_none());
        assert_eq!(
            parse_run_dir_name("19700101T000001Z-0123abcd"),
            Some(UNIX_EPOCH + Duration::from_secs(1))
        );
    }
}
