# Board Bring-Up Guide

> Full content added in Phase 5. This stub ensures the file exists for cross-references.

## Target: RP2350 / Raspberry Pi Pico 2

### Prerequisites

- Rust target installed: `rustup target add thumbv8m.main-none-eabihf`
- `probe-rs` installed: `cargo install probe-rs-tools`
- `flip-link` installed: `cargo install flip-link`
- A SWD debug probe (e.g. Raspberry Pi Debug Probe, J-Link, or another Pico running `picoprobe`)

### Wiring

_Documented in Phase 5._

### Flashing

```bash
# Debug build (symbols, defmt RTT)
cargo fw

# Release build + flash
cargo fw-flash
```

### RTT / defmt output

```bash
probe-rs attach --chip RP2350 --protocol swd
```

_Or use `cargo embed` with `Embed.toml` (added in Phase 5)._

## Target: Raspberry Pi 4/5 (Yocto image)

See [yocto.md](yocto.md) for the full embedded Linux bring-up procedure.
