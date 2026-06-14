//! Backend-facing traits that decouple the portable pipeline core from hardware.
//!
//! - [`PacketSource`]: produce raw bytes (UART, SPI, file, TCP, …).
//! - [`Storage`]: persist serialised records (SD card, file, …).
//! - [`Clock`]: provide a monotonic timestamp in **microseconds** (`u64`).
//!
//! All three traits are implemented for real hardware in the platform-specific
//! crates (`telemetry-std`, `telemetry-linux`, `telemetry-fw`).  In-memory
//! implementations for testing live in [`test_support`].

// ──────────────────────────────────────────────────────────────────────────────
// PacketSource
// ──────────────────────────────────────────────────────────────────────────────

/// Source of raw frame bytes (UART, SPI, file, socket, …).
pub trait PacketSource {
    /// Error type returned when reading fails.
    type Error;

    /// Read up to `buf.len()` bytes into `buf`, returning the number of bytes read.
    ///
    /// # Errors
    ///
    /// Returns `Err(Self::Error)` if the underlying transport fails.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error>;
}

// ──────────────────────────────────────────────────────────────────────────────
// Storage
// ──────────────────────────────────────────────────────────────────────────────

/// Persistent-storage sink for encoded records.
pub trait Storage {
    /// Error type returned when writing or flushing fails.
    type Error;

    /// Write all bytes in `data` to storage.
    ///
    /// # Errors
    ///
    /// Returns `Err(Self::Error)` if the write fails.
    fn write(&mut self, data: &[u8]) -> Result<(), Self::Error>;

    /// Flush any buffered data to the underlying medium.
    ///
    /// # Errors
    ///
    /// Returns `Err(Self::Error)` if the flush fails.
    fn flush(&mut self) -> Result<(), Self::Error>;
}

// ──────────────────────────────────────────────────────────────────────────────
// Clock
// ──────────────────────────────────────────────────────────────────────────────

/// Monotonic clock providing microsecond timestamps.
///
/// Implementations must return a value that never decreases during a single
/// power cycle.  The epoch (zero point) is platform-defined; only differences
/// between timestamps are meaningful.
pub trait Clock {
    /// Return the current monotonic time in **microseconds**.
    fn now(&self) -> u64;
}

// ──────────────────────────────────────────────────────────────────────────────
// Test support (std, cfg(test) only)
// ──────────────────────────────────────────────────────────────────────────────

/// In-memory trait implementations for unit testing.
///
/// Available in test builds and when the `testutils` feature is enabled.
/// These implementations use `std::vec::Vec<u8>` for capture buffers.
/// They require `std` and must **not** be enabled in production firmware builds.
#[cfg(any(test, feature = "testutils"))]
pub mod test_support {
    use std::vec::Vec as StdVec;

    use super::{Clock, Storage};

    // ── SimClock ──────────────────────────────────────────────────────────────

    /// Deterministic clock for testing: returns `.micros` unchanged.
    #[derive(Debug)]
    pub struct SimClock {
        /// The timestamp value returned by [`Clock::now`].
        pub micros: u64,
    }

    impl Clock for SimClock {
        fn now(&self) -> u64 {
            self.micros
        }
    }

    // ── CapturingStorage ─────────────────────────────────────────────────────

    /// Storage implementation that captures all written bytes in memory.
    #[derive(Debug)]
    pub struct CapturingStorage {
        data: StdVec<u8>,
        /// Number of times `flush` has been called.
        pub flush_count: u32,
    }

    impl CapturingStorage {
        /// Create an empty capturing storage.
        pub fn new() -> Self {
            Self {
                data: StdVec::new(),
                flush_count: 0,
            }
        }

        /// All bytes written so far, in order.
        pub fn written(&self) -> &[u8] {
            &self.data
        }
    }

    impl Storage for CapturingStorage {
        type Error = ();

        fn write(&mut self, data: &[u8]) -> Result<(), ()> {
            self.data.extend_from_slice(data);
            Ok(())
        }

        fn flush(&mut self) -> Result<(), ()> {
            self.flush_count += 1;
            Ok(())
        }
    }

    // ── FailStorage ───────────────────────────────────────────────────────────

    /// Storage implementation that always returns `Err(())`.
    #[derive(Debug)]
    pub struct FailStorage;

    impl Storage for FailStorage {
        type Error = ();

        fn write(&mut self, _data: &[u8]) -> Result<(), ()> {
            Err(())
        }

        fn flush(&mut self) -> Result<(), ()> {
            Err(())
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::test_support::{CapturingStorage, FailStorage, SimClock};
    use super::{Clock, Storage};

    #[test]
    fn sim_clock_returns_micros() {
        let c = SimClock { micros: 42_000 };
        assert_eq!(c.now(), 42_000);
    }

    #[test]
    fn capturing_storage_captures_bytes() {
        let mut s = CapturingStorage::new();
        s.write(b"hello").unwrap();
        s.write(b" world").unwrap();
        assert_eq!(s.written(), b"hello world");
    }

    #[test]
    fn capturing_storage_flush_increments_counter() {
        let mut s = CapturingStorage::new();
        assert_eq!(s.flush_count, 0);
        s.flush().unwrap();
        s.flush().unwrap();
        assert_eq!(s.flush_count, 2);
    }

    #[test]
    fn fail_storage_write_returns_err() {
        let mut s = FailStorage;
        assert!(s.write(b"data").is_err());
    }

    #[test]
    fn fail_storage_flush_returns_err() {
        let mut s = FailStorage;
        assert!(s.flush().is_err());
    }
}
