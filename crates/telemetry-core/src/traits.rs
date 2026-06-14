//! Backend-facing traits that decouple the portable pipeline core from hardware.
//!
//! - [`PacketSource`]: produce raw bytes (UART, SPI, file, TCP, …).
//! - [`Storage`]: persist serialised records (SD card, file, …).
//! - [`Clock`]: provide a monotonic timestamp in **microseconds**.
