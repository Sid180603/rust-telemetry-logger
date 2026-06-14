//! Telemetry pipeline statistics counters.
//!
//! All counters are `u64` (never overflow in practice).
//! `queue_high_water` is `u32` — bounded by `QUEUE_DEPTH` const-generic.
