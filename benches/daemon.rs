//! High-performance benchmark comparing Process Fork/Exec vs. Zero-Fork UDS Daemon.
//! Evaluates tail latency distributions, Coordinated Omission, frame deadline miss rates,
//! and 60/120 Hz frame budget utilization for Yazi preview navigation and Neovim live typing.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(
    dead_code,
    clippy::too_many_lines,
    clippy::many_single_char_names,
    clippy::similar_names,
    clippy::print_stdout,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "benchmark statistical telemetry and interactive terminal dashboard formatting"
)]

use std::fmt::Write as _;
use std::fs;
use std::hint::black_box;
use std::io::{self, IsTerminal, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use ii::ipc::{Cmd, Header};

/// Sample Markdown document representative of a typical Yazi file preview or Neovim buffer.
const BENCH_PAYLOAD: &[u8] = b"# Industrial Performance Telemetry\n\
Zero-overhead terminal graphical coprocessor for mathematical typesetting and architectural diagrams.\n\n\
$$\\oint_{\\partial \\Omega} \\mathbf{E} \\cdot d\\mathbf{l} = -\\frac{\\partial}{\\partial t} \\iint_{\\Omega} \\mathbf{B} \\cdot d\\mathbf{A}$$\n\n\
Chemical reaction kinetics:\n\
$\\ce{2H2 + O2 -> 2H2O}$\n\n\
```mermaid\ngraph LR; Host-->|UDS Socket| Daemon; Daemon-->|Kitty Placeholders| Terminal;\n```\n";

const BUDGET_60HZ_NS: u64 = 16_666_667;
const BUDGET_120HZ_NS: u64 = 8_333_333;
const MAX_SYNTHETIC_SAMPLES_PER_EVENT: usize = 512;

/// High-resolution monotonic timestamp in nanoseconds via `std::time::Instant`.
#[inline]
fn get_time_ns() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let start = START.get_or_init(Instant::now);
    start.elapsed().as_nanos() as u64
}

/// RAII guard ensuring temporary benchmark files and directories are unlinked on exit.
struct TempArtifactGuard {
    paths: Vec<PathBuf>,
}

impl TempArtifactGuard {
    const fn new(paths: Vec<PathBuf>) -> Self {
        Self { paths }
    }
}

impl Drop for TempArtifactGuard {
    fn drop(&mut self) {
        for path in &self.paths {
            if path.is_dir() {
                let _ = fs::remove_dir_all(path);
            } else {
                let _ = fs::remove_file(path);
            }
        }
    }
}

/// Statistical aggregator computing continuous Type-7 HDR percentiles, MAD jitter,
/// Gil Tene Coordinated Omission corrections, and frame deadline violations.
#[derive(Debug, Default)]
struct MetricsAggregator {
    samples_ns: Vec<u64>,
    corrected_samples_ns: Vec<u64>,
}

impl MetricsAggregator {
    fn with_capacity(cap: usize) -> Self {
        Self {
            samples_ns: Vec::with_capacity(cap),
            corrected_samples_ns: Vec::with_capacity(cap.saturating_mul(2)),
        }
    }

    fn record(&mut self, lat_ns: u64, expected_interval_ns: u64) {
        self.samples_ns.push(lat_ns);
        self.corrected_samples_ns.push(lat_ns);

        if expected_interval_ns > 0 && lat_ns > expected_interval_ns {
            let mut synthetic = lat_ns.saturating_sub(expected_interval_ns);
            let mut count = 0usize;
            while synthetic >= expected_interval_ns && count < MAX_SYNTHETIC_SAMPLES_PER_EVENT {
                self.corrected_samples_ns.push(synthetic);
                synthetic = synthetic.saturating_sub(expected_interval_ns);
                count += 1;
            }
        }
    }

    fn analyze(&mut self) -> AnalysisResult {
        if self.samples_ns.is_empty() {
            return AnalysisResult::default();
        }

        self.samples_ns.sort_unstable();
        self.corrected_samples_ns.sort_unstable();

        let n = self.samples_ns.len();
        let sum_ns: u128 = self.samples_ns.iter().copied().map(u128::from).sum();
        let mean_us = (sum_ns as f64) / (n as f64) / 1000.0;

        let var = self
            .samples_ns
            .iter()
            .map(|&s| {
                let diff = (s as f64 / 1000.0) - mean_us;
                diff * diff
            })
            .sum::<f64>()
            / (n.max(1) as f64);
        let std_dev_us = var.sqrt();

        let p50_us = quantile_type7_us(&self.samples_ns, 0.50);

        let mut abs_devs: Vec<f64> = self
            .samples_ns
            .iter()
            .map(|&s| ((s as f64 / 1000.0) - p50_us).abs())
            .collect();
        abs_devs.sort_by(f64::total_cmp);
        let mad_us = if abs_devs.is_empty() {
            0.0
        } else {
            abs_devs[abs_devs.len() / 2]
        };

        let miss_60 = self
            .samples_ns
            .iter()
            .filter(|&&s| s > BUDGET_60HZ_NS)
            .count() as f64
            / (n as f64)
            * 100.0;
        let miss_120 = self
            .samples_ns
            .iter()
            .filter(|&&s| s > BUDGET_120HZ_NS)
            .count() as f64
            / (n as f64)
            * 100.0;

        let min_us = (self.samples_ns.first().copied().unwrap_or(0) as f64) / 1000.0;
        let max_us = (self.samples_ns.last().copied().unwrap_or(0) as f64) / 1000.0;
        let throughput_fps = if mean_us > 0.0 {
            1_000_000.0 / mean_us
        } else {
            0.0
        };

        AnalysisResult {
            count: n,
            mean_us,
            std_dev_us,
            mad_us,
            min_us,
            p50_us,
            p90_us: quantile_type7_us(&self.samples_ns, 0.90),
            p95_us: quantile_type7_us(&self.samples_ns, 0.95),
            p99_us: quantile_type7_us(&self.samples_ns, 0.99),
            p999_us: quantile_type7_us(&self.samples_ns, 0.999),
            max_us,
            co_p99_us: quantile_type7_us(&self.corrected_samples_ns, 0.99),
            miss_rate_60hz: miss_60,
            miss_rate_120hz: miss_120,
            throughput_fps,
        }
    }
}

/// Hyndman–Fan Type 7 continuous linear-interpolated quantile estimator (in microseconds).
#[inline]
fn quantile_type7_us(sorted_ns: &[u64], q: f64) -> f64 {
    let n = sorted_ns.len();
    if n == 0 {
        return 0.0;
    }
    if n == 1 {
        return (sorted_ns[0] as f64) / 1000.0;
    }
    let clamped_q = q.clamp(0.0, 1.0);
    let pos = clamped_q * ((n - 1) as f64);
    let lo = (pos.floor() as usize).min(n - 1);
    let hi = (lo + 1).min(n - 1);
    let frac = pos - (lo as f64);
    let v_lo = (sorted_ns[lo] as f64) / 1000.0;
    let v_hi = (sorted_ns[hi] as f64) / 1000.0;
    frac.mul_add(v_hi - v_lo, v_lo)
}

#[derive(Debug, Default, Clone, Copy)]
struct AnalysisResult {
    count: usize,
    mean_us: f64,
    std_dev_us: f64,
    mad_us: f64,
    min_us: f64,
    p50_us: f64,
    p90_us: f64,
    p95_us: f64,
    p99_us: f64,
    p999_us: f64,
    max_us: f64,
    co_p99_us: f64,
    miss_rate_60hz: f64,
    miss_rate_120hz: f64,
    throughput_fps: f64,
}

/// Resolves path to compiled `ii` binary for CLI execution.
fn resolve_ii_binary() -> PathBuf {
    if let Some(env_bin) = std::env::var_os("CARGO_BIN_EXE_ii") {
        let p = PathBuf::from(env_bin);
        if p.exists() {
            return p;
        }
    }
    let release_candidate = PathBuf::from("./target/release/ii");
    if release_candidate.exists() {
        return release_candidate;
    }
    let debug_candidate = PathBuf::from("./target/debug/ii");
    if debug_candidate.exists() {
        return debug_candidate;
    }
    PathBuf::from("ii")
}

fn send_raw(sock: &Path, hdr: &Header, payload: &[u8]) -> io::Result<(Header, Vec<u8>)> {
    let mut stream = UnixStream::connect(sock)?;
    let mut req = Vec::with_capacity(32 + payload.len());
    req.extend_from_slice(&hdr.to_le_bytes());
    req.extend_from_slice(payload);
    stream.write_all(&req)?;

    let mut raw_hdr = [0u8; 32];
    stream.read_exact(&mut raw_hdr)?;
    let resp_hdr = Header::from_le_bytes(&raw_hdr);
    let mut body = vec![0u8; resp_hdr.payload_len as usize];
    stream.read_exact(&mut body)?;
    Ok((resp_hdr, body))
}

fn spawn_bench_daemon(runtime_dir: &Path, bin: &Path) -> Option<(Child, PathBuf)> {
    let sock_path = runtime_dir.join("ii").join("ii.sock");
    let child = Command::new(bin)
        .arg("--daemon")
        .env("XDG_RUNTIME_DIR", runtime_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let deadline = Instant::now() + Duration::from_secs(2);
    while !sock_path.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    if !sock_path.exists() {
        return None;
    }
    Some((child, sock_path))
}

fn shutdown_daemon(sock_path: &Path, mut child: Child) {
    let hdr = Header::new(Cmd::Shutdown, 0);
    let _ = send_raw(sock_path, &hdr, &[]);
    let _ = child.wait();
}

fn run_yazi_navigation_benchmark() {
    const WARMUP_ITERS: usize = 3;
    const ITERATIONS: usize = 60;

    let pid = std::process::id();
    let temp_dir = std::env::temp_dir();
    let temp_doc_path = temp_dir.join(format!("ii_bench_doc_{pid}.md"));
    let runtime_dir = temp_dir.join(format!("ii_bench_rt_{pid}"));

    let _cleanup = TempArtifactGuard::new(vec![temp_doc_path.clone(), runtime_dir.clone()]);

    if fs::write(&temp_doc_path, BENCH_PAYLOAD).is_err() {
        return;
    }

    let ii_bin = resolve_ii_binary();

    // 1. Process Fork/Exec Model
    for _ in 0..WARMUP_ITERS {
        let _ = Command::new(&ii_bin).arg(&temp_doc_path).output();
    }

    let mut fork_metrics = MetricsAggregator::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        let t0 = get_time_ns();
        let output = Command::new(&ii_bin).arg(&temp_doc_path).output();
        let t1 = get_time_ns();
        if let Ok(out) = output
            && out.status.success()
        {
            fork_metrics.record(t1.saturating_sub(t0), BUDGET_60HZ_NS);
        }
    }
    let fork_analysis = fork_metrics.analyze();

    // 2. Zero-Fork UDS Daemon Model
    let Some((daemon_child, sock_path)) = spawn_bench_daemon(&runtime_dir, &ii_bin) else {
        return;
    };

    let hdr = Header::new(Cmd::ComposeAnsi, BENCH_PAYLOAD.len() as u32);
    for _ in 0..WARMUP_ITERS {
        let _ = send_raw(&sock_path, &hdr, BENCH_PAYLOAD);
    }

    let mut uds_metrics = MetricsAggregator::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        let t0 = get_time_ns();
        let ok = send_raw(&sock_path, &hdr, black_box(BENCH_PAYLOAD)).is_ok();
        let t1 = get_time_ns();
        if ok {
            uds_metrics.record(t1.saturating_sub(t0), BUDGET_60HZ_NS);
        }
    }
    let uds_analysis = uds_metrics.analyze();

    shutdown_daemon(&sock_path, daemon_child);
    render_executive_telemetry_dashboard(&fork_analysis, &uds_analysis, ITERATIONS);
}

/// ANSI styling palette that automatically degrades to plain text when `NO_COLOR` is set.
struct UiStyle {
    enabled: bool,
}

impl UiStyle {
    fn detect() -> Self {
        let no_color = std::env::var_os("NO_COLOR").is_some();
        let is_tty = io::stdout().is_terminal();
        Self {
            enabled: !no_color && is_tty,
        }
    }

    #[inline]
    const fn border(&self) -> &'static str {
        if self.enabled { "\x1b[38;5;240m" } else { "" }
    }
    #[inline]
    const fn title(&self) -> &'static str {
        if self.enabled { "\x1b[1;38;5;117m" } else { "" }
    }
    #[inline]
    const fn header(&self) -> &'static str {
        if self.enabled { "\x1b[1;38;5;252m" } else { "" }
    }
    #[inline]
    const fn dim(&self) -> &'static str {
        if self.enabled { "\x1b[38;5;245m" } else { "" }
    }
    #[inline]
    const fn good(&self) -> &'static str {
        if self.enabled { "\x1b[1;38;5;114m" } else { "" }
    }
    #[inline]
    const fn warn(&self) -> &'static str {
        if self.enabled { "\x1b[1;38;5;215m" } else { "" }
    }
    #[inline]
    const fn bad(&self) -> &'static str {
        if self.enabled { "\x1b[1;38;5;203m" } else { "" }
    }
    #[inline]
    const fn reset(&self) -> &'static str {
        if self.enabled { "\x1b[0m" } else { "" }
    }
}

fn format_utilization_bar(ratio: f64, width: usize) -> String {
    const BLOCKS: [char; 8] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];
    let clamped = ratio.clamp(0.0, 1.0);
    let total_eighths = (clamped * (width as f64) * 8.0).round() as usize;
    let full_cells = (total_eighths / 8).min(width);
    let rem_eighth = total_eighths % 8;
    let mut out = String::with_capacity(width * 4);
    for _ in 0..full_cells {
        out.push('█');
    }
    if full_cells < width && rem_eighth > 0 {
        out.push(BLOCKS[rem_eighth.saturating_sub(1)]);
    }
    while out.chars().count() < width {
        out.push(' ');
    }
    out
}

fn render_executive_telemetry_dashboard(
    fork: &AnalysisResult,
    uds: &AnalysisResult,
    iterations: usize,
) {
    let ui = UiStyle::detect();
    let b = ui.border();
    let t = ui.title();
    let h = ui.header();
    let d = ui.dim();
    let g = ui.good();
    let w = ui.warn();
    let r = ui.reset();

    let speedup = if uds.mean_us > 0.0 {
        fork.mean_us / uds.mean_us
    } else {
        1.0
    };
    let speedup_p99 = if uds.p99_us > 0.0 {
        fork.p99_us / uds.p99_us
    } else {
        1.0
    };

    let bar_fork_60 = format_utilization_bar(fork.p99_us / 16_666.67, 24);
    let bar_uds_60 = format_utilization_bar(uds.p99_us / 16_666.67, 24);
    let bar_uds_120 = format_utilization_bar(uds.p99_us / 8_333.33, 24);

    let fork_60_color = if fork.miss_rate_60hz > 0.0 {
        ui.bad()
    } else {
        ui.good()
    };
    let uds_60_color = if uds.miss_rate_60hz > 0.0 {
        ui.bad()
    } else {
        ui.good()
    };
    let uds_col_color = if uds.miss_rate_120hz > 0.0 {
        ui.warn()
    } else {
        ui.good()
    };

    let mut out = String::with_capacity(4096);
    let _ = writeln!(
        out,
        "\n{b}╭─────────────────────────────────────────────────────────────────────────────────────────────────╮{r}"
    );
    let _ = writeln!(
        out,
        "{b}│{r}  {t}INDUSTRIAL COPROCESSOR TELEMETRY: FORK/EXEC vs ZERO-FORK UDS DAEMON{r}                         {b}│{r}"
    );
    let _ = writeln!(
        out,
        "{b}│{r}  {d}Benchmark: Yazi/Neovim 60-frame interactive burst ({iterations} iterations, warm VRAM + cache){r}     {b}│{r}"
    );
    let _ = writeln!(
        out,
        "{b}├────────────────────────┬─────────────────────────┬─────────────────────────┬────────────────────┤{r}"
    );
    let _ = writeln!(
        out,
        "{b}│{r} {h}{:<22}{r} {b}│{r} {h}{:>23}{r} {b}│{r} {h}{:>23}{r} {b}│{r} {h}{:>18}{r} {b}│{r}",
        "Metric / Percentile", "Process Fork/Exec", "Zero-Fork UDS Daemon", "Delta / Multiplier"
    );
    let _ = writeln!(
        out,
        "{b}├────────────────────────┼─────────────────────────┼─────────────────────────┼────────────────────┤{r}"
    );

    let _ = writeln!(
        out,
        "{b}│{r} {:<22} {b}│{r} {:>20.2} µs {b}│{r} {:>20.2} µs {b}│{r} {g}{:>16.1}x{r} {b}│{r}",
        "Mean Latency", fork.mean_us, uds.mean_us, speedup
    );
    let _ = writeln!(
        out,
        "{b}│{r} {:<22} {b}│{r} {:>20.2} µs {b}│{r} {:>20.2} µs {b}│{r} {:>17} {b}│{r}",
        "Median (p50)",
        fork.p50_us,
        uds.p50_us,
        format!("{:.1}x", fork.p50_us / uds.p50_us.max(0.1))
    );
    let _ = writeln!(
        out,
        "{b}│{r} {:<22} {b}│{r} {:>20.2} µs {b}│{r} {:>20.2} µs {b}│{r} {:>17} {b}│{r}",
        "p90 Latency",
        fork.p90_us,
        uds.p90_us,
        format!("{:.1}x", fork.p90_us / uds.p90_us.max(0.1))
    );
    let _ = writeln!(
        out,
        "{b}│{r} {:<22} {b}│{r} {:>20.2} µs {b}│{r} {:>20.2} µs {b}│{r} {:>17} {b}│{r}",
        "p95 Latency",
        fork.p95_us,
        uds.p95_us,
        format!("{:.1}x", fork.p95_us / uds.p95_us.max(0.1))
    );
    let _ = writeln!(
        out,
        "{b}│{r} {w}{:<22}{r} {b}│{r} {:>20.2} µs {b}│{r} {:>20.2} µs {b}│{r} {g}{:>16.1}x{r} {b}│{r}",
        "Tail Latency (p99)", fork.p99_us, uds.p99_us, speedup_p99
    );
    let _ = writeln!(
        out,
        "{b}│{r} {:<22} {b}│{r} {:>20.2} µs {b}│{r} {:>20.2} µs {b}│{r} {:>17} {b}│{r}",
        "p99.9 Latency",
        fork.p999_us,
        uds.p999_us,
        format!("{:.1}x", fork.p999_us / uds.p999_us.max(0.1))
    );
    let _ = writeln!(
        out,
        "{b}│{r} {:<22} {b}│{r} {:>20.2} µs {b}│{r} {:>20.2} µs {b}│{r} {:>18} {b}│{r}",
        "Max Tail Spike", fork.max_us, uds.max_us, "-"
    );
    let _ = writeln!(
        out,
        "{b}│{r} {:<22} {b}│{r} {:>20.2} µs {b}│{r} {:>20.2} µs {b}│{r} {:>18} {b}│{r}",
        "Jitter (MAD)", fork.mad_us, uds.mad_us, "-"
    );
    let _ = writeln!(
        out,
        "{b}│{r} {:<22} {b}│{r} {:>20.2} µs {b}│{r} {:>20.2} µs {b}│{r} {:>18} {b}│{r}",
        "Std Deviation (σ)", fork.std_dev_us, uds.std_dev_us, "-"
    );
    let _ = writeln!(
        out,
        "{b}│{r} {:<22} {b}│{r} {:>20.2} µs {b}│{r} {:>20.2} µs {b}│{r} {:>18} {b}│{r}",
        "Coordinated Omission p99", fork.co_p99_us, uds.co_p99_us, "-"
    );
    let _ = writeln!(
        out,
        "{b}│{r} {:<22} {b}│{r} {:>21.1}   {b}│{r} {:>21.1}   {b}│{r} {g}{:>16.1}x{r} {b}│{r}",
        "Throughput (FPS cap)", fork.throughput_fps, uds.throughput_fps, speedup
    );
    let _ = writeln!(
        out,
        "{b}│{r} {:<22} {b}│{r} {:>20.1}%  {b}│{r} {:>20.1}%  {b}│{r} {:>18} {b}│{r}",
        "60 Hz Miss Rate (>16ms)",
        fork.miss_rate_60hz,
        uds.miss_rate_60hz,
        if uds.miss_rate_60hz == 0.0 {
            "ZERO MISS"
        } else {
            "-"
        }
    );
    let _ = writeln!(
        out,
        "{b}│{r} {:<22} {b}│{r} {:>20.1}%  {b}│{r} {:>20.1}%  {b}│{r} {:>18} {b}│{r}",
        "120 Hz Miss Rate (>8ms)",
        fork.miss_rate_120hz,
        uds.miss_rate_120hz,
        if uds.miss_rate_120hz == 0.0 {
            "ZERO MISS"
        } else {
            "-"
        }
    );
    let _ = writeln!(
        out,
        "{b}├─────────────────────────────────────────────────────────────────────────────────────────────────┤{r}"
    );
    let _ = writeln!(
        out,
        "{b}│{r}  {t}FRAME BUDGET CONSUMPTION AT TAIL LATENCY (p99){r}                                              {b}│{r}"
    );
    let _ = writeln!(
        out,
        "{b}│{r}   Fork/Exec @  60 Hz (16.67 ms) : {fork_60_color}{bar_fork_60}{r} {:>6.2}% of frame budget                 {b}│{r}",
        (fork.p99_us / 16_666.67) * 100.0
    );
    let _ = writeln!(
        out,
        "{b}│{r}   Zero-Fork @  60 Hz (16.67 ms) : {uds_60_color}{bar_uds_60}{r} {:>6.2}% of frame budget                 {b}│{r}",
        (uds.p99_us / 16_666.67) * 100.0
    );
    let _ = writeln!(
        out,
        "{b}│{r}   Zero-Fork @ 120 Hz ( 8.33 ms) : {uds_col_color}{bar_uds_120}{r} {:>6.2}% of frame budget                 {b}│{r}",
        (uds.p99_us / 8_333.33) * 100.0
    );
    let _ = writeln!(
        out,
        "{b}╰─────────────────────────────────────────────────────────────────────────────────────────────────╯{r}\n"
    );
    print!("{out}");
}

fn criterion_bench_uds_roundtrip(c: &mut Criterion) {
    let mut group = c.benchmark_group("uds_transport");
    group.throughput(Throughput::Bytes(BENCH_PAYLOAD.len() as u64));

    let pid = std::process::id();
    let temp_dir = std::env::temp_dir();
    let runtime_dir = temp_dir.join(format!("ii_crit_rt_{pid}"));
    let _cleanup = TempArtifactGuard::new(vec![runtime_dir.clone()]);

    let ii_bin = resolve_ii_binary();
    let Some((daemon_child, sock_path)) = spawn_bench_daemon(&runtime_dir, &ii_bin) else {
        return;
    };

    let hdr = Header::new(Cmd::ComposeAnsi, BENCH_PAYLOAD.len() as u32);
    // Prime daemon cache
    let _ = send_raw(&sock_path, &hdr, BENCH_PAYLOAD);

    group.bench_function("uds_client_roundtrip_warm", |b| {
        b.iter(|| {
            let ok = send_raw(&sock_path, &hdr, black_box(BENCH_PAYLOAD)).is_ok();
            black_box(ok)
        });
    });

    shutdown_daemon(&sock_path, daemon_child);
    group.finish();
}

fn run_benchmarks(c: &mut Criterion) {
    run_yazi_navigation_benchmark();
    criterion_bench_uds_roundtrip(c);
}

criterion_group!(benches, run_benchmarks);
criterion_main!(benches);
