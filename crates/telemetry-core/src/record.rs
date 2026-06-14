//! `StoredRecord` enum — versioned, postcard-serialised storage records.
//!
//! Each record is serialised as: `COBS(postcard(StoredRecord::V1{..}) ++ CRC-32/ISO-HDLC)`
//! The postcard enum discriminant byte *is* the version number, and the CRC-32 covers it.
