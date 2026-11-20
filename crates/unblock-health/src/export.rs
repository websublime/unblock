//! The export scan — the one call of `unblock_sync::scan_conflict_markers` (D58 clause (4)).
//!
//! Health has no conflict-marker scanner of its own. The export can be the database file or a
//! sidecar (its own JSONL name, a hard link or a symlink), and the scan runs inside a live session,
//! so it takes the session's files: a path that is one of them is never opened (`howtocorrupt.html`
//! section 2.2) and is reported as `ExportScan::SessionFile` (PRD §4 D5 amendment).

use std::io::ErrorKind;
use std::path::Path;

use unblock_sync::{PathReject, SessionFiles, SyncError, scan_conflict_markers};

use crate::facts::{ExportFacts, ExportScan};

/// Scan the JSONL export at `path` for git conflict markers.
///
/// `None` or a missing file is [`ExportScan::Absent`]. A path that is one of `session`'s files —
/// refused by the scanner's stat before any open, or recognized on the opened descriptor, which is
/// then kept open — is [`ExportScan::SessionFile`] (PRD §4 D5 amendment, `ub-c15`). The scanner's
/// other I/O, file-size and line-length guard errors become [`ExportScan::Unscannable`].
#[must_use]
pub fn scan_export(path: Option<&Path>, session: &SessionFiles) -> ExportFacts {
    let Some(path) = path else {
        return ExportFacts {
            path: None,
            scan: ExportScan::Absent,
        };
    };
    let scan = match scan_conflict_markers(path, session) {
        Ok(markers) => {
            markers
                .first()
                .map_or(ExportScan::Clean, |marker| ExportScan::ConflictMarkers {
                    first_line: marker.line,
                })
        }
        Err(SyncError::Io { source, .. }) if source.kind() == ErrorKind::NotFound => {
            ExportScan::Absent
        }
        Err(SyncError::PathTraversal {
            reason: PathReject::SessionDatabase,
            ..
        }) => ExportScan::SessionFile,
        Err(err) => ExportScan::Unscannable {
            reason: err.to_string(),
        },
    };
    ExportFacts {
        path: Some(path.to_path_buf()),
        scan,
    }
}
