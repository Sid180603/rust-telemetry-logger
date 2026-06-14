//! Property tests for `StoredRecord` encode/decode round-trip.

use heapless::Vec;
use proptest::prelude::*;
use telemetry_core::{
    protocol::MAX_PAYLOAD,
    record::{self, MAX_STORED_RECORD_BYTES, RecordV1, StoredRecord},
};

proptest! {
    /// Encoding then decoding a `StoredRecord::V1` always yields the original value.
    #[test]
    fn stored_record_roundtrip(
        ts      in any::<u64>(),
        seq     in any::<u32>(),
        pt      in 0x01u8..=0x05u8,   // valid PacketType range
        sev     in 0x00u8..=0x04u8,   // valid Severity range
        payload in prop::collection::vec(any::<u8>(), 0..480usize),
    ) {
        let mut p: Vec<u8, MAX_PAYLOAD> = Vec::new();
        for &b in &payload {
            p.push(b).ok();
        }
        let original = StoredRecord::V1(RecordV1 {
            timestamp_us: ts,
            sequence:     seq,
            packet_type:  pt,
            severity:     sev,
            payload:      p,
        });

        let mut buf = [0u8; MAX_STORED_RECORD_BYTES];
        let n = record::encode(&original, &mut buf).expect("encode must succeed");
        let decoded = record::decode(&buf[..n]).expect("decode must succeed");
        prop_assert_eq!(&original, &decoded);
    }

    /// Feeding a corrupted encoded buffer to `decode` must never panic.
    /// (A CRC or COBS failure returns `Err`; it does not unwrap or panic.)
    #[test]
    fn corrupt_byte_never_panics(
        ts      in any::<u64>(),
        seq     in any::<u32>(),
        payload in prop::collection::vec(any::<u8>(), 1..20usize),
        // Pick a byte position within the first half of the encoded output (avoids delimiter).
        pos_mod in 1usize..16usize,
    ) {
        let mut p: Vec<u8, MAX_PAYLOAD> = Vec::new();
        for &b in &payload {
            p.push(b).ok();
        }
        let record = StoredRecord::V1(RecordV1 {
            timestamp_us: ts,
            sequence:     seq,
            packet_type:  0x01,
            severity:     0x01,
            payload:      p,
        });

        let mut buf = [0u8; MAX_STORED_RECORD_BYTES];
        let n = record::encode(&record, &mut buf).expect("encode must succeed");
        if n < 4 { return Ok(()); } // too small to corrupt meaningfully

        // Corrupt a non-delimiter byte in the COBS region.
        let pos = (pos_mod % (n - 1)).max(1); // skip index 0 (COBS overhead byte)
        if buf[pos] == 0x00 { return Ok(()); } // 0x00 is always a delimiter; skip
        buf[pos] ^= 0xFF;

        // Decode may succeed (if the corruption doesn't affect CRC) but that's
        // allowed — we only assert that the crate doesn't panic.
        let _ = record::decode(&buf[..n]);
    }
}
