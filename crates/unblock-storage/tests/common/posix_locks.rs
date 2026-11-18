//! Who holds a POSIX advisory lock on a file, read from ANOTHER process (PRD §4 D58 criterion (n),
//! `ub-2vp`).
//!
//! `F_GETLK` (through `nix`, so no unsafe code) asks the kernel which lock would conflict with a
//! write lock on a byte range. It reports only another process's lock, and only one of them, so the
//! cells keep exactly one holder alive at each observation. On Linux `/proc/locks` is read as well,
//! and the two witnesses must agree, after the bounded retry that `try_observe` describes.
//!
//! `SQLite`'s SHARED lock on the database file is a read lock on the 510 bytes at `0x4000_0002`
//! (`PENDING_BYTE + 2`; libsql-ffi 0.9.30 `bundled/src/sqlite3.c:16242-16248`). In WAL mode every
//! connection holds it for as long as it is open.

use std::fs::OpenOptions;
use std::path::Path;

use nix::fcntl::{FcntlArg, fcntl};
use nix::libc;

/// The first byte of `SQLite`'s SHARED range on the database file.
pub const SHARED_FIRST: libc::off_t = 0x4000_0002;
/// The length of `SQLite`'s SHARED range (inclusive end `0x4000_01FF` = 1073742335).
pub const SHARED_SIZE: libc::off_t = 510;

/// The kind of a held POSIX lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockKind {
    /// `F_RDLCK`, `/proc/locks` `READ`.
    Read,
    /// `F_WRLCK`, `/proc/locks` `WRITE`.
    Write,
}

/// One lock another process holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldLock {
    /// Read or write.
    pub kind: LockKind,
    /// The owner's pid (`l_pid`; for classic POSIX locks the owner on both OSes).
    pub pid: i32,
    /// The first locked byte.
    pub start: i64,
    /// The locked length; 0 means "to the end of the file".
    pub len: i64,
    /// The Linux `/proc/locks` `maj:min` of the device, matched against the file's device with the
    /// btrfs fallback that `parse_proc_locks` describes.
    pub dev: Option<String>,
}

/// What one observation of a byte range saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    /// The conflicting lock the `F_GETLK` witness reports, if any.
    pub getlk: Option<HeldLock>,
    /// The raw `l_type` `F_GETLK` returned (`F_UNLCK` when nothing conflicts).
    pub l_type: i64,
    /// The `/proc/locks` witness on Linux (the matching lines); `None` on other OSes.
    pub proc_locks: Option<Vec<HeldLock>>,
    /// The attempts the observation took to accept both witnesses.
    pub attempts: usize,
    /// The `read()` calls on `/proc/locks`, summed over every attempt (0 off Linux).
    pub proc_reads: usize,
    /// The bytes read from `/proc/locks`, summed over every attempt (0 off Linux).
    pub proc_bytes: usize,
}

impl Observation {
    /// The lock kind the observation found.
    pub fn kind(&self) -> Option<LockKind> {
        self.getlk.as_ref().map(|lock| lock.kind)
    }
}

/// Convert a libc constant into the integer type of an `flock` field. `F_*LCK` is `c_short` on
/// Apple and `c_int` on Linux, so the conversion is generic rather than an `as` cast.
fn short<I: Into<i64>, T: TryFrom<i64>>(value: I) -> T {
    T::try_from(value.into())
        .unwrap_or_else(|_| panic!("a libc lock constant fits its flock field"))
}

/// The lock another process holds that conflicts with a write lock on `len` bytes at `start` of
/// `path` (`len` 0 = to the end of the file).
///
/// # Panics
///
/// When this process itself has a libsql connection to `path` (its own close of the descriptor
/// opened here would drop that connection's locks), or when the open or `fcntl` fails.
pub fn conflicting_lock(
    path: &Path,
    start: libc::off_t,
    len: libc::off_t,
) -> (Option<HeldLock>, i64) {
    assert_eq!(
        unblock_storage::live_connections_to(path).expect("live_connections_to"),
        0,
        "the observer must hold no connection to {}",
        path.display()
    );
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap_or_else(|error| panic!("open {} for F_GETLK failed with {error}", path.display()));
    let mut lock = libc::flock {
        l_type: short(libc::F_WRLCK),
        l_whence: short(libc::SEEK_SET),
        l_start: start,
        l_len: len,
        l_pid: 0,
    };
    fcntl(&file, FcntlArg::F_GETLK(&mut lock)).expect("fcntl(F_GETLK)");
    drop(file);
    let l_type = i64::from(lock.l_type);
    let (unlocked, read, write): (libc::c_short, libc::c_short, libc::c_short) = (
        short(libc::F_UNLCK),
        short(libc::F_RDLCK),
        short(libc::F_WRLCK),
    );
    if lock.l_type == unlocked {
        return (None, l_type);
    }
    let kind = if lock.l_type == read {
        LockKind::Read
    } else if lock.l_type == write {
        LockKind::Write
    } else {
        panic!("F_GETLK returned l_type {}", lock.l_type)
    };
    let held = HeldLock {
        kind,
        pid: lock.l_pid,
        start: lock.l_start,
        len: lock.l_len,
        dev: None,
    };
    (Some(held), l_type)
}

/// Decode a `st_dev` into its major and minor numbers the way glibc's `gnu_dev_major` and
/// `gnu_dev_minor` do.
#[cfg(target_os = "linux")]
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
#[cfg(target_os = "linux")]
pub const PROC_LOCKS_ATTEMPTS: usize = 10;

/// One read of `/proc/locks`.
#[cfg(target_os = "linux")]
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
#[cfg(target_os = "linux")]
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
#[cfg(target_os = "linux")]
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
#[cfg(target_os = "linux")]
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
#[cfg(target_os = "linux")]
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
#[cfg(target_os = "linux")]
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

/// Observe `len` bytes at `start` of `path`. With `pid` given, any holder must be that process;
/// without it, any process counts.
///
/// On Linux both witnesses must agree, and a disagreement retakes both, up to
/// [`PROC_LOCKS_ATTEMPTS`] times, because one `/proc/locks` read can skip a line (see
/// [`read_proc_locks`]). A held lock passes once one read shows a line of `F_GETLK`'s kind and
/// `F_GETLK` gave the identical answer on every attempt. "No lock" passes only when every read
/// shows none and `F_GETLK` agrees. The retry is bounded and fails safe. A torn read can only drop
/// or repeat a real line, so a miss can cause a false failure but never a false pass, and every
/// failure prints the evidence of every attempt. A real selection defect disagrees on every
/// attempt and still fails.
///
/// # Errors
///
/// The instrument-defect report, with every raw `/proc/locks` line on the inode from every attempt.
fn try_observe(
    path: &Path,
    pid: Option<i32>,
    start: libc::off_t,
    len: libc::off_t,
) -> Result<Observation, String> {
    let (getlk, l_type) = conflicting_lock(path, start, len);
    if let (Some(lock), Some(pid)) = (&getlk, pid)
        && lock.pid != pid
    {
        return Err(format!(
            "F_GETLK on {} reports a lock by pid {} while only pid {pid} may hold one",
            path.display(),
            lock.pid
        ));
    }
    confirm(path, pid, start, len, getlk, l_type)
}

/// Take the `/proc/locks` witness and retake both witnesses until they agree, as [`try_observe`]
/// describes.
#[cfg(target_os = "linux")]
fn confirm(
    path: &Path,
    pid: Option<i32>,
    start: libc::off_t,
    len: libc::off_t,
    getlk: Option<HeldLock>,
    l_type: i64,
) -> Result<Observation, String> {
    let expected = getlk.as_ref().map(|lock| lock.kind);
    let mut attempts = 0;
    let mut proc_reads = 0;
    let mut proc_bytes = 0;
    let mut evidence = Vec::new();
    let mut agreed: Option<Vec<HeldLock>> = None;
    let mut absent_every_time = true;
    loop {
        attempts += 1;
        let (attempt_getlk, attempt_l_type) = if attempts == 1 {
            (getlk.clone(), l_type)
        } else {
            conflicting_lock(path, start, len)
        };
        let snapshot = proc_locks(path, pid);
        proc_reads += snapshot.reads;
        proc_bytes += snapshot.bytes;
        let lines: Vec<HeldLock> = snapshot
            .lines
            .into_iter()
            .filter(|lock| len == 0 || (lock.start == start && lock.len == len))
            .collect();
        let proc_kinds: Vec<LockKind> = lines.iter().map(|lock| lock.kind).collect();
        evidence.push(format!(
            "attempt {attempts} saw F_GETLK {attempt_getlk:?} (l_type {attempt_l_type}), \
             /proc/locks kinds {proc_kinds:?}, raw lines on the inode {:?}",
            snapshot.raw_on_inode
        ));
        if attempt_getlk != getlk {
            return Err(format!(
                "instrument defect on {}, because F_GETLK changed between attempts ({getlk:?} \
                 then {attempt_getlk:?}); {}",
                path.display(),
                evidence.join("; ")
            ));
        }
        absent_every_time &= lines.is_empty();
        let matches_getlk = match expected {
            Some(kind) => proc_kinds.contains(&kind),
            None => lines.is_empty(),
        };
        if matches_getlk && agreed.is_none() {
            agreed = Some(lines);
            if expected.is_some() {
                break;
            }
        }
        if attempts == PROC_LOCKS_ATTEMPTS {
            break;
        }
    }
    let accepted = match (expected, agreed) {
        (Some(_), Some(lines)) => Some(lines),
        (None, Some(lines)) if absent_every_time => Some(lines),
        _ => None,
    };
    let Some(lines) = accepted else {
        let what = if expected.is_some() {
            "every /proc/locks read disagreed with F_GETLK"
        } else {
            "F_GETLK saw no lock but some /proc/locks read showed one"
        };
        return Err(format!(
            "/proc/locks and F_GETLK disagree on {} (instrument defect), because {what} over \
             {attempts} attempts with F_GETLK identical on each; the /proc/locks lines match \
             the inode and the stat device, or the inode alone when no line carries the stat \
             device (btrfs); {}",
            path.display(),
            evidence.join("; ")
        ));
    };
    Ok(Observation {
        getlk,
        l_type,
        proc_locks: Some(lines),
        attempts,
        proc_reads,
        proc_bytes,
    })
}

/// Off Linux `F_GETLK` is the only witness, so one attempt accepts it.
#[cfg(not(target_os = "linux"))]
#[allow(clippy::unnecessary_wraps)]
fn confirm(
    _path: &Path,
    _pid: Option<i32>,
    _start: libc::off_t,
    _len: libc::off_t,
    getlk: Option<HeldLock>,
    l_type: i64,
) -> Result<Observation, String> {
    Ok(Observation {
        getlk,
        l_type,
        proc_locks: None,
        attempts: 1,
        proc_reads: 0,
        proc_bytes: 0,
    })
}

/// [`try_observe`], panicking with its instrument-defect report.
fn observe(path: &Path, pid: Option<i32>, start: libc::off_t, len: libc::off_t) -> Observation {
    try_observe(path, pid, start, len).unwrap_or_else(|report| panic!("{report}"))
}

/// Who holds `SQLite`'s SHARED range on the database file `db`; any holder must be `pid`.
pub fn shared_range(db: &Path, pid: i32) -> Observation {
    observe(db, Some(pid), SHARED_FIRST, SHARED_SIZE)
}

/// Any lock, by any process, on the whole of `path` (start 0, length 0).
pub fn whole_file(path: &Path) -> Observation {
    observe(path, None, 0, 0)
}

/// The evidence leg, which is `<os>/<$GITHUB_JOB>` in CI and `<os>/local` elsewhere.
pub fn leg() -> String {
    format!(
        "{}/{}",
        std::env::consts::OS,
        std::env::var("GITHUB_JOB").unwrap_or_else(|_| "local".to_owned())
    )
}

/// The `UB2VP-MEASURE` evidence line for one observation. It reads every field explicitly, so the
/// evidence names the cell, the leg and the full lock record.
pub fn measure_line(
    cell: &str,
    point: &str,
    holder_pid: i32,
    observation: &Observation,
    observer: Option<&str>,
    live: Option<usize>,
) -> String {
    let leg = leg();
    let sh = match observation.kind() {
        Some(LockKind::Read) => "Read",
        Some(LockKind::Write) => "Write",
        None => "None",
    };
    let l_type = observation.l_type;
    let (l_kind, l_pid, l_start, l_len, dev) = match &observation.getlk {
        Some(HeldLock {
            kind,
            pid,
            start,
            len,
            dev,
        }) => (
            format!("{kind:?}"),
            pid.to_string(),
            start.to_string(),
            len.to_string(),
            dev.clone().unwrap_or_else(|| "-".to_owned()),
        ),
        None => (
            "None".to_owned(),
            "-".to_owned(),
            "-".to_owned(),
            "-".to_owned(),
            "-".to_owned(),
        ),
    };
    let proc_locks = match &observation.proc_locks {
        Some(lines) if lines.is_empty() => "none".to_owned(),
        Some(lines) => lines
            .iter()
            .map(|lock| {
                format!(
                    "{:?}:{}:{}:{}:{}",
                    lock.kind,
                    lock.pid,
                    lock.start,
                    lock.len,
                    lock.dev.as_deref().unwrap_or("-")
                )
            })
            .collect::<Vec<_>>()
            .join(","),
        None => "-".to_owned(),
    };
    format!(
        "UB2VP-MEASURE leg={leg} cell={cell} point={point} holder_pid={holder_pid} sh={sh} \
         l_type={l_type} l_kind={l_kind} l_pid={l_pid} l_start={l_start} l_len={l_len} dev={dev} \
         proc_locks={proc_locks} attempts={} proc_reads={} proc_bytes={} o={} live={}",
        observation.attempts,
        observation.proc_reads,
        observation.proc_bytes,
        observer.unwrap_or("-"),
        live.map_or_else(|| "-".to_owned(), |n| n.to_string())
    )
}
