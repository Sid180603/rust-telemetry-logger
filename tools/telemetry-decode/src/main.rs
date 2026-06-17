//! `telemetry-decode` — binary log decoder.
//!
//! Reads `seg-NNNNN.bin` segment files and prints records in human-readable
//! form, reusing [`telemetry_core::record::StoredRecord`] and
//! [`telemetry_std::read_segment_file`] directly — proving the portable core.
//!
//! # Usage
//!
//! ```text
//! telemetry-decode [OPTIONS] [FILES]...
//!
//! # Table (default)
//! telemetry-decode logs/seg-00001.bin
//!
//! # NDJSON piped to jq
//! telemetry-decode --in-dir logs --format ndjson | jq .
//!
//! # Errors and stats only
//! telemetry-decode logs/ --min-severity 3 --stats
//! ```

use std::path::{Path, PathBuf};

use clap::{Parser, ValueEnum};
use comfy_table::{Cell, Table};
use telemetry_core::record::StoredRecord;
use telemetry_std::read_segment_file;

// ─── CLI ─────────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(
    name = "telemetry-decode",
    about = "Decode binary telemetry log segments to table, JSON, or NDJSON"
)]
struct Args {
    /// Segment files to decode.
    files: Vec<PathBuf>,

    /// Decode all `seg-*.bin` files found in a directory (sorted by name).
    #[arg(long = "in-dir")]
    in_dir: Option<PathBuf>,

    /// Output format.
    #[arg(short, long, default_value = "table")]
    format: Format,

    /// Skip records whose sequence number is less than this value.
    #[arg(long)]
    since: Option<u32>,

    /// Minimum severity level to show (0=Debug, 1=Info, 2=Warning, 3=Error, 4=Fatal).
    #[arg(long, default_value = "0")]
    min_severity: u8,

    /// Only show records of this packet type (raw byte, repeatable).
    #[arg(long = "type", short = 't')]
    packet_types: Vec<u8>,

    /// Print a statistics summary after all records.
    #[arg(long)]
    stats: bool,
}

#[derive(ValueEnum, Debug, Clone, PartialEq, Eq)]
enum Format {
    /// Aligned text table.
    Table,
    /// Single JSON array of all matching records.
    Json,
    /// One JSON object per line (newline-delimited JSON).
    Ndjson,
}

// ─── Decode-time statistics ───────────────────────────────────────────────────

#[derive(Debug, Default)]
struct DecodeStats {
    total_seen: u64,
    decode_errors: u64,
    passed_filter: u64,
    by_type: [u64; 6],     // indices 0–5 map to raw type bytes 0x00–0x05
    by_severity: [u64; 5], // indices 0–4 map to Debug–Fatal
}

impl DecodeStats {
    fn record_seen(&mut self, ok: bool) {
        self.total_seen += 1;
        if !ok {
            self.decode_errors += 1;
        }
    }

    fn record_passed(&mut self, ptype: u8, severity: u8) {
        self.passed_filter += 1;
        if (ptype as usize) < self.by_type.len() {
            self.by_type[ptype as usize] += 1;
        }
        if (severity as usize) < self.by_severity.len() {
            self.by_severity[severity as usize] += 1;
        }
    }

    fn print(&self) {
        eprintln!("─── decode statistics ────────────────────────────────");
        eprintln!("  total records seen   : {}", self.total_seen);
        eprintln!("  decode errors        : {}", self.decode_errors);
        eprintln!("  passed filter        : {}", self.passed_filter);
        let type_names = [
            "?",
            "Heartbeat",
            "SensorData",
            "Event",
            "Diagnostic",
            "Control",
        ];
        for (i, &count) in self.by_type.iter().enumerate() {
            if count > 0 {
                let name = type_names.get(i).copied().unwrap_or("?");
                eprintln!("  type {i:02x} ({name:<12}): {count}");
            }
        }
        let sev_names = ["Debug", "Info", "Warning", "Error", "Fatal"];
        for (i, &count) in self.by_severity.iter().enumerate() {
            if count > 0 {
                let name = sev_names.get(i).copied().unwrap_or("?");
                eprintln!("  severity {i} ({name:<9}): {count}");
            }
        }
        eprintln!("─────────────────────────────────────────────────────");
    }
}

// ─── Filter predicate ─────────────────────────────────────────────────────────

fn passes_filter(r: &StoredRecord, since: Option<u32>, min_severity: u8, types: &[u8]) -> bool {
    match r {
        StoredRecord::V1(v) => {
            if let Some(since_seq) = since {
                if v.sequence < since_seq {
                    return false;
                }
            }
            if v.severity < min_severity {
                return false;
            }
            if !types.is_empty() && !types.contains(&v.packet_type) {
                return false;
            }
            true
        }
        _ => false,
    }
}

// ─── Formatters ───────────────────────────────────────────────────────────────

fn type_name(raw: u8) -> &'static str {
    match raw {
        0x01 => "Heartbeat",
        0x02 => "SensorData",
        0x03 => "Event",
        0x04 => "Diagnostic",
        0x05 => "Control",
        _ => "Unknown",
    }
}

fn severity_name(raw: u8) -> &'static str {
    match raw {
        0x00 => "Debug",
        0x01 => "Info",
        0x02 => "Warning",
        0x03 => "Error",
        0x04 => "Fatal",
        _ => "Unknown",
    }
}

fn payload_preview(payload: &[u8]) -> String {
    // Show first 8 bytes as hex, then "…" if longer.
    let shown = payload.len().min(8);
    let hex: String = payload[..shown]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ");
    if payload.len() > 8 {
        format!("{hex} … ({} bytes)", payload.len())
    } else if payload.is_empty() {
        String::from("(empty)")
    } else {
        hex
    }
}

fn record_to_json(r: &StoredRecord) -> String {
    let StoredRecord::V1(v) = r else {
        return String::from("{}");
    };
    let payload_hex: String = v.payload.iter().fold(String::new(), |mut s, b| {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
        s
    });
    format!(
        r#"{{"seq":{},"ts_us":{},"type":{},"type_name":"{}","severity":{},"severity_name":"{}","payload_hex":"{}","payload_len":{}}}"#,
        v.sequence,
        v.timestamp_us,
        v.packet_type,
        type_name(v.packet_type),
        v.severity,
        severity_name(v.severity),
        payload_hex,
        v.payload.len(),
    )
}

// ─── Main ─────────────────────────────────────────────────────────────────────

fn main() {
    let args = Args::parse();

    // Collect segment paths: explicit files + optional --in-dir.
    let mut paths: Vec<PathBuf> = args.files.clone();
    if let Some(ref dir) = args.in_dir {
        match collect_segments_in_dir(dir) {
            Ok(mut found) => paths.append(&mut found),
            Err(e) => {
                eprintln!("error reading directory {}: {e}", dir.display());
                std::process::exit(1);
            }
        }
    }

    if paths.is_empty() {
        eprintln!("error: no input files. Pass segment files or use --in-dir.");
        std::process::exit(1);
    }

    // Sort so that seg-00001.bin < seg-00002.bin etc.
    paths.sort();

    // Collect all records (respecting filter), printing as we go for NDJSON/table,
    // or accumulating for JSON array.
    let mut stats = DecodeStats::default();
    let mut table_rows: Vec<StoredRecord> = Vec::new();
    let mut json_records: Vec<String> = Vec::new();

    for path in &paths {
        match read_segment_file(path) {
            Err(e) => {
                eprintln!("warning: cannot read {}: {e}", path.display());
            }
            Ok(records) => {
                for result in records {
                    let ok = result.is_ok();
                    stats.record_seen(ok);

                    let record = match result {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("  decode error in {}: {e:?}", path.display());
                            continue;
                        }
                    };

                    if !passes_filter(&record, args.since, args.min_severity, &args.packet_types) {
                        continue;
                    }

                    let (ptype, severity) = match &record {
                        StoredRecord::V1(v) => (v.packet_type, v.severity),
                        _ => continue,
                    };
                    stats.record_passed(ptype, severity);

                    match args.format {
                        Format::Ndjson => {
                            println!("{}", record_to_json(&record));
                        }
                        Format::Json => {
                            json_records.push(record_to_json(&record));
                        }
                        Format::Table => {
                            table_rows.push(record);
                        }
                    }
                }
            }
        }
    }

    // Emit table or JSON array after processing all files.
    match args.format {
        Format::Table => {
            print_table(&table_rows);
        }
        Format::Json => {
            println!("[{}]", json_records.join(",\n "));
        }
        Format::Ndjson => { /* already printed inline */ }
    }

    if args.stats {
        stats.print();
    }
}

fn print_table(records: &[StoredRecord]) {
    if records.is_empty() {
        eprintln!("(no records match the filter)");
        return;
    }

    let mut table = Table::new();
    table.set_header(vec![
        Cell::new("Seq"),
        Cell::new("Timestamp (μs)"),
        Cell::new("Type"),
        Cell::new("Severity"),
        Cell::new("Payload"),
    ]);

    for r in records {
        let StoredRecord::V1(v) = r else { continue };
        table.add_row(vec![
            Cell::new(v.sequence),
            Cell::new(v.timestamp_us),
            Cell::new(format!(
                "{:02x} ({})",
                v.packet_type,
                type_name(v.packet_type)
            )),
            Cell::new(format!(
                "{:02x} ({})",
                v.severity,
                severity_name(v.severity)
            )),
            Cell::new(payload_preview(&v.payload)),
        ]);
    }

    println!("{table}");
}

fn collect_segments_in_dir(dir: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.starts_with("seg-")
                && std::path::Path::new(name)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("bin"))
            {
                found.push(path);
            }
        }
    }
    Ok(found)
}
