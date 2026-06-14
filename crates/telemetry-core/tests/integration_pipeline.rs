//! Integration tests for the full `telemetry-core` pipeline.
//!
//! These tests exercise the end-to-end path:
//! `ingest(bytes, clock) → framer → validator → filter → ring-buffer → drain(storage)`

use telemetry_core::{
    config::DefaultPipeline,
    error::Error,
    filter::FilterConfig,
    frame::{Frame, Header, MAX_COBS_FRAME_BYTES},
    pipeline::Pipeline,
    protocol::{MAX_PAYLOAD, PROTOCOL_VERSION},
    record,
    record::StoredRecord,
    traits::test_support::{CapturingStorage, FailStorage, SimClock},
};

fn make_cobs(seq: u32, packet_type: u8, severity: u8, payload: &[u8]) -> Vec<u8> {
    let mut p: heapless::Vec<u8, MAX_PAYLOAD> = heapless::Vec::new();
    for &b in payload {
        p.push(b).ok();
    }
    let frame = Frame {
        header: Header {
            version: PROTOCOL_VERSION,
            packet_type,
            severity,
            sequence: seq,
            payload_len: p.len() as u16,
        },
        payload: p,
        crc_bytes: [0; 2],
    };
    let mut buf = [0u8; MAX_COBS_FRAME_BYTES];
    let n = frame.encode_cobs(&mut buf).expect("frame fits");
    buf[..n].to_vec()
}

fn valid(seq: u32) -> Vec<u8> {
    make_cobs(
        seq, 0x01, /* Heartbeat */
        0x01, /* Info */
        b"payload",
    )
}

/// Decode all `0x00`-delimited records from a flat byte slice.
fn decode_all(raw: &[u8]) -> Vec<StoredRecord> {
    let mut records = Vec::new();
    let mut pos = 0;
    while pos < raw.len() {
        // Find next 0x00 delimiter.
        let rel = raw[pos..].iter().position(|&b| b == 0x00);
        let Some(rel_end) = rel else { break };
        let end = pos + rel_end + 1; // inclusive of 0x00
        match record::decode(&raw[pos..end]) {
            Ok(r) => records.push(r),
            Err(_) => {} // skip corrupt record
        }
        pos = end;
    }
    records
}

// ──────────────────────────────────────────────────────────────────────────────
// Test 1: mixed stream → correct Stats
// ──────────────────────────────────────────────────────────────────────────────

#[test]
fn test_ingest_mixed_stream() {
    let clock = SimClock { micros: 0 };
    let mut pipeline = DefaultPipeline::new(FilterConfig::allow_all());

    // Frame 0: valid seq=0 → ok
    pipeline.ingest(&valid(0), &clock);
    // Frame 1: valid seq=1 → ok
    pipeline.ingest(&valid(1), &clock);
    // Unknown packet_type=0xFF (valid COBS+CRC but type unknown → filtered)
    pipeline.ingest(&make_cobs(2, 0xFF, 0x01, b""), &clock);
    // Seq=2 was skipped so seq=3 causes a gap (expected 2, got 3 → seq_gap).
    // Note: after gap, validator re-syncs to 3.
    pipeline.ingest(&valid(3), &clock);

    let s = pipeline.stats();
    assert_eq!(s.ok, 2, "ok");
    assert_eq!(s.filtered, 1, "filtered (unknown type)");
    assert_eq!(s.seq_gap, 1, "seq_gap");
    assert_eq!(s.crc_fail, 0, "crc_fail");
}

// ──────────────────────────────────────────────────────────────────────────────
// Test 2: drain round-trip — decode records match ingested frames
// ──────────────────────────────────────────────────────────────────────────────

#[test]
fn test_drain_roundtrip() {
    let clock = SimClock { micros: 99_000 };
    let mut pipeline = Pipeline::<512, 8>::new(FilterConfig::allow_all());

    for seq in 0..3u32 {
        pipeline.ingest(&make_cobs(seq, 0x02, 0x02, b"sensor"), &clock);
    }
    assert_eq!(pipeline.stats().ok, 3);

    let mut storage = CapturingStorage::new();
    let drained = pipeline.drain(&mut storage).expect("drain succeeds");
    assert_eq!(drained, 3);
    assert_eq!(storage.flush_count, 1);

    // Decode all records from the captured bytes.
    let records = decode_all(storage.written());
    assert_eq!(records.len(), 3);

    for (i, r) in records.iter().enumerate() {
        let StoredRecord::V1(v1) = r else {
            panic!("expected V1")
        };
        assert_eq!(v1.timestamp_us, 99_000);
        assert_eq!(v1.sequence, i as u32);
        assert_eq!(v1.packet_type, 0x02);
        assert_eq!(v1.severity, 0x02);
        assert_eq!(&v1.payload[..], b"sensor");
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Test 3: buffer overflow → buffer_full + dropped increment
// ──────────────────────────────────────────────────────────────────────────────

#[test]
fn test_buffer_overflow() {
    let clock = SimClock { micros: 0 };
    // Only 2 slots in the queue.
    let mut pipeline = Pipeline::<512, 2>::new(FilterConfig::allow_all());

    for seq in 0..5u32 {
        pipeline.ingest(&valid(seq), &clock);
    }

    let s = pipeline.stats();
    assert_eq!(s.ok, 2, "only 2 fit in the queue");
    assert_eq!(s.dropped, 3, "3 frames dropped due to ring-buffer overflow");
}

// ──────────────────────────────────────────────────────────────────────────────
// Test 4: FailStorage → Error::Storage + write_fail incremented
// ──────────────────────────────────────────────────────────────────────────────

#[test]
fn test_drain_write_fail() {
    let clock = SimClock { micros: 0 };
    let mut pipeline = Pipeline::<512, 4>::new(FilterConfig::allow_all());
    pipeline.ingest(&valid(0), &clock);
    assert_eq!(pipeline.stats().ok, 1);

    let mut fail = FailStorage;
    let result = pipeline.drain(&mut fail);
    assert!(
        matches!(result, Err(Error::Storage)),
        "expected Storage error"
    );
    assert_eq!(pipeline.stats().write_fail, 1);
}

// ──────────────────────────────────────────────────────────────────────────────
// Test 5: drain on empty queue returns Ok(0) and still calls flush
// ──────────────────────────────────────────────────────────────────────────────

#[test]
fn test_drain_empty_queue() {
    let mut pipeline = DefaultPipeline::new(FilterConfig::allow_all());
    let mut storage = CapturingStorage::new();
    let n = pipeline.drain(&mut storage).expect("drain empty queue");
    assert_eq!(n, 0);
    assert_eq!(storage.flush_count, 1);
    assert_eq!(storage.written().len(), 0);
}

// ──────────────────────────────────────────────────────────────────────────────
// Test 6: severity filter blocks low-severity frames
// ──────────────────────────────────────────────────────────────────────────────

#[test]
fn test_severity_filter() {
    let clock = SimClock { micros: 0 };
    let cfg = FilterConfig {
        min_severity: 0x02, // Warning and above
        type_allowlist: None,
    };
    let mut pipeline = DefaultPipeline::new(cfg);

    // severity=0x01 (Info) → filtered
    pipeline.ingest(&make_cobs(0, 0x01, 0x01, b""), &clock);
    // severity=0x02 (Warning) → ok
    pipeline.ingest(&make_cobs(1, 0x01, 0x02, b""), &clock);

    assert_eq!(pipeline.stats().filtered, 1);
    assert_eq!(pipeline.stats().ok, 1);
}

// ──────────────────────────────────────────────────────────────────────────────
// Test 7: stats reset via stats_mut
// ──────────────────────────────────────────────────────────────────────────────

#[test]
fn test_stats_reset() {
    let clock = SimClock { micros: 0 };
    let mut pipeline = DefaultPipeline::new(FilterConfig::allow_all());
    pipeline.ingest(&valid(0), &clock);
    assert_eq!(pipeline.stats().ok, 1);

    *pipeline.stats_mut() = Default::default();
    assert_eq!(pipeline.stats().ok, 0);
}

// ──────────────────────────────────────────────────────────────────────────────
// Test 8: multi-drain — queue empties after first drain
// ──────────────────────────────────────────────────────────────────────────────

#[test]
fn test_multi_drain() {
    let clock = SimClock { micros: 0 };
    let mut pipeline = Pipeline::<512, 4>::new(FilterConfig::allow_all());
    pipeline.ingest(&valid(0), &clock);
    pipeline.ingest(&valid(1), &clock);

    let mut s = CapturingStorage::new();
    assert_eq!(pipeline.drain(&mut s).unwrap(), 2);
    // Second drain on empty queue
    assert_eq!(pipeline.drain(&mut s).unwrap(), 0);
}
