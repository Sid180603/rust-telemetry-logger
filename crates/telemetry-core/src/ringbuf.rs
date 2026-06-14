//! Fixed-capacity ring buffer wrapping [`heapless::Deque`].
//!
//! Tracks overflow count (frames lost because the buffer was full) and
//! high-water mark (peak occupancy since creation).

use heapless::Deque;

use crate::record::StoredRecord;

// ──────────────────────────────────────────────────────────────────────────────
// RingBuf
// ──────────────────────────────────────────────────────────────────────────────

/// Fixed-capacity FIFO queue of [`StoredRecord`]s.
///
/// `N` is the maximum number of records the queue can hold simultaneously.
/// When the queue is full, [`RingBuf::push`] increments the overflow counter
/// and returns `Err(())` without dropping the oldest record (the caller
/// decides what to count/report).
#[derive(Debug)]
pub struct RingBuf<const N: usize> {
    queue: Deque<StoredRecord, N>,
    overflow_count: u64,
    high_water: u32,
}

impl<const N: usize> RingBuf<N> {
    /// Create an empty ring buffer.
    pub fn new() -> Self {
        Self {
            queue: Deque::new(),
            overflow_count: 0,
            high_water: 0,
        }
    }

    /// Push a record onto the back of the queue.
    ///
    /// On success the high-water mark is updated.  If the queue is full,
    /// `overflow_count` is incremented and `Err(())` is returned — the record
    /// is **not** stored.
    ///
    /// # Errors
    ///
    /// Returns `Err(())` when the queue is at capacity.
    #[allow(clippy::result_unit_err)] // () is the correct sentinel; no info to carry
    pub fn push(&mut self, record: StoredRecord) -> Result<(), ()> {
        if self.queue.is_full() {
            self.overflow_count += 1;
            return Err(());
        }
        // Queue is not full — push_back always succeeds here.
        self.queue.push_back(record).map_err(|_| ())?;
        // QUEUE_DEPTH is always a small const generic (fits in u32).
        #[allow(clippy::cast_possible_truncation)]
        let len = self.queue.len() as u32;
        if len > self.high_water {
            self.high_water = len;
        }
        Ok(())
    }

    /// Pop the oldest record from the front of the queue.
    ///
    /// Returns `None` if the queue is empty.
    pub fn pop(&mut self) -> Option<StoredRecord> {
        self.queue.pop_front()
    }

    /// Current number of records in the queue.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Returns `true` if the queue contains no records.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Total number of records that were dropped because the queue was full.
    pub fn overflow_count(&self) -> u64 {
        self.overflow_count
    }

    /// Peak queue length observed since this instance was created.
    pub fn high_water(&self) -> u32 {
        self.high_water
    }
}

impl<const N: usize> Default for RingBuf<N> {
    fn default() -> Self {
        Self::new()
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::RecordV1;

    fn make_record(seq: u32) -> StoredRecord {
        StoredRecord::V1(RecordV1 {
            timestamp_us: u64::from(seq),
            sequence: seq,
            packet_type: 0x01,
            severity: 0x01,
            payload: heapless::Vec::new(),
        })
    }

    #[test]
    fn fifo_order() {
        let mut rb: RingBuf<4> = RingBuf::new();
        rb.push(make_record(10)).unwrap();
        rb.push(make_record(20)).unwrap();
        rb.push(make_record(30)).unwrap();

        let StoredRecord::V1(r) = rb.pop().unwrap();
        assert_eq!(r.sequence, 10);
        let StoredRecord::V1(r) = rb.pop().unwrap();
        assert_eq!(r.sequence, 20);
        let StoredRecord::V1(r) = rb.pop().unwrap();
        assert_eq!(r.sequence, 30);
        assert!(rb.pop().is_none());
    }

    #[test]
    fn full_returns_err_and_increments_overflow() {
        let mut rb: RingBuf<2> = RingBuf::new();
        assert!(rb.push(make_record(0)).is_ok());
        assert!(rb.push(make_record(1)).is_ok());
        // Queue is now full.
        assert!(rb.push(make_record(2)).is_err());
        assert_eq!(rb.overflow_count(), 1);
        assert_eq!(rb.len(), 2);
    }

    #[test]
    fn high_water_tracks_peak() {
        let mut rb: RingBuf<8> = RingBuf::new();
        // Push 3
        for i in 0..3 {
            rb.push(make_record(i)).unwrap();
        }
        assert_eq!(rb.high_water(), 3);
        // Pop all
        while rb.pop().is_some() {}
        assert_eq!(rb.high_water(), 3); // peak is preserved
        // Push 5
        for i in 0..5 {
            rb.push(make_record(i)).unwrap();
        }
        assert_eq!(rb.high_water(), 5);
    }

    #[test]
    fn len_and_is_empty() {
        let mut rb: RingBuf<4> = RingBuf::new();
        assert!(rb.is_empty());
        rb.push(make_record(0)).unwrap();
        assert_eq!(rb.len(), 1);
        assert!(!rb.is_empty());
        rb.pop();
        assert!(rb.is_empty());
    }

    #[test]
    fn multiple_overflows_counted() {
        let mut rb: RingBuf<1> = RingBuf::new();
        rb.push(make_record(0)).unwrap(); // fills it
        rb.push(make_record(1)).unwrap_err(); // overflow 1
        rb.push(make_record(2)).unwrap_err(); // overflow 2
        assert_eq!(rb.overflow_count(), 2);
    }
}
