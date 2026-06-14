//! Stateful validator: `CRC-16/IBM-SDLC` wire check, length sanity, and sequence-gap detection.
//!
//! The validator is *stateful* because it tracks the last-seen sequence number
//! to detect gaps across successive calls.
//!
//! # Check order
//!
//! 1. `CRC-16/IBM-SDLC` — computed over all header fields + payload; compared with
//!    the trailing `crc_bytes` from the wire.
//! 2. Known [`crate::protocol::PacketType`] — unrecognised type byte → `Reason::Filtered`.
//! 3. Known [`crate::protocol::Severity`] — unrecognised severity byte → `Reason::Filtered`.
//! 4. Length sanity — `payload_len` field vs actual payload length (defensive).
//! 5. Sequence gap — wrap-aware comparison with the last accepted sequence number.
//!
//! Rejections are [`Outcome::Rejected`], not errors.

use crc::{CRC_16_IBM_SDLC, Crc};

use crate::error::{Outcome, Reason};
use crate::frame::Frame;
use crate::protocol::{PacketType, Severity};

// ──────────────────────────────────────────────────────────────────────────────
// Validator
// ──────────────────────────────────────────────────────────────────────────────

/// Stateful frame validator.
///
/// Maintains the last accepted sequence number to detect gaps.  Create one
/// instance per logical source stream; reset by creating a new instance.
#[derive(Debug, Clone)]
pub struct Validator {
    last_seq: Option<u32>,
}

impl Validator {
    /// Create a new validator; the first frame it sees is always accepted
    /// (no prior sequence number to compare against).
    pub fn new() -> Self {
        Self { last_seq: None }
    }

    /// Validate `frame`, returning [`Outcome::Accepted`] or [`Outcome::Rejected`].
    ///
    /// On acceptance, the internal sequence state is updated.  On rejection with
    /// [`Reason::SequenceGap`], the state is also updated (we re-sync to the
    /// received sequence number so future frames can be accepted).
    pub fn check(&mut self, frame: Frame) -> Outcome<Frame> {
        // ── 1. CRC-16/IBM-SDLC ────────────────────────────────────────────────
        // DELIBERATELY INDEPENDENT of Frame::compute_crc(): the validator recomputes
        // the wire CRC from raw header+payload bytes as a second witness.  This is
        // defense-in-depth — a hand-coded mistake in the encoder’s field-serialization
        // order cannot silently pass validation here.  Both sides use the same
        // CRC_16_IBM_SDLC primitive but independently serialize the fields, so they
        // guard against each other’s field-ordering bugs.
        // The test `validator_crc_matches_frame_compute_crc` enforces agreement.
        let crc16 = Crc::<u16>::new(&CRC_16_IBM_SDLC);
        let mut digest = crc16.digest();
        digest.update(&[
            frame.header.version,
            frame.header.packet_type,
            frame.header.severity,
        ]);
        digest.update(&frame.header.sequence.to_le_bytes());
        digest.update(&frame.header.payload_len.to_le_bytes());
        digest.update(&frame.payload);
        let computed_crc = digest.finalize();
        let stored_crc = u16::from_le_bytes(frame.crc_bytes);
        if computed_crc != stored_crc {
            return Outcome::Rejected(Reason::Crc {
                expected: computed_crc,
                actual: stored_crc,
            });
        }

        // ── 2. Known PacketType ───────────────────────────────────────────────
        if PacketType::try_from(frame.header.packet_type).is_err() {
            return Outcome::Rejected(Reason::Filtered);
        }

        // ── 3. Known Severity ─────────────────────────────────────────────────
        if Severity::try_from(frame.header.severity).is_err() {
            return Outcome::Rejected(Reason::Filtered);
        }

        // ── 4. Length sanity (defensive; framer guarantees this) ──────────────
        if usize::from(frame.header.payload_len) != frame.payload.len() {
            return Outcome::Rejected(Reason::BadLength);
        }

        // ── 5. Sequence gap (wrap-aware) ──────────────────────────────────────
        let seq = frame.header.sequence;
        if let Some(last) = self.last_seq {
            let expected = last.wrapping_add(1);
            if seq != expected {
                self.last_seq = Some(seq);
                return Outcome::Rejected(Reason::SequenceGap { expected, got: seq });
            }
        }
        self.last_seq = Some(seq);

        Outcome::Accepted(frame)
    }
}

impl Default for Validator {
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
    use crate::frame::Header;
    use crate::protocol::{MAX_PAYLOAD, MIN_FRAME_LEN, PROTOCOL_VERSION};

    fn make_frame(seq: u32, packet_type: u8, severity: u8, payload: &[u8]) -> Frame {
        use heapless::Vec;
        let mut p: Vec<u8, MAX_PAYLOAD> = Vec::new();
        for &b in payload {
            p.push(b).ok();
        }
        let payload_len = p.len() as u16;
        let mut frame = Frame {
            header: Header {
                version: PROTOCOL_VERSION,
                packet_type,
                severity,
                sequence: seq,
                payload_len,
            },
            payload: p,
            crc_bytes: [0; 2],
        };
        // Encode to get correct CRC bytes.
        let mut raw = [0u8; MIN_FRAME_LEN + MAX_PAYLOAD];
        frame.encode_raw(&mut raw).expect("raw buffer large enough");
        // Last 2 raw bytes are the CRC.
        let raw_len = frame.raw_len();
        frame.crc_bytes = [raw[raw_len - 2], raw[raw_len - 1]];
        frame
    }

    #[test]
    fn valid_frame_accepted() {
        let mut v = Validator::new();
        let frame = make_frame(0, 0x01, 0x01, b"test");
        assert!(matches!(v.check(frame), Outcome::Accepted(_)));
    }

    #[test]
    fn bad_crc_rejected() {
        let mut v = Validator::new();
        let mut frame = make_frame(0, 0x01, 0x01, b"data");
        // Corrupt CRC bytes.
        frame.crc_bytes = [0xDE, 0xAD];
        let result = v.check(frame);
        assert!(
            matches!(result, Outcome::Rejected(Reason::Crc { .. })),
            "expected Crc rejection, got {:?}",
            result
        );
    }

    #[test]
    fn bad_crc_contains_expected_and_actual() {
        let mut v = Validator::new();
        let frame = make_frame(0, 0x01, 0x01, b"payload");
        let correct_crc = u16::from_le_bytes(frame.crc_bytes);
        let mut bad = frame.clone();
        bad.crc_bytes = [0xDE, 0xAD];
        match v.check(bad) {
            Outcome::Rejected(Reason::Crc { expected, actual }) => {
                assert_eq!(expected, correct_crc);
                assert_eq!(actual, 0xADDE);
            }
            other => panic!("unexpected outcome: {:?}", other),
        }
    }

    #[test]
    fn sequence_gap_rejected() {
        let mut v = Validator::new();
        let _ = v.check(make_frame(3, 0x01, 0x01, b"a")); // seed last_seq = 3
        let result = v.check(make_frame(5, 0x01, 0x01, b"b")); // expected 4, got 5
        assert!(
            matches!(
                result,
                Outcome::Rejected(Reason::SequenceGap {
                    expected: 4,
                    got: 5
                })
            ),
            "got {:?}",
            result
        );
    }

    #[test]
    fn sequence_wrap_around_accepted() {
        let mut v = Validator::new();
        // Seed near max.
        let _ = v.check(make_frame(u32::MAX - 1, 0x01, 0x01, b""));
        // u32::MAX is expected.
        assert!(matches!(
            v.check(make_frame(u32::MAX, 0x01, 0x01, b"")),
            Outcome::Accepted(_)
        ));
        // 0 is expected (wraps).
        assert!(matches!(
            v.check(make_frame(0, 0x01, 0x01, b"")),
            Outcome::Accepted(_)
        ));
        // 1 is expected.
        assert!(matches!(
            v.check(make_frame(1, 0x01, 0x01, b"")),
            Outcome::Accepted(_)
        ));
    }

    #[test]
    fn first_frame_any_sequence_accepted() {
        let mut v = Validator::new();
        assert!(matches!(
            v.check(make_frame(999, 0x01, 0x01, b"")),
            Outcome::Accepted(_)
        ));
    }

    #[test]
    fn unknown_packet_type_filtered() {
        let mut v = Validator::new();
        let frame = make_frame(0, 0xFF, 0x01, b""); // 0xFF is unknown
        assert!(matches!(
            v.check(frame),
            Outcome::Rejected(Reason::Filtered)
        ));
    }

    #[test]
    fn unknown_severity_filtered() {
        let mut v = Validator::new();
        let frame = make_frame(0, 0x01, 0xFF, b""); // 0xFF is unknown severity
        assert!(matches!(
            v.check(frame),
            Outcome::Rejected(Reason::Filtered)
        ));
    }

    #[test]
    fn sequence_gap_resyncs_state() {
        // After a gap, the validator re-syncs so the next frame (if in order) is accepted.
        let mut v = Validator::new();
        let _ = v.check(make_frame(0, 0x01, 0x01, b""));
        let _ = v.check(make_frame(5, 0x01, 0x01, b"")); // gap → rejected but re-synced to 5
        let result = v.check(make_frame(6, 0x01, 0x01, b"")); // now 6 is expected
        assert!(matches!(result, Outcome::Accepted(_)), "got {:?}", result);
    }

    /// The validator’s independent CRC must agree with [`Frame::compute_crc`] byte-for-byte.
    ///
    /// This test is the enforcement mechanism for the “two independent witnesses” design.
    /// If either side changes its field-serialization order, this fails immediately.
    #[test]
    fn validator_crc_matches_frame_compute_crc() {
        // Build a frame with the correct CRC set by Frame::compute_crc().
        let frame = make_frame(0xDEAD_BEEF, 0x01, 0x02, b"cross-check payload");
        // frame already has crc_bytes = compute_crc() from make_frame.
        // The validator must accept it (both CRC implementations agree).
        let mut v = Validator::new();
        assert!(
            matches!(v.check(frame), Outcome::Accepted(_)),
            "validator CRC computation disagrees with Frame::compute_crc()"
        );
    }
}
