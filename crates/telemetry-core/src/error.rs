//! Error types and outcome enum for the telemetry pipeline.
//!
//! # Design
//!
//! - [`Error`] is the *unrecoverable fault* type — returned when the pipeline itself
//!   cannot continue (full buffer, storage failure, codec failure).
//! - [`Outcome`] separates *accepted* frames from *rejected* frames.  Rejections are
//!   counted in [`crate::stats::Stats`] but never propagated up the call stack.
//! - [`Reason`] describes why a frame was rejected; each variant maps to exactly one
//!   [`crate::stats::Stats`] counter.

use core::fmt;

// ──────────────────────────────────────────────────────────────────────────────
// Reason
// ──────────────────────────────────────────────────────────────────────────────

/// Why a frame was rejected during validation or filtering.
///
/// Rejections are normal data-quality outcomes, not pipeline failures.
/// Each variant increments exactly one counter in [`crate::stats::Stats`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Reason {
    /// `CRC-16/IBM-SDLC` mismatch on the wire frame.
    Crc {
        /// CRC value the validator computed from the frame fields.
        expected: u16,
        /// CRC value stored in the frame's trailing `crc` field.
        actual: u16,
    },
    /// Sequence number gap — one or more frames were dropped upstream.
    SequenceGap {
        /// Sequence number the validator expected next.
        expected: u32,
        /// Sequence number the frame actually carried.
        got: u32,
    },
    /// Frame was dropped by [`crate::filter::FilterConfig`] (type or severity policy),
    /// or contained an unrecognised `type` / `severity` byte.
    Filtered,
    /// Frame `payload_len` field does not match the number of decoded payload bytes.
    BadLength,
}

// ──────────────────────────────────────────────────────────────────────────────
// Outcome
// ──────────────────────────────────────────────────────────────────────────────

/// The outcome of processing a single frame through the validation/filter stages.
///
/// `T` is the accepted value type (typically [`crate::frame::Frame`]).
#[must_use]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome<T> {
    /// Frame passed all checks; the validated value is carried here.
    Accepted(T),
    /// Frame failed a check; the reason is carried here.
    Rejected(Reason),
}

// ──────────────────────────────────────────────────────────────────────────────
// Error
// ──────────────────────────────────────────────────────────────────────────────

/// Unrecoverable pipeline faults that are propagated to the caller.
///
/// Every variant is reachable in the codebase.  There is no catch-all
/// `Internal` variant — if a condition cannot actually occur, it is not modelled.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Error {
    /// The ring buffer was full; the record could not be enqueued.
    BufferFull,
    /// The storage backend returned an error during `write` or `flush`.
    Storage,
    /// A record could not be serialised or deserialised (postcard / `COBS` codec).
    Codec,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::BufferFull => f.write_str("ring buffer full"),
            Error::Storage => f.write_str("storage write/flush failed"),
            Error::Codec => f.write_str("record encode/decode failed"),
        }
    }
}

impl core::error::Error for Error {}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use core::fmt::Write as _;
    use heapless::String;

    fn display_str<const N: usize>(err: &Error) -> String<N> {
        let mut s: String<N> = String::new();
        write!(s, "{err}").ok();
        s
    }

    #[test]
    fn error_display_buffer_full() {
        assert_eq!(
            display_str::<64>(&Error::BufferFull).as_str(),
            "ring buffer full"
        );
    }

    #[test]
    fn error_display_storage() {
        assert_eq!(
            display_str::<64>(&Error::Storage).as_str(),
            "storage write/flush failed"
        );
    }

    #[test]
    fn error_display_codec() {
        assert_eq!(
            display_str::<64>(&Error::Codec).as_str(),
            "record encode/decode failed"
        );
    }

    #[test]
    fn error_implements_core_error() {
        fn assert_error<E: core::error::Error>(_: &E) {}
        assert_error(&Error::Codec);
        assert_error(&Error::Storage);
        assert_error(&Error::BufferFull);
    }

    #[test]
    fn outcome_accepted_discriminates() {
        let v: Outcome<i32> = Outcome::Accepted(42);
        assert!(matches!(v, Outcome::Accepted(42)));
    }

    #[test]
    fn outcome_rejected_discriminates() {
        let v: Outcome<i32> = Outcome::Rejected(Reason::Filtered);
        assert!(matches!(v, Outcome::Rejected(Reason::Filtered)));
    }

    #[test]
    fn reason_crc_fields_round_trip() {
        let r = Reason::Crc {
            expected: 0x1234,
            actual: 0x5678,
        };
        assert!(matches!(
            r,
            Reason::Crc {
                expected: 0x1234,
                actual: 0x5678
            }
        ));
    }

    #[test]
    fn reason_seq_gap_fields_round_trip() {
        let r = Reason::SequenceGap {
            expected: 5,
            got: 7,
        };
        assert!(matches!(
            r,
            Reason::SequenceGap {
                expected: 5,
                got: 7
            }
        ));
    }

    #[test]
    fn error_clone_and_eq() {
        assert_eq!(Error::Codec.clone(), Error::Codec);
        assert_ne!(Error::Codec, Error::Storage);
    }
}
