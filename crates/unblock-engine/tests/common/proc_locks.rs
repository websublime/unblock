//! The Linux `/proc/locks` parser, COPIED from `unblock-storage`'s
//! `tests/common/posix_locks.rs` (PRD §4 D58 criterion (n), `ub-2vp`). The engine's G3 cell reads
//! this process's own locks with it, and `proc_locks_parser_reads_canned_lines` (G3p) pins this
//! copy's behaviour on the same canned lines as fdp. The reader, the parser and the snapshot are
//! byte-identical to the storage copy apart from its cfg lines.

use std::path::Path;

/// The first byte of `SQLite`'s SHARED range on the database file (`PENDING_BYTE + 2`).
pub const SHARED_FIRST: i64 = 0x4000_0002;
/// The length of `SQLite`'s SHARED range.
pub const SHARED_SIZE: i64 = 510;

/// The kind of a held POSIX lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockKind {
    /// `/proc/locks` `READ`.
    Read,
    /// `/proc/locks` `WRITE`.
    Write,
}

/// One lock a process holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldLock {
    /// Read or write.
    pub kind: LockKind,
    /// The owner's pid.
    pub pid: i32,
    /// The first locked byte.
    pub start: i64,
    /// The locked length; 0 means "to the end of the file".
    pub len: i64,
    /// The `maj:min` of the device, matched against the file's device with the btrfs fallback that
    /// `parse_proc_locks` describes.
    pub dev: Option<String>,
}

/// Decode a `st_dev` into its major and minor numbers the way glibc's `gnu_dev_major` and
/// `gnu_dev_minor` do.
pub fn dev_major_minor(dev: u64) -> (u32, u32) {
    let major = ((dev >> 8) & 0xfff) | ((dev >> 32) & 0xffff_f000);
    let minor = (dev & 0xff) | ((dev >> 12) & 0xffff_ff00);
    (
        u32::try_from(major).expect("the major number fits u32"),
        u32::try_from(minor).expect("the minor number fits u32"),
    )
}

/// How many times an observation reads both witnesses before it calls a disagreement an
/// instrument defect.
pub const PROC_LOCKS_ATTEMPTS: usize = 10;

/// One read of `/proc/locks`.
pub struct ProcLocksRead {
    /// The whole text of the read.
    pub text: String,
    /// The `read()` calls the read took.
    pub reads: usize,
    /// The bytes the read returned.
    pub bytes: usize,
}

/// Read `/proc/locks` with `File::read` into one 256 KiB buffer, which grows if it fills.
///
/// The kernel serves `/proc/locks` through `seq_file`. Every `read()` call restarts the walk of the
/// global lock list (`locks_start`, through `seq_hlist_start_percpu`) at the entry number where
/// the previous call stopped, and `locks_stop` releases the list between calls. A lock removed
/// earlier in the list between two calls shifts later entries back by one, so the next call skips
/// a line, and a lock added there repeats one. `read_to_string` reads in small growing chunks and
/// so restarts the walk many times. One large buffer restarts it only once per page, because one
/// `read()` returns at most one page.
///
/// # Panics
///
/// When `/proc/locks` cannot be opened, a read fails with anything but `Interrupted` (which is
/// retried), or the text is not UTF-8.
pub fn read_proc_locks() -> ProcLocksRead {
    use std::io::Read as _;

    let mut file = std::fs::File::open("/proc/locks").expect("open /proc/locks");
    let mut buffer = vec![0_u8; 256 * 1024];
    let mut bytes = 0;
    let mut reads = 0;
    loop {
        if bytes == buffer.len() {
            buffer.resize(buffer.len() * 2, 0);
        }
        let read = match file.read(&mut buffer[bytes..]) {
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => panic!("read /proc/locks failed with {error}"),
        };
        reads += 1;
        if read == 0 {
            break;
        }
        bytes += read;
    }
    buffer.truncate(bytes);
    let text = String::from_utf8(buffer).expect("/proc/locks is UTF-8");
    ProcLocksRead { text, reads, bytes }
}

/// Every raw `/proc/locks` line on inode `ino`, from any device and including waiters. It is the
/// failure evidence an instrument defect prints.
pub fn raw_lines_on_inode(text: &str, ino: u64) -> Vec<String> {
    let ino = ino.to_string();
    text.lines()
        .filter(|line| {
            line.split_whitespace().any(|field| {
                field
                    .rsplit_once(':')
                    .is_some_and(|(dev, field_ino)| dev.contains(':') && field_ino == ino)
            })
        })
        .map(str::to_owned)
        .collect()
}

/// Parse the `POSIX` lines of `/proc/locks` text for the file with device `dev` (major, minor) and
/// inode `ino`, of `pid` when given. The kernel prints the device as hex `maj:min`. A `->` line is a
/// blocked waiter and is skipped, and a range end of `EOF` is a length of 0. Identical (pid, kind,
/// start, end, device) records are kept once, because only a torn read can repeat a line.
///
/// The match selects the lines on inode `ino` first. When at least one of them carries device
/// `dev`, only those lines count. When none does, every line on the inode counts. That fallback
/// exists for btrfs, where `stat` reports the subvolume's anonymous device while `/proc/locks`
/// prints the superblock's device, so no line would ever carry the `stat` device. The CRIU
/// checkpoint/restore project matches `/proc/locks` lines against `stat` with the same btrfs
/// exception.
///
/// # Panics
///
/// On a line it cannot parse, or on a matching line whose class is not `POSIX`.
pub fn parse_proc_locks(text: &str, dev: (u32, u32), ino: u64, pid: Option<i32>) -> Vec<HeldLock> {
    let mut on_inode = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.get(1) == Some(&"->") || fields.len() < 8 {
            continue;
        }
        let (line_dev, line_ino) = fields[5]
            .rsplit_once(':')
            .unwrap_or_else(|| panic!("dev:ino in /proc/locks line {line:?}"));
        let line_ino: u64 = line_ino
            .parse()
            .unwrap_or_else(|_| panic!("inode in /proc/locks line {line:?}"));
        if line_ino != ino {
            continue;
        }
        let line_major_minor = line_dev
            .split_once(':')
            .and_then(|(major, minor)| {
                Some((
                    u32::from_str_radix(major, 16).ok()?,
                    u32::from_str_radix(minor, 16).ok()?,
                ))
            })
            .unwrap_or_else(|| panic!("maj:min in /proc/locks line {line:?}"));
        on_inode.push((line, fields, line_dev, line_major_minor));
    }
    let on_device = on_inode
        .iter()
        .any(|(_, _, _, line_major_minor)| *line_major_minor == dev);
    let mut held = Vec::new();
    for (line, fields, line_dev, line_major_minor) in on_inode {
        if on_device && line_major_minor != dev {
            continue;
        }
        let (class, kind, owner, start, end) =
            (fields[1], fields[3], fields[4], fields[6], fields[7]);
        let owner: i32 = owner
            .parse()
            .unwrap_or_else(|_| panic!("pid in /proc/locks line {line:?}"));
        if pid.is_some_and(|pid| pid != owner) {
            continue;
        }
        assert_eq!(
            class, "POSIX",
            "a lock on the file that is not POSIX, in {line:?}"
        );
        let kind = match kind {
            "READ" => LockKind::Read,
            "WRITE" => LockKind::Write,
            other => panic!("lock type {other:?} in /proc/locks line {line:?}"),
        };
        let start: i64 = start
            .parse()
            .unwrap_or_else(|_| panic!("start in /proc/locks line {line:?}"));
        let len = if end == "EOF" {
            0
        } else {
            let end: i64 = end
                .parse()
                .unwrap_or_else(|_| panic!("end in /proc/locks line {line:?}"));
            end - start + 1
        };
        let lock = HeldLock {
            kind,
            pid: owner,
            start,
            len,
            dev: Some(line_dev.to_owned()),
        };
        // The kernel merges one process's adjacent locks of one type, so two identical records
        // can only come from a torn read that repeated a line.
        if !held.contains(&lock) {
            held.push(lock);
        }
    }
    held
}

/// One `/proc/locks` snapshot of a file.
pub struct ProcLocksSnapshot {
    /// The parsed lines on the file, of the requested pid when given.
    pub lines: Vec<HeldLock>,
    /// Every raw line on the file's inode, from any device.
    pub raw_on_inode: Vec<String>,
    /// The `read()` calls the snapshot took.
    pub reads: usize,
    /// The bytes the snapshot read.
    pub bytes: usize,
}

/// One `/proc/locks` snapshot of `path`'s device and inode, of `pid` when given.
pub fn proc_locks(path: &Path, pid: Option<i32>) -> ProcLocksSnapshot {
    use std::os::unix::fs::MetadataExt as _;

    let metadata = std::fs::metadata(path).expect("metadata");
    let read = read_proc_locks();
    ProcLocksSnapshot {
        lines: parse_proc_locks(
            &read.text,
            dev_major_minor(metadata.dev()),
            metadata.ino(),
            pid,
        ),
        raw_on_inode: raw_lines_on_inode(&read.text, metadata.ino()),
        reads: read.reads,
        bytes: read.bytes,
    }
}

/// What [`shared_range`] saw over its reads.
pub struct SharedRangeReads {
    /// `Read` when some read showed `pid`'s read lock on the SHARED range. Otherwise the first other
    /// kind a read showed there, or `None` when no read showed a lock.
    pub kind: Option<LockKind>,
    /// The reads taken, at most [`PROC_LOCKS_ATTEMPTS`].
    pub attempts: usize,
    /// The `read()` calls on `/proc/locks`, summed over every read.
    pub proc_reads: usize,
    /// The bytes read from `/proc/locks`, summed over every read.
    pub proc_bytes: usize,
    /// Every raw line on the inode, from any device, for every read.
    pub raw_on_inode: Vec<Vec<String>>,
}

/// The lock `pid` holds on `SQLite`'s SHARED range of the database file `db`.
///
/// One `/proc/locks` read can skip a line (see [`read_proc_locks`]), so this reads up to
/// [`PROC_LOCKS_ATTEMPTS`] times and stops only at a read that shows the read lock. A lock that was
/// really dropped never comes back, so absence on every read is a real absence. A read that shows
/// another kind does not stop the reads, and the result keeps that kind as evidence.
pub fn shared_range(db: &Path, pid: i32) -> SharedRangeReads {
    let mut seen = SharedRangeReads {
        kind: None,
        attempts: 0,
        proc_reads: 0,
        proc_bytes: 0,
        raw_on_inode: Vec::new(),
    };
    while seen.attempts < PROC_LOCKS_ATTEMPTS {
        seen.attempts += 1;
        let snapshot = proc_locks(db, Some(pid));
        seen.proc_reads += snapshot.reads;
        seen.proc_bytes += snapshot.bytes;
        seen.raw_on_inode.push(snapshot.raw_on_inode);
        let kinds: Vec<LockKind> = snapshot
            .lines
            .iter()
            .filter(|lock| lock.start == SHARED_FIRST && lock.len == SHARED_SIZE)
            .map(|lock| lock.kind)
            .collect();
        if kinds.contains(&LockKind::Read) {
            seen.kind = Some(LockKind::Read);
            break;
        }
        if seen.kind.is_none() {
            seen.kind = kinds.first().copied();
        }
    }
    seen
}
