//! `telemetry-core` — portable, `no_std`, heap-free telemetry pipeline.
//!
//! This crate implements the full data pipeline:
//! `Input → Framer → Validator → Filter → RingBuffer → RecordEncoder → Stats`
//!
//! It has **no dependency on `std`** or heap allocation; all buffers use fixed
//! capacities via const-generic parameters.  The same code runs identically on:
//! - a host simulation (`std` binary),
//! - an embedded Linux daemon (Yocto / systemd), and
//! - bare-metal firmware (RP2350 / Pico 2, `no_std`).
//!
//! The [`Pipeline`] struct is the main entry point.  Consumers wire it up by
//! implementing the [`traits::PacketSource`], [`traits::Storage`], and
//! [`traits::Clock`] traits.
#![no_std]
#![forbid(unsafe_code)]

// In test builds the harness links std; declare it so test modules can use
// `std::` paths (e.g. std::vec::Vec for growing test buffers).
// When compiled with the `testutils` feature outside a test build, std is
// available through the normal link path on std targets.
#[cfg(any(test, feature = "testutils"))]
extern crate std;

// Re-export the public surface.  Modules are stubbed now and filled in Phase 1.
pub mod config;
pub mod error;
pub mod filter;
pub mod frame;
pub mod framer;
pub mod pipeline;
pub mod protocol;
pub mod record;
pub mod ringbuf;
pub mod stats;
pub mod traits;
pub mod validator;
