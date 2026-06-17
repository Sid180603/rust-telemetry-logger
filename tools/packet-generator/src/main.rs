//! `packet-generator` — test frame emitter.
//!
//! Generates COBS-framed telemetry frames (valid, corrupt, truncated, bursty)
//! and streams them to stdout, a file, a TCP connection, or a UDP socket.
//!
//! # Output sink formats
//!
//! | Flag value      | Behaviour                                     |
//! |-----------------|-----------------------------------------------|
//! | `-` (default)   | Write to stdout                               |
//! | `file:<PATH>`   | Write to a file (created/truncated)           |
//! | `tcp:<HOST:PORT>`| Connect to HOST:PORT and write               |
//! | `udp:<HOST:PORT>`| Send each COBS frame as one UDP datagram     |
//!
//! # Corruption modes
//!
//! | Mode      | Effect on wire                                               |
//! |-----------|--------------------------------------------------------------|
//! | `bad-crc` | Flip a byte inside the COBS payload → `crc_fail` in pipeline|
//! | `truncate`| Emit a partial frame + 0x00 → COBS parse error              |
//! | `seq-gap` | Skip sequence numbers → next valid frame triggers `seq_gap` |
//! | `burst`   | Emit `burst-size` extra valid frames with monotonic seq      |

use std::io::{self, Write};
use std::net::{TcpStream, UdpSocket};
use std::str::FromStr;
use std::{fmt, fs};

use clap::Parser;
use rand::Rng as _;
use rand::SeedableRng as _;
use rand_chacha::ChaCha8Rng;
use telemetry_core::frame::{Frame, MAX_COBS_FRAME_BYTES};

// ─── CLI ─────────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(
    name = "packet-generator",
    about = "Generate COBS-framed telemetry frames for testing"
)]
struct Args {
    /// Number of frames to generate.
    #[arg(short = 'c', long, default_value = "100")]
    count: u32,

    /// RNG seed (deterministic output for CI / regression testing).
    #[arg(short = 's', long, default_value = "42")]
    seed: u64,

    /// Output sink: `-` (stdout), `file:<PATH>`, `tcp:<HOST:PORT>`,
    /// `udp:<HOST:PORT>`.
    #[arg(short = 'o', long = "out", default_value = "-")]
    sink: String,

    /// Corruption modes (comma-separated).
    /// Choices: `bad-crc`, `truncate`, `seq-gap`, `burst`.
    #[arg(long, value_delimiter = ',', default_value = "")]
    corrupt: Vec<String>,

    /// Probability [0.0–1.0] of applying corruption to each frame.
    #[arg(long, default_value = "0.1")]
    corrupt_rate: f64,

    /// For `burst` mode: extra frames emitted per burst event.
    #[arg(long, default_value = "5")]
    burst_size: u32,
}

// ─── Corruption mode enum ────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CorruptMode {
    BadCrc,
    Truncate,
    SeqGap,
    Burst,
}

impl FromStr for CorruptMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "bad-crc" => Ok(Self::BadCrc),
            "truncate" => Ok(Self::Truncate),
            "seq-gap" => Ok(Self::SeqGap),
            "burst" => Ok(Self::Burst),
            other => Err(format!("unknown corruption mode: {other:?}")),
        }
    }
}

// ─── Output sink ─────────────────────────────────────────────────────────────

enum Sink {
    Stdout(io::Stdout),
    File(fs::File),
    Tcp(TcpStream),
    Udp { socket: UdpSocket, addr: String },
}

impl fmt::Debug for Sink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stdout(_) => write!(f, "Sink::Stdout"),
            Self::File(_) => write!(f, "Sink::File"),
            Self::Tcp(_) => write!(f, "Sink::Tcp"),
            Self::Udp { addr, .. } => write!(f, "Sink::Udp({addr})"),
        }
    }
}

impl Sink {
    fn from_spec(spec: &str) -> Result<Self, String> {
        if spec == "-" {
            return Ok(Self::Stdout(io::stdout()));
        }
        if let Some(path) = spec.strip_prefix("file:") {
            let f =
                fs::File::create(path).map_err(|e| format!("cannot create file {path:?}: {e}"))?;
            return Ok(Self::File(f));
        }
        if let Some(addr) = spec.strip_prefix("tcp:") {
            let stream = TcpStream::connect(addr)
                .map_err(|e| format!("TCP connect to {addr:?} failed: {e}"))?;
            return Ok(Self::Tcp(stream));
        }
        if let Some(addr) = spec.strip_prefix("udp:") {
            // Bind to an ephemeral local port; send to the specified address.
            let socket =
                UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("UDP bind failed: {e}"))?;
            return Ok(Self::Udp {
                socket,
                addr: addr.to_owned(),
            });
        }
        Err(format!(
            "unrecognised sink {spec:?}; use -, file:<PATH>, tcp:<HOST:PORT>, or udp:<HOST:PORT>"
        ))
    }

    /// Write all bytes in `buf` to the sink.
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        match self {
            Self::Stdout(s) => s.write_all(buf),
            Self::File(f) => f.write_all(buf),
            Self::Tcp(s) => s.write_all(buf),
            Self::Udp { socket, addr } => {
                // Each COBS frame is sent as a single UDP datagram.
                socket.send_to(buf, addr.as_str())?;
                Ok(())
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Stdout(s) => s.flush(),
            Self::File(f) => f.flush(),
            Self::Tcp(s) => s.flush(),
            Self::Udp { .. } => Ok(()),
        }
    }
}

// ─── Generator stats ─────────────────────────────────────────────────────────

#[derive(Debug, Default)]
struct GenStats {
    valid: u32,
    bad_crc: u32,
    truncated: u32,
    seq_gap: u32,
    burst_extra: u32,
}

// ─── Packet-type / severity tables ───────────────────────────────────────────

/// Raw `PacketType` discriminants — must match `protocol.rs`.
const TYPES: &[u8] = &[0x01, 0x02, 0x03, 0x04, 0x05];

/// Severity weights: more Info/Warning, fewer Error/Fatal.
const SEVERITIES: &[u8] = &[
    0x00, // Debug (1)
    0x01, // Info  (3)
    0x01, 0x01, 0x02, // Warning (2)
    0x02, 0x03, // Error (1)
    0x04, // Fatal (1)
];

// ─── Main ─────────────────────────────────────────────────────────────────────

fn main() {
    let args = Args::parse();

    // Parse corruption modes (empty string means no corruption).
    let modes: Vec<CorruptMode> = {
        let mut v = Vec::new();
        for s in args.corrupt.iter().filter(|s| !s.is_empty()) {
            match s.parse::<CorruptMode>() {
                Ok(m) => v.push(m),
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            }
        }
        v
    };

    let corrupt_rate = args.corrupt_rate.clamp(0.0, 1.0);

    // Open the output sink.
    let mut sink = Sink::from_spec(&args.sink).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        std::process::exit(1);
    });

    let mut rng = ChaCha8Rng::seed_from_u64(args.seed);
    let mut seq: u32 = 0;
    let mut stats = GenStats::default();
    let mut frame_buf = [0u8; MAX_COBS_FRAME_BYTES];

    for _ in 0..args.count {
        // ── Seq-gap: skip sequence numbers before building the frame. ──────
        if modes.contains(&CorruptMode::SeqGap) && rng.random::<f64>() < corrupt_rate {
            let skip: u32 = rng.random_range(2..6);
            seq = seq.wrapping_add(skip);
            stats.seq_gap += 1;
        }

        // ── Build frame ────────────────────────────────────────────────────
        let ptype = TYPES[rng.random_range(0..TYPES.len())];
        let severity = SEVERITIES[rng.random_range(0..SEVERITIES.len())];
        let payload_len: usize = rng.random_range(0..17);
        let mut payload = [0u8; 16];
        for b in payload.iter_mut().take(payload_len) {
            *b = rng.random();
        }

        // payload_len ≤ 16 ≤ MAX_PAYLOAD; frame_buf = MAX_COBS_FRAME_BYTES.
        // Neither call can return None with these inputs, but handle it
        // defensively without using expect/panic.
        let Some(frame) = Frame::new(ptype, severity, seq, &payload[..payload_len]) else {
            continue; // unreachable
        };
        let Some(n) = frame.encode_cobs(&mut frame_buf) else {
            continue; // unreachable
        };

        // ── Apply corruption (only one mode per frame) ─────────────────────
        let corrupted = apply_corruption(
            &frame_buf[..n],
            &modes,
            corrupt_rate,
            args.burst_size,
            &mut rng,
            &mut stats,
        );

        match corrupted {
            CorruptResult::Original => {
                emit(&mut sink, &frame_buf[..n]);
                stats.valid += 1;
            }
            CorruptResult::Bytes(ref bytes) => {
                emit(&mut sink, bytes);
                // stat already incremented inside apply_corruption
            }
            CorruptResult::BurstFrames(ref frames) => {
                for f in frames {
                    emit(&mut sink, f);
                }
                stats.valid += 1; // the original frame is valid
            }
        }

        seq = seq.wrapping_add(1);
    }

    if let Err(e) = sink.flush() {
        eprintln!("flush error: {e}");
        std::process::exit(1);
    }

    // Print generation summary to stderr (doesn't pollute stdout pipe).
    eprintln!(
        "generated {} frames: {} valid, {} bad-crc, {} truncated, {} seq-gaps, {} burst-extra",
        args.count, stats.valid, stats.bad_crc, stats.truncated, stats.seq_gap, stats.burst_extra,
    );
}

// ─── Corruption helper ────────────────────────────────────────────────────────

enum CorruptResult {
    /// Emit the original encoded bytes unchanged.
    Original,
    /// Replace with these bytes.
    Bytes(Vec<u8>),
    /// Burst: replace with multiple frames (first is the valid original, rest
    /// are additional valid frames with incrementing sequence).
    BurstFrames(Vec<Vec<u8>>),
}

fn apply_corruption(
    encoded: &[u8],
    modes: &[CorruptMode],
    rate: f64,
    burst_size: u32,
    rng: &mut ChaCha8Rng,
    stats: &mut GenStats,
) -> CorruptResult {
    // Select first matching corruption (one per frame).
    for &mode in modes {
        if rng.random::<f64>() >= rate {
            continue;
        }
        match mode {
            CorruptMode::BadCrc => {
                // Flip a byte anywhere in the COBS payload (not the 0x00 delimiter).
                if encoded.len() < 2 {
                    return CorruptResult::Original;
                }
                let mut corrupted = encoded.to_vec();
                let pos = rng.random_range(0..corrupted.len() - 1);
                corrupted[pos] ^= 0xAA;
                stats.bad_crc += 1;
                return CorruptResult::Bytes(corrupted);
            }
            CorruptMode::Truncate => {
                // Emit a partial frame (no closing 0x00) followed by 0x00.
                // The framer accumulates the partial bytes, then on the 0x00
                // it tries to COBS-decode a truncated chunk → ParseError.
                if encoded.len() < 3 {
                    return CorruptResult::Original;
                }
                let trunc_len = rng.random_range(1..encoded.len() - 1);
                let mut truncated = encoded[..trunc_len].to_vec();
                truncated.push(0x00);
                stats.truncated += 1;
                return CorruptResult::Bytes(truncated);
            }
            CorruptMode::SeqGap => {
                // SeqGap is handled before frame construction (sequence skip).
                // Don't double-apply here.
            }
            CorruptMode::Burst => {
                // Emit `burst_size` extra copies of the current frame.
                // The validator will flag seq-gaps for the duplicates — that
                // tests ring-buffer pressure and the pipeline's recovery path.
                let mut frames: Vec<Vec<u8>> = Vec::with_capacity(burst_size as usize + 1);
                frames.push(encoded.to_vec());
                for _ in 0..burst_size {
                    frames.push(encoded.to_vec());
                }
                stats.burst_extra += burst_size;
                return CorruptResult::BurstFrames(frames);
            }
        }
    }
    CorruptResult::Original
}

fn emit(sink: &mut Sink, data: &[u8]) {
    if let Err(e) = sink.write_all(data) {
        eprintln!("write error: {e}");
        std::process::exit(1);
    }
}
