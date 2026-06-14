//! [`StoredRecord`] — versioned, postcard-serialised storage records.
//!
//! # On-disk format
//!
//! ```text
//! COBS( postcard_bytes(StoredRecord) ++ CRC-32/ISO-HDLC ) 0x00
//! ```
//!
//! - `postcard` serialises `StoredRecord` as a leading discriminant byte (the
//!   version number) followed by the [`RecordV1`] fields.
//! - The `CRC-32` is appended *after* the postcard bytes and covers all of them
//!   (including the version discriminant).
//! - The entire `[postcard || CRC32]` blob is `COBS`-encoded for stream framing,
//!   then the `0x00` delimiter is appended.
//!
//! # Schema evolution
//!
//! Add `V2(RecordV2)` when fields need to change.  Old decoders see an unknown
//! postcard discriminant and return [`crate::error::Error::Codec`]; they skip
//! the record gracefully.  Never re-use a discriminant value.

use crc::{CRC_32_ISO_HDLC, Crc};
use heapless::Vec;
use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::protocol::MAX_PAYLOAD;

// ──────────────────────────────────────────────────────────────────────────────
// Constants
// ──────────────────────────────────────────────────────────────────────────────

/// Maximum size (bytes) of one encoded record written to storage.
///
/// `COBS(postcard_bytes(~500) + CRC32(4)) + 0x00 delimiter ≈ 507 bytes`.
/// 512 provides comfortable margin.
pub const MAX_STORED_RECORD_BYTES: usize = 512;

// ──────────────────────────────────────────────────────────────────────────────
// RecordV1
// ──────────────────────────────────────────────────────────────────────────────

/// Version-1 storage record: one validated and filtered wire frame plus metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordV1 {
    /// Monotonic source timestamp in **microseconds** since boot.
    pub timestamp_us: u64,
    /// Wire frame sequence number.
    pub sequence: u32,
    /// Raw `PacketType` discriminant byte.
    pub packet_type: u8,
    /// Raw `Severity` discriminant byte.
    pub severity: u8,
    /// Application payload bytes.
    pub payload: Vec<u8, MAX_PAYLOAD>,
}

// ──────────────────────────────────────────────────────────────────────────────
// StoredRecord
// ──────────────────────────────────────────────────────────────────────────────

/// Versioned envelope for storage records.
///
/// The `postcard` enum discriminant byte **is** the version number:
/// - `0x00` → `V1`
///
/// A decoder that sees an unrecognised discriminant returns
/// [`Error::Codec`] rather than panicking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum StoredRecord {
    /// Version-1 record.
    V1(RecordV1),
}

// ──────────────────────────────────────────────────────────────────────────────
// encode / decode
// ──────────────────────────────────────────────────────────────────────────────

/// Encode a [`StoredRecord`] into `buf` using the on-disk format.
///
/// Writes `COBS(postcard(record) ++ CRC-32) 0x00` into `buf`.
///
/// # Errors
///
/// Returns [`Error::Codec`] if `buf` is too small or serialisation fails.
pub fn encode(record: &StoredRecord, buf: &mut [u8]) -> Result<usize, Error> {
    // Stack buffer: postcard bytes + 4-byte CRC-32 (max ~504 bytes total).
    let mut pre_cobs = [0u8; MAX_STORED_RECORD_BYTES];

    // Serialise; capture length and release the borrow on pre_cobs.
    let postcard_len = postcard::to_slice(record, &mut pre_cobs)
        .map_err(|_| Error::Codec)?
        .len();

    // Append CRC-32/ISO-HDLC over the postcard bytes.
    let crc_value = Crc::<u32>::new(&CRC_32_ISO_HDLC).checksum(&pre_cobs[..postcard_len]);
    let crc_bytes = crc_value.to_le_bytes();
    let pre_cobs_end = postcard_len + 4;
    if pre_cobs_end > pre_cobs.len() {
        return Err(Error::Codec);
    }
    pre_cobs[postcard_len..pre_cobs_end].copy_from_slice(&crc_bytes);

    // COBS-encode into the output buffer, then append 0x00 delimiter.
    let max_cobs = cobs::max_encoding_length(pre_cobs_end);
    if buf.len() < max_cobs + 1 {
        return Err(Error::Codec);
    }
    let cobs_len = cobs::encode(&pre_cobs[..pre_cobs_end], buf);
    buf[cobs_len] = 0x00;
    Ok(cobs_len + 1)
}

/// Decode a [`StoredRecord`] from `buf` (with or without the trailing `0x00`).
///
/// Verifies the `CRC-32` before deserialising.  Returns [`Error::Codec`] if the
/// `COBS` envelope is malformed, the `CRC` does not match, or the postcard bytes
/// contain an unrecognised version discriminant.
///
/// # Errors
///
/// Returns [`Error::Codec`] on any structural or integrity failure.
pub fn decode(buf: &[u8]) -> Result<StoredRecord, Error> {
    // Strip trailing 0x00 delimiter if present.
    let cobs_bytes = if buf.last() == Some(&0x00) {
        &buf[..buf.len() - 1]
    } else {
        buf
    };

    if cobs_bytes.is_empty() {
        return Err(Error::Codec);
    }

    // COBS-decode into a stack buffer.
    let mut decode_buf = [0u8; MAX_STORED_RECORD_BYTES];
    if cobs_bytes.len() > decode_buf.len() {
        return Err(Error::Codec);
    }
    decode_buf[..cobs_bytes.len()].copy_from_slice(cobs_bytes);
    let decoded_len =
        cobs::decode_in_place(&mut decode_buf[..cobs_bytes.len()]).map_err(|_| Error::Codec)?;

    // Must have at least 4 bytes for the CRC-32.
    if decoded_len < 4 {
        return Err(Error::Codec);
    }
    let postcard_end = decoded_len - 4;

    // Verify CRC-32.
    let stored_crc = {
        let crc_slice: [u8; 4] = decode_buf[postcard_end..decoded_len]
            .try_into()
            .map_err(|_| Error::Codec)?;
        u32::from_le_bytes(crc_slice)
    };
    let computed_crc = Crc::<u32>::new(&CRC_32_ISO_HDLC).checksum(&decode_buf[..postcard_end]);
    if computed_crc != stored_crc {
        return Err(Error::Codec);
    }

    // Deserialise.
    postcard::from_bytes::<StoredRecord>(&decode_buf[..postcard_end]).map_err(|_| Error::Codec)
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_v1(ts: u64, seq: u32, payload: &[u8]) -> StoredRecord {
        let mut p: Vec<u8, MAX_PAYLOAD> = Vec::new();
        for &b in payload {
            p.push(b).ok();
        }
        StoredRecord::V1(RecordV1 {
            timestamp_us: ts,
            sequence: seq,
            packet_type: 0x01,
            severity: 0x01,
            payload: p,
        })
    }

    #[test]
    fn round_trip_v1() {
        let record = make_v1(123_456_789, 42, b"hello telemetry");
        let mut buf = [0u8; MAX_STORED_RECORD_BYTES];
        let n = encode(&record, &mut buf).expect("encode succeeds");
        let decoded = decode(&buf[..n]).expect("decode succeeds");
        assert_eq!(record, decoded);
    }

    #[test]
    fn round_trip_empty_payload() {
        let record = make_v1(0, 0, b"");
        let mut buf = [0u8; MAX_STORED_RECORD_BYTES];
        let n = encode(&record, &mut buf).expect("encode succeeds");
        let decoded = decode(&buf[..n]).expect("decode succeeds");
        assert_eq!(record, decoded);
    }

    #[test]
    fn round_trip_max_payload() {
        let big = [0xABu8; MAX_PAYLOAD];
        let record = make_v1(u64::MAX, u32::MAX, &big);
        let mut buf = [0u8; MAX_STORED_RECORD_BYTES];
        let n = encode(&record, &mut buf).expect("encode succeeds for max payload");
        let decoded = decode(&buf[..n]).expect("decode succeeds");
        assert_eq!(record, decoded);
    }

    #[test]
    fn decode_without_trailing_delimiter() {
        let record = make_v1(1, 2, b"abc");
        let mut buf = [0u8; MAX_STORED_RECORD_BYTES];
        let n = encode(&record, &mut buf).expect("encode succeeds");
        // Strip the trailing 0x00
        assert_eq!(buf[n - 1], 0x00);
        let decoded = decode(&buf[..n - 1]).expect("decode without delimiter");
        assert_eq!(record, decoded);
    }

    #[test]
    fn unknown_discriminant_returns_codec_error() {
        // Manually craft bytes with postcard discriminant 0xFF (no such variant).
        // Build: COBS([0xFF, ...any valid postcard bytes...] || CRC32) 0x00
        // Simplest: encode a valid record, then flip the first decoded byte.
        let record = make_v1(1, 1, b"x");
        let mut buf = [0u8; MAX_STORED_RECORD_BYTES];
        let n = encode(&record, &mut buf).expect("encode succeeds");

        // Decode COBS, flip discriminant, re-encode, then try to decode.
        let cobs_bytes = &buf[..n - 1];
        let mut decoded_raw = [0u8; MAX_STORED_RECORD_BYTES];
        decoded_raw[..cobs_bytes.len()].copy_from_slice(cobs_bytes);
        let dec_len = cobs::decode_in_place(&mut decoded_raw[..cobs_bytes.len()]).unwrap();

        // First byte is the postcard enum discriminant (0x00 for V1).
        assert_eq!(decoded_raw[0], 0x00, "V1 discriminant must be 0x00");

        // Flip it to an unknown variant.
        decoded_raw[0] = 0xFF;

        // Recompute CRC-32 over modified postcard bytes.
        let postcard_end = dec_len - 4;
        let new_crc = Crc::<u32>::new(&CRC_32_ISO_HDLC).checksum(&decoded_raw[..postcard_end]);
        decoded_raw[postcard_end..postcard_end + 4].copy_from_slice(&new_crc.to_le_bytes());

        // Re-encode with COBS.
        let mut re_encoded = [0u8; MAX_STORED_RECORD_BYTES];
        let re_len = cobs::encode(&decoded_raw[..dec_len], &mut re_encoded);
        re_encoded[re_len] = 0x00;

        let result = decode(&re_encoded[..re_len + 1]);
        assert!(
            matches!(result, Err(Error::Codec)),
            "unknown discriminant must fail"
        );
    }

    #[test]
    fn truncated_input_returns_codec_error() {
        let record = make_v1(1, 1, b"data");
        let mut buf = [0u8; MAX_STORED_RECORD_BYTES];
        let n = encode(&record, &mut buf).expect("encode succeeds");
        // Truncate to half
        let result = decode(&buf[..n / 2]);
        assert!(matches!(result, Err(Error::Codec)));
    }

    #[test]
    fn v1_postcard_discriminant_is_zero() {
        // The postcard discriminant for the first enum variant (V1) is 0x00.
        let record = make_v1(0, 0, b"");
        let mut postcard_buf = [0u8; 64];
        let bytes = postcard::to_slice(&record, &mut postcard_buf).expect("serialize");
        assert_eq!(bytes[0], 0x00, "V1 discriminant must be 0x00");
    }

    #[test]
    fn corrupted_crc_returns_codec_error() {
        let record = make_v1(42, 7, b"test");
        let mut buf = [0u8; MAX_STORED_RECORD_BYTES];
        let n = encode(&record, &mut buf).expect("encode succeeds");
        // Flip a byte in the middle of the COBS payload (not the delimiter)
        buf[n / 2] ^= 0xFF;
        let result = decode(&buf[..n]);
        assert!(matches!(result, Err(Error::Codec)));
    }
}
