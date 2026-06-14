# Architecture

> Full content added in Phase 7. This stub ensures the file exists for cross-references.

## Overview

The system is structured as a Cargo workspace with a single portable core crate
(`telemetry-core`) that implements the complete data pipeline, plus separate backend
crates that wire in hardware-specific I/O.

See the [README mermaid diagram](../README.md) for the high-level view.

## Pipeline stages

| Stage | Crate | Responsibility |
|---|---|---|
| Framer | `telemetry-core::framer` | COBS destuff → reconstruct frames |
| Validator | `telemetry-core::validator` | CRC-16 + length + seq-gap |
| Filter | `telemetry-core::filter` | Allow/block by type + severity |
| Ring buffer | `telemetry-core::ringbuf` | Burst absorption, overflow counter |
| Record encoder | `telemetry-core::record` | `postcard` enum → CRC-32 |
| Stats | `telemetry-core::stats` | Counters: ok, dropped, crc_fail, … |

## Crate dependency graph

```
telemetry-fw  ──────────────────────────────┐
telemetry-linux  ──── telemetry-std ────────┤──► telemetry-core
telemetry-host   ──── telemetry-std ────────┘
tools/packet-generator ──────────────────────────► telemetry-core
tools/telemetry-decode ──────────────────────────► telemetry-core
```
