//! Path-confinement preflight (NFR-7/8) — a faithful, shrunk port of
//! `temp/beads_rust-main/src/sync/path.rs`.
//!
//! Every sync I/O operation MUST pass through [`validate_sync_path`] before touching a file. The
//! reject ORDER is a layered defence (each layer a distinct [`PathReject`]):
//!
//! 1. `.git` component — **ALWAYS** rejected, even under `allow_external` (raw path AND every
//!    canonicalized existing ancestor).
//!
//!    (1a) lexical name check (`ub-b9z`) — **ALWAYS** rejected, even under `allow_external`. It
//!    runs between the two halves of step 1 (after the raw-path `.git` test, before any existing
//!    ancestor is canonicalized), so no filesystem probe precedes it: a leaf containing `:` on
//!    every platform (an NTFS alternate data stream on Windows, a non-portable name elsewhere)
//!    and, on Windows, every normal component containing `:` (prefix components never checked)
//!    → [`PathReject::ColonInName`]; on every platform, a leaf that is a Windows reserved device
//!    name → [`PathReject::ReservedDeviceName`].
//! 2. lexical `..` normalization (pop on `ParentDir`; a pop past the root escapes → reject).
//! 3. symlink-escape of an existing ancestor outside `confine_root`.
//! 4. containment under `confine_root` — the **only** check `allow_external` relaxes.
//! 5. (non-existent target) canonicalize the parent, confirm confined, validate ext/name.
//! 6. (existing target) reject a non-regular file / an escaping symlink, then ext/name.
//! 7. extension/exact-name allowlist (`*.jsonl`, `issues.jsonl`, `*.jsonl.<pid>.tmp`).
//!
//! **Invariant (NFR-8):** `allow_external = true` relaxes ONLY step 4 — steps 1/1a/2/3 + the
//! regular-file check are NEVER relaxed. (The ancestor-symlink check for external paths is a
//! deliberate TIGHTENING over the original, which checked the leaf only there, SH-1.)

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use crate::error::SyncError;

/// The extensions a sync path may carry (D5-shrunk: only `jsonl` + the compound temp form).
pub const ALLOWED_EXTENSIONS: &[&str] = &["jsonl", "jsonl.tmp"];

/// The exact file names a sync path may carry.
pub const ALLOWED_EXACT_NAMES: &[&str] = &["issues.jsonl"];

/// Which defence layer rejected a sync path (carried by [`SyncError::PathTraversal`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathReject {
    /// The path targets a `.git` component (raw or via a canonicalized ancestor).
    GitComponent,
    /// A lexical `..` escaped above the root.
    ParentEscape,
    /// An existing ancestor is a symlink pointing outside `confine_root`.
    SymlinkEscape,
    /// The file name has no extension and is not an allowed exact name.
    DisallowedName,
    /// The file extension is not in the allowlist.
    DisallowedExtension,
    /// The path resolves outside `confine_root` (and `allow_external` was not set).
    OutsideConfineRoot,
    /// The existing target is not a regular file.
    NonRegularFile,
    /// A temp-file allocation collided (`create_new` `AlreadyExists`).
    TempCollision,
    /// PRD §4 D5 amendment (v1.1, `ub-c15`): the target is the session's own database or one of its
    /// `-wal`/`-shm`/`-journal` sidecars (same `(dev, ino)` on unix, under any name): importing it
    /// would open and close a descriptor on a file the process holds `SQLite` locks on; exporting
    /// would rename over it.
    SessionDatabase,
    /// NFR-7 (v1.1, `ub-b9z`): a name in the path contains `:`. On NTFS `name:stream` addresses an
    /// alternate data stream of `name` (`unblock.db:x.jsonl` would write into a stream of the live
    /// database). Refused in the leaf on every platform, so a `.unblock/` tree stays usable on
    /// Windows, and on Windows in every `Component::Normal` of the path (prefix components such as
    /// `C:` or `\\?\C:` are never checked). Lexical, checked right after the raw-path `.git` test
    /// and before any filesystem probe; never relaxed by `allow_external`.
    ColonInName,
    /// NFR-7 (v1.1, `ub-b9z`): the leaf is a Windows reserved device name: its stem (the text
    /// before the first `.`, trailing spaces trimmed, any ASCII case) is `CON`, `PRN`, `AUX`,
    /// `NUL`, `CONIN$`, `CONOUT$`, or `COM`/`LPT` plus a digit `0`–`9` or a superscript
    /// `¹`/`²`/`³` (`NUL.jsonl`, `nul.x.jsonl`, `NUL .jsonl`, a bare `NUL`). Win32 opens such a
    /// name as a device on Windows 10 and earlier; Windows 11 still opens a bare `NUL` as one, and
    /// other forms become files that some tools mishandle. Refused on every platform for
    /// portability. Lexical, checked with `ColonInName`; never relaxed by `allow_external`.
    ReservedDeviceName,
}

/// The `SQLite` write-ahead-log sidecar suffix.
pub const WAL_SUFFIX: &str = "-wal";
/// The `SQLite` shared-memory sidecar suffix.
pub const SHM_SUFFIX: &str = "-shm";
/// The `SQLite` rollback-journal sidecar suffix.
pub const JOURNAL_SUFFIX: &str = "-journal";
/// The three `SQLite` sidecar suffixes, in the cli's report order (`-wal`, `-shm`, `-journal`).
///
/// PRD §4 D5 amendment (`ub-c15`): `unblock-sync` owns these names above L2; `unblock-health` and
/// the engine root re-export them (`unblock-storage`, at L2, keeps its own private literals, NFR-15).
pub const SIDECAR_SUFFIXES: [&str; 3] = [WAL_SUFFIX, SHM_SUFFIX, JOURNAL_SUFFIX];

/// Derive a `SQLite` sidecar path by **appending** `suffix` to the db path's raw bytes (`{db}{suffix}`),
/// matching `SQLite`'s own `-wal`/`-shm`/`-journal` naming — NOT `Path::with_extension`, which would
/// corrupt a db path that has no extension or a multi-dot name.
///
/// Uses [`OsString`](std::ffi::OsString) concatenation rather than string formatting so a non-UTF-8
/// db path is preserved byte-for-byte; the result is identical for every valid UTF-8 path.
#[must_use]
pub fn sidecar(db: &Path, suffix: &str) -> PathBuf {
    let mut raw = db.as_os_str().to_os_string();
    raw.push(suffix);
    PathBuf::from(raw)
}

/// The most `(dev, ino)` pairs a [`SessionFiles`] holds: two bases (the db path as given and its
/// canonical form), each the database plus its three sidecars.
#[cfg(unix)]
const SESSION_FILES_CAP: usize = 2 * (1 + SIDECAR_SUFFIXES.len());

/// The files the live session holds open — the database and its `-wal`, `-shm`, `-journal` sidecars
/// (PRD §4 D5 amendment, v1.1, `ub-c15`; NFR-7). The workspace's ONE session-file type: every sync
/// target and `unblock-health`'s export scan and evidence copy compare against it.
///
/// - unix: up to 8 `(dev, ino)` pairs in a fixed array (the set holds no heap), taken by
///   `std::fs::metadata` (a stat, no descriptor; a missing file is skipped).
/// - The sidecar names are derived from the db path as given AND from its canonical
///   (symlink-resolved) form: `SQLite` resolves a symlinked database and keeps its
///   `-wal`/`-shm`/`-journal` next to the TARGET, so both locations are covered (`canonicalize` is a
///   path walk, it opens no descriptor).
/// - non-unix: each path canonicalized (the raw path when it does not exist), compared by equality
///   with the canonicalized target. A hard link is not caught there; that is harmless: Windows locks
///   belong to their handle, so no close-drop exists there.
#[derive(Debug, Clone)]
pub struct SessionFiles {
    #[cfg(unix)]
    ids: [(u64, u64); SESSION_FILES_CAP],
    #[cfg(unix)]
    len: u8,
    #[cfg(not(unix))]
    paths: Vec<PathBuf>,
}

impl SessionFiles {
    /// Stat `db` and its sidecars NOW, at `db` as given and at `dunce::canonicalize(db)` when that
    /// differs (the engine builds one per call, so a sidecar created since the session opened is
    /// included). Building allocates only the canonical `PathBuf` and one sidecar-name buffer per
    /// base, reused across the three suffixes.
    #[must_use]
    pub fn of_database(db: &Path) -> Self {
        let mut files = Self::none();
        files.add_base(db);
        if let Ok(real) = dunce::canonicalize(db)
            && real != db
        {
            files.add_base(&real);
        }
        files
    }

    /// No session files: matches nothing, and allocates nothing (tests; callers outside a session).
    #[must_use]
    pub fn none() -> Self {
        Self {
            #[cfg(unix)]
            ids: [(0, 0); SESSION_FILES_CAP],
            #[cfg(unix)]
            len: 0,
            #[cfg(not(unix))]
            paths: Vec::new(),
        }
    }

    /// Add `base` and its three sidecars, reusing one name buffer: push the suffix, stat, truncate
    /// back.
    #[cfg(unix)]
    fn add_base(&mut self, base: &Path) {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt as _;

        self.add(base);
        let base_len = base.as_os_str().len();
        let mut name = OsString::with_capacity(base_len + JOURNAL_SUFFIX.len());
        name.push(base.as_os_str());
        for suffix in SIDECAR_SUFFIXES {
            name.push(suffix);
            self.add(Path::new(&name));
            let mut bytes = name.into_vec();
            bytes.truncate(base_len);
            name = OsString::from_vec(bytes);
        }
    }

    /// Add `base` and its three sidecars (each stored as its canonical path).
    #[cfg(not(unix))]
    fn add_base(&mut self, base: &Path) {
        self.add(base);
        for suffix in SIDECAR_SUFFIXES {
            self.add(&sidecar(base, suffix));
        }
    }

    /// Record `p`'s `(dev, ino)` when it exists and is not yet in the set.
    #[cfg(unix)]
    fn add(&mut self, p: &Path) {
        let len = usize::from(self.len);
        if let Some(id) = file_id(p)
            && len < SESSION_FILES_CAP
            && !self.ids[..len].contains(&id)
        {
            self.ids[len] = id;
            self.len += 1;
        }
    }

    /// Record `p`'s canonical form when it is not yet in the set.
    #[cfg(not(unix))]
    fn add(&mut self, p: &Path) {
        let path = canonical_or_raw(p);
        if !self.paths.contains(&path) {
            self.paths.push(path);
        }
    }

    /// The recorded identities (unix).
    #[cfg(unix)]
    fn ids(&self) -> &[(u64, u64)] {
        &self.ids[..usize::from(self.len)]
    }

    /// Stat `p` (no descriptor): is it one of the session's files?
    #[must_use]
    pub fn contains(&self, p: &Path) -> bool {
        #[cfg(unix)]
        {
            let ids = self.ids();
            !ids.is_empty() && file_id(p).is_some_and(|id| ids.contains(&id))
        }
        #[cfg(not(unix))]
        {
            !self.paths.is_empty() && self.paths.contains(&canonical_or_raw(p))
        }
    }

    /// Refuse `path` when it stats as a session file (no descriptor is opened).
    ///
    /// # Errors
    ///
    /// [`SyncError::PathTraversal`] with [`PathReject::SessionDatabase`] when `path` is a session file.
    pub fn reject(&self, path: &Path) -> Result<(), SyncError> {
        if self.contains(path) {
            return Err(SyncError::PathTraversal {
                path: path.to_path_buf(),
                reason: PathReject::SessionDatabase,
            });
        }
        Ok(())
    }

    /// The open + check half: `File::open(p)`, then `fstat` the OPENED descriptor. If its
    /// `(dev, ino)` is a session file — `p` was renamed or linked onto one after the caller's stat —
    /// the descriptor is deliberately kept open with [`std::mem::forget`] and `Ok(None)` is returned;
    /// else `Ok(Some(file))`.
    ///
    /// Never closing it is REQUIRED, not merely sufficient: the session's locks live until the session
    /// closes (for `unblock mcp`, effectively process exit), and a close at any earlier point would
    /// release them (`sqlite.org` `howtocorrupt.html` §2.2). `SQLite` likewise never closes such a
    /// descriptor while it holds locks on the inode (`unixUnusedFd`). Bound: at most one descriptor per
    /// racing call, because every caller returns right after `Ok(None)`; for the long-lived
    /// `unblock mcp` server that is one per raced call for the server's lifetime, reachable only by a
    /// concurrent local writer to `.unblock/`, which could delete the database anyway. Off unix this is
    /// a plain open (no close-drop exists there). With an empty set ([`SessionFiles::none`]) it is a
    /// plain open too. Residual: a FIFO swapped in between the caller's stat guard and this blocking
    /// open blocks the open; adversarial only.
    ///
    /// # Errors
    ///
    /// The `File::open` error, or the `fstat` error of the opened descriptor — in which case the
    /// descriptor is KEPT open (its identity is unknown, so it is treated like a match).
    #[cfg_attr(not(unix), allow(clippy::unused_self))]
    pub fn open_verified(&self, p: &Path) -> std::io::Result<Option<std::fs::File>> {
        let file = std::fs::File::open(p)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            let ids = self.ids();
            if ids.is_empty() {
                return Ok(Some(file));
            }
            let meta = match file.metadata() {
                Ok(meta) => meta,
                Err(err) => {
                    // The descriptor's identity is unknown: it may be a session file, so it is kept
                    // open rather than closed (the same rule as a match).
                    std::mem::forget(file);
                    return Err(err);
                }
            };
            if ids.contains(&(meta.dev(), meta.ino())) {
                std::mem::forget(file);
                return Ok(None);
            }
        }
        Ok(Some(file))
    }

    /// For an import reader: replaces ONLY the `File::open(path)` call after the reader's existing
    /// stat guards (regular file, size cap), so a FIFO is still never opened. An open error maps to
    /// the same [`SyncError::Io`] (`"opening"`) as before; a session file to
    /// [`PathReject::SessionDatabase`], its descriptor kept open.
    ///
    /// # Errors
    ///
    /// As described.
    pub(crate) fn open_import_source(&self, path: &Path) -> Result<std::fs::File, SyncError> {
        match self.open_verified(path) {
            Ok(Some(file)) => Ok(file),
            Ok(None) => Err(SyncError::PathTraversal {
                path: path.to_path_buf(),
                reason: PathReject::SessionDatabase,
            }),
            Err(source) => Err(SyncError::Io {
                path: path.to_path_buf(),
                action: "opening",
                source,
            }),
        }
    }
}

/// `(dev, ino)` of `p` by `stat` (follows symlinks; no descriptor); `None` when `p` cannot be stated.
#[cfg(unix)]
fn file_id(p: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt as _;
    std::fs::metadata(p).ok().map(|m| (m.dev(), m.ino()))
}

/// The canonical form of `p`, or `p` itself when it cannot be canonicalized (it does not exist).
#[cfg(not(unix))]
fn canonical_or_raw(p: &Path) -> PathBuf {
    dunce::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Reject an `allow_external` override that carries NO written reason (NFR-5/D30 forward seam).
///
/// The reason gates `allow_external` at the sync boundary: writing/reading outside `confine_root`
/// requires an operator-written reason (which then rides the NFR-13 force-override INFO at the
/// honored-`allow_external` site). A no-op when `allow_external` is `false`. Each orchestrator
/// (`export_jsonl` / `import_jsonl` / `import_bd`) calls this at the TOP of its body. Unreachable in
/// v1 — the engine forces `allow_external: false` on every write path.
///
/// # Errors
///
/// [`SyncError::ExternalOverrideWithoutReason`] (→ `ErrorCode::PathTraversal`, exit-6) when
/// `allow_external` is set but `external_reason` is `None`.
pub(crate) fn reject_external_without_reason(
    path: &Path,
    allow_external: bool,
    external_reason: Option<&str>,
) -> Result<(), SyncError> {
    if allow_external && external_reason.is_none() {
        return Err(SyncError::ExternalOverrideWithoutReason {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

/// Validate `path` for a sync read/write, returning the canonicalized confined path.
///
/// `confine_root` is the absolute `.unblock/` dir (canonicalized once here). `allow_external`
/// relaxes ONLY the containment check (step 4) — never `.git`/name/`..`/symlink-escape.
///
/// # Errors
///
/// [`SyncError::PathTraversal`] carrying the precise [`PathReject`] layer that fired.
pub fn validate_sync_path(
    path: &Path,
    confine_root: &Path,
    allow_external: bool,
) -> Result<PathBuf, SyncError> {
    // (1) `.git` — checked FIRST, never relaxed: the raw path here, its existing ancestors below.
    reject_git_component(path)?;

    // (1a) NFR-7 (`ub-b9z`): an NTFS stream (`:`) or a Windows device name. Lexical, on the RAW
    // path (the `..` normalization below rebuilds it and can turn `d:s.jsonl` into a drive
    // prefix), never relaxed, and before step 1 canonicalizes any ancestor: no probe touches a
    // refused name.
    if let Some(reason) = unportable_name_reject(path) {
        return Err(SyncError::PathTraversal {
            path: path.to_path_buf(),
            reason,
        });
    }

    // (1) continued: an existing ancestor that resolves into `.git` (a symlinked parent).
    reject_git_ancestors(path)?;

    // (2) lexical `..` normalization. A `..` popping past the filesystem root is an outright escape.
    let had_parent_dir = path.components().any(|c| matches!(c, Component::ParentDir));
    let normalized = normalize_lexically(path).ok_or_else(|| SyncError::PathTraversal {
        path: path.to_path_buf(),
        reason: PathReject::ParentEscape,
    })?;

    // Canonicalize the confine root once (best-effort: if it does not yet exist, keep it as-is so a
    // fresh workspace's `.unblock/` still validates a to-be-created `issues.jsonl`).
    let canonical_root =
        dunce::canonicalize(confine_root).unwrap_or_else(|_| confine_root.to_path_buf());

    // A `..` that, after lexical normalization, leaves the confine root is a traversal attempt
    // (faithful port of the original `had_parent_dir` guard) — reported as `ParentEscape`, and NOT
    // relaxed by `allow_external` (steps 1/1a/2/3 are never relaxed, NFR-8).
    if had_parent_dir
        && !normalized.starts_with(&canonical_root)
        && !normalized.starts_with(confine_root)
    {
        return Err(SyncError::PathTraversal {
            path: path.to_path_buf(),
            reason: PathReject::ParentEscape,
        });
    }

    // (3) symlink-escape of an existing ancestor (runs regardless of `allow_external`).
    if let Some(escaped) = symlink_escape_ancestor(&normalized, &canonical_root) {
        return Err(SyncError::PathTraversal {
            path: escaped,
            reason: PathReject::SymlinkEscape,
        });
    }

    // (4) containment — the ONLY check `allow_external` relaxes.
    let effective = effective_canonical(&normalized, &canonical_root)?;
    let contained = effective.starts_with(&canonical_root) || normalized.starts_with(confine_root);
    if !contained && !allow_external {
        return Err(SyncError::PathTraversal {
            path: path.to_path_buf(),
            reason: PathReject::OutsideConfineRoot,
        });
    }

    // (6) existing target: reject a non-regular file (never relaxed).
    if normalized.exists() {
        match std::fs::symlink_metadata(&normalized) {
            Ok(meta) if !meta.is_file() => {
                return Err(SyncError::PathTraversal {
                    path: path.to_path_buf(),
                    reason: PathReject::NonRegularFile,
                });
            }
            Ok(_) => {}
            Err(source) => {
                return Err(SyncError::Io {
                    path: normalized.clone(),
                    action: "reading metadata for",
                    source,
                });
            }
        }
    }

    // (7) extension / exact-name allowlist.
    validate_extension_and_name(&normalized)?;

    Ok(normalized)
}

/// Validate a temp file path just before its rename over `final_path`.
///
/// The temp must be a valid sync path (allowlist covers `*.jsonl.<pid>.tmp` / `*.jsonl.tmp`) and
/// sit in the same directory as `final_path` (both under `confine_root`).
///
/// # Errors
///
/// [`SyncError::PathTraversal`] if the temp path fails validation or is not a sibling of `final_path`.
pub fn validate_temp_path(
    temp: &Path,
    final_path: &Path,
    confine_root: &Path,
) -> Result<(), SyncError> {
    validate_sync_path(temp, confine_root, false)?;
    // A temp must be a sibling of the final path (same parent dir) — never cross-directory.
    if temp.parent() != final_path.parent() {
        return Err(SyncError::PathTraversal {
            path: temp.to_path_buf(),
            reason: PathReject::OutsideConfineRoot,
        });
    }
    Ok(())
}

/// Step 1, first half: reject a raw path with a `.git` component — the hard NGI-3 invariant (never
/// relaxed). Lexical: no filesystem access, so it runs before step (1a).
fn reject_git_component(path: &Path) -> Result<(), SyncError> {
    if has_git_component(path) {
        return Err(SyncError::PathTraversal {
            path: path.to_path_buf(),
            reason: PathReject::GitComponent,
        });
    }
    Ok(())
}

/// Step 1, second half, after step (1a): resolve each existing ancestor, because a higher
/// symlinked ancestor can still target `.git` (never relaxed).
fn reject_git_ancestors(path: &Path) -> Result<(), SyncError> {
    for ancestor in path.ancestors() {
        if let Ok(canonical) = dunce::canonicalize(ancestor)
            && has_git_component(&canonical)
        {
            return Err(SyncError::PathTraversal {
                path: canonical,
                reason: PathReject::GitComponent,
            });
        }
    }
    Ok(())
}

/// Whether any path component is `.git` (or the string contains a `.git` path segment).
fn has_git_component(candidate: &Path) -> bool {
    for component in candidate.components() {
        if let Component::Normal(name) = component
            && name == ".git"
        {
            return true;
        }
    }
    let s = candidate.to_string_lossy();
    s.contains("/.git/") || s.contains("\\.git\\") || s.ends_with("/.git") || s.ends_with("\\.git")
}

/// The three-letter Windows reserved device names, compared ASCII-case-insensitively.
const DEVICE_NAMES: [&[u8]; 4] = [b"CON", b"PRN", b"AUX", b"NUL"];

/// The console pseudo-devices `CreateFile` opens as console handles.
const CONSOLE_NAMES: [&[u8]; 2] = [b"CONIN$", b"CONOUT$"];

/// Whether one path component (a file or directory name, never a whole path) contains `:`, the
/// NTFS alternate-data-stream separator (NFR-7, `ub-b9z`).
///
/// Lexical and allocation-free: `:` is ASCII, so the platform's encoded bytes contain it exactly
/// when the name does, also for a name that is not valid Unicode.
#[must_use]
pub fn has_stream_colon(name: &OsStr) -> bool {
    name.as_encoded_bytes().contains(&b':')
}

/// The portability refusal for a file name used as a leaf (NFR-7, `ub-b9z`):
/// [`PathReject::ColonInName`] if it contains `:`, else [`PathReject::ReservedDeviceName`] if it is
/// a Windows reserved device name, else `None`. Platform-independent, lexical, allocation-free.
/// `unblock-config` reuses it for the configured file names (`ub-gp0`).
#[must_use]
pub fn leaf_name_reject(name: &OsStr) -> Option<PathReject> {
    if has_stream_colon(name) {
        Some(PathReject::ColonInName)
    } else if is_reserved_device_name(name) {
        Some(PathReject::ReservedDeviceName)
    } else {
        None
    }
}

/// The path-level refusal of step (1a) (NFR-7, `ub-b9z`): on Windows, any [`Component::Normal`]
/// containing `:` → [`PathReject::ColonInName`]; on every platform, the last name that no later
/// `..` cancels (a `..` above the first name cancels nothing), through [`leaf_name_reject`]. Prefix
/// components (`C:`, `\\?\C:`) are never checked. Lexical (no filesystem access), allocation-free.
/// `unblock-config` reuses it for `--db` (`ub-gp0`).
#[must_use]
pub fn unportable_name_reject(path: &Path) -> Option<PathReject> {
    reject_unportable_names(path, cfg!(windows))
}

/// [`unportable_name_reject`] with its platform scope as an argument, so the Linux and macOS cells
/// exercise the every-component branch.
///
/// - `every_component` (`cfg!(windows)` in [`unportable_name_reject`]): no [`Component::Normal`]
///   of the RAW `path` may contain `:`. The raw path, because on Windows `PathBuf::push` of a
///   component such as `d:s.jsonl` (a drive prefix) replaces the buffer and would hide the `:`
///   from the normalized path. Prefix components (`C:`, `\\?\C:`) are never checked.
/// - always: the leaf that the `..` normalization leaves must pass [`leaf_name_reject`].
fn reject_unportable_names(path: &Path, every_component: bool) -> Option<PathReject> {
    let colon_component = every_component
        && path
            .components()
            .any(|c| matches!(c, Component::Normal(name) if has_stream_colon(name)));
    if colon_component {
        Some(PathReject::ColonInName)
    } else {
        lexical_leaf(path).and_then(leaf_name_reject)
    }
}

/// Whether the leaf `name` is a Windows reserved device name: its stem (the bytes before the first
/// `.`, trailing spaces removed) is `CON`/`PRN`/`AUX`/`NUL`, `COM`/`LPT` plus one port digit, or
/// `CONIN$`/`CONOUT$`, in any ASCII case (`NUL.tar.gz` is `NUL`). Win32 also ends the stem at a
/// `:`; a name holding one never gets here, because [`leaf_name_reject`] refuses it first.
fn is_reserved_device_name(name: &OsStr) -> bool {
    let mut stem = name
        .as_encoded_bytes()
        .split(|&b| b == b'.')
        .next()
        .unwrap_or_default();
    while let [rest @ .., b' '] = stem {
        stem = rest;
    }
    match stem {
        [_, _, _] => DEVICE_NAMES
            .iter()
            .any(|device| stem.eq_ignore_ascii_case(device)),
        [p0, p1, p2, digit @ ..] if is_port_digit(digit) => {
            let port = [*p0, *p1, *p2];
            port.eq_ignore_ascii_case(b"COM") || port.eq_ignore_ascii_case(b"LPT")
        }
        _ => CONSOLE_NAMES
            .iter()
            .any(|console| stem.eq_ignore_ascii_case(console)),
    }
}

/// A COM/LPT port digit: one ASCII `0`–`9`, or a superscript `¹` (U+00B9), `²` (U+00B2) or `³`
/// (U+00B3) in its UTF-8 (= WTF-8) encoding, which Windows also treats as a port digit.
fn is_port_digit(digit: &[u8]) -> bool {
    matches!(digit, [b'0'..=b'9'] | [0xC2, 0xB2 | 0xB3 | 0xB9])
}

/// The leaf [`normalize_lexically`] would leave, found without allocating: walk the components
/// backwards, letting each `..` cancel the next `Normal` name. `None` when every name is
/// cancelled; unlike [`normalize_lexically`], a `..` past the root is ignored rather than failing.
fn lexical_leaf(path: &Path) -> Option<&OsStr> {
    let mut pending_pops = 0_usize;
    for component in path.components().rev() {
        match component {
            Component::ParentDir => pending_pops += 1,
            Component::CurDir => {}
            Component::Normal(name) if pending_pops == 0 => return Some(name),
            Component::Normal(_) => pending_pops -= 1,
            Component::Prefix(_) | Component::RootDir => return None,
        }
    }
    None
}

/// Purely-lexical `..` normalization: pop on `ParentDir`, drop `CurDir`. `None` if a `..` escapes
/// above the root.
fn normalize_lexically(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(component.as_os_str()),
            Component::CurDir => {}
            Component::Normal(part) => out.push(part),
            Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
        }
    }
    Some(out)
}

/// Detect an existing ancestor that is a symlink pointing outside `canonical_root`.
///
/// Only ancestors *inside* the confined subtree are checked — an ancestor that is itself a prefix of
/// (i.e. above) the root (e.g. `/var` on macOS, a symlink to `/private/var`) is NOT an escape: it is
/// part of the path *to* the root, and the root's own canonicalization already accounts for it.
fn symlink_escape_ancestor(path: &Path, canonical_root: &Path) -> Option<PathBuf> {
    for ancestor in path.ancestors() {
        // Skip ancestors at or above the confine root — those are the path *to* the root, not
        // in-tree components a caller could use to escape.
        let Ok(canonical_ancestor) = dunce::canonicalize(ancestor) else {
            continue;
        };
        if canonical_root.starts_with(&canonical_ancestor) {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(ancestor) else {
            continue;
        };
        if !meta.file_type().is_symlink() {
            continue;
        }
        let target = std::fs::read_link(ancestor).map_or_else(
            |_| ancestor.to_path_buf(),
            |t| resolve_symlink_target(ancestor, &t),
        );
        if !target.starts_with(canonical_root) {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

/// Resolve a symlink target (absolute or anchored to the link's parent) to a normalized, best-effort
/// canonicalized path.
fn resolve_symlink_target(link: &Path, target: &Path) -> PathBuf {
    let anchored = if target.is_absolute() {
        target.to_path_buf()
    } else {
        link.parent().unwrap_or_else(|| Path::new("")).join(target)
    };
    let normalized = normalize_lexically(&anchored).unwrap_or(anchored);
    dunce::canonicalize(&normalized).unwrap_or(normalized)
}

/// The effective canonical path used for the containment check: the canonicalized target if it
/// exists, otherwise the canonicalized parent + the file name.
fn effective_canonical(normalized: &Path, canonical_root: &Path) -> Result<PathBuf, SyncError> {
    if normalized.exists() {
        return dunce::canonicalize(normalized).map_err(|source| SyncError::Io {
            path: normalized.to_path_buf(),
            action: "canonicalizing",
            source,
        });
    }
    // Non-existent target: canonicalize the parent (it must exist to be confined) + rejoin the name.
    match normalized.parent() {
        Some(parent) if parent.exists() => {
            let canonical_parent = dunce::canonicalize(parent).map_err(|source| SyncError::Io {
                path: parent.to_path_buf(),
                action: "canonicalizing parent of",
                source,
            })?;
            Ok(canonical_parent.join(normalized.file_name().unwrap_or_default()))
        }
        // The parent does not exist yet: fall back to the lexical path for the prefix check against
        // the root (a fresh workspace where `.unblock/` itself is being created).
        _ => {
            let _ = canonical_root;
            Ok(normalized.to_path_buf())
        }
    }
}

/// Validate the file name / extension against the allowlist.
fn validate_extension_and_name(path: &Path) -> Result<(), SyncError> {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    if ALLOWED_EXACT_NAMES.iter().any(|&name| file_name == name) {
        return Ok(());
    }
    if is_allowed_jsonl_temp_name(&file_name) {
        return Ok(());
    }
    for ext in ALLOWED_EXTENSIONS {
        if file_name.ends_with(&format!(".{ext}")) {
            return Ok(());
        }
    }
    // Distinguish a bad exact-name (has a name but no allowed extension) from a bad extension.
    let reason = if path.extension().is_none() {
        PathReject::DisallowedName
    } else {
        PathReject::DisallowedExtension
    };
    Err(SyncError::PathTraversal {
        path: path.to_path_buf(),
        reason,
    })
}

/// Whether `file_name` matches the pid-scoped temp pattern (`*.jsonl.tmp` or `*.jsonl.<digits>.tmp`).
fn is_allowed_jsonl_temp_name(file_name: &str) -> bool {
    if file_name.ends_with(".jsonl.tmp") {
        return true;
    }
    let Some(prefix) = file_name.strip_suffix(".tmp") else {
        return false;
    };
    let Some((base, pid)) = prefix.rsplit_once(".jsonl.") else {
        return false;
    };
    !base.is_empty() && !pid.is_empty() && pid.chars().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::{
        JOURNAL_SUFFIX, PathReject, SHM_SUFFIX, SIDECAR_SUFFIXES, WAL_SUFFIX, has_stream_colon,
        is_allowed_jsonl_temp_name, leaf_name_reject, lexical_leaf, normalize_lexically,
        reject_external_without_reason, reject_unportable_names, sidecar, validate_sync_path,
        validate_temp_path,
    };
    use crate::error::SyncError;
    use std::ffi::OsStr;
    use std::path::{Path, PathBuf};

    fn reason_of(err: &SyncError) -> PathReject {
        match err {
            SyncError::PathTraversal { reason, .. } => *reason,
            other => panic!("expected PathTraversal, got {other:?}"),
        }
    }

    fn tmp_root() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn confined_new_file_is_allowed() {
        let root = tmp_root();
        let path = root.path().join("issues.jsonl");
        let ok = validate_sync_path(&path, root.path(), false).expect("confined new file ok");
        assert!(ok.ends_with("issues.jsonl"));
    }

    #[test]
    fn parent_escape_rejected() {
        let root = tmp_root();
        let path = root.path().join("../escape.jsonl");
        let err = validate_sync_path(&path, root.path(), false).expect_err("escape");
        assert_eq!(reason_of(&err), PathReject::ParentEscape);
    }

    #[test]
    fn git_component_rejected() {
        let root = tmp_root();
        let path = root.path().join(".git").join("config.jsonl");
        let err = validate_sync_path(&path, root.path(), false).expect_err("git");
        assert_eq!(reason_of(&err), PathReject::GitComponent);
    }

    #[test]
    fn git_rejected_even_under_allow_external() {
        let root = tmp_root();
        let path = root.path().join(".git").join("x.jsonl");
        let err = validate_sync_path(&path, root.path(), true).expect_err("git");
        assert_eq!(reason_of(&err), PathReject::GitComponent);
    }

    #[test]
    fn disallowed_extension_rejected() {
        let root = tmp_root();
        let path = root.path().join("notes.txt");
        let err = validate_sync_path(&path, root.path(), false).expect_err("ext");
        assert_eq!(reason_of(&err), PathReject::DisallowedExtension);
    }

    // ---- step (1a), `ub-b9z`: `:` and Windows reserved device names (NFR-7) ----
    // Every test name puts at least two characters before a `:`: on Windows a single ASCII letter
    // followed by `:` parses as a drive prefix, and `Path::join` would replace the whole path.

    /// Leaves that cell `reserved_device_name_rejected` refuses.
    const DEVICE_LEAVES: [&str; 18] = [
        "nul.jsonl",
        "NUL.jsonl",
        "NUL .jsonl",
        "NUL.x.jsonl",
        "CON.x.jsonl",
        "nul..jsonl",
        "prn.jsonl",
        "aux.jsonl",
        "CONIN$.jsonl",
        "conout$.jsonl",
        "com¹.jsonl",
        "LPT².jsonl",
        "COM³.jsonl",
        "COM0.jsonl",
        "LPT0.jsonl",
        "COM1.jsonl",
        "lpt9.jsonl",
        "con.txt",
    ];

    /// Device-like leaves that stay accepted.
    const DEVICE_LIKE_LEAVES: [&str; 12] = [
        " NUL.jsonl",
        "NULL.jsonl",
        "null.jsonl",
        ".nul.jsonl",
        "console.jsonl",
        "CLOCK$.jsonl",
        "COM10.jsonl",
        "COM.jsonl",
        "LPT.jsonl",
        "issues.jsonl",
        "COM½.jsonl",
        "NUL\t.jsonl",
    ];

    #[test]
    fn colon_in_leaf_rejected() {
        let root = tmp_root();
        let path = root.path().join("unblock.db:x.jsonl");
        let err = validate_sync_path(&path, root.path(), false).expect_err("stream name");
        assert_eq!(reason_of(&err), PathReject::ColonInName);
    }

    #[test]
    fn colon_in_leaf_rejected_even_under_allow_external() {
        let root = tmp_root();
        let other = tmp_root();
        for path in [
            root.path().join("unblock.db:x.jsonl"),
            other.path().join("unblock.db:x.jsonl"),
        ] {
            let err = validate_sync_path(&path, root.path(), true).expect_err("never relaxed");
            assert_eq!(
                reason_of(&err),
                PathReject::ColonInName,
                "{}",
                path.display()
            );
        }
    }

    #[test]
    fn colon_in_sidecar_leaf_rejected() {
        let root = tmp_root();
        for suffix in SIDECAR_SUFFIXES {
            let path = root.path().join(format!("unblock.db{suffix}:x.jsonl"));
            let err = validate_sync_path(&path, root.path(), false).expect_err("sidecar stream");
            assert_eq!(reason_of(&err), PathReject::ColonInName, "{suffix}");
        }
    }

    #[test]
    fn colon_in_temp_leaf_rejected() {
        let root = tmp_root();
        let final_path = root.path().join("issues.jsonl");
        let temp = root.path().join("unblock.db:x.jsonl.123.tmp");
        let err = validate_temp_path(&temp, &final_path, root.path()).expect_err("temp stream");
        assert_eq!(reason_of(&err), PathReject::ColonInName);
        let temp = root.path().join("unblock.db:x.jsonl.tmp");
        let err = validate_sync_path(&temp, root.path(), false).expect_err("temp stream");
        assert_eq!(reason_of(&err), PathReject::ColonInName);
    }

    #[test]
    fn git_component_precedes_name_checks() {
        let root = tmp_root();
        for leaf in ["ab:c.jsonl", "NUL.jsonl"] {
            let path = root.path().join(".git").join(leaf);
            for allow_external in [false, true] {
                let err = validate_sync_path(&path, root.path(), allow_external).expect_err("git");
                assert_eq!(reason_of(&err), PathReject::GitComponent, "{leaf}");
            }
        }
    }

    /// Step (1a) runs before step 1 canonicalizes the existing ancestors (no probe touches a
    /// refused name); the control row is the ancestor loop's own cell.
    #[cfg(unix)]
    #[test]
    fn name_checks_precede_symlinked_git_ancestor() {
        let root = tmp_root();
        let elsewhere = tmp_root();
        let git_dir = elsewhere.path().join(".git");
        std::fs::create_dir(&git_dir).expect("mkdir .git");
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&git_dir, &link).expect("symlink");
        for (leaf, expected) in [
            ("ab:c.jsonl", PathReject::ColonInName),
            ("NUL.jsonl", PathReject::ReservedDeviceName),
            ("issues.jsonl", PathReject::GitComponent),
        ] {
            for allow_external in [false, true] {
                let path = link.join(leaf);
                let err = validate_sync_path(&path, root.path(), allow_external).expect_err(leaf);
                assert_eq!(reason_of(&err), expected, "{leaf}");
            }
        }
    }

    #[test]
    fn colon_precedes_parent_escape_and_extension() {
        let root = tmp_root();
        for path in [
            PathBuf::from("/../ab:c.jsonl"),
            root.path().join("../ab:c.jsonl"),
            root.path().join("ab:c.txt"),
        ] {
            let err = validate_sync_path(&path, root.path(), false).expect_err("name first");
            assert_eq!(
                reason_of(&err),
                PathReject::ColonInName,
                "{}",
                path.display()
            );
        }
    }

    #[test]
    fn colon_precedes_containment_and_existence() {
        let root = tmp_root();
        let other = tmp_root();
        let outside = other.path().join("ab:c.jsonl");
        let err = validate_sync_path(&outside, root.path(), false).expect_err("outside");
        assert_eq!(reason_of(&err), PathReject::ColonInName);
        #[cfg(unix)]
        {
            let dir = root.path().join("ab:c.jsonl");
            std::fs::create_dir(&dir).expect("mkdir");
            let err = validate_sync_path(&dir, root.path(), false).expect_err("directory");
            assert_eq!(reason_of(&err), PathReject::ColonInName);
        }
    }

    #[test]
    fn colon_leaf_reached_through_dotdot_rejected() {
        let root = tmp_root();
        let one = root.path().join("ab:c.jsonl").join("x").join("..");
        let two = root
            .path()
            .join("ab:c.jsonl")
            .join("x")
            .join("y")
            .join("..")
            .join("..");
        for path in [one, two] {
            let err = validate_sync_path(&path, root.path(), false).expect_err("normalized leaf");
            assert_eq!(reason_of(&err), PathReject::ColonInName);
        }
    }

    /// Ruling: off Windows only the leaf is checked, so a `:` above or below the confine root in a
    /// directory name is accepted.
    #[cfg(unix)]
    #[test]
    fn colon_in_directory_accepted_on_unix() {
        let base = tmp_root();
        let root = base.path().join("ab:c").join(".unblock");
        let dir = root.join("cd:e");
        std::fs::create_dir_all(&dir).expect("mkdir");
        validate_sync_path(&dir.join("issues.jsonl"), &root, false).expect("leaf-only off Windows");
    }

    /// The Windows half of the scope; no CI job runs it (no Windows test job).
    #[cfg(windows)]
    #[test]
    fn colon_in_directory_rejected_on_windows() {
        let root = tmp_root();
        let path = root.path().join("ds:t").join("x.jsonl");
        let err = validate_sync_path(&path, root.path(), false).expect_err("every component");
        assert_eq!(reason_of(&err), PathReject::ColonInName);
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_leaf_with_colon_rejected() {
        use std::os::unix::ffi::OsStrExt;
        let root = tmp_root();
        let path = root.path().join(OsStr::from_bytes(b"ab\xff:c.jsonl"));
        let err = validate_sync_path(&path, root.path(), false).expect_err("non-UTF-8 stream");
        assert_eq!(reason_of(&err), PathReject::ColonInName);
    }

    #[test]
    fn colon_component_cases() {
        assert!(has_stream_colon(OsStr::new("ab:c.jsonl")));
        assert!(!has_stream_colon(OsStr::new("issues.jsonl")));
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            assert!(has_stream_colon(OsStr::from_bytes(b"ab\xff:c")));
        }
        let folder = Path::new("xy:z/issues.jsonl");
        assert_eq!(reject_unportable_names(folder, false), None);
        assert_eq!(
            reject_unportable_names(folder, true),
            Some(PathReject::ColonInName)
        );
        assert_eq!(
            reject_unportable_names(Path::new("ab:c.jsonl"), false),
            Some(PathReject::ColonInName)
        );
        assert_eq!(
            reject_unportable_names(Path::new("issues.jsonl"), true),
            None
        );
        // A drive or verbatim prefix is never checked (prefixes parse only on Windows).
        #[cfg(windows)]
        for prefixed in [r"C:\x\issues.jsonl", r"\\?\C:\x\issues.jsonl"] {
            assert_eq!(reject_unportable_names(Path::new(prefixed), true), None);
        }
    }

    #[test]
    fn reserved_device_name_rejected() {
        let root = tmp_root();
        for leaf in DEVICE_LEAVES.into_iter().chain(["NUL"]) {
            let path = root.path().join(leaf);
            for allow_external in [false, true] {
                let err = validate_sync_path(&path, root.path(), allow_external).expect_err(leaf);
                assert_eq!(reason_of(&err), PathReject::ReservedDeviceName, "{leaf}");
            }
        }
        let final_path = root.path().join("issues.jsonl");
        let temp = root.path().join("LPT9.jsonl.123.tmp");
        let err = validate_temp_path(&temp, &final_path, root.path()).expect_err("device temp");
        assert_eq!(reason_of(&err), PathReject::ReservedDeviceName);
    }

    #[test]
    fn device_like_names_accepted() {
        let root = tmp_root();
        for leaf in DEVICE_LIKE_LEAVES {
            validate_sync_path(&root.path().join(leaf), root.path(), false).expect(leaf);
        }
    }

    #[test]
    fn leaf_name_reject_cases() {
        for leaf in DEVICE_LEAVES.into_iter().chain(["NUL"]) {
            let reason = leaf_name_reject(OsStr::new(leaf));
            assert_eq!(reason, Some(PathReject::ReservedDeviceName), "{leaf}");
        }
        for leaf in DEVICE_LIKE_LEAVES {
            assert_eq!(leaf_name_reject(OsStr::new(leaf)), None, "{leaf}");
        }
        for leaf in ["ab:c.jsonl", "NUL.x:y.jsonl"] {
            let reason = leaf_name_reject(OsStr::new(leaf));
            assert_eq!(reason, Some(PathReject::ColonInName), "{leaf}");
        }
    }

    #[test]
    fn colon_checked_before_device_name() {
        let root = tmp_root();
        let path = root.path().join("NUL.x:y.jsonl");
        let err = validate_sync_path(&path, root.path(), false).expect_err("colon first");
        assert_eq!(reason_of(&err), PathReject::ColonInName);
    }

    #[test]
    fn issues_jsonl_still_accepted() {
        let root = tmp_root();
        let final_path = root.path().join("issues.jsonl");
        validate_sync_path(&final_path, root.path(), false).expect("issues.jsonl");
        let temp = root.path().join("issues.jsonl.123.tmp");
        validate_temp_path(&temp, &final_path, root.path()).expect("its temp");
    }

    #[test]
    fn lexical_leaf_cases() {
        let leaf = |p: &str| lexical_leaf(Path::new(p)).map(OsStr::to_os_string);
        assert_eq!(leaf("a/b/.."), Some("a".into()));
        assert_eq!(leaf("a/./b"), Some("b".into()));
        assert_eq!(leaf("ab:c.jsonl/x/.."), Some("ab:c.jsonl".into()));
        assert_eq!(leaf("ab:c.jsonl/x/y/../.."), Some("ab:c.jsonl".into()));
        assert_eq!(leaf("a/b/c/../.."), Some("a".into()));
        assert_eq!(leaf("x/.."), None);
        assert_eq!(leaf("/.."), None);
        assert_eq!(leaf("/"), None);
        // Deliberately differs from `normalize_lexically`, which returns `None` here.
        assert_eq!(leaf("/../a"), Some("a".into()));
    }

    #[test]
    fn outside_confine_root_rejected_without_external() {
        let root = tmp_root();
        let other = tmp_root();
        let path = other.path().join("issues.jsonl");
        let err = validate_sync_path(&path, root.path(), false).expect_err("outside");
        assert_eq!(reason_of(&err), PathReject::OutsideConfineRoot);
    }

    #[test]
    fn outside_confine_root_allowed_with_external() {
        let root = tmp_root();
        let other = tmp_root();
        let path = other.path().join("issues.jsonl");
        // `allow_external` relaxes ONLY containment; a valid `.jsonl` outside the root is now ok.
        validate_sync_path(&path, root.path(), true).expect("external ok");
    }

    #[cfg(unix)]
    #[test]
    fn ancestor_symlink_escape_rejected_even_under_allow_external() {
        // A deliberate TIGHTENING over the original (which checked the leaf only for external, SH-1):
        // an ancestor symlink escaping the root is rejected regardless of `allow_external`.
        let root = tmp_root();
        let outside = tmp_root();
        let link = root.path().join("evil");
        std::os::unix::fs::symlink(outside.path(), &link).expect("symlink");
        let path = link.join("issues.jsonl");
        let err = validate_sync_path(&path, root.path(), true).expect_err("symlink escape");
        assert_eq!(reason_of(&err), PathReject::SymlinkEscape);
    }

    #[test]
    fn non_regular_file_rejected() {
        let root = tmp_root();
        // A subdirectory named like a jsonl file is not a regular file.
        let dir = root.path().join("issues.jsonl");
        std::fs::create_dir(&dir).expect("mkdir");
        let err = validate_sync_path(&dir, root.path(), false).expect_err("non-regular");
        assert_eq!(reason_of(&err), PathReject::NonRegularFile);
    }

    #[test]
    fn temp_path_must_be_sibling() {
        let root = tmp_root();
        let final_path = root.path().join("issues.jsonl");
        let temp = root.path().join("issues.jsonl.123.tmp");
        validate_temp_path(&temp, &final_path, root.path()).expect("sibling temp ok");
    }

    #[test]
    fn normalize_lexically_pops_parents() {
        assert_eq!(
            normalize_lexically(Path::new("/a/b/../c")),
            Some(PathBuf::from("/a/c"))
        );
        assert_eq!(normalize_lexically(Path::new("/../x")), None);
        assert_eq!(
            normalize_lexically(Path::new("/a/./b")),
            Some(PathBuf::from("/a/b"))
        );
    }

    #[test]
    fn temp_name_pattern() {
        assert!(is_allowed_jsonl_temp_name("issues.jsonl.tmp"));
        assert!(is_allowed_jsonl_temp_name("issues.jsonl.12345.tmp"));
        assert!(!is_allowed_jsonl_temp_name("issues.jsonl.abc.tmp"));
        assert!(!is_allowed_jsonl_temp_name(".jsonl.1.tmp"));
        assert!(!is_allowed_jsonl_temp_name("issues.txt.1.tmp"));
    }

    #[test]
    fn reason_gate_rejects_external_override_without_reason() {
        // The LOCKED D30-2 reason-gate predicate: `allow_external && external_reason.is_none()` is the
        // SOLE reject condition. This drives all four (allow_external × reason) cells so neutering the
        // predicate flips exactly one of them — behavioral coverage the exit-6 mapping test lacks.
        let path = Path::new("/some/external/issues.jsonl");

        // (1) external override WITHOUT a reason → the ONLY rejected cell.
        let err = reject_external_without_reason(path, true, None)
            .expect_err("external override without a reason must be rejected");
        assert!(
            matches!(
                &err,
                SyncError::ExternalOverrideWithoutReason { path: rejected } if rejected == path
            ),
            "expected ExternalOverrideWithoutReason carrying the offending path, got {err:?}"
        );

        // (2) external override WITH a written reason → allowed.
        reject_external_without_reason(path, true, Some("operator reason"))
            .expect("a written reason satisfies the gate");

        // (3) no external override, no reason → no-op (the reason is irrelevant when not external).
        reject_external_without_reason(path, false, None)
            .expect("no external override is always allowed");

        // (4) no external override, reason present → still a no-op.
        reject_external_without_reason(path, false, Some("operator reason"))
            .expect("no external override is always allowed, reason ignored");
    }

    #[test]
    fn sidecar_appends_to_a_dotted_db_name() {
        let db = Path::new("/ws/.unblock/unblock.db");
        assert_eq!(
            sidecar(db, WAL_SUFFIX),
            PathBuf::from("/ws/.unblock/unblock.db-wal")
        );
        assert_eq!(
            sidecar(db, SHM_SUFFIX),
            PathBuf::from("/ws/.unblock/unblock.db-shm")
        );
        assert_eq!(
            sidecar(db, JOURNAL_SUFFIX),
            PathBuf::from("/ws/.unblock/unblock.db-journal")
        );
    }

    #[test]
    fn sidecar_appends_to_an_extensionless_db_name() {
        // `with_extension("-wal")` would produce a WRONG `unblock-wal`-as-extension; append is right.
        let db = Path::new("/ws/unblock");
        assert_eq!(sidecar(db, WAL_SUFFIX), PathBuf::from("/ws/unblock-wal"));
    }

    #[test]
    fn sidecar_appends_to_a_multi_dot_db_name() {
        let db = Path::new("issues.sqlite");
        assert_eq!(sidecar(db, SHM_SUFFIX), PathBuf::from("issues.sqlite-shm"));
    }

    #[test]
    fn sidecar_is_pure_no_side_effects() {
        // A relative, non-existent path derives without touching the filesystem.
        let db = Path::new("does/not/exist.db");
        assert_eq!(
            sidecar(db, WAL_SUFFIX),
            PathBuf::from("does/not/exist.db-wal")
        );
    }

    /// S7b (PRD §4 D5 amendment, `ub-c15`): the crate-internal reader open refuses a session file
    /// recognized on the OPENED descriptor (`SessionDatabase`) and keeps that descriptor open
    /// (fd witness +1): closing it would drop the session's locks.
    #[cfg(unix)]
    #[test]
    fn open_import_source_refuses_and_keeps_a_session_file_open() {
        let root = tmp_root();
        let db = root.path().join("unblock.db");
        std::fs::write(&db, b"db").unwrap();
        let alias = root.path().join("alias.jsonl");
        std::fs::hard_link(&db, &alias).expect("hard link");
        let session = super::SessionFiles::of_database(&db);
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        let before = {
            crate::fd_witness::assert_witness_sees(&db);
            crate::fd_witness::descriptors_on(&db)
        };

        let err = session
            .open_import_source(&alias)
            .expect_err("a session file is refused at the open");
        assert_eq!(reason_of(&err), PathReject::SessionDatabase);

        #[cfg(any(target_os = "linux", target_os = "macos"))]
        assert_eq!(
            crate::fd_witness::descriptors_on(&db),
            before + 1,
            "the session file's descriptor is kept open, never closed"
        );
    }
}
