//! Typestate COBS-framer: converts a raw byte stream into validated [`crate::frame::Frame`]s.
//!
//! States: `WaitDelimiter → Header → Payload → Crc → Complete`
//! COBS destuffing uses the `cobs` crate.  CRC is *not* checked here —
//! that responsibility belongs to [`crate::validator::Validator`].
