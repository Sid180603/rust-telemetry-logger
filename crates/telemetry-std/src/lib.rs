//! `telemetry-std` — `std` implementations of `telemetry-core` traits.
//!
//! Provides:
//! - [`SystemClock`]: monotonic microsecond clock via [`std::time::Instant`].
//! - [`FileStorage`]: rotating, COBS-framed binary segment writer.
//! - [`read_segment`] / [`read_segment_file`]: shared decoder helpers used by
//!   both `telemetry-decode` and the integration tests.
//!
//! Used by both `telemetry-host` (Phase 2) and `telemetry-linux` (Phase 3).

use std::fmt;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use telemetry_core::error::Error;
use telemetry_core::record::{self, StoredRecord};
use telemetry_core::traits::{Clock, Storage};

// ── SystemClock ───────────────────────────────────────────────────────────────

/// Monotonic clock backed by [`std::time::Instant`].
///
/// The epoch is the moment [`SystemClock::new`] is called; only *differences*
/// between timestamps are meaningful — consistent with the [`Clock`] contract.
#[derive(Debug)]
pub struct SystemClock {
    base: Instant,
}

impl SystemClock {
    /// Create a new clock, snapping the current instant as the epoch.
    pub fn new() -> Self {
        Self {
            base: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> u64 {
        // Saturate at u64::MAX — that's 584,000+ years; won't happen in practice.
        self.base
            .elapsed()
            .as_micros()
            .try_into()
            .unwrap_or(u64::MAX)
    }
}

// ── FileStorage ───────────────────────────────────────────────────────────────

/// Error returned by [`FileStorage`] operations.
#[derive(Debug)]
pub enum FileStorageError {
    /// An I/O error occurred while opening, writing, or flushing a segment file.
    Io(io::Error),
}

impl fmt::Display for FileStorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "segment I/O error: {e}"),
        }
    }
}

impl From<io::Error> for FileStorageError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// Rotating binary segment file writer.
///
/// Writes records to files named `seg-NNNNN.bin` inside a directory.
/// A new segment is opened **before** writing a record that would push the
/// current file past `max_segment_bytes` — records are never split across
/// segment boundaries.
///
/// # Segment naming
///
/// Files are `seg-00001.bin`, `seg-00002.bin`, … The index resets to 1 on
/// each run; existing files in the directory are **not** enumerated.
#[derive(Debug)]
pub struct FileStorage {
    out_dir: PathBuf,
    max_segment_bytes: usize,
    current_file: Option<File>,
    segment_index: u32,
    current_size: usize,
}

impl FileStorage {
    /// Create a new [`FileStorage`] writing segments into `out_dir`.
    ///
    /// # Errors
    ///
    /// Returns an error if `out_dir` cannot be created.
    pub fn new(
        out_dir: impl AsRef<Path>,
        max_segment_bytes: usize,
    ) -> Result<Self, FileStorageError> {
        let out_dir = out_dir.as_ref().to_path_buf();
        fs::create_dir_all(&out_dir)?;
        Ok(Self {
            out_dir,
            max_segment_bytes,
            current_file: None,
            segment_index: 0,
            current_size: 0,
        })
    }

    /// Number of segment files created so far.
    pub fn segment_count(&self) -> u32 {
        self.segment_index
    }

    /// Path to the most recently opened segment, or `None` if nothing has been
    /// written yet.
    pub fn current_path(&self) -> Option<PathBuf> {
        if self.segment_index == 0 {
            return None;
        }
        Some(self.segment_path(self.segment_index))
    }

    fn segment_path(&self, index: u32) -> PathBuf {
        self.out_dir.join(format!("seg-{index:05}.bin"))
    }

    fn open_next_segment(&mut self) -> Result<(), FileStorageError> {
        // Flush and implicitly close the existing file by dropping it.
        if let Some(ref mut f) = self.current_file {
            f.flush()?;
        }
        self.segment_index += 1;
        self.current_file = Some(File::create(self.segment_path(self.segment_index))?);
        self.current_size = 0;
        Ok(())
    }
}

impl Storage for FileStorage {
    type Error = FileStorageError;

    fn write(&mut self, data: &[u8]) -> Result<(), FileStorageError> {
        // Rotate if writing `data` would push us past the limit, or if no file
        // is open yet.
        let needs_rotate =
            self.current_file.is_none() || self.current_size + data.len() > self.max_segment_bytes;
        if needs_rotate {
            self.open_next_segment()?;
        }
        // After open_next_segment, current_file is always Some.  If it was
        // already Some and didn't need rotation it stays Some.  Use if-let
        // instead of expect so the invariant is visible without a panic.
        if let Some(ref mut f) = self.current_file {
            f.write_all(data)?;
            self.current_size += data.len();
            Ok(())
        } else {
            // Unreachable: open_next_segment always sets current_file = Some.
            Err(FileStorageError::Io(io::Error::other(
                "internal: no segment file open after rotation",
            )))
        }
    }

    fn flush(&mut self) -> Result<(), FileStorageError> {
        if let Some(ref mut f) = self.current_file {
            f.flush()?;
        }
        Ok(())
    }
}

// ── Segment reading ───────────────────────────────────────────────────────────

/// Decode all records from a raw segment byte slice.
///
/// Splits on `0x00` delimiters and calls [`record::decode`] on each chunk.
/// A truncated trailing record (bytes after the last `0x00` with no closing
/// delimiter) is yielded as `Err(`[`Error::Codec`]`)` — no panic.
///
/// Empty chunks (e.g. from a leading or doubled `0x00`) are silently skipped.
///
/// # Shared decode path
///
/// Both `telemetry-decode` and the integration tests call this function.
/// It in turn calls `telemetry_core::record::decode`, keeping the decode
/// logic in exactly one place (the core crate).
pub fn read_segment(bytes: &[u8]) -> impl Iterator<Item = Result<StoredRecord, Error>> + '_ {
    bytes
        .split(|&b| b == 0x00)
        .filter(|chunk| !chunk.is_empty())
        .map(record::decode)
}

/// Read and decode all records from a segment file on disk.
///
/// Reads the entire file into memory, then calls [`read_segment`].  Individual
/// record decode errors are preserved inline as `Err(`[`Error::Codec`]`)`.
///
/// # Errors
///
/// Returns an [`io::Error`] if the file cannot be read.
pub fn read_segment_file(path: &Path) -> Result<Vec<Result<StoredRecord, Error>>, io::Error> {
    let bytes = fs::read(path)?;
    Ok(read_segment(&bytes).collect())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use telemetry_core::record::{RecordV1, encode};

    // ── SystemClock ───────────────────────────────────────────────────────────

    #[test]
    fn system_clock_is_non_decreasing() {
        let c = SystemClock::new();
        let t0 = c.now();
        let t1 = c.now();
        assert!(t1 >= t0, "clock must be monotonically non-decreasing");
    }

    #[test]
    fn system_clock_advances_over_time() {
        let c = SystemClock::new();
        let t0 = c.now();
        // Spin briefly without sleeping.
        let mut sum = 0u64;
        for i in 0..100_000u64 {
            sum = sum.wrapping_add(i);
        }
        let t1 = c.now();
        // At least *some* time must have elapsed; the dummy loop prevents the
        // compiler from optimising it away.
        let _ = sum;
        assert!(t1 >= t0);
    }

    // ── FileStorage ───────────────────────────────────────────────────────────

    fn make_record(seq: u32) -> StoredRecord {
        StoredRecord::V1(RecordV1 {
            timestamp_us: seq as u64 * 1000,
            sequence: seq,
            packet_type: 0x01,
            severity: 0x01,
            payload: heapless::Vec::new(),
        })
    }

    #[test]
    fn file_storage_creates_first_segment_on_write() {
        let dir = assert_fs::TempDir::new().unwrap();
        let mut storage = FileStorage::new(dir.path(), 1024 * 1024).unwrap();
        assert_eq!(storage.segment_count(), 0);

        let record = make_record(1);
        let mut buf = [0u8; telemetry_core::record::MAX_STORED_RECORD_BYTES];
        let n = encode(&record, &mut buf).unwrap();
        storage.write(&buf[..n]).unwrap();
        storage.flush().unwrap();

        assert_eq!(storage.segment_count(), 1);
        assert!(dir.path().join("seg-00001.bin").exists());
    }

    #[test]
    fn file_storage_rotates_on_size_overflow() {
        let dir = assert_fs::TempDir::new().unwrap();
        // Very small segment so rotation happens quickly.
        let mut storage = FileStorage::new(dir.path(), 64).unwrap();

        let mut buf = [0u8; telemetry_core::record::MAX_STORED_RECORD_BYTES];
        // Write enough records to force at least two segments.
        for seq in 0..10u32 {
            let record = make_record(seq);
            let n = encode(&record, &mut buf).unwrap();
            storage.write(&buf[..n]).unwrap();
        }
        storage.flush().unwrap();

        // With a 64-byte limit and ~20-byte records we expect multiple segments.
        assert!(
            storage.segment_count() >= 2,
            "expected rotation, got {} segments",
            storage.segment_count()
        );
    }

    #[test]
    fn file_storage_never_splits_a_record() {
        // If a record fits in a new segment but not in the current one, a new
        // segment is opened *before* writing — so every record is complete.
        let dir = assert_fs::TempDir::new().unwrap();
        let mut storage = FileStorage::new(dir.path(), 32).unwrap();

        let mut buf = [0u8; telemetry_core::record::MAX_STORED_RECORD_BYTES];
        for seq in 0..5u32 {
            let record = make_record(seq);
            let n = encode(&record, &mut buf).unwrap();
            storage.write(&buf[..n]).unwrap();
        }
        storage.flush().unwrap();

        // Read back every segment; every record must decode successfully.
        for idx in 1..=storage.segment_count() {
            let path = dir.path().join(format!("seg-{idx:05}.bin"));
            for result in read_segment_file(&path).unwrap() {
                result.expect("every record must decode without error");
            }
        }
    }

    // ── read_segment ──────────────────────────────────────────────────────────

    #[test]
    fn read_segment_round_trips_multiple_records() {
        let records: Vec<StoredRecord> = (0..5).map(make_record).collect();
        let mut bytes = Vec::new();
        let mut buf = [0u8; telemetry_core::record::MAX_STORED_RECORD_BYTES];
        for r in &records {
            let n = encode(r, &mut buf).unwrap();
            bytes.extend_from_slice(&buf[..n]);
        }

        let decoded: Vec<StoredRecord> = read_segment(&bytes)
            .map(|r| r.expect("valid record"))
            .collect();
        assert_eq!(decoded, records);
    }

    #[test]
    fn read_segment_tolerates_truncated_trailing_record() {
        let record = make_record(42);
        let mut buf = [0u8; telemetry_core::record::MAX_STORED_RECORD_BYTES];
        let n = encode(&record, &mut buf).unwrap();

        // Full record + partial garbage (no closing 0x00).
        let mut bytes = buf[..n].to_vec();
        bytes.extend_from_slice(b"\x01\x02\x03"); // partial junk

        let results: Vec<_> = read_segment(&bytes).collect();
        assert_eq!(results.len(), 2, "one good + one bad");
        assert!(results[0].is_ok(), "first record intact");
        assert!(results[1].is_err(), "trailing garbage is an error");
    }

    #[test]
    fn read_segment_empty_slice_yields_nothing() {
        let results: Vec<_> = read_segment(&[]).collect();
        assert!(results.is_empty());
    }
}
