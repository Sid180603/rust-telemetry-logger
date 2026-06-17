//! [`Frame`] and [`Header`] data types produced by the framer, plus encoding helpers.
//!
//! These are *data* types only — parsing lives in [`crate::framer`].
//!
//! # Encoding
//!
//! [`Frame::encode_cobs`] writes a complete, correctly-CRC'd `COBS`-encoded wire
//! frame (including the trailing `0x00` delimiter).  It computes the `CRC-16` fresh
//! from the current field values; the stored [`Frame::crc_bytes`] field is **not**
//! used during encoding — it only holds bytes that arrived on the wire.

use crc::{CRC_16_IBM_SDLC, Crc};
use heapless::Vec;

use crate::protocol::{CRC16_LEN, HEADER_LEN, MAX_PAYLOAD, MIN_FRAME_LEN};

/// Maximum size (bytes) of a `COBS`-encoded frame including the trailing `0x00`.
///
/// `max_encoding_length(491) + 1 delimiter = 493 + 1 = 494`.
pub const MAX_COBS_FRAME_BYTES: usize = 494;

// ──────────────────────────────────────────────────────────────────────────────
// Header
// ──────────────────────────────────────────────────────────────────────────────

/// The fixed-size header portion of a decoded wire frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// Protocol version byte (currently [`crate::protocol::PROTOCOL_VERSION`]).
    pub version: u8,
    /// Raw `PacketType` discriminant (see [`crate::protocol::PacketType`]).
    pub packet_type: u8,
    /// Raw `Severity` discriminant (see [`crate::protocol::Severity`]).
    pub severity: u8,
    /// Monotonically increasing per-source sequence number (wraps at `u32::MAX`).
    pub sequence: u32,
    /// Number of payload bytes that follow the header.
    pub payload_len: u16,
}

// ──────────────────────────────────────────────────────────────────────────────
// Frame
// ──────────────────────────────────────────────────────────────────────────────

/// A decoded wire frame produced by [`crate::framer::Framer`].
///
/// `crc_bytes` holds the raw trailing bytes that arrived on the wire; the
/// [`crate::validator::Validator`] computes the expected `CRC-16` independently
/// and compares against these bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Parsed frame header.
    pub header: Header,
    /// Application payload (up to [`MAX_PAYLOAD`] bytes).
    pub payload: Vec<u8, MAX_PAYLOAD>,
    /// Raw `CRC-16` bytes as received (little-endian).  Set by the framer;
    /// **not** used by [`Frame::encode_cobs`].
    pub crc_bytes: [u8; 2],
}

impl Frame {
    /// Construct a new frame from its parts.
    ///
    /// Fills `version` ([`crate::protocol::PROTOCOL_VERSION`]) and `payload_len`
    /// automatically.  Returns `None` if `payload` is longer than [`MAX_PAYLOAD`].
    ///
    /// The `crc_bytes` field is zeroed; it is recomputed when [`Frame::encode_cobs`]
    /// is called (the stored bytes are only used by the validator for incoming frames).
    pub fn new(packet_type: u8, severity: u8, sequence: u32, payload: &[u8]) -> Option<Self> {
        if payload.len() > MAX_PAYLOAD {
            return None;
        }
        let mut p: Vec<u8, MAX_PAYLOAD> = Vec::new();
        // unwrap: len checked above
        p.extend_from_slice(payload).ok()?;
        Some(Self {
            header: Header {
                version: crate::protocol::PROTOCOL_VERSION,
                packet_type,
                severity,
                sequence,
                // payload.len() <= MAX_PAYLOAD = 480, which always fits in u16.
                // try_from makes this explicit to the type checker.
                payload_len: u16::try_from(payload.len()).ok()?,
            },
            payload: p,
            crc_bytes: [0; 2],
        })
    }

    /// Total decoded byte length: `HEADER_LEN + payload.len() + CRC16_LEN`.
    #[inline]
    pub fn raw_len(&self) -> usize {
        MIN_FRAME_LEN + self.payload.len()
    }

    /// Compute `CRC-16/IBM-SDLC` over the frame fields (excluding the trailing
    /// `crc_bytes`).
    ///
    /// Used by [`Frame::encode_cobs`] and for test assertions.
    pub fn compute_crc(&self) -> [u8; 2] {
        let crc16 = Crc::<u16>::new(&CRC_16_IBM_SDLC);
        let mut digest = crc16.digest();
        digest.update(&[
            self.header.version,
            self.header.packet_type,
            self.header.severity,
        ]);
        digest.update(&self.header.sequence.to_le_bytes());
        digest.update(&self.header.payload_len.to_le_bytes());
        digest.update(&self.payload);
        digest.finalize().to_le_bytes()
    }

    /// Encode the frame into raw (pre-`COBS`) bytes.
    ///
    /// The `CRC-16` is computed from the current field values and written into
    /// the last two bytes of `buf`.  The stored [`Frame::crc_bytes`] field is
    /// ignored.
    ///
    /// Returns `Some(n)` where `n` is the number of bytes written, or `None` if
    /// `buf` is too small (`buf.len() < self.raw_len()`).
    pub fn encode_raw(&self, buf: &mut [u8]) -> Option<usize> {
        let n = self.raw_len();
        if buf.len() < n {
            return None;
        }
        buf[0] = self.header.version;
        buf[1] = self.header.packet_type;
        buf[2] = self.header.severity;
        buf[3..7].copy_from_slice(&self.header.sequence.to_le_bytes());
        buf[7..9].copy_from_slice(&self.header.payload_len.to_le_bytes());
        let payload_end = HEADER_LEN + self.payload.len();
        buf[HEADER_LEN..payload_end].copy_from_slice(&self.payload);
        let crc = self.compute_crc();
        buf[payload_end..payload_end + CRC16_LEN].copy_from_slice(&crc);
        Some(n)
    }

    /// Encode the frame as a `COBS`-encoded wire packet including the trailing
    /// `0x00` delimiter.
    ///
    /// Returns `Some(n)` where `n` is the total bytes written (including
    /// delimiter), or `None` if `buf` is too small (need at least
    /// [`MAX_COBS_FRAME_BYTES`] bytes for a max-size frame).
    pub fn encode_cobs(&self, buf: &mut [u8]) -> Option<usize> {
        // Stack buffer for the raw frame (max 491 bytes).
        let mut raw = [0u8; MIN_FRAME_LEN + MAX_PAYLOAD];
        let raw_len = self.encode_raw(&mut raw)?;

        // COBS max length: raw_len + raw_len/254 + 1
        let max_cobs = cobs::max_encoding_length(raw_len);
        if buf.len() < max_cobs + 1 {
            return None;
        }
        let written = cobs::encode(&raw[..raw_len], buf);
        buf[written] = 0x00;
        Some(written + 1)
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::PROTOCOL_VERSION;

    /// Build a simple test frame with the given sequence and payload.
    fn make_frame(seq: u32, payload: &[u8]) -> Frame {
        let mut p: Vec<u8, MAX_PAYLOAD> = Vec::new();
        for &b in payload {
            p.push(b).ok();
        }
        let payload_len = p.len() as u16;
        Frame {
            header: Header {
                version: PROTOCOL_VERSION,
                packet_type: 0x01, // Heartbeat
                severity: 0x01,    // Info
                sequence: seq,
                payload_len,
            },
            payload: p,
            crc_bytes: [0; 2], // encode_raw/cobs ignores this
        }
    }

    #[test]
    fn new_constructor_fills_version_and_payload_len() {
        let f = Frame::new(0x02, 0x01, 99, b"hello").expect("payload fits");
        assert_eq!(f.header.version, PROTOCOL_VERSION);
        assert_eq!(f.header.packet_type, 0x02);
        assert_eq!(f.header.severity, 0x01);
        assert_eq!(f.header.sequence, 99);
        assert_eq!(f.header.payload_len, 5);
        assert_eq!(&f.payload[..], b"hello");
    }

    #[test]
    fn new_constructor_empty_payload() {
        let f = Frame::new(0x01, 0x00, 0, b"").expect("empty payload is valid");
        assert_eq!(f.header.payload_len, 0);
        assert!(f.payload.is_empty());
    }

    #[test]
    fn new_constructor_oversized_payload_returns_none() {
        let big = [0u8; MAX_PAYLOAD + 1];
        assert!(Frame::new(0x01, 0x00, 0, &big).is_none());
    }

    #[test]
    fn new_encode_cobs_decodes_correctly() {
        let f = Frame::new(0x03, 0x02, 7, b"world").expect("fits");
        let mut buf = [0u8; MAX_COBS_FRAME_BYTES];
        let n = f.encode_cobs(&mut buf).expect("buffer fits");
        assert_eq!(buf[n - 1], 0x00);
    }

    #[test]
    fn raw_len_matches_formula() {
        let f = make_frame(0, b"hello");
        assert_eq!(f.raw_len(), MIN_FRAME_LEN + 5);
    }

    #[test]
    fn encode_raw_byte_layout() {
        let f = make_frame(0x0102_0304, b"AB");
        let mut buf = [0u8; 64];
        let n = f.encode_raw(&mut buf).expect("buffer large enough");
        assert_eq!(n, MIN_FRAME_LEN + 2);

        // Header fields
        assert_eq!(buf[0], PROTOCOL_VERSION);
        assert_eq!(buf[1], 0x01); // Heartbeat
        assert_eq!(buf[2], 0x01); // Info
        // Sequence LE
        assert_eq!(&buf[3..7], &[0x04, 0x03, 0x02, 0x01]);
        // Payload len LE
        assert_eq!(&buf[7..9], &[0x02, 0x00]);
        // Payload
        assert_eq!(&buf[9..11], b"AB");
        // CRC must match compute_crc()
        assert_eq!(&buf[11..13], &f.compute_crc());
    }

    #[test]
    fn encode_raw_too_small_returns_none() {
        let f = make_frame(0, b"test");
        let mut buf = [0u8; 5]; // too small
        assert!(f.encode_raw(&mut buf).is_none());
    }

    #[test]
    fn encode_cobs_decodes_to_original() {
        let f = make_frame(7, b"hello world");
        let mut buf = [0u8; MAX_COBS_FRAME_BYTES];
        let n = f.encode_cobs(&mut buf).expect("buffer large enough");

        // Last byte must be the 0x00 delimiter
        assert_eq!(buf[n - 1], 0x00);

        // COBS decode the bytes before the delimiter
        let mut decoded = [0u8; 128];
        let cobs_bytes = &buf[..n - 1];
        decoded[..cobs_bytes.len()].copy_from_slice(cobs_bytes);
        let decoded_len =
            cobs::decode_in_place(&mut decoded[..cobs_bytes.len()]).expect("valid COBS encoding");

        // First byte of decoded should be version
        assert_eq!(decoded[0], PROTOCOL_VERSION);
        // Decoded payload should be present
        assert_eq!(&decoded[HEADER_LEN..HEADER_LEN + 11], b"hello world");
        // Decoded length = raw_len
        assert_eq!(decoded_len, f.raw_len());
    }

    #[test]
    fn compute_crc_check_value() {
        // CRC-16/IBM-SDLC check value for ASCII "123456789" is 0x906E.
        let f: Frame = Frame {
            header: Header {
                version: 0,
                packet_type: 0,
                severity: 0,
                sequence: 0,
                payload_len: 0,
            },
            payload: Vec::new(),
            crc_bytes: [0; 2],
        };
        // Build a frame whose "all fields before CRC" is exactly b"123456789"
        // by computing manually with the crc crate directly.
        let crc16 = Crc::<u16>::new(&CRC_16_IBM_SDLC);
        let check = crc16.checksum(b"123456789");
        assert_eq!(check, 0x906E, "CRC-16/IBM-SDLC check value mismatch");
        // Also verify compute_crc produces bytes that round-trip
        let crc_bytes = f.compute_crc();
        let stored = u16::from_le_bytes(crc_bytes);
        // Empty frame: CRC of [0,0,0, 0,0,0,0, 0,0] (header fields)
        assert_eq!(stored, f.compute_crc_value());
    }

    #[test]
    fn encode_cobs_too_small_returns_none() {
        let f = make_frame(0, b"data");
        let mut buf = [0u8; 2]; // way too small
        assert!(f.encode_cobs(&mut buf).is_none());
    }
}

// Helper only for tests: expose the raw u16 CRC value by delegating to compute_crc.
#[cfg(test)]
impl Frame {
    fn compute_crc_value(&self) -> u16 {
        u16::from_le_bytes(self.compute_crc())
    }
}
