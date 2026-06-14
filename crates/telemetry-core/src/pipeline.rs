//! The top-level telemetry pipeline with a split `ingest` / `drain` API.
//!
//! ```text
//!  ingest(bytes, clock) → framer → validator → filter → ring-buffer + stats
//!  drain(storage)       → pop ring-buffer → encode → write → flush
//! ```
//!
//! This split maps cleanly to both blocking v1 (call both in a loop) and
//! async v2 (two Embassy tasks share the ring-buffer via a channel).
//! The core crate is **unchanged** between the two firmware versions.
