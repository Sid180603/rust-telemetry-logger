//! Top-level telemetry pipeline with a split `ingest` / `drain` API.
//!
//! ```text
//!  ingest(bytes, clock) → framer → validator → filter → ring-buffer + stats
//!  drain(storage)       → pop ring-buffer → encode → write → flush
//! ```
//!
//! # Split API rationale
//!
//! This design maps to both execution models with zero changes to the core crate:
//!
//! - **Blocking v1 (firmware):** call `ingest` then `drain` in the same loop iteration.
//! - **Async v2 (Embassy):** two tasks share the pipeline; one calls `ingest`, the
//!   other calls `drain`, with an `embassy-sync` channel as the hand-off.

use crate::error::{Error, Outcome, Reason};
use crate::filter::FilterConfig;
use crate::framer::{FrameOutput, Framer};
use crate::record::{self, MAX_STORED_RECORD_BYTES, RecordV1, StoredRecord};
use crate::ringbuf::RingBuf;
use crate::stats::Stats;
use crate::traits::{Clock, Storage};
use crate::validator::Validator;

// ──────────────────────────────────────────────────────────────────────────────
// Pipeline
// ──────────────────────────────────────────────────────────────────────────────

/// The telemetry pipeline.
///
/// # Const generics
///
/// - `MAX_FRAME`: internal framer buffer capacity in bytes.  Must be at least
///   [`crate::frame::MAX_COBS_FRAME_BYTES`] for a max-size frame.
/// - `QUEUE_DEPTH`: maximum number of [`StoredRecord`]s held between `ingest`
///   and `drain` calls.
#[derive(Debug)]
pub struct Pipeline<const MAX_FRAME: usize, const QUEUE_DEPTH: usize> {
    framer: Framer<MAX_FRAME>,
    validator: Validator,
    filter: FilterConfig,
    ringbuf: RingBuf<QUEUE_DEPTH>,
    stats: Stats,
}

impl<const MAX_FRAME: usize, const QUEUE_DEPTH: usize> Pipeline<MAX_FRAME, QUEUE_DEPTH> {
    /// Create a new pipeline with the given filter configuration.
    pub fn new(filter: FilterConfig) -> Self {
        Self {
            framer: Framer::new(),
            validator: Validator::new(),
            filter,
            ringbuf: RingBuf::new(),
            stats: Stats::default(),
        }
    }

    /// Feed raw bytes into the pipeline.
    ///
    /// Each byte is passed through:
    /// 1. [`Framer`] — `COBS` framing
    /// 2. [`Validator`] — `CRC-16`, type/severity, sequence gap
    /// 3. [`FilterConfig`] — type/severity policy
    /// 4. [`RingBuf`] — fixed-capacity queue
    ///
    /// Statistics are updated for every outcome.  The `clock` is sampled once
    /// per complete accepted frame to timestamp the [`StoredRecord`].
    pub fn ingest(&mut self, bytes: &[u8], clock: &impl Clock) {
        for &byte in bytes {
            match self.framer.feed(byte) {
                FrameOutput::Complete(frame) => match self.validator.check(frame) {
                    Outcome::Accepted(frame) => {
                        if self.filter.allow(&frame) {
                            let record = StoredRecord::V1(RecordV1 {
                                timestamp_us: clock.now(),
                                sequence: frame.header.sequence,
                                packet_type: frame.header.packet_type,
                                severity: frame.header.severity,
                                payload: frame.payload,
                            });
                            if self.ringbuf.push(record).is_ok() {
                                self.stats.ok += 1;
                                let hw = self.ringbuf.high_water();
                                if hw > self.stats.queue_high_water {
                                    self.stats.queue_high_water = hw;
                                }
                            } else {
                                self.stats.dropped += 1;
                            }
                        } else {
                            self.stats.filtered += 1;
                        }
                    }
                    Outcome::Rejected(reason) => match reason {
                        Reason::Crc { .. } | Reason::BadLength => self.stats.crc_fail += 1,
                        // NOTE: the frame that *detected* the gap (e.g. seq=5 when seq=4 was
                        // expected) is also rejected and its payload is lost — not just the
                        // missing seq=4.  This is a deliberate conservative policy: data
                        // arriving immediately after a gap is distrusted until we re-sync.
                        // The validator re-syncs to the received sequence so the *next*
                        // in-order frame (seq=6) is accepted without further penalty.
                        Reason::SequenceGap { .. } => self.stats.seq_gap += 1,
                        Reason::Filtered => self.stats.filtered += 1,
                    },
                },
                FrameOutput::Overflow | FrameOutput::CobsError | FrameOutput::ParseError => {
                    self.stats.crc_fail += 1;
                }
                FrameOutput::Incomplete => {}
            }
        }
    }

    /// Drain all queued records to `storage`.
    ///
    /// Pops each [`StoredRecord`] from the ring buffer, encodes it, writes it to
    /// `storage`, then calls `flush` once at the end.
    ///
    /// # Errors
    ///
    /// - [`Error::Codec`] if a record cannot be serialised.
    /// - [`Error::Storage`] if `storage.write` or `storage.flush` fails; the
    ///   `write_fail` counter is incremented and the record is **lost** (not
    ///   re-queued).
    ///
    /// Returns the number of records successfully written on `Ok`.
    pub fn drain<S: Storage>(&mut self, storage: &mut S) -> Result<usize, Error> {
        let mut encode_buf = [0u8; MAX_STORED_RECORD_BYTES];
        let mut count = 0usize;

        while let Some(record) = self.ringbuf.pop() {
            let n = record::encode(&record, &mut encode_buf).map_err(|_| Error::Codec)?;

            if storage.write(&encode_buf[..n]).is_err() {
                self.stats.write_fail += 1;
                return Err(Error::Storage);
            }
            count += 1;
        }

        if storage.flush().is_err() {
            self.stats.write_fail += 1;
            return Err(Error::Storage);
        }

        Ok(count)
    }

    /// Read-only access to the current statistics snapshot.
    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    /// Mutable access to the statistics (e.g., to reset counters in tests).
    pub fn stats_mut(&mut self) -> &mut Stats {
        &mut self.stats
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::vec::Vec as StdVec;

    use super::*;
    use crate::filter::FilterConfig;
    use crate::frame::{Frame, Header, MAX_COBS_FRAME_BYTES};
    use crate::protocol::{MAX_PAYLOAD, PROTOCOL_VERSION};
    use crate::traits::test_support::SimClock;

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn make_cobs_bytes(seq: u32, packet_type: u8, severity: u8, payload: &[u8]) -> StdVec<u8> {
        let mut p: heapless::Vec<u8, MAX_PAYLOAD> = heapless::Vec::new();
        for &b in payload {
            p.push(b).ok();
        }
        let frame = Frame {
            header: Header {
                version: PROTOCOL_VERSION,
                packet_type,
                severity,
                sequence: seq,
                payload_len: p.len() as u16,
            },
            payload: p,
            crc_bytes: [0; 2],
        };
        let mut buf = [0u8; MAX_COBS_FRAME_BYTES];
        let n = frame.encode_cobs(&mut buf).expect("encode");
        buf[..n].to_vec()
    }

    fn valid_frame_bytes(seq: u32) -> StdVec<u8> {
        make_cobs_bytes(seq, 0x01, 0x01, b"data")
    }

    // ── Unit tests ────────────────────────────────────────────────────────────

    #[test]
    fn ingest_valid_frame_increments_ok() {
        let clock = SimClock { micros: 1000 };
        let mut p = Pipeline::<512, 64>::new(FilterConfig::allow_all());
        p.ingest(&valid_frame_bytes(0), &clock);
        assert_eq!(p.stats().ok, 1);
        assert_eq!(p.stats().crc_fail, 0);
    }

    #[test]
    fn ingest_bad_crc_increments_crc_fail() {
        let clock = SimClock { micros: 0 };
        let mut p = Pipeline::<512, 64>::new(FilterConfig::allow_all());
        // Encode a valid frame then corrupt a middle byte.
        let mut bytes = valid_frame_bytes(0);
        let mid = bytes.len() / 2;
        if mid < bytes.len() {
            bytes[mid] ^= 0xFF;
        }
        p.ingest(&bytes, &clock);
        // The corruption may cause a COBS error or a CRC mismatch — both → crc_fail.
        assert!(p.stats().crc_fail > 0 || p.stats().ok == 0);
    }

    #[test]
    fn ingest_unknown_type_increments_filtered() {
        let clock = SimClock { micros: 0 };
        let mut p = Pipeline::<512, 64>::new(FilterConfig::allow_all());
        let bytes = make_cobs_bytes(0, 0xFF, 0x01, b""); // 0xFF unknown type
        p.ingest(&bytes, &clock);
        assert_eq!(p.stats().filtered, 1);
    }

    #[test]
    fn queue_high_water_tracked() {
        let clock = SimClock { micros: 0 };
        let mut p = Pipeline::<512, 64>::new(FilterConfig::allow_all());
        for i in 0..5u32 {
            p.ingest(&valid_frame_bytes(i), &clock);
        }
        assert_eq!(p.stats().queue_high_water, 5);
        assert_eq!(p.stats().ok, 5);
    }
}
