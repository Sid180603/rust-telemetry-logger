//! End-to-end integration tests for the Phase 2 host simulation loop.
//!
//! These tests drive `packet-generator`, `telemetry-host`, and
//! `telemetry-decode` as real child processes (via `assert_cmd`) and verify the
//! closed loop:
//!
//! ```text
//!   packet-generator → telemetry-host (pipeline + FileStorage) → telemetry-decode
//! ```
//!
//! Additionally, a pure-Rust round-trip test exercises `telemetry-std`'s
//! `FileStorage` + `read_segment_file` without spawning any processes.

use assert_cmd::Command;
use assert_fs::TempDir;
use predicates::prelude::*;

// ── helpers ───────────────────────────────────────────────────────────────────

fn generator() -> Command {
    Command::cargo_bin("packet-generator").expect("packet-generator binary must be built")
}

fn host() -> Command {
    Command::cargo_bin("telemetry-host").expect("telemetry-host binary must be built")
}

fn decode() -> Command {
    Command::cargo_bin("telemetry-decode").expect("telemetry-decode binary must be built")
}

// ── Round-trip: encode → FileStorage → read_segment_file → decode ─────────────

/// Directly exercise `telemetry-std` without spawning processes.
#[test]
fn roundtrip_encode_file_storage_decode() {
    use telemetry_core::config::DefaultPipeline;
    use telemetry_core::filter::FilterConfig;
    use telemetry_core::record::StoredRecord;
    use telemetry_core::traits::Storage;
    use telemetry_std::{FileStorage, SystemClock, read_segment_file};

    let dir = TempDir::new().unwrap();
    let mut storage = FileStorage::new(dir.path(), 1024 * 1024).unwrap();
    let clock = SystemClock::new();

    // Build a tiny pipeline; ingest a valid frame, then drain to storage.
    let mut pipeline = DefaultPipeline::new(FilterConfig::allow_all());
    let frame = telemetry_core::frame::Frame::new(0x02, 0x01, 0, b"hello").expect("fits");
    let mut buf = [0u8; telemetry_core::frame::MAX_COBS_FRAME_BYTES];
    let n = frame.encode_cobs(&mut buf).expect("fits");
    pipeline.ingest(&buf[..n], &clock);
    pipeline.drain(&mut storage).expect("drain succeeds");
    storage.flush().unwrap();

    let seg = dir.path().join("seg-00001.bin");
    let records = read_segment_file(&seg).expect("file readable");
    let decoded: Vec<_> = records.into_iter().map(|r| r.expect("decode ok")).collect();

    assert_eq!(decoded.len(), 1, "one record per frame");
    let StoredRecord::V1(v) = &decoded[0] else {
        panic!("expected V1")
    };
    assert_eq!(v.sequence, 0);
    assert_eq!(v.packet_type, 0x02);
    assert_eq!(v.severity, 0x01);
    assert_eq!(&v.payload[..], b"hello");

    let _ = (&clock, &pipeline); // suppress unused warnings
}

// ── generator → file ─────────────────────────────────────────────────────────

/// `packet-generator` writes a deterministic byte stream to a file.
#[test]
fn generator_writes_to_file() {
    let dir = TempDir::new().unwrap();
    let out = dir.path().join("frames.bin");

    generator()
        .args([
            "--count",
            "20",
            "--seed",
            "42",
            "--out",
            &format!("file:{}", out.display()),
        ])
        .assert()
        .success();

    assert!(out.exists(), "output file must exist");
    let bytes = std::fs::read(&out).unwrap();
    assert!(!bytes.is_empty(), "output file must be non-empty");
    // COBS frames are delimited by 0x00; we should see at least one.
    assert!(bytes.contains(&0x00), "output must contain COBS delimiters");
}

// ── generator → host → segments ──────────────────────────────────────────────

/// Full pipeline: generator → host → at least one segment file created.
#[test]
fn host_creates_segments() {
    let gen_dir = TempDir::new().unwrap();
    let frames_path = gen_dir.path().join("frames.bin");
    let logs_dir = TempDir::new().unwrap();

    // Step 1: generate frames.
    generator()
        .args([
            "--count",
            "50",
            "--seed",
            "42",
            "--out",
            &format!("file:{}", frames_path.display()),
        ])
        .assert()
        .success();

    // Step 2: process with host.
    host()
        .args([
            "--in",
            &format!("file:{}", frames_path.display()),
            "--out-dir",
            logs_dir.path().to_str().unwrap(),
            "--stats-interval",
            "0",
        ])
        .assert()
        .success();

    // At least one segment must have been created.
    let segs: Vec<_> = std::fs::read_dir(logs_dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_str()
                .map(|n| n.starts_with("seg-") && n.ends_with(".bin"))
                .unwrap_or(false)
        })
        .collect();
    assert!(
        !segs.is_empty(),
        "host must produce at least one segment file"
    );
}

// ── generator stats: bad-crc frames increment crc_fail ───────────────────────

/// When all frames are corrupted, stats should show crc_fail > 0 and ok == 0.
#[test]
fn host_counts_bad_crc_frames() {
    let gen_dir = TempDir::new().unwrap();
    let frames_path = gen_dir.path().join("frames.bin");
    let logs_dir = TempDir::new().unwrap();

    generator()
        .args([
            "--count",
            "30",
            "--seed",
            "1",
            "--corrupt",
            "bad-crc",
            "--corrupt-rate",
            "1.0",
            "--out",
            &format!("file:{}", frames_path.display()),
        ])
        .assert()
        .success();

    // Host stderr will contain the stats line.
    host()
        .args([
            "--in",
            &format!("file:{}", frames_path.display()),
            "--out-dir",
            logs_dir.path().to_str().unwrap(),
            "--stats-interval",
            "0",
        ])
        .assert()
        .success()
        // The stats line "crc_fail=N" must show a non-zero count.
        .stderr(
            predicate::str::contains("crc_fail=")
                .and(predicate::str::is_match(r"crc_fail=[1-9]").unwrap()),
        );
}

// ── generator seq-gap → seq_gap counter ──────────────────────────────────────

/// When seq-gap corruption is applied at 100 % rate, seq_gap > 0 in the stats.
#[test]
fn host_counts_seq_gap_frames() {
    let gen_dir = TempDir::new().unwrap();
    let frames_path = gen_dir.path().join("frames.bin");
    let logs_dir = TempDir::new().unwrap();

    generator()
        .args([
            "--count",
            "40",
            "--seed",
            "2",
            "--corrupt",
            "seq-gap",
            "--corrupt-rate",
            "1.0",
            "--out",
            &format!("file:{}", frames_path.display()),
        ])
        .assert()
        .success();

    host()
        .args([
            "--in",
            &format!("file:{}", frames_path.display()),
            "--out-dir",
            logs_dir.path().to_str().unwrap(),
            "--stats-interval",
            "0",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_match(r"seq_gap=[1-9]").unwrap());
}

// ── segment rotation ──────────────────────────────────────────────────────────

/// With a tiny segment size, multiple segments must be created.
#[test]
fn host_rotates_segments() {
    let gen_dir = TempDir::new().unwrap();
    let frames_path = gen_dir.path().join("frames.bin");
    let logs_dir = TempDir::new().unwrap();

    // Generate enough frames that 64-byte segments will definitely overflow.
    generator()
        .args([
            "--count",
            "30",
            "--seed",
            "3",
            "--out",
            &format!("file:{}", frames_path.display()),
        ])
        .assert()
        .success();

    host()
        .args([
            "--in",
            &format!("file:{}", frames_path.display()),
            "--out-dir",
            logs_dir.path().to_str().unwrap(),
            "--segment-size",
            "64",
            "--stats-interval",
            "0",
        ])
        .assert()
        .success();

    let count = std::fs::read_dir(logs_dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_str()
                .map(|n| n.starts_with("seg-") && n.ends_with(".bin"))
                .unwrap_or(false)
        })
        .count();
    assert!(count >= 2, "expected multiple segments, got {count}");
}

// ── decode: table output ──────────────────────────────────────────────────────

/// Decode produces a non-empty table with the expected column headers.
#[test]
fn decode_table_output() {
    let gen_dir = TempDir::new().unwrap();
    let frames_path = gen_dir.path().join("frames.bin");
    let logs_dir = TempDir::new().unwrap();

    generator()
        .args([
            "--count",
            "10",
            "--seed",
            "10",
            "--out",
            &format!("file:{}", frames_path.display()),
        ])
        .assert()
        .success();

    host()
        .args([
            "--in",
            &format!("file:{}", frames_path.display()),
            "--out-dir",
            logs_dir.path().to_str().unwrap(),
            "--stats-interval",
            "0",
        ])
        .assert()
        .success();

    let seg = logs_dir.path().join("seg-00001.bin");
    decode()
        .args([seg.to_str().unwrap(), "--format", "table"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Seq"))
        .stdout(predicate::str::contains("Timestamp"))
        .stdout(predicate::str::contains("Type"))
        .stdout(predicate::str::contains("Severity"));
}

// ── decode: NDJSON output ─────────────────────────────────────────────────────

/// Decode with `--format ndjson` produces valid JSON objects (one per line).
#[test]
fn decode_ndjson_output() {
    let gen_dir = TempDir::new().unwrap();
    let frames_path = gen_dir.path().join("frames.bin");
    let logs_dir = TempDir::new().unwrap();

    generator()
        .args([
            "--count",
            "5",
            "--seed",
            "99",
            "--out",
            &format!("file:{}", frames_path.display()),
        ])
        .assert()
        .success();

    host()
        .args([
            "--in",
            &format!("file:{}", frames_path.display()),
            "--out-dir",
            logs_dir.path().to_str().unwrap(),
            "--stats-interval",
            "0",
        ])
        .assert()
        .success();

    let seg = logs_dir.path().join("seg-00001.bin");
    let output = decode()
        .args([seg.to_str().unwrap(), "--format", "ndjson"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = std::str::from_utf8(&output).unwrap();
    // Every non-empty line must parse as JSON.
    for line in stdout.lines().filter(|l| !l.is_empty()) {
        let parsed: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("invalid JSON line {line:?}: {e}"));
        assert!(parsed.get("seq").is_some(), "JSON must have 'seq' field");
        assert!(
            parsed.get("type_name").is_some(),
            "JSON must have 'type_name' field"
        );
    }
}

// ── decode: snapshot tests ────────────────────────────────────────────────────

/// Snapshot of NDJSON output for a fully deterministic 5-frame run.
/// On first run, `insta` writes the snapshot to `tests/snapshots/`.
/// On subsequent runs it compares against the saved snapshot.
/// The timestamp field (`ts_us`) is normalised to 0 before comparison so the
/// snapshot is stable across machines and timing variations.
#[test]
fn decode_ndjson_snapshot() {
    let gen_dir = TempDir::new().unwrap();
    let frames_path = gen_dir.path().join("frames.bin");
    let logs_dir = TempDir::new().unwrap();

    generator()
        .args([
            "--count",
            "5",
            "--seed",
            "777",
            "--out",
            &format!("file:{}", frames_path.display()),
        ])
        .assert()
        .success();

    host()
        .args([
            "--in",
            &format!("file:{}", frames_path.display()),
            "--out-dir",
            logs_dir.path().to_str().unwrap(),
            "--stats-interval",
            "0",
        ])
        .assert()
        .success();

    let seg = logs_dir.path().join("seg-00001.bin");
    let output = decode()
        .args([seg.to_str().unwrap(), "--format", "ndjson"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = std::str::from_utf8(&output).unwrap().trim().to_owned();
    let stable = normalize_json_timestamps(&stdout);
    insta::assert_snapshot!("ndjson_seed_777", stable);
}

/// Snapshot of table output for a fully deterministic 5-frame run.
///
/// The timestamp column is normalised to a fixed `<ts>` placeholder before
/// snapshotting by identifying it **by column position** (second `|`-delimited
/// cell), not by digit length.  This is the correct, stable approach — digit-
/// length heuristics break when the system clock hasn't advanced much at test
/// startup, producing a different number of digits between runs.
#[test]
fn decode_table_snapshot() {
    let gen_dir = TempDir::new().unwrap();
    let frames_path = gen_dir.path().join("frames.bin");
    let logs_dir = TempDir::new().unwrap();

    generator()
        .args([
            "--count",
            "5",
            "--seed",
            "777",
            "--out",
            &format!("file:{}", frames_path.display()),
        ])
        .assert()
        .success();

    host()
        .args([
            "--in",
            &format!("file:{}", frames_path.display()),
            "--out-dir",
            logs_dir.path().to_str().unwrap(),
            "--stats-interval",
            "0",
        ])
        .assert()
        .success();

    let seg = logs_dir.path().join("seg-00001.bin");
    let output = decode()
        .args([seg.to_str().unwrap(), "--format", "table"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = std::str::from_utf8(&output).unwrap().trim().to_owned();
    let stable = normalize_table_timestamps(&stdout);
    insta::assert_snapshot!("table_seed_777", stable);
}

// ── decode: --since filter ────────────────────────────────────────────────────

/// `--since N` drops records with sequence < N.
#[test]
fn decode_since_filter() {
    let gen_dir = TempDir::new().unwrap();
    let frames_path = gen_dir.path().join("frames.bin");
    let logs_dir = TempDir::new().unwrap();

    generator()
        .args([
            "--count",
            "20",
            "--seed",
            "5",
            "--out",
            &format!("file:{}", frames_path.display()),
        ])
        .assert()
        .success();

    host()
        .args([
            "--in",
            &format!("file:{}", frames_path.display()),
            "--out-dir",
            logs_dir.path().to_str().unwrap(),
            "--stats-interval",
            "0",
        ])
        .assert()
        .success();

    let seg = logs_dir.path().join("seg-00001.bin");

    // All records.
    let all_output = decode()
        .args([seg.to_str().unwrap(), "--format", "ndjson"])
        .output()
        .unwrap();
    let all_lines = std::str::from_utf8(&all_output.stdout)
        .unwrap()
        .lines()
        .filter(|l| !l.is_empty())
        .count();

    // Records from seq >= 10.
    let filtered_output = decode()
        .args([seg.to_str().unwrap(), "--format", "ndjson", "--since", "10"])
        .output()
        .unwrap();
    let filtered_lines = std::str::from_utf8(&filtered_output.stdout)
        .unwrap()
        .lines()
        .filter(|l| !l.is_empty())
        .count();

    assert!(
        filtered_lines < all_lines,
        "--since should reduce the record count ({filtered_lines} < {all_lines})"
    );
}

// ── decode: --stats flag ──────────────────────────────────────────────────────

#[test]
fn decode_stats_flag_prints_summary() {
    let gen_dir = TempDir::new().unwrap();
    let frames_path = gen_dir.path().join("frames.bin");
    let logs_dir = TempDir::new().unwrap();

    generator()
        .args([
            "--count",
            "10",
            "--seed",
            "6",
            "--out",
            &format!("file:{}", frames_path.display()),
        ])
        .assert()
        .success();

    host()
        .args([
            "--in",
            &format!("file:{}", frames_path.display()),
            "--out-dir",
            logs_dir.path().to_str().unwrap(),
            "--stats-interval",
            "0",
        ])
        .assert()
        .success();

    let seg = logs_dir.path().join("seg-00001.bin");
    decode()
        .args([seg.to_str().unwrap(), "--stats"])
        .assert()
        .success()
        .stderr(predicate::str::contains("decode statistics"))
        .stderr(predicate::str::contains("passed filter"));
}

// ── helpers: normalise variable timestamps for stable snapshots ───────────────

/// Replace `"ts_us":<number>` with `"ts_us":0` throughout a JSON/NDJSON string.
///
/// Operates on the raw text after the known key so it works for any timestamp
/// magnitude — no digit-length heuristics.
fn normalize_json_timestamps(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let key = "\"ts_us\":";
    let mut rest = s;
    while let Some(pos) = rest.find(key) {
        out.push_str(&rest[..pos + key.len()]);
        let after = &rest[pos + key.len()..];
        let digit_end = after
            .char_indices()
            .find(|(_, c)| !c.is_ascii_digit())
            .map(|(i, _)| i)
            .unwrap_or(after.len());
        out.push('0');
        rest = &after[digit_end..];
    }
    out.push_str(rest);
    out
}

/// Canonical placeholder substituted for the variable timestamp value in table
/// snapshots.  Using a fixed token (rather than a run of `'0'`s) makes the
/// normalised output independent of the timestamp's *digit count*, which varies
/// between runs depending on how far the monotonic clock has advanced.
const TABLE_TS_PLACEHOLDER: &str = "<ts>";

/// Replace the timestamp value in the timestamp column of a `comfy-table`
/// output with a fixed-width placeholder, identified **by column position**
/// (the second `|`-delimited cell).
///
/// Why a fixed token and not a per-digit `'0'` swap: `comfy-table` left-aligns
/// the cell value and pads the remainder with spaces to a stable, header-driven
/// column width.  A per-digit replacement therefore leaks the original digit
/// count (`00` vs `0000`), so two runs whose clocks advanced by different
/// magnitudes produce different snapshots.  Substituting the whole value with a
/// constant token and re-padding to the original cell width removes that
/// dependency while preserving the column width, so the snapshot still catches
/// alignment and layout regressions.
///
/// Header rows, separator rows, and every other cell are left untouched.
fn normalize_table_timestamps(s: &str) -> String {
    s.lines()
        .map(|line| {
            // Only process data rows.  Data rows in comfy-table:
            //   - start with '|'
            //   - are NOT separator rows (which look like `|-----+...`)
            //   - are NOT the header row (which contains "Timestamp")
            if !line.starts_with('|')
                || line.starts_with("|---")
                || line.starts_with("|===")
                || line.contains("Timestamp")
            {
                return line.to_owned();
            }
            // Split on '|'. For a 5-column table the parts are:
            //   parts[0] = ""            (before leading |)
            //   parts[1] = " seq "
            //   parts[2] = " timestamp " ← normalise this one
            //   parts[3] = " type "
            //   parts[4] = " severity "
            //   parts[5] = " payload "
            //   parts[6] = ""            (after trailing |)
            let mut parts: Vec<String> = line.split('|').map(str::to_owned).collect();
            if let Some(cell) = parts.get_mut(2) {
                // Preserve the exact cell width so the table layout is unchanged.
                // comfy-table pads each cell with one leading space and left-
                // aligns the value, so we rebuild it the same way: a leading
                // space, then the placeholder left-aligned within the remaining
                // width.
                let width = cell.chars().count();
                let inner = width.saturating_sub(1);
                *cell = format!(" {TABLE_TS_PLACEHOLDER:<inner$}");
            }
            parts.join("|")
        })
        .collect::<Vec<_>>()
        .join("\n")
}
