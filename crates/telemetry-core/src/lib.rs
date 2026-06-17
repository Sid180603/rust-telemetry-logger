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
//! # Feature tiers
//!
//! Cargo features are **additive**: `full` is a strict superset of `encode`.
//! - **`encode`** (base slice): the wire contract only — [`protocol`] constants
//!   plus [`frame::Frame`] / [`frame::Header`] and [`frame::Frame::encode_cobs`].
//!   This is all a *sender* device needs.
//! - **`full`** (default): adds the receive/store pipeline on top of `encode`.
#![cfg_attr(
    feature = "full",
    doc = "\nThe [`pipeline::Pipeline`] struct is the main entry point.  Consumers wire it up by implementing the [`traits::PacketSource`], [`traits::Storage`], and [`traits::Clock`] traits."
)]
#![no_std]
#![forbid(unsafe_code)]

// In test builds the harness links std; declare it so test modules can use
// `std::` paths (e.g. std::vec::Vec for growing test buffers).
// When compiled with the `testutils` feature outside a test build, std is
// available through the normal link path on std targets.
#[cfg(any(test, feature = "testutils"))]
extern crate std;

// Public surface, split into two additive feature tiers.
//
// `encode` (the base slice): the irreducible wire contract — protocol
// constants/enums plus the Frame/Header data types and `encode_cobs`.  A
// sender device needs only this.
//
// `full` (= `encode` + everything below): the receive/store pipeline.  All
// existing consumers build with `default = ["full"]`, so they are unaffected.
#[cfg(feature = "encode")]
pub mod frame;
#[cfg(feature = "encode")]
pub mod protocol;

#[cfg(feature = "full")]
pub mod config;
#[cfg(feature = "full")]
pub mod error;
#[cfg(feature = "full")]
pub mod filter;
#[cfg(feature = "full")]
pub mod framer;
#[cfg(feature = "full")]
pub mod pipeline;
#[cfg(feature = "full")]
pub mod record;
#[cfg(feature = "full")]
pub mod ringbuf;
#[cfg(feature = "full")]
pub mod stats;
#[cfg(feature = "full")]
pub mod traits;
#[cfg(feature = "full")]
pub mod validator;
