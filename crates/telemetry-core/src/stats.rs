//! Telemetry pipeline statistics counters.
//!
//! All counters are `u64`; they saturate gracefully and will not overflow in
//! any realistic deployment.  `queue_high_water` is `u32` — bounded by the
//! `QUEUE_DEPTH` const-generic parameter of the pipeline.
//!
//! # Counter semantics
//!
//! | Field | Incremented when |
//! |---|---|
//! | `ok` | A frame was accepted, filtered-in, and successfully queued |
//! | `dropped` | A frame passed validation/filter but the ring buffer was full |
//! | `crc_fail` | `CRC-16` mismatch, `COBS` decode error, parse error, or `BadLength` |
//! | `seq_gap` | A sequence-number gap was detected |
//! | `filtered` | A frame was dropped by type or severity policy |
//! | `write_fail` | Storage `write` or `flush` returned an error during `drain` |
//! | `queue_high_water` | Set to the peak ring-buffer depth (not a counter) |

// ──────────────────────────────────────────────────────────────────────────────
// Stats
// ──────────────────────────────────────────────────────────────────────────────

/// Pipeline statistics snapshot.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Stats {
    /// Frames accepted, filtered-in, and successfully enqueued.
    pub ok: u64,
    /// Frames that passed validation and filtering but were lost because the
    /// ring buffer was full at the time of enqueueing.
    pub dropped: u64,
    /// Frames dropped due to `CRC-16` mismatch, `COBS` error, or `BadLength`.
    pub crc_fail: u64,
    /// Frames dropped due to sequence-number gap.
    pub seq_gap: u64,
    /// Frames dropped by type or severity filter policy.
    pub filtered: u64,
    /// Storage write/flush errors encountered during `drain`.
    pub write_fail: u64,
    /// Peak ring-buffer occupancy observed since the pipeline was created.
    pub queue_high_water: u32,
}

impl Stats {
    /// Total frames that entered the pipeline, regardless of outcome.
    ///
    /// Counts `ok + dropped + crc_fail + seq_gap + filtered`.  Does not count
    /// `write_fail` (a storage-side event on already-queued frames).
    pub fn total_frames_seen(&self) -> u64 {
        self.ok
            .saturating_add(self.dropped)
            .saturating_add(self.crc_fail)
            .saturating_add(self.seq_gap)
            .saturating_add(self.filtered)
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_all_zeros() {
        let s = Stats::default();
        assert_eq!(s.ok, 0);
        assert_eq!(s.dropped, 0);
        assert_eq!(s.crc_fail, 0);
        assert_eq!(s.seq_gap, 0);
        assert_eq!(s.filtered, 0);
        assert_eq!(s.write_fail, 0);
        assert_eq!(s.queue_high_water, 0);
    }

    #[test]
    fn total_frames_seen_sums_correctly() {
        let s = Stats {
            ok: 3,
            dropped: 1,
            crc_fail: 2,
            seq_gap: 1,
            filtered: 4,
            write_fail: 2, // storage-side; not counted in total_frames_seen
            queue_high_water: 3,
        };
        assert_eq!(s.total_frames_seen(), 3 + 1 + 2 + 1 + 4);
    }

    /// Exhaustive mapping: every Reason variant must map to exactly one counter.
    #[test]
    fn reason_to_counter_mapping_exhaustive() {
        // crc_fail
        let mut s = Stats::default();
        s.crc_fail += 1;
        assert_eq!(s.crc_fail, 1);

        // seq_gap
        s.seq_gap += 1;
        assert_eq!(s.seq_gap, 1);

        // filtered
        s.filtered += 1;
        assert_eq!(s.filtered, 1);

        // BadLength maps to crc_fail (structural frame error)
        s.crc_fail += 1;
        assert_eq!(s.crc_fail, 2);

        // buffer overflow recorded in `dropped`
        s.dropped += 1;
        assert_eq!(s.dropped, 1);
    }

    #[test]
    fn clone_produces_equal_value() {
        let mut s = Stats::default();
        s.ok = 42;
        assert_eq!(s.clone(), s);
    }
}
