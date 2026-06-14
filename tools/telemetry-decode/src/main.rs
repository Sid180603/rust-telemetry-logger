//! `telemetry-decode` — binary log decoder.
//!
//! Reads COBS+CRC-32 segment files and prints records as a table, JSON,
//! or NDJSON.  Reuses `telemetry-core::record::StoredRecord` and
//! `telemetry-core::stats::Stats` directly — proving the portable core.
//!
//! Flags: `--format table|json|ndjson`, `--filter`, `--since <seq>`, `--stats`.
//! Filled in during Phase 2.

fn main() {
    println!("telemetry-decode stub — Phase 2 will implement this.");
}
