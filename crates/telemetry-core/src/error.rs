//! Error types and outcome enum for the telemetry pipeline.
//!
//! Design rules (Phase 1 will fill these in):
//! - Single `#[non_exhaustive]` `Error` enum — only real, reachable faults.
//! - `Outcome` separates accepted records from rejected ones.
//! - Rejections are *counted* in [`crate::stats::Stats`], not propagated as errors.
