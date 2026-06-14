# Memory Model

> Full content added in Phase 7. This stub ensures the file exists for cross-references.

## Design principle

All buffers in `telemetry-core` are **statically allocated** with capacities set at
compile time via const-generic parameters.  There is no heap allocation (`no_std`,
no `alloc`).

## Buffer inventory (worst-case)

| Buffer | Type | Capacity | Located in |
|---|---|---|---|
| Framer receive buffer | `heapless::Vec<u8, MAX_FRAME>` | `MAX_FRAME` bytes | `framer::Framer` |
| Ring buffer | `heapless::Deque<StoredRecord, QUEUE_DEPTH>` | `QUEUE_DEPTH` records | `ringbuf::RingBuf` |

Default capacities (subject to change in Phase 1):
- `MAX_FRAME` = 512 bytes
- `QUEUE_DEPTH` = 64 records

## Unsafe inventory

`telemetry-core` uses `#![forbid(unsafe_code)]` — zero `unsafe` blocks.

Hardware crate (`telemetry-fw`) unsafe usage:
- _To be documented in Phase 5/6._

## Stack usage

_To be measured with `cargo-call-stack` in Phase 7._

## Panic strategy

- **Firmware:** `panic-probe` (prints panic info via defmt RTT then halts).
- **Host / Linux:** normal Rust unwinding.
- Hot paths in `telemetry-core` are designed to never panic (no `unwrap`/`expect`
  in library code; all fallible paths return `Result` or `Outcome`).
