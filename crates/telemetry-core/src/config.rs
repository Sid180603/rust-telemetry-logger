//! Convenient [`Pipeline`] type aliases with pre-sized const generics.
//!
//! Use [`DefaultPipeline`] for host simulation and embedded-Linux targets.
//! Use [`McuPipeline`] for the bare-metal `RP2350` firmware where `SRAM` is
//! the scarce resource (8-record queue depth instead of 64).

use crate::pipeline::Pipeline;

// ──────────────────────────────────────────────────────────────────────────────
// Size constants
// ──────────────────────────────────────────────────────────────────────────────

/// Framer buffer capacity (bytes) for both host and MCU targets.
///
/// Must accommodate the largest possible `COBS`-encoded frame, which is
/// `HEADER_LEN(9) + MAX_PAYLOAD(480) + CRC16_LEN(2)` raw bytes, `COBS`-encoded
/// to ≤ 493 bytes.  512 provides a safety margin.
pub const DEFAULT_MAX_FRAME: usize = 512;

/// Ring-buffer depth for host / embedded-Linux targets (64 records).
pub const HOST_QUEUE_DEPTH: usize = 64;

/// Ring-buffer depth for bare-metal `MCU` targets (8 records, minimises `SRAM` use).
pub const MCU_QUEUE_DEPTH: usize = 8;

// ──────────────────────────────────────────────────────────────────────────────
// Type aliases
// ──────────────────────────────────────────────────────────────────────────────

/// Pipeline pre-configured for host simulation and embedded-Linux targets.
///
/// 512-byte framer buffer, 64-record queue.
pub type DefaultPipeline = Pipeline<DEFAULT_MAX_FRAME, HOST_QUEUE_DEPTH>;

/// Pipeline pre-configured for bare-metal `MCU` targets.
///
/// 512-byte framer buffer, 8-record queue.
pub type McuPipeline = Pipeline<DEFAULT_MAX_FRAME, MCU_QUEUE_DEPTH>;

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::FilterConfig;

    #[test]
    fn default_pipeline_constructs() {
        let _ = DefaultPipeline::new(FilterConfig::allow_all());
    }

    #[test]
    fn mcu_pipeline_constructs() {
        let _ = McuPipeline::new(FilterConfig::allow_all());
    }
}
