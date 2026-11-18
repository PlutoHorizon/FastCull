//! Optional JSON Lines timing trace for the managed RAW benchmark.
//!
//! Nothing is written unless `FASTCULL_RAW_TRACE_DIR` names a directory. Each
//! process appends to its own `raw-trace-<pid>.jsonl`, so the culler parent and
//! the editor child can both trace without interleaving writes; the harness
//! merges the files by `unix_us`. `mono_ms` is only comparable inside one file.
//!
//! Field values are pre-rendered JSON so callers can pass numbers, booleans or
//! escaped strings: `("bytes", n.to_string())`, `("path", string(path))`.
use std::{
    fmt::Write as _,
    fs::{self, OpenOptions},
    io::Write as _,
    path::PathBuf,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

static FILE: OnceLock<Option<Mutex<fs::File>>> = OnceLock::new();
static START: OnceLock<Instant> = OnceLock::new();
static IO_BYTES: AtomicU64 = AtomicU64::new(0);
static H2D_BYTES: AtomicU64 = AtomicU64::new(0);
static D2H_BYTES: AtomicU64 = AtomicU64::new(0);
static COPIES: AtomicU64 = AtomicU64::new(0);

pub fn unix_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}
fn start() -> &'static Instant {
    START.get_or_init(Instant::now)
}
fn trace_id() -> String {
    std::env::var("FASTCULL_RAW_TRACE_ID").unwrap_or_else(|_| format!("pid-{}", std::process::id()))
}
fn escape(value: &str, out: &mut String) {
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
}
/// Render a JSON string literal; use for paths and enum-like values.
pub fn string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    escape(value, &mut out);
    out.push('"');
    out
}
fn file() -> Option<&'static Mutex<fs::File>> {
    FILE.get_or_init(|| {
        let directory = PathBuf::from(std::env::var_os("FASTCULL_RAW_TRACE_DIR")?);
        fs::create_dir_all(&directory).ok()?;
        let path = directory.join(format!("raw-trace-{}.jsonl", std::process::id()));
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()
            .map(Mutex::new)
    })
    .as_ref()
}
/// Append one event. The caller owns the schema; stages used by the benchmark
/// harness are documented in docs/diagnostics/RAW-TRACE-SCHEMA-2026-11-11.md.
pub fn event(stage: &str, fields: Vec<(&str, String)>) {
    let Some(file) = file() else {
        return;
    };
    let mut line = String::with_capacity(192);
    line.push_str("{\"trace\":");
    line.push_str(&string(&trace_id()));
    let _ = write!(
        line,
        ",\"pid\":{},\"mono_ms\":{:.3},\"unix_us\":{},\"stage\":",
        std::process::id(),
        start().elapsed().as_secs_f64() * 1000.,
        unix_us()
    );
    line.push_str(&string(stage));
    for (key, value) in fields {
        line.push(',');
        line.push_str(&string(key));
        line.push(':');
        line.push_str(&value);
    }
    line.push_str("}\n");
    if let Ok(mut file) = file.lock() {
        let _ = file.write_all(line.as_bytes());
    }
}
/// Log process start once; `mode` is the CLI mode or `ui`.
pub fn install(mode: &str) {
    event(
        "process_start",
        vec![
            ("mode", string(mode)),
            (
                "exe",
                string(
                    &std::env::current_exe()
                        .map_or_else(|_| "?".into(), |p| p.to_string_lossy().into_owned()),
                ),
            ),
        ],
    );
}
pub fn add_io(bytes: u64) {
    IO_BYTES.fetch_add(bytes, Ordering::Relaxed);
}
pub fn add_h2d(bytes: u64) {
    H2D_BYTES.fetch_add(bytes, Ordering::Relaxed);
}
pub fn add_d2h(bytes: u64) {
    D2H_BYTES.fetch_add(bytes, Ordering::Relaxed);
}
pub fn add_copy(count: u64) {
    COPIES.fetch_add(count, Ordering::Relaxed);
}
/// Cumulative transfer counters, ready to append to an event.
pub fn counters() -> Vec<(&'static str, String)> {
    vec![
        ("io_bytes", IO_BYTES.load(Ordering::Relaxed).to_string()),
        ("h2d_bytes", H2D_BYTES.load(Ordering::Relaxed).to_string()),
        ("d2h_bytes", D2H_BYTES.load(Ordering::Relaxed).to_string()),
        ("copies", COPIES.load(Ordering::Relaxed).to_string()),
    ]
}
