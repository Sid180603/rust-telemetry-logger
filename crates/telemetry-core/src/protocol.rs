//! Wire protocol constants, version, and packet-type / severity enumerations.
//!
//! The authoritative specification lives in `docs/protocol.md`.  Every constant
//! and enum value here must match that document exactly; any discrepancy is a bug.

// ──────────────────────────────────────────────────────────────────────────────
// Constants
// ──────────────────────────────────────────────────────────────────────────────

/// Wire protocol version embedded in every frame header (`version` field = `0x01`).
pub const PROTOCOL_VERSION: u8 = 0x01;

/// Maximum payload bytes allowed in a single wire frame.
pub const MAX_PAYLOAD: usize = 480;

/// Number of bytes in the frame header, before the payload and CRC-16.
///
/// Layout: `version(1) + type(1) + severity(1) + sequence(4) + payload_len(2)` = 9 bytes.
pub const HEADER_LEN: usize = 9;

/// Number of bytes occupied by the `CRC-16` field at the end of each frame.
pub const CRC16_LEN: usize = 2;

/// Minimum decoded frame length (zero-length payload): `HEADER_LEN + CRC16_LEN` = 11 bytes.
pub const MIN_FRAME_LEN: usize = HEADER_LEN + CRC16_LEN;

/// `CRC-16/IBM-SDLC` residue constant.
///
/// Computing the `CRC-16` over a correctly-formed frame (including the `crc` bytes)
/// always yields this value, enabling single-pass validation without splitting off the
/// trailing CRC field.
pub const CRC16_RESIDUE: u16 = 0xF0B8;

// ──────────────────────────────────────────────────────────────────────────────
// PacketType
// ──────────────────────────────────────────────────────────────────────────────

/// Packet type discriminant embedded in the `type` field of every wire frame.
///
/// Use [`TryFrom<u8>`] to convert a raw byte; an unrecognised byte yields `Err(())`.
#[cfg_attr(
    feature = "full",
    doc = "\nFrames with unrecognised types are rejected with [`crate::error::Reason::Filtered`]."
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
#[repr(u8)]
pub enum PacketType {
    /// Periodic keep-alive.
    Heartbeat = 0x01,
    /// Numerical sensor reading(s).
    SensorData = 0x02,
    /// Discrete event notification.
    Event = 0x03,
    /// Internal firmware diagnostics.
    Diagnostic = 0x04,
    /// Actuation / control command response.
    Control = 0x05,
    // 0x06–0xFE reserved for future types.
}

impl TryFrom<u8> for PacketType {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x01 => Ok(PacketType::Heartbeat),
            0x02 => Ok(PacketType::SensorData),
            0x03 => Ok(PacketType::Event),
            0x04 => Ok(PacketType::Diagnostic),
            0x05 => Ok(PacketType::Control),
            _ => Err(()),
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Severity
// ──────────────────────────────────────────────────────────────────────────────

/// Severity level embedded in the `severity` field of every wire frame.
///
/// Severity levels are **ordered** (`Debug < Info < Warning < Error < Fatal`).
#[cfg_attr(
    feature = "full",
    doc = "\n[`crate::filter::FilterConfig`] uses this ordering to implement a minimum-severity threshold."
)]
///
/// Use [`TryFrom<u8>`] to convert a raw byte; an unrecognised byte yields `Err(())`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
#[repr(u8)]
pub enum Severity {
    /// Verbose debug output.
    Debug = 0x00,
    /// Informational operational message.
    Info = 0x01,
    /// Non-fatal condition requiring attention.
    Warning = 0x02,
    /// Recoverable error condition.
    Error = 0x03,
    /// Unrecoverable / system-halting condition.
    Fatal = 0x04,
    // 0x05–0xFF reserved.
}

impl TryFrom<u8> for Severity {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, ()> {
        match value {
            0x00 => Ok(Severity::Debug),
            0x01 => Ok(Severity::Info),
            0x02 => Ok(Severity::Warning),
            0x03 => Ok(Severity::Error),
            0x04 => Ok(Severity::Fatal),
            _ => Err(()),
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_version_is_one() {
        assert_eq!(PROTOCOL_VERSION, 1);
    }

    #[test]
    fn min_frame_len_is_eleven() {
        assert_eq!(MIN_FRAME_LEN, 11);
        assert_eq!(HEADER_LEN + CRC16_LEN, 11);
    }

    #[test]
    fn packet_type_round_trips() {
        let cases: &[(u8, PacketType)] = &[
            (0x01, PacketType::Heartbeat),
            (0x02, PacketType::SensorData),
            (0x03, PacketType::Event),
            (0x04, PacketType::Diagnostic),
            (0x05, PacketType::Control),
        ];
        for &(byte, variant) in cases {
            let got = PacketType::try_from(byte).expect("known byte must convert");
            assert_eq!(got, variant);
            assert_eq!(got as u8, byte);
        }
    }

    #[test]
    fn packet_type_unknown_returns_err() {
        assert!(PacketType::try_from(0x00).is_err());
        assert!(PacketType::try_from(0x06).is_err());
        assert!(PacketType::try_from(0xFF).is_err());
    }

    #[test]
    fn severity_round_trips() {
        let cases: &[(u8, Severity)] = &[
            (0x00, Severity::Debug),
            (0x01, Severity::Info),
            (0x02, Severity::Warning),
            (0x03, Severity::Error),
            (0x04, Severity::Fatal),
        ];
        for &(byte, variant) in cases {
            let got = Severity::try_from(byte).expect("known byte must convert");
            assert_eq!(got, variant);
            assert_eq!(got as u8, byte);
        }
    }

    #[test]
    fn severity_unknown_returns_err() {
        assert!(Severity::try_from(0x05).is_err());
        assert!(Severity::try_from(0xFF).is_err());
    }

    #[test]
    fn severity_ordering() {
        assert!(Severity::Debug < Severity::Info);
        assert!(Severity::Info < Severity::Warning);
        assert!(Severity::Warning < Severity::Error);
        assert!(Severity::Error < Severity::Fatal);
    }
}
