//! `COBS`-framer: converts a raw byte stream into [`Frame`]s.
//!
//! # State machine
//!
//! ```text
//!  Collecting ──(byte)──► Collecting  (accumulate)
//!  Collecting ──(full)──► Discarding  (overflow)
//!  Collecting ──(0x00)──► Collecting  (process + reset)
//!  Discarding ──(any) ──► Discarding  (skip)
//!  Discarding ──(0x00)──► Collecting  (resync)
//! ```
//!
//! - `CRC-16` is **not** checked here — that is the validator's responsibility.
//! - `COBS` destuffing uses the `cobs` crate (`cobs::decode_in_place`).
//! - In `cobs` 0.3, `decode_in_place` returns `Result<usize, _>`, not `Option`.

use heapless::Vec;

use crate::frame::{Frame, Header};
use crate::protocol::{CRC16_LEN, HEADER_LEN, MAX_PAYLOAD, MIN_FRAME_LEN};

// ──────────────────────────────────────────────────────────────────────────────
// State
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
enum FramerState {
    /// Normal operation: accumulating bytes between `0x00` delimiters.
    Collecting,
    /// Buffer overflowed; discarding bytes until the next `0x00`.
    Discarding,
}

// ──────────────────────────────────────────────────────────────────────────────
// FrameOutput
// ──────────────────────────────────────────────────────────────────────────────

/// Result returned by [`Framer::feed`] for each input byte.
#[must_use]
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)] // Frame is large by design; no alloc in no_std
pub enum FrameOutput {
    /// No complete frame yet; keep feeding bytes.
    Incomplete,
    /// A complete frame was decoded.  `CRC-16` is not yet verified.
    Complete(Frame),
    /// Frame exceeded the framer buffer capacity `N`; dropped.
    Overflow,
    /// `COBS` decoding failed (stream corruption); frame dropped.
    CobsError,
    /// Decoded bytes do not match the expected frame structure; frame dropped.
    ParseError,
}

// ──────────────────────────────────────────────────────────────────────────────
// Framer
// ──────────────────────────────────────────────────────────────────────────────

/// Byte-stream framer: accepts bytes one at a time, emits [`FrameOutput`].
///
/// `N` is the internal buffer capacity in bytes.  It must be large enough to
/// hold the `COBS`-encoded representation of the largest expected frame
/// (see [`crate::frame::MAX_COBS_FRAME_BYTES`]).
#[derive(Debug)]
pub struct Framer<const N: usize> {
    buf: Vec<u8, N>,
    state: FramerState,
    /// Number of times the framer had to discard bytes to re-synchronise.
    pub resync_count: u32,
}

impl<const N: usize> Framer<N> {
    /// Create a new, empty framer ready to receive bytes.
    pub fn new() -> Self {
        Self {
            buf: Vec::new(),
            state: FramerState::Collecting,
            resync_count: 0,
        }
    }

    /// Feed one byte into the framer.
    ///
    /// Returns a [`FrameOutput`] indicating whether a complete frame is ready.
    pub fn feed(&mut self, byte: u8) -> FrameOutput {
        match self.state {
            FramerState::Discarding => {
                if byte == 0x00 {
                    self.buf.clear();
                    self.state = FramerState::Collecting;
                }
                FrameOutput::Incomplete
            }
            FramerState::Collecting => {
                if byte != 0x00 {
                    if self.buf.push(byte).is_err() {
                        // Buffer full — transition to Discarding.
                        self.state = FramerState::Discarding;
                        self.resync_count = self.resync_count.saturating_add(1);
                        self.buf.clear();
                        return FrameOutput::Overflow;
                    }
                    FrameOutput::Incomplete
                } else {
                    // Received 0x00 delimiter.
                    if self.buf.is_empty() {
                        // Leading delimiter or consecutive delimiters — skip silently.
                        return FrameOutput::Incomplete;
                    }
                    let result = self.process();
                    self.buf.clear();
                    result
                }
            }
        }
    }

    /// Process the accumulated bytes once a `0x00` delimiter is received.
    fn process(&mut self) -> FrameOutput {
        // COBS-decode in place.  cobs 0.3 returns Result<usize, _>.
        let Ok(decoded_len) = cobs::decode_in_place(self.buf.as_mut_slice()) else {
            self.resync_count = self.resync_count.saturating_add(1);
            return FrameOutput::CobsError;
        };

        // Must have at least MIN_FRAME_LEN decoded bytes.
        if decoded_len < MIN_FRAME_LEN {
            return FrameOutput::ParseError;
        }

        // Read payload length from header (stored as u16 LE at bytes 7-8).
        let payload_len_u16 = u16::from_le_bytes([self.buf[7], self.buf[8]]);
        let payload_len = usize::from(payload_len_u16);

        // Reject payloads that exceed the defined maximum.
        if payload_len > MAX_PAYLOAD {
            return FrameOutput::ParseError;
        }

        // Structural check: decoded_len must equal HEADER_LEN + payload_len + CRC16_LEN.
        if decoded_len != HEADER_LEN + payload_len + CRC16_LEN {
            return FrameOutput::ParseError;
        }

        // Parse header.
        let sequence = u32::from_le_bytes([self.buf[3], self.buf[4], self.buf[5], self.buf[6]]);
        let header = Header {
            version: self.buf[0],
            packet_type: self.buf[1],
            severity: self.buf[2],
            sequence,
            payload_len: payload_len_u16,
        };

        // Copy payload.
        let mut payload: Vec<u8, MAX_PAYLOAD> = Vec::new();
        if payload
            .extend_from_slice(&self.buf[HEADER_LEN..HEADER_LEN + payload_len])
            .is_err()
        {
            // payload_len <= MAX_PAYLOAD was checked above; this should not happen.
            return FrameOutput::ParseError;
        }

        // Extract CRC bytes (last two decoded bytes).
        let crc_start = HEADER_LEN + payload_len;
        let crc_bytes = [self.buf[crc_start], self.buf[crc_start + 1]];

        FrameOutput::Complete(Frame {
            header,
            payload,
            crc_bytes,
        })
    }
}

impl<const N: usize> Default for Framer<N> {
    fn default() -> Self {
        Self::new()
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::vec::Vec as StdVec;

    use super::*;
    use crate::frame::MAX_COBS_FRAME_BYTES;
    use crate::protocol::PROTOCOL_VERSION;

    /// Build a COBS-encoded wire frame and return the bytes.
    fn make_cobs_frame(seq: u32, payload: &[u8]) -> heapless::Vec<u8, MAX_COBS_FRAME_BYTES> {
        let mut p: Vec<u8, MAX_PAYLOAD> = Vec::new();
        for &b in payload {
            p.push(b).ok();
        }
        let frame = Frame {
            header: Header {
                version: PROTOCOL_VERSION,
                packet_type: 0x01,
                severity: 0x01,
                sequence: seq,
                payload_len: p.len() as u16,
            },
            payload: p,
            crc_bytes: [0; 2],
        };
        let mut buf = [0u8; MAX_COBS_FRAME_BYTES];
        let n = frame.encode_cobs(&mut buf).expect("frame fits in buffer");
        let mut out: heapless::Vec<u8, MAX_COBS_FRAME_BYTES> = heapless::Vec::new();
        for &b in &buf[..n] {
            out.push(b).ok();
        }
        out
    }

    fn feed_all<const N: usize>(framer: &mut Framer<N>, bytes: &[u8]) -> Vec<FrameOutput, 16> {
        let mut results: Vec<FrameOutput, 16> = Vec::new();
        for &b in bytes {
            let r = framer.feed(b);
            if !matches!(r, FrameOutput::Incomplete) {
                results.push(r).ok();
            }
        }
        results
    }

    #[test]
    fn clean_parse_returns_complete() {
        let encoded = make_cobs_frame(0, b"hello");
        let mut framer = Framer::<512>::new();
        let results = feed_all(&mut framer, &encoded);
        assert_eq!(results.len(), 1);
        let FrameOutput::Complete(frame) = &results[0] else {
            panic!("expected Complete, got {:?}", results[0]);
        };
        assert_eq!(frame.header.sequence, 0);
        assert_eq!(&frame.payload[..], b"hello");
    }

    #[test]
    fn split_feed_completes_on_last_byte() {
        let encoded = make_cobs_frame(1, b"AB");
        let mut framer = Framer::<512>::new();

        // Feed all bytes except the last (the 0x00 delimiter)
        for &b in &encoded[..encoded.len() - 1] {
            let r = framer.feed(b);
            assert!(
                matches!(r, FrameOutput::Incomplete),
                "expected Incomplete, got {r:?}"
            );
        }
        // Feed the delimiter
        let r = framer.feed(encoded[encoded.len() - 1]);
        assert!(
            matches!(r, FrameOutput::Complete(_)),
            "expected Complete, got {r:?}"
        );
    }

    #[test]
    fn leading_garbage_then_valid_frame() {
        let garbage = [0x11u8, 0x22, 0x33, 0x44]; // non-zero bytes, no frame structure
        let encoded = make_cobs_frame(2, b"ok");
        let mut framer = Framer::<512>::new();

        // Feed garbage followed by 0x00 (forces garbage processing → ParseError or CobsError)
        let mut all_bytes: StdVec<u8> = garbage.to_vec();
        all_bytes.push(0x00);
        all_bytes.extend_from_slice(&encoded);

        let results = feed_all(&mut framer, &all_bytes);

        // First result: garbage decoded to error
        assert!(
            results.len() >= 2,
            "expected at least 2 results (error + complete), got {:?}",
            results.len()
        );
        // Last result: the valid frame
        assert!(
            matches!(results.last(), Some(FrameOutput::Complete(_))),
            "last result must be Complete, got {:?}",
            results.last()
        );
    }

    #[test]
    fn overflow_then_recovery() {
        // Framer with tiny buffer (32 bytes) — max valid frame is larger.
        let mut framer = Framer::<32>::new();

        // Feed 33 non-zero bytes to overflow the buffer.
        for _ in 0..33 {
            let _ = framer.feed(0x11);
        }
        // Now feed 0x00 to trigger Overflow result
        // (Overflow was already returned when the 33rd byte was pushed)
        // Reset with 0x00
        let _ = framer.feed(0x00);

        // Now feed a small valid frame (fits in 32 bytes).
        let encoded = make_cobs_frame(3, b""); // empty payload: raw_len = 11, COBS <= 13 bytes
        let results = feed_all(&mut framer, &encoded);
        assert!(
            matches!(results.last(), Some(FrameOutput::Complete(_))),
            "should recover after overflow, got {:?}",
            results
        );
    }

    #[test]
    fn truncated_frame_stays_incomplete() {
        let encoded = make_cobs_frame(4, b"data");
        let mut framer = Framer::<512>::new();

        // Feed everything except the final 0x00 delimiter.
        for &b in &encoded[..encoded.len() - 1] {
            let r = framer.feed(b);
            assert!(
                matches!(r, FrameOutput::Incomplete),
                "expected Incomplete before delimiter"
            );
        }
    }

    #[test]
    fn back_to_back_two_frames() {
        let enc0 = make_cobs_frame(0, b"first");
        let enc1 = make_cobs_frame(1, b"second");
        let mut all_bytes: StdVec<u8> = enc0.to_vec();
        all_bytes.extend_from_slice(&enc1);

        let mut framer = Framer::<512>::new();
        let results = feed_all(&mut framer, &all_bytes);

        assert_eq!(results.len(), 2, "expected 2 Complete results");
        let FrameOutput::Complete(f0) = &results[0] else {
            panic!()
        };
        let FrameOutput::Complete(f1) = &results[1] else {
            panic!()
        };
        assert_eq!(f0.header.sequence, 0);
        assert_eq!(f1.header.sequence, 1);
    }

    #[test]
    fn leading_delimiter_is_skipped() {
        // Leading 0x00 before a valid frame must not cause issues.
        let encoded = make_cobs_frame(5, b"x");
        let mut all_bytes: StdVec<u8> = std::vec![0x00, 0x00];
        all_bytes.extend_from_slice(&encoded);

        let mut framer = Framer::<512>::new();
        let results = feed_all(&mut framer, &all_bytes);
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], FrameOutput::Complete(_)));
    }

    #[test]
    fn resync_count_increments_on_overflow() {
        let mut framer = Framer::<8>::new(); // tiny buffer
        for _ in 0..9 {
            let _ = framer.feed(0x11);
        }
        assert_eq!(framer.resync_count, 1);
    }
}
