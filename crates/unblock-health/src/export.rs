//! The export scan — the one call of `unblock_sync::scan_conflict_markers` (D58 clause (4)).
//!
//! Health has no conflict-marker scanner of its own. The export is never the database file, so this
//! read is outside the `howtocorrupt.html` section 2.2 rule.

use std::io::ErrorKind;
use std::path::Path;

use unblock_sync::{SyncError, scan_conflict_markers};

use crate::facts::{ExportFacts, ExportScan};

/// Scan the JSONL export at `path` for git conflict markers.
///
/// `None` or a missing file is [`ExportScan::Absent`]. The scanner's I/O, file-size and line-length
/// guard errors become [`ExportScan::Unscannable`].
#[must_use]
pub fn scan_export(path: Option<&Path>) -> ExportFacts {
    let Some(path) = path else {
        return ExportFacts {
            path: None,
            scan: ExportScan::Absent,
        };
    };
    let scan = match scan_conflict_markers(path) {
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
        Err(err) => ExportScan::Unscannable {
            reason: err.to_string(),
        },
    };
    ExportFacts {
        path: Some(path.to_path_buf()),
        scan,
    }
}
