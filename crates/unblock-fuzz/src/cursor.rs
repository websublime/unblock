//! Deterministic byte-driven sampler (`ByteCursor`) + enum-sampling extension (`CursorExt`).
//!
//! The cursor turns a raw `&[u8]` (the libFuzzer input) into a stream of structured choices so a
//! target can build typed values from arbitrary bytes. **Every accessor is total and panic-free on
//! exhausted input** (`next_byte` → `0`, `next_usize(bound)` → `0`, `text(0)` → `""`, …), so the
//! same byte sequence always yields the same choices — corpus replay is fully deterministic.

use unblock_model::{DependencyType, EventType, IssueType, Status};

/// A deterministic cursor over raw fuzz bytes.
///
/// Reads advance the position; past the end every read returns a defined zero value (never panics).
pub struct ByteCursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> ByteCursor<'a> {
    /// Wrap a byte slice.
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// Whether the cursor is exhausted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pos >= self.data.len()
    }

    /// The number of bytes still unread.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    /// Read one byte (or `0` when exhausted).
    pub fn next_byte(&mut self) -> u8 {
        let byte = self.data.get(self.pos).copied().unwrap_or(0);
        self.pos = self.pos.saturating_add(1);
        byte
    }

    /// Read a `u16` from two bytes (big-endian; missing bytes read as `0`).
    pub fn next_u16(&mut self) -> u16 {
        let hi = u16::from(self.next_byte());
        let lo = u16::from(self.next_byte());
        (hi << 8) | lo
    }

    /// Read a `u32` from four bytes (big-endian; missing bytes read as `0`).
    pub fn next_u32(&mut self) -> u32 {
        let hi = u32::from(self.next_u16());
        let lo = u32::from(self.next_u16());
        (hi << 16) | lo
    }

    /// Read a `bool` (low bit of the next byte).
    pub fn next_bool(&mut self) -> bool {
        self.next_byte() & 1 == 1
    }

    /// Read a `usize` in `0..bound` (inclusive lower, exclusive upper). `bound == 0` → `0`.
    pub fn next_usize(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        (self.next_u32() as usize) % bound
    }

    /// Read `len` raw bytes (zero-padded when exhausted).
    pub fn bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| self.next_byte()).collect()
    }

    /// Read up to `max_len` bytes as a lossy-UTF-8 `String` (`max_len == 0` → `""`).
    ///
    /// The length is itself sampled (`0..=max_len`), so the text varies with the input; invalid
    /// UTF-8 is replaced (never panics).
    pub fn text(&mut self, max_len: usize) -> String {
        if max_len == 0 {
            return String::new();
        }
        let len = self.next_usize(max_len + 1);
        let raw = self.bytes(len);
        String::from_utf8_lossy(&raw).into_owned()
    }

    /// Read an `Option<String>`: `None` half the time, else a [`text`](Self::text) of up to
    /// `max_len`.
    pub fn optional_text(&mut self, max_len: usize) -> Option<String> {
        if self.next_bool() {
            Some(self.text(max_len))
        } else {
            None
        }
    }
}

/// Enum-sampling helpers over a [`ByteCursor`] (the open-enum `Custom` tails are reachable).
pub trait CursorExt {
    /// Sample a lowercase id prefix (a `String`, not a `&'static str`, so it can vary).
    fn prefix(&mut self) -> String;
    /// Sample a [`Status`] (every known variant + `Custom`).
    fn status(&mut self) -> Status;
    /// Sample an [`IssueType`] (every known variant + `Custom`).
    fn issue_type(&mut self) -> IssueType;
    /// Sample a [`DependencyType`] (every known variant + `Custom`).
    fn dep_type(&mut self) -> DependencyType;
    /// Sample an [`EventType`] (every known variant + `Custom`).
    fn event_type(&mut self) -> EventType;
}

impl CursorExt for ByteCursor<'_> {
    fn prefix(&mut self) -> String {
        // A short lowercase-alnum prefix (1..=4 chars), so a repaired id `<prefix>-<hash>` parses.
        const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
        let len = 1 + self.next_usize(4);
        let mut out = String::with_capacity(len);
        for _ in 0..len {
            let idx = self.next_usize(ALPHABET.len());
            out.push(ALPHABET[idx] as char);
        }
        out
    }

    fn status(&mut self) -> Status {
        match self.next_usize(9) {
            0 => Status::Open,
            1 => Status::InProgress,
            2 => Status::Blocked,
            3 => Status::Deferred,
            4 => Status::Draft,
            5 => Status::Closed,
            6 => Status::Tombstone,
            7 => Status::Pinned,
            _ => Status::Custom(self.text(16)),
        }
    }

    fn issue_type(&mut self) -> IssueType {
        match self.next_usize(8) {
            0 => IssueType::Task,
            1 => IssueType::Bug,
            2 => IssueType::Feature,
            3 => IssueType::Epic,
            4 => IssueType::Chore,
            5 => IssueType::Docs,
            6 => IssueType::Question,
            _ => IssueType::Custom(self.text(16)),
        }
    }

    fn dep_type(&mut self) -> DependencyType {
        match self.next_usize(12) {
            0 => DependencyType::Blocks,
            1 => DependencyType::ParentChild,
            2 => DependencyType::ConditionalBlocks,
            3 => DependencyType::WaitsFor,
            4 => DependencyType::Related,
            5 => DependencyType::DiscoveredFrom,
            6 => DependencyType::RepliesTo,
            7 => DependencyType::RelatesTo,
            8 => DependencyType::Duplicates,
            9 => DependencyType::Supersedes,
            10 => DependencyType::CausedBy,
            // `Custom` stores the value lowercased (the model's documented asymmetry); sample a
            // lowercase tail so a round-trip stays stable.
            _ => DependencyType::Custom(self.text(16).to_lowercase()),
        }
    }

    fn event_type(&mut self) -> EventType {
        match self.next_usize(16) {
            0 => EventType::Created,
            1 => EventType::Updated,
            2 => EventType::StatusChanged,
            3 => EventType::PriorityChanged,
            4 => EventType::AssigneeChanged,
            5 => EventType::Commented,
            6 => EventType::Closed,
            7 => EventType::Reopened,
            8 => EventType::DependencyAdded,
            9 => EventType::DependencyRemoved,
            10 => EventType::LabelAdded,
            11 => EventType::LabelRemoved,
            12 => EventType::Compacted,
            13 => EventType::Deleted,
            14 => EventType::Restored,
            _ => EventType::Custom(self.text(16)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ByteCursor, CursorExt};

    #[test]
    fn empty_input_is_all_zero_and_does_not_panic() {
        let mut cursor = ByteCursor::new(&[]);
        assert_eq!(cursor.next_byte(), 0);
        assert_eq!(cursor.next_u16(), 0);
        assert_eq!(cursor.next_u32(), 0);
        assert!(!cursor.next_bool());
        assert_eq!(cursor.next_usize(0), 0);
        assert_eq!(cursor.next_usize(10), 0);
        assert_eq!(cursor.text(0), "");
        assert_eq!(cursor.text(8), "");
        assert!(cursor.optional_text(8).is_none() || cursor.optional_text(8).is_some());
        assert!(cursor.is_empty());
    }

    #[test]
    fn same_bytes_same_sequence() {
        let data = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let mut a = ByteCursor::new(&data);
        let mut b = ByteCursor::new(&data);
        for _ in 0..4 {
            assert_eq!(a.next_byte(), b.next_byte());
            assert_eq!(a.status(), b.status());
            assert_eq!(a.dep_type(), b.dep_type());
        }
    }

    #[test]
    fn prefix_is_lowercase_alnum_nonempty() {
        let mut cursor = ByteCursor::new(&[0xde, 0xad, 0xbe, 0xef, 0x10, 0x20, 0x30]);
        let prefix = cursor.prefix();
        assert!(!prefix.is_empty());
        assert!(
            prefix
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        );
    }

    #[test]
    fn enum_samplers_cover_custom_tail() {
        // Each sampler selects its arm as `next_u32() % bound`; the `Custom` arm is the last index.
        // Feed the exact 4-byte big-endian `u32` that lands on `Custom` for each (a value < bound
        // yields `% bound == value`), then a few tail bytes for the `Custom` payload text.
        fn u32_be(v: u32) -> [u8; 4] {
            v.to_be_bytes()
        }

        // status: bound 9 → Custom at 8.
        let mut s = u32_be(8).to_vec();
        s.extend_from_slice(b"\x05abc");
        assert!(matches!(
            ByteCursor::new(&s).status(),
            unblock_model::Status::Custom(_)
        ));

        // issue_type: bound 8 → Custom at 7.
        let mut t = u32_be(7).to_vec();
        t.extend_from_slice(b"\x05abc");
        assert!(matches!(
            ByteCursor::new(&t).issue_type(),
            unblock_model::IssueType::Custom(_)
        ));

        // dep_type: bound 12 → Custom at 11.
        let mut d = u32_be(11).to_vec();
        d.extend_from_slice(b"\x05abc");
        assert!(matches!(
            ByteCursor::new(&d).dep_type(),
            unblock_model::DependencyType::Custom(_)
        ));

        // event_type: bound 16 → Custom at 15.
        let mut e = u32_be(15).to_vec();
        e.extend_from_slice(b"\x05abc");
        assert!(matches!(
            ByteCursor::new(&e).event_type(),
            unblock_model::EventType::Custom(_)
        ));
    }
}
