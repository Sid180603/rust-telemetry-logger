//! Stateful validator: CRC-16/IBM-SDLC wire check + length + sequence-gap detection.
//!
//! The validator is *stateful* because it tracks the last-seen sequence number
//! to detect gaps across successive calls.
