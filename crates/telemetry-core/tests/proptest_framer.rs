//! Property tests for the `COBS` framer.
//!
//! These tests use `proptest` to verify two properties:
//! 1. Feeding arbitrary bytes never panics.
//! 2. A correctly-encoded frame always decodes to `FrameOutput::Complete`.

use heapless::Vec;
use proptest::prelude::*;
use telemetry_core::{
    frame::{Frame, Header, MAX_COBS_FRAME_BYTES},
    framer::{FrameOutput, Framer},
    protocol::{MAX_PAYLOAD, PROTOCOL_VERSION},
};

// ──────────────────────────────────────────────────────────────────────────────
// Helper
// ──────────────────────────────────────────────────────────────────────────────

fn make_encoded_frame(seq: u32, payload_bytes: &[u8]) -> [u8; MAX_COBS_FRAME_BYTES] {
    let mut p: Vec<u8, MAX_PAYLOAD> = Vec::new();
    for &b in payload_bytes.iter().take(MAX_PAYLOAD) {
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
    frame.encode_cobs(&mut buf).expect("frame fits in buffer");
    buf
}

// ──────────────────────────────────────────────────────────────────────────────
// Properties
// ──────────────────────────────────────────────────────────────────────────────

proptest! {
    /// Feeding arbitrary byte sequences never panics and always terminates.
    #[test]
    fn framer_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..1024usize)) {
        let mut framer = Framer::<512>::new();
        for b in bytes {
            let _ = framer.feed(b);
        }
    }

    /// A correctly-encoded frame always produces `FrameOutput::Complete`.
    #[test]
    fn framer_accepts_valid_encoded_frame(
        seq     in any::<u32>(),
        payload in prop::collection::vec(any::<u8>(), 0..480usize),
    ) {
        let encoded = make_encoded_frame(seq, &payload);
        // Find the length by looking for the first 0x00 (delimiter).
        let len = encoded.iter().position(|&b| b == 0x00).map_or(MAX_COBS_FRAME_BYTES, |i| i + 1);

        let mut framer = Framer::<512>::new();
        let mut last = FrameOutput::Incomplete;
        for &b in &encoded[..len] {
            last = framer.feed(b);
        }
        prop_assert!(
            matches!(last, FrameOutput::Complete(_)),
            "expected Complete, got {:?}",
            last
        );
    }

    /// `resync_count` never decreases.
    #[test]
    fn resync_count_non_decreasing(bytes in prop::collection::vec(any::<u8>(), 0..512usize)) {
        let mut framer = Framer::<64>::new(); // small buffer to trigger overflows
        let mut last_count = 0u32;
        for b in bytes {
            let _ = framer.feed(b);
            prop_assert!(framer.resync_count >= last_count);
            last_count = framer.resync_count;
        }
    }
}
