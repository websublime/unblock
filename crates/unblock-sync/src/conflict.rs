//! Conflict-marker scanner + ingestion size guards (FR-8/NFR-8/NFR-18).
//!
//! An import file carrying git conflict markers (`<<<<<<<`/`=======`/`>>>>>>>`) is a merge accident,
//! never valid input — it is rejected at preflight with ZERO DB writes. The scan also enforces the
//! FORK-3 ingestion caps: a fd-metadata file-size guard (BEFORE any read) and a **bounded** per-line
//! read (`take(MAX_LINE_BYTES + 1).read_until(b'\n', …)`, MF-3 — never `.lines()`, which would
//! materialize the whole line before the check → OOM).

use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use crate::error::SyncError;

/// The `<<<<<<<` conflict-start marker prefix (7 chars).
pub const CONFLICT_START: &str = "<<<<<<<";
/// The `=======` conflict-separator marker prefix (7 chars).
pub const SEPARATOR: &str = "=======";
/// The `>>>>>>>` conflict-end marker prefix (7 chars).
pub const END: &str = ">>>>>>>";

/// Max import file size (FORK-3, NFR-18): 2 GiB, enforced via fd-metadata BEFORE any read.
///
/// Overridable-later WITHOUT a v1 signature break: the engine MAY thread a value through an additive
/// `ImportOptions` field. v1 ships the const.
pub const MAX_IMPORT_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Max single-line length (FORK-3, NFR-18): 4 MiB, enforced per-line by a bounded read (MF-3).
pub const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;

/// The kind of a detected conflict marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictMarkerType {
    /// `<<<<<<<` — the start of the local side.
    Start,
    /// `=======` — the separator.
    Separator,
    /// `>>>>>>>` — the end of the incoming side.
    End,
}

/// A conflict marker located in an import file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictMarker {
    /// The 1-based line number.
    pub line: usize,
    /// The marker kind.
    pub marker_type: ConflictMarkerType,
    /// The branch/label tail after the marker (e.g. `HEAD` after `<<<<<<< HEAD`), if any.
    pub branch: Option<String>,
}

/// Classify a line as a conflict marker (prefix-based; a longer run still matches).
#[must_use]
pub fn detect_conflict_marker(line: &str) -> Option<(ConflictMarkerType, Option<String>)> {
    let trimmed = line.trim_end_matches(['\r', '\n']);
    for (prefix, kind) in [
        (CONFLICT_START, ConflictMarkerType::Start),
        (SEPARATOR, ConflictMarkerType::Separator),
        (END, ConflictMarkerType::End),
    ] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            let tail = rest.trim();
            let branch = (!tail.is_empty()).then(|| tail.to_string());
            return Some((kind, branch));
        }
    }
    None
}

/// Open + size-guard the import file, returning a bounded `BufReader`.
///
/// Rejects a non-regular file / a file over [`MAX_IMPORT_FILE_BYTES`] BEFORE any read.
fn open_guarded(path: &Path) -> Result<BufReader<File>, SyncError> {
    let meta = std::fs::metadata(path).map_err(|source| SyncError::Io {
        path: path.to_path_buf(),
        action: "reading metadata for",
        source,
    })?;
    if !meta.is_file() {
        return Err(SyncError::PathTraversal {
            path: path.to_path_buf(),
            reason: crate::path::PathReject::NonRegularFile,
        });
    }
    if meta.len() > MAX_IMPORT_FILE_BYTES {
        return Err(SyncError::FileTooLarge {
            path: path.to_path_buf(),
            size: meta.len(),
            cap: MAX_IMPORT_FILE_BYTES,
        });
    }
    let file = File::open(path).map_err(|source| SyncError::Io {
        path: path.to_path_buf(),
        action: "opening",
        source,
    })?;
    Ok(BufReader::with_capacity(2 * 1024 * 1024, file))
}

/// Read the next line into `buf` with a HARD [`MAX_LINE_BYTES`] cap (MF-3), returning the number of
/// bytes read (0 = EOF). Aborts with [`SyncError::LineTooLong`] at the cap WITHOUT materializing the
/// whole line.
pub(crate) fn read_line_bounded<R: BufRead>(
    reader: &mut R,
    buf: &mut Vec<u8>,
    line_no: usize,
    path: &Path,
) -> Result<usize, SyncError> {
    buf.clear();
    // Read at most MAX_LINE_BYTES + 1 bytes: the +1 sentinel lets us detect an over-cap line without
    // buffering the (potentially 2-GiB) remainder of the line.
    let cap = u64::try_from(MAX_LINE_BYTES)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let read = reader
        .take(cap)
        .read_until(b'\n', buf)
        .map_err(|source| SyncError::Io {
            path: path.to_path_buf(),
            action: "reading a line from",
            source,
        })?;
    // If we filled the sentinel byte AND the line was not terminated within the cap, it is too long.
    if buf.len() > MAX_LINE_BYTES && buf.last() != Some(&b'\n') {
        return Err(SyncError::LineTooLong {
            line: line_no,
            len: buf.len(),
            cap: MAX_LINE_BYTES,
        });
    }
    Ok(read)
}

/// Scan `path` for conflict markers (streaming, bounded), returning them in line order.
///
/// # Errors
///
/// [`SyncError::Io`]/[`SyncError::FileTooLarge`]/[`SyncError::LineTooLong`] on the ingestion guards.
pub fn scan_conflict_markers(path: &Path) -> Result<Vec<ConflictMarker>, SyncError> {
    let mut reader = open_guarded(path)?;
    let mut markers = Vec::new();
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let mut line_no = 0usize;
    loop {
        line_no += 1;
        let read = read_line_bounded(&mut reader, &mut buf, line_no, path)?;
        if read == 0 {
            break;
        }
        // Marker detection only needs the leading bytes; a non-UTF-8 line simply won't match a
        // marker prefix (lossy is safe here — the parse pass validates JSONL separately).
        let line = String::from_utf8_lossy(&buf);
        if let Some((marker_type, branch)) = detect_conflict_marker(&line) {
            markers.push(ConflictMarker {
                line: line_no,
                marker_type,
                branch,
            });
        }
    }
    Ok(markers)
}

/// Ensure `path` has NO conflict markers; on any marker, error with a ≤5-marker preview.
///
/// # Errors
///
/// [`SyncError::ConflictMarkers`] (exit-6) with a short preview, or an ingestion-guard error.
pub fn ensure_no_conflict_markers(path: &Path) -> Result<(), SyncError> {
    let markers = scan_conflict_markers(path)?;
    if markers.is_empty() {
        return Ok(());
    }
    let preview = markers
        .iter()
        .take(5)
        .map(|m| {
            let kind = match m.marker_type {
                ConflictMarkerType::Start => "start",
                ConflictMarkerType::Separator => "separator",
                ConflictMarkerType::End => "end",
            };
            format!("line {} ({kind})", m.line)
        })
        .collect::<Vec<_>>()
        .join(", ");
    Err(SyncError::ConflictMarkers {
        path: path.to_path_buf(),
        preview,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        ConflictMarkerType, MAX_LINE_BYTES, detect_conflict_marker, ensure_no_conflict_markers,
        scan_conflict_markers,
    };
    use crate::error::SyncError;
    use std::io::Write;

    fn write_file(dir: &tempfile::TempDir, name: &str, content: &[u8]) -> std::path::PathBuf {
        let path = dir.path().join(name);
        let mut f = std::fs::File::create(&path).expect("create");
        f.write_all(content).expect("write");
        path
    }

    #[test]
    fn clean_file_has_no_markers() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(&dir, "issues.jsonl", b"{\"a\":1}\n{\"b\":2}\n");
        assert!(scan_conflict_markers(&path).unwrap().is_empty());
        ensure_no_conflict_markers(&path).expect("clean");
    }

    #[test]
    fn empty_file_is_clean() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(&dir, "issues.jsonl", b"");
        assert!(scan_conflict_markers(&path).unwrap().is_empty());
    }

    #[test]
    fn each_marker_kind_detected_with_branch() {
        assert_eq!(
            detect_conflict_marker("<<<<<<< HEAD"),
            Some((ConflictMarkerType::Start, Some("HEAD".to_string())))
        );
        assert_eq!(
            detect_conflict_marker("======="),
            Some((ConflictMarkerType::Separator, None))
        );
        assert_eq!(
            detect_conflict_marker(">>>>>>> feature/x\n"),
            Some((ConflictMarkerType::End, Some("feature/x".to_string())))
        );
        assert_eq!(detect_conflict_marker("{\"id\":\"ub-1\"}"), None);
    }

    #[test]
    fn markers_reported_in_line_order() {
        let dir = tempfile::tempdir().unwrap();
        let content = b"a\n<<<<<<< HEAD\nb\n=======\nc\n>>>>>>> theirs\n";
        let path = write_file(&dir, "issues.jsonl", content);
        let markers = scan_conflict_markers(&path).unwrap();
        assert_eq!(markers.len(), 3);
        assert_eq!(markers[0].line, 2);
        assert_eq!(markers[0].marker_type, ConflictMarkerType::Start);
        assert_eq!(markers[1].line, 4);
        assert_eq!(markers[2].line, 6);
    }

    #[test]
    fn ensure_errors_with_preview() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(&dir, "issues.jsonl", b"<<<<<<< HEAD\n=======\n");
        let err = ensure_no_conflict_markers(&path).expect_err("markers");
        match err {
            SyncError::ConflictMarkers { preview, .. } => {
                assert!(preview.contains("line 1"), "preview: {preview}");
            }
            other => panic!("expected ConflictMarkers, got {other:?}"),
        }
    }

    #[test]
    fn crlf_lines_detected() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(&dir, "issues.jsonl", b"<<<<<<< HEAD\r\n=======\r\n");
        let markers = scan_conflict_markers(&path).unwrap();
        assert_eq!(markers.len(), 2);
        assert_eq!(markers[0].branch.as_deref(), Some("HEAD"));
    }

    #[test]
    fn over_cap_single_line_is_line_too_long() {
        let dir = tempfile::tempdir().unwrap();
        // A single line of (cap + 1) bytes with no newline → LineTooLong, bounded peak alloc.
        let big = vec![b'x'; MAX_LINE_BYTES + 1];
        let path = write_file(&dir, "issues.jsonl", &big);
        let err = scan_conflict_markers(&path).expect_err("too long");
        match err {
            SyncError::LineTooLong { line, cap, .. } => {
                assert_eq!(line, 1);
                assert_eq!(cap, MAX_LINE_BYTES);
            }
            other => panic!("expected LineTooLong, got {other:?}"),
        }
    }
}
