//! Latency distribution harness: writes percentiles per workload as JSON.
//!
//! Each workload is restored (untimed) and its commands are applied in
//! fixed-size chunks; each chunk is timed with `Instant` and contributes one
//! sample, the mean time per command in that chunk. Chunking keeps samples
//! well above the timer resolution, which is measured and reported (about
//! 100 ns on Windows). Percentiles are therefore of chunk means, not of
//! individual commands; this is stated in the output.
//!
//! Usage: latency [--out FILE] [--rounds N] [--chunk N] [--machine TEXT] [--commit SHA]

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::float_arithmetic,
    clippy::arithmetic_side_effects,
    reason = "statistics over timing samples; not engine accounting"
)]

use std::fmt::Write as _;
use std::fs;
use std::hint::black_box;
use std::time::{Duration, Instant};

use lob::Event;
use lob_bench::Workload;

struct Args {
    out: String,
    rounds: usize,
    chunk: Option<usize>,
    machine: String,
    commit: String,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        out: "target/latency.json".into(),
        rounds: 50,
        chunk: None,
        machine: "unspecified".into(),
        commit: "unspecified".into(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--out" => args.out = value()?,
            "--rounds" => args.rounds = value()?.parse().map_err(|e| format!("--rounds: {e}"))?,
            "--chunk" => {
                let n: usize = value()?.parse().map_err(|e| format!("--chunk: {e}"))?;
                args.chunk = Some(n.max(1));
            }
            "--machine" => args.machine = value()?,
            "--commit" => args.commit = value()?,
            _ => return Err(format!("unknown argument {flag}")),
        }
    }
    Ok(args)
}

/// Smallest non-zero step between consecutive `Instant::now()` readings and
/// the mean cost of one reading.
fn timer_characteristics() -> (Duration, Duration) {
    let mut resolution = Duration::MAX;
    for _ in 0..10_000 {
        let a = Instant::now();
        let mut b = Instant::now();
        while b == a {
            b = Instant::now();
        }
        resolution = resolution.min(b - a);
    }
    let n = 1_000_000u32;
    let t = Instant::now();
    for _ in 0..n {
        black_box(Instant::now());
    }
    (resolution, t.elapsed() / n)
}

/// Commands per timed chunk: enough that a chunk spans many timer ticks.
fn chunk_size(w: &Workload, forced: Option<usize>) -> usize {
    if let Some(n) = forced {
        return n;
    }
    match w.name {
        "sweep" => 1, // ~5 µs per command already
        _ => 16,
    }
}

struct Stats {
    samples: usize,
    chunk: usize,
    rounds: usize,
    p50: f64,
    p95: f64,
    p99: f64,
    max: f64,
    mean: f64,
    throughput: f64,
}

fn measure(w: &Workload, rounds: usize, forced_chunk: Option<usize>) -> Stats {
    let chunk = chunk_size(w, forced_chunk);
    let mut samples = Vec::new();
    let mut total_ns = 0u128;
    let mut total_cmds = 0usize;
    let warmup = 3;
    for round in 0..warmup + rounds {
        let mut engine = w.engine();
        let mut out: Vec<Event> = Vec::with_capacity(256);
        for cmds in w.commands.chunks(chunk) {
            let t = Instant::now();
            for cmd in cmds {
                out.clear();
                engine.apply(*cmd, &mut out);
                black_box(&out);
            }
            let ns = t.elapsed().as_nanos();
            if round >= warmup {
                samples.push(ns as f64 / cmds.len() as f64);
                total_ns += ns;
                total_cmds += cmds.len();
            }
        }
        black_box(engine);
    }
    samples.sort_by(f64::total_cmp);
    let pct = |p: f64| samples[((samples.len() - 1) as f64 * p).round() as usize];
    Stats {
        samples: samples.len(),
        chunk,
        rounds,
        p50: pct(0.50),
        p95: pct(0.95),
        p99: pct(0.99),
        max: *samples.last().unwrap(),
        mean: samples.iter().sum::<f64>() / samples.len() as f64,
        throughput: total_cmds as f64 / (total_ns as f64 / 1e9),
    }
}

fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => write!(out, "\\u{:04x}", c as u32).unwrap(),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!(
                "{e}\nusage: latency [--out FILE] [--rounds N] [--chunk N] [--machine TEXT] [--commit SHA]"
            );
            std::process::exit(2);
        }
    };
    if cfg!(debug_assertions) {
        eprintln!("warning: debug build; use `cargo run --release`");
    }
    let (resolution, overhead) = timer_characteristics();
    println!(
        "timer resolution {} ns, Instant::now cost {} ns",
        resolution.as_nanos(),
        overhead.as_nanos()
    );
    println!(
        "{:<13} {:>6} {:>8} {:>9} {:>9} {:>9} {:>11}",
        "workload", "chunk", "samples", "p50 ns", "p95 ns", "p99 ns", "cmds/s"
    );

    let mut rows = Vec::new();
    for w in lob_bench::all() {
        let s = measure(&w, args.rounds, args.chunk);
        println!(
            "{:<13} {:>6} {:>8} {:>9.0} {:>9.0} {:>9.0} {:>11.0}",
            w.name, s.chunk, s.samples, s.p50, s.p95, s.p99, s.throughput
        );
        rows.push(format!(
            "    {{\"name\": {}, \"description\": {}, \"commands_per_round\": {}, \"rounds\": {}, \
             \"chunk\": {}, \"samples\": {}, \"p50_ns\": {:.1}, \"p95_ns\": {:.1}, \"p99_ns\": {:.1}, \
             \"max_ns\": {:.1}, \"mean_ns\": {:.1}, \"throughput_per_s\": {:.0}}}",
            json_str(w.name),
            json_str(w.description),
            w.commands.len(),
            s.rounds,
            s.chunk,
            s.samples,
            s.p50,
            s.p95,
            s.p99,
            s.max,
            s.mean,
            s.throughput
        ));
    }

    let json = format!(
        "{{\n  \"schema\": 1,\n  \"sample_definition\": \"mean ns per command over a timed chunk of consecutive commands\",\n  \
         \"machine\": {},\n  \"commit\": {},\n  \"os\": {},\n  \"arch\": {},\n  \"logical_cpus\": {},\n  \
         \"profile\": {},\n  \"timer_resolution_ns\": {},\n  \"timer_overhead_ns\": {},\n  \"warmup_rounds\": 3,\n  \
         \"workloads\": [\n{}\n  ]\n}}\n",
        json_str(&args.machine),
        json_str(&args.commit),
        json_str(std::env::consts::OS),
        json_str(std::env::consts::ARCH),
        std::thread::available_parallelism().map_or(0, std::num::NonZero::get),
        json_str(if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }),
        resolution.as_nanos(),
        overhead.as_nanos(),
        rows.join(",\n")
    );
    if let Err(e) = fs::write(&args.out, json) {
        eprintln!("{}: {e}", args.out);
        std::process::exit(1);
    }
    println!("wrote {}", args.out);
}
