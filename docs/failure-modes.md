# Failure Modes

> Full content added in Phase 7. This stub ensures the file exists for cross-references.

## Failure taxonomy

| Failure | Detection | Recovery |
|---|---|---|
| CRC mismatch on wire frame | `validator::Validator` | Frame rejected, `Stats::crc_fail++`, framer resyncs |
| Truncated frame | Framer timeout / delimiter | Framer resyncs to next `0x00` delimiter |
| Sequence gap | `validator::Validator` | Rejection with `Reason::SequenceGap`, counted in stats |
| Ring buffer overflow | `ringbuf::RingBuf` | Oldest record dropped, `Stats::buffer_full++` |
| Storage write failure | `pipeline::Pipeline::drain` | `Error::Storage` returned, `Stats::write_fail++` |
| Partial write (power loss) | COBS record framing in segment | On-boot scan: truncate to last valid `0x00`-terminated record |
| SD card init failure (firmware) | `embedded-sdmmc` init | _Phase 5: log via defmt, attempt retry with backoff_ |

## Fault injection tests

_To be added in Phase 6 (firmware watchdog + fault injection) and Phase 2 (packet-generator corruption modes)._
