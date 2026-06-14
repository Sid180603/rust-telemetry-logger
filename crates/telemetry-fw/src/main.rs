//! `telemetry-fw` — RP2350 / Pico 2 bare-metal firmware.
//!
//! v1 (Phase 5): blocking embedded-hal, UART ISR → ring-buffer → SD card.
//! v2 (Phase 6): Embassy async, ingestion task + storage task.
//!
//! `telemetry-core` is reused **unchanged** from the host builds.
#![no_std]
#![no_main]

// Stub — Phase 5 will wire up rp235x-hal, cortex-m-rt, defmt, etc.
// This file must stay `#![no_std] #![no_main]` so the workspace
// can verify the no_std build target compiles cleanly.

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}
