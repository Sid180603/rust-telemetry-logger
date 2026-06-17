//! `telemetry-host` — host simulation binary.
//!
//! Wires a packet source (file / stdin / TCP / UDP) → [`DefaultPipeline`] →
//! [`FileStorage`] and prints periodic statistics to stderr.
//!
//! # Input source formats
//!
//! | Flag value       | Behaviour                                       |
//! |------------------|-------------------------------------------------|
//! | `-` (default)    | Read from stdin                                 |
//! | `file:<PATH>`    | Read from a file                                |
//! | `tcp:<HOST:PORT>`| Listen on HOST:PORT, accept one connection      |
//! | `udp:<HOST:PORT>`| Bind to HOST:PORT, receive datagrams            |

use std::fmt;
use std::io::{self, Read};
use std::net::{TcpListener, UdpSocket};

use clap::Parser;
use telemetry_core::config::DefaultPipeline;
use telemetry_core::filter::FilterConfig;
use telemetry_core::traits::{PacketSource, Storage};
use telemetry_std::{FileStorage, SystemClock};

// ─── CLI ─────────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(
    name = "telemetry-host",
    about = "Receive telemetry frames → pipeline → rotating segment files"
)]
struct Args {
    /// Input source: `-` (stdin), `file:<PATH>`, `tcp:<HOST:PORT>`,
    /// `udp:<HOST:PORT>`.
    #[arg(short = 'i', long = "in", default_value = "-")]
    input: String,

    /// Directory to write segment files into.
    #[arg(short = 'd', long = "out-dir", default_value = "logs")]
    out_dir: String,

    /// Maximum bytes per segment file before rotation.
    #[arg(long, default_value = "1048576")]
    segment_size: usize,

    /// Minimum severity level to accept (0=Debug … 4=Fatal).
    #[arg(long, default_value = "0")]
    min_severity: u8,

    /// Packet type allowlist (raw byte, repeatable).  Omit to allow all types.
    #[arg(long = "allow-type", value_name = "TYPE")]
    allow_types: Vec<u8>,

    /// Print a stats line every N accepted records (0 = never).
    #[arg(long, default_value = "50")]
    stats_interval: u64,
}

// ─── PacketSource implementations ────────────────────────────────────────────

/// Wraps any `std::io::Read` as a [`PacketSource`].
struct ReadSource<R: Read> {
    inner: R,
}

impl<R: Read> ReadSource<R> {
    fn new(inner: R) -> Self {
        Self { inner }
    }
}

impl<R: Read> fmt::Debug for ReadSource<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ReadSource")
    }
}

impl<R: Read> PacketSource for ReadSource<R> {
    type Error = io::Error;

    fn read(&mut self, buf: &mut [u8]) -> Result<usize, io::Error> {
        self.inner.read(buf)
    }
}

/// Wraps a [`UdpSocket`] as a [`PacketSource`] (datagram-oriented).
struct UdpSource(UdpSocket);

impl fmt::Debug for UdpSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "UdpSource")
    }
}

impl PacketSource for UdpSource {
    type Error = io::Error;

    fn read(&mut self, buf: &mut [u8]) -> Result<usize, io::Error> {
        let (n, _addr) = self.0.recv_from(buf)?;
        Ok(n)
    }
}

// ─── Source factory ───────────────────────────────────────────────────────────

/// Open the requested input source and run the pipeline loop.
fn run_with_source(
    source_spec: &str,
    mut pipeline: DefaultPipeline,
    mut storage: FileStorage,
    clock: &SystemClock,
    stats_interval: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    if source_spec == "-" {
        let src = ReadSource::new(io::stdin());
        run_loop(src, &mut pipeline, &mut storage, clock, stats_interval)?;
    } else if let Some(path) = source_spec.strip_prefix("file:") {
        let f = std::fs::File::open(path)?;
        run_loop(
            ReadSource::new(f),
            &mut pipeline,
            &mut storage,
            clock,
            stats_interval,
        )?;
    } else if let Some(addr) = source_spec.strip_prefix("tcp:") {
        let listener = TcpListener::bind(addr)?;
        eprintln!("Listening on {addr} …");
        let (stream, peer) = listener.accept()?;
        eprintln!("Connection from {peer}");
        run_loop(
            ReadSource::new(stream),
            &mut pipeline,
            &mut storage,
            clock,
            stats_interval,
        )?;
    } else if let Some(addr) = source_spec.strip_prefix("udp:") {
        let socket = UdpSocket::bind(addr)?;
        eprintln!("UDP bound to {addr}");
        run_loop(
            UdpSource(socket),
            &mut pipeline,
            &mut storage,
            clock,
            stats_interval,
        )?;
    } else {
        return Err(format!(
            "unrecognised source {source_spec:?}; use -, file:<PATH>, tcp:<HOST:PORT>, or udp:<HOST:PORT>"
        )
        .into());
    }

    Ok(())
}

// ─── Pipeline loop ─────────────────────────────────────────────────────────────

fn run_loop<S: PacketSource<Error = io::Error>>(
    mut source: S,
    pipeline: &mut DefaultPipeline,
    storage: &mut FileStorage,
    clock: &SystemClock,
    stats_interval: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut buf = [0u8; 4096];
    let mut last_print_ok: u64 = 0;

    loop {
        let n = match source.read(&mut buf) {
            Ok(0) => break, // EOF
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => break,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        };

        pipeline.ingest(&buf[..n], clock);

        if let Err(e) = pipeline.drain(storage) {
            eprintln!("drain error (storage failure): {e:?}");
        }

        // Periodic stats (rate-limit by ok counter to avoid flooding stderr).
        if stats_interval > 0 {
            let ok = pipeline.stats().ok;
            if ok >= last_print_ok + stats_interval {
                print_stats(pipeline.stats());
                last_print_ok = ok;
            }
        }
    }

    // Final flush and stats.
    storage
        .flush()
        .map_err(|e| format!("final flush failed: {e}"))?;
    print_stats(pipeline.stats());

    Ok(())
}

fn print_stats(s: &telemetry_core::stats::Stats) {
    eprintln!(
        "stats: ok={} dropped={} crc_fail={} seq_gap={} filtered={} write_fail={} hwm={}",
        s.ok, s.dropped, s.crc_fail, s.seq_gap, s.filtered, s.write_fail, s.queue_high_water,
    );
}

// ─── Main ─────────────────────────────────────────────────────────────────────

fn main() {
    let args = Args::parse();

    // Build filter config.
    let filter = if args.allow_types.is_empty() {
        FilterConfig {
            min_severity: args.min_severity,
            type_allowlist: None,
        }
    } else {
        let mut list = heapless::Vec::<u8, { telemetry_core::filter::MAX_FILTER_TYPES }>::new();
        for &t in &args.allow_types {
            if list.push(t).is_err() {
                eprintln!(
                    "warning: too many --allow-type values (max {}); ignoring the rest",
                    telemetry_core::filter::MAX_FILTER_TYPES
                );
                break;
            }
        }
        FilterConfig {
            min_severity: args.min_severity,
            type_allowlist: Some(list),
        }
    };

    let pipeline = DefaultPipeline::new(filter);

    let storage = FileStorage::new(&args.out_dir, args.segment_size).unwrap_or_else(|e| {
        eprintln!(
            "error: cannot open output directory {:?}: {e}",
            args.out_dir
        );
        std::process::exit(1);
    });

    let clock = SystemClock::new();

    if let Err(e) = run_with_source(&args.input, pipeline, storage, &clock, args.stats_interval) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
