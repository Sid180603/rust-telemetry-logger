# Wire Protocol and Storage Format

This document is the **authoritative specification** for all frame and record
formats used by the telemetry logger.  All code in `telemetry-core` is derived
from this spec; any discrepancy is a bug.

---

## 1. Wire Frame Format

Frames are transmitted over UART (or any byte-stream transport) using
**COBS encoding** (Consistent Overhead Byte Stuffing, RFC-style) with the
`0x00` byte as the packet delimiter.

### 1.1 COBS Overview

COBS encodes an arbitrary byte sequence so that `0x00` never appears in the
encoded payload.  The encoder replaces all `0x00` bytes and inserts overhead
bytes to preserve the structure; the decoded output is the original payload.

- **Delimiter:** `0x00` — unambiguously marks the start/end of a packet.
- **Self-synchronising:** after any corruption or stream interruption, the
  receiver can re-sync by waiting for the next `0x00` delimiter.
- **Zero-copy destuffing:** the `cobs` crate (`cobs::decode_in_place`) handles
  this in `O(n)` with no allocation.

The framer is a **hand-rolled typestate state machine** (not
`postcard::CobsAccumulator`).  The typestate approach gives compile-time
proof of correct state transitions and is a deliberate portfolio signal.

### 1.2 Frame Field Layout (inside the COBS envelope)

After COBS decoding the received bytes (up to and excluding the `0x00`
delimiter) look like:

```
┌──────────┬──────────┬────────────┬──────────┬─────────┬─────────────────┬──────────────┐
│ version  │  type    │  severity  │ sequence │  len    │    payload      │   CRC-16     │
│  1 byte  │  1 byte  │   1 byte   │  4 bytes │ 2 bytes │  `len` bytes    │   2 bytes    │
└──────────┴──────────┴────────────┴──────────┴─────────┴─────────────────┴──────────────┘
 ^                                                                          ^
 CRC-16 covers all fields including this byte                              CRC-16 (LE)
```

| Field | Type | Description |
|---|---|---|
| `version` | `u8` | Protocol version. Currently `0x01`. Increment only for breaking wire changes. |
| `type` | `u8` | Packet type. See §1.3. |
| `severity` | `u8` | Severity level. See §1.4. |
| `sequence` | `u32` LE | Monotonically increasing sequence number (per-source, wraps at `u32::MAX`). |
| `len` | `u16` LE | Length of the `payload` field in bytes. Max `MAX_PAYLOAD` (default 480). |
| `payload` | `[u8]` | Application data, `len` bytes long. |
| `crc` | `u16` LE | CRC-16/IBM-SDLC (X.25) over all preceding bytes. See §1.5. |

**Total minimum frame size (zero-length payload):** 11 bytes before COBS encoding.

### 1.3 Packet Types (`type` field)

```rust
#[non_exhaustive]
#[repr(u8)]
pub enum PacketType {
    Heartbeat  = 0x01,
    SensorData = 0x02,
    Event      = 0x03,
    Diagnostic = 0x04,
    Control    = 0x05,
    // 0x06–0xFE reserved for future types
    // 0xFF = unknown / unrecognised (not transmitted; used internally)
}
```

Frames with an unrecognised `type` byte are **rejected** with
`Reason::Filtered` (not a CRC error) and counted in `Stats::filtered`.

### 1.4 Severity Levels (`severity` field)

```rust
#[non_exhaustive]
#[repr(u8)]
pub enum Severity {
    Debug   = 0x00,
    Info    = 0x01,
    Warning = 0x02,
    Error   = 0x03,
    Fatal   = 0x04,
    // 0x05–0xFF reserved
}
```

The filter uses a **threshold**: frames with `severity < threshold` are
dropped.  The default threshold is `Severity::Debug` (pass all).

Frames with an unrecognised `severity` byte are rejected with `Reason::Filtered`.

### 1.5 Wire CRC: CRC-16/IBM-SDLC (X.25)

```
Polynomial:  0x1021
Init:        0xFFFF
RefIn:       true
RefOut:      true
XorOut:      0xFFFF
Check:       0x906E          (CRC of ASCII "123456789")
Residue:     0xF0B8
```

The **residue trick**: computing the CRC over `[all fields including crc bytes]`
yields the constant `0xF0B8` on a valid frame.  The validator can therefore
check validity without splitting off the CRC field.

Crate constant: `crc::CRC_16_IBM_SDLC`.

---

## 2. Storage Record Format

After a frame passes validation and filtering, it is combined with metadata
and persisted as a **`StoredRecord`**.

### 2.1 `StoredRecord` Enum (postcard versioning)

```rust
#[derive(serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum StoredRecord {
    V1(RecordV1),
    // Future: V2(RecordV2), etc.
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct RecordV1 {
    pub timestamp_us: u64,       // monotonic microseconds since boot
    pub sequence:     u32,       // wire frame sequence number
    pub packet_type:  u8,        // PacketType discriminant
    pub severity:     u8,        // Severity discriminant
    pub payload:      heapless::Vec<u8, MAX_PAYLOAD>,
}
```

**Versioning mechanism:** `postcard` serialises enums with a leading
discriminant byte.  For `V1`, this byte is `0x00`.  A decoder that sees an
unknown discriminant rejects the record gracefully (logs a warning, moves on)
without panicking.  This is the schema-evolution contract.

**To add a new version:** add `V2(RecordV2)` to the enum.  Old decoders skip
`V2` records; new decoders can read both.  Never reuse a discriminant.

### 2.2 On-Disk Record Framing (within a segment file)

Each stored record is wrapped in a second COBS envelope:

```
COBS( postcard_bytes(StoredRecord) ++ CRC-32/ISO-HDLC(postcard_bytes) ) 0x00
```

- The `0x00` delimiter after the COBS envelope enables **partial-write recovery**:
  on boot, the reader scans for the last complete `0x00`-terminated record and
  truncates any partial write beyond it.
- The CRC-32 covers the postcard bytes (including the `StoredRecord` enum
  discriminant / version byte), so version and content are integrity-protected.

**postcard API used:** `postcard::to_slice_crc32` (encode) and
`postcard::from_bytes_crc32` (decode), both with the CRC-32/ISO-HDLC algorithm.

### 2.3 Storage CRC: CRC-32/ISO-HDLC

```
Polynomial:  0x04C11DB7
Init:        0xFFFFFFFF
RefIn:       true
RefOut:      true
XorOut:      0xFFFFFFFF
Check:       0xCBF43926   (same as Ethernet / zlib / PNG CRC-32)
```

Crate constant: `crc::CRC_32_ISO_HDLC`.

### 2.4 Segment Files

The `FileStorage` implementation (in `telemetry-std`) writes records into
**rotating segment files**:

```
logs/
  seg-00000.bin    ← current segment
  seg-00001.bin    ← rolled (full)
  seg-00002.bin    ← rolled (full)
  ...
```

A new segment is started when the current file reaches `max_segment_bytes`
(configurable; default 1 MiB).  Segment files never have a partial last record
after a clean shutdown (the storage flushes on drop / `SIGTERM`).

---

## 3. Timestamp Convention

All timestamps are **monotonic microseconds** (`u64`) since the source's boot
or process start.  Rationale:

- The RP2350 hardware timer is a native 64-bit 1 µs counter — zero-cost to read.
- Embassy `Instant` is µs-based — no conversion needed.
- `u64` microseconds overflows in ~584,000 years — not a concern.
- Sub-millisecond resolution is useful for burst/latency profiling in stats.

Wall-clock correlation: if needed, backends record a boot-wall-clock offset
once (at startup) and store it in a sidecar file or segment header.

---

## 4. Schema Evolution Rules

1. **Wire version:** bump `version` only for **breaking** wire-format changes.
   Receivers reject unknown versions after logging a warning.
2. **StoredRecord:** add a new enum variant (`V2`, `V3`, …).  Never reuse a
   discriminant value.  Old decoders skip unknown variants; they do **not** crash.
3. **RecordV1 fields:** only **append** new optional fields at the end
   (postcard deserialisation is forward-compatible for appended fields).
4. **PacketType / Severity enums:** mark `#[non_exhaustive]`.  New variants
   are assigned unused byte values.  Receivers that don't know a variant
   treat it as `Filtered`.

---

## 5. Constants Reference

| Constant | Value | Notes |
|---|---|---|
| `PROTOCOL_VERSION` | `0x01` | Wire frame version byte |
| `MAX_PAYLOAD` | `480` | Max payload bytes per frame |
| `MAX_FRAME` | `512` | Max COBS-decoded frame bytes (`MAX_PAYLOAD` + 11 + overhead) |
| `QUEUE_DEPTH` | `64` | Default ring buffer capacity (records) |
| `DEFAULT_SEGMENT_BYTES` | `1_048_576` | 1 MiB segment rotation threshold |
| CRC-16 polynomial | `0x1021` | IBM-SDLC / X.25 |
| CRC-32 polynomial | `0x04C11DB7` | ISO-HDLC / Ethernet / zlib |
