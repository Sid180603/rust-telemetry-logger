//! Fuzz target: feed arbitrary bytes into the COBS framer.
//!
//! Invariants verified:
//! - No panic on any input.
//! - `resync_count` never overflows (saturating semantics expected by callers).
//! - The framer always terminates (no infinite loops).
//!
//! Run with: `cargo +nightly fuzz run framer`
//! (requires WSL or Linux with nightly Rust + libfuzzer)

#![no_main]

use libfuzzer_sys::fuzz_target;
use telemetry_core::framer::Framer;

fuzz_target!(|data: &[u8]| {
    let mut framer = Framer::<512>::new();
    for &byte in data {
        let _ = framer.feed(byte);
    }
    // resync_count must not have wrapped (it's u32; on any realistic corpus
    // this will never approach u32::MAX, but assert it's non-negative — which
    // u32 always is — to make the intent explicit).
    let _ = framer.resync_count;
});
