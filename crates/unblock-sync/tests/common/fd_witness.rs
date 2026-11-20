//! The fd witness (PRD §4 D5 amendment, `ub-c15`): how many of THIS process's open descriptors refer
//! to a given file. Linux stats each `/proc/self/fd` entry, whose magic link resolves to the file,
//! and keys on `(st_dev, st_ino)`. macOS stats each `/dev/fd` entry, which reports devfs's `st_dev`
//! but the file's own inode and birth time, and keys on `(st_ino, st_birthtime, st_birthtime_nsec)`.
//! An entry whose stat fails (the listing's own descriptor once closed, or one another test thread
//! closed meanwhile) is skipped, never unwrapped. Every assertion is a delta against a count taken
//! in the same cell. Compiled only under `any(target_os = "linux", target_os = "macos")`, gated at
//! each include site; each includer uses a subset.
#![allow(dead_code)]

use std::fs::{File, Metadata};
use std::path::Path;

#[cfg(target_os = "linux")]
const FD_DIR: &str = "/proc/self/fd";
#[cfg(target_os = "macos")]
const FD_DIR: &str = "/dev/fd";

#[cfg(target_os = "linux")]
type Key = (u64, u64);
#[cfg(target_os = "macos")]
type Key = (u64, i64, i64);

#[cfg(target_os = "linux")]
fn key(m: &Metadata) -> Key {
    use std::os::unix::fs::MetadataExt;
    (m.dev(), m.ino())
}

#[cfg(target_os = "macos")]
fn key(m: &Metadata) -> Key {
    use std::os::macos::fs::MetadataExt;
    (m.st_ino(), m.st_birthtime(), m.st_birthtime_nsec())
}

/// The number of this process's open descriptors on `p`'s file.
///
/// # Panics
/// If `p` cannot be stat-ed or the descriptor directory cannot be listed.
#[must_use]
pub fn descriptors_on(p: &Path) -> usize {
    let want = key(&std::fs::metadata(p).expect("stat the witnessed file"));
    std::fs::read_dir(FD_DIR)
        .expect("list the descriptor directory")
        .filter_map(Result::ok)
        .filter_map(|entry| std::fs::metadata(entry.path()).ok())
        .filter(|m| key(m) == want)
        .count()
}

/// The positive control every witnessed cell runs first: one extra `File::open` of `p` counts +1,
/// and its drop counts it back. A blind instrument fails here, loudly, before the cell asserts.
///
/// # Panics
/// If the witness does not see the open, or does not see the close.
pub fn assert_witness_sees(p: &Path) {
    let base = descriptors_on(p);
    let probe = File::open(p).expect("open the witnessed file");
    assert_eq!(
        descriptors_on(p),
        base + 1,
        "the fd witness cannot see an open of {}",
        p.display()
    );
    drop(probe);
    assert_eq!(
        descriptors_on(p),
        base,
        "the fd witness cannot see a close of {}",
        p.display()
    );
}
