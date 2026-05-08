//! Tiny measurement harness for the H1 perf baseline.
//!
//! One mode per metric so the bench script can keep them straight:
//! - `ping` — N IPC `ping` round-trips against an already-running daemon
//! - `db-read` — N `account.list` round-trips against an already-running daemon
//! - `parse` — N in-process `mail::parser::parse` calls on a fixed corpus
//!
//! Each mode prints a single line to stdout: `min,p50,p95,max` in microseconds
//! for ping/db-read, or `messages,elapsed_ms,msgs_per_sec` for parse. Stderr
//! gets human-readable progress; bench/run.sh consumes stdout.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{anyhow, Context};
use serde_json::json;

use postblox::ipc::client::Client;
use postblox::ipc::default_socket_path;
use postblox::mail;

const PING_DEFAULT: usize = 1000;
const DB_READ_DEFAULT: usize = 1000;
const PARSE_CORPUS: usize = 100;
const PARSE_DEFAULT: usize = 10_000;

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mode = args.next().context("usage: postblox-bench <mode> [n]")?;
    let n: Option<usize> = args.next().and_then(|s| s.parse().ok());

    match mode.as_str() {
        "ping" => bench_ping(n.unwrap_or(PING_DEFAULT)).await,
        "db-read" => bench_db_read(n.unwrap_or(DB_READ_DEFAULT)).await,
        "parse" => {
            bench_parse(n.unwrap_or(PARSE_DEFAULT));
            Ok(())
        }
        other => Err(anyhow!("unknown mode: {other}")),
    }
}

fn socket_path() -> PathBuf {
    std::env::var_os("POSTBLOX_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(default_socket_path)
}

async fn bench_ping(n: usize) -> anyhow::Result<()> {
    let path = socket_path();
    let mut client = Client::connect(&path)
        .await
        .with_context(|| format!("connect to {}", path.display()))?;

    // Warmup so the first JIT / page-fault doesn't skew the median.
    for _ in 0..16 {
        let resp = client.request("ping", json!({})).await?;
        if !resp.ok {
            return Err(anyhow!("ping failed during warmup"));
        }
    }

    let mut samples_us: Vec<u64> = Vec::with_capacity(n);
    for _ in 0..n {
        let t0 = Instant::now();
        let resp = client.request("ping", json!({})).await?;
        let elapsed = t0.elapsed();
        if !resp.ok {
            return Err(anyhow!("ping returned !ok"));
        }
        samples_us.push(elapsed.as_micros() as u64);
    }
    print_latency_summary("ping", &mut samples_us);
    Ok(())
}

async fn bench_db_read(n: usize) -> anyhow::Result<()> {
    let path = socket_path();
    let mut client = Client::connect(&path)
        .await
        .with_context(|| format!("connect to {}", path.display()))?;

    for _ in 0..16 {
        let resp = client.request("account.list", json!({})).await?;
        if !resp.ok {
            return Err(anyhow!("account.list failed during warmup"));
        }
    }

    let mut samples_us: Vec<u64> = Vec::with_capacity(n);
    for _ in 0..n {
        let t0 = Instant::now();
        let resp = client.request("account.list", json!({})).await?;
        let elapsed = t0.elapsed();
        if !resp.ok {
            return Err(anyhow!("account.list returned !ok"));
        }
        samples_us.push(elapsed.as_micros() as u64);
    }
    print_latency_summary("db-read", &mut samples_us);
    Ok(())
}

fn bench_parse(n: usize) {
    let corpus: Vec<Vec<u8>> = (0..PARSE_CORPUS).map(synthetic_email).collect();

    // Warmup
    for raw in corpus.iter().take(16) {
        let _ = mail::parser::parse(raw).expect("warmup parse");
    }

    let t0 = Instant::now();
    let mut parsed = 0u64;
    for i in 0..n {
        let raw = &corpus[i % corpus.len()];
        let _ = mail::parser::parse(raw).expect("parse");
        parsed += 1;
    }
    let elapsed = t0.elapsed();
    let elapsed_ms = elapsed.as_secs_f64() * 1000.0;
    let per_sec = (parsed as f64) / elapsed.as_secs_f64();
    eprintln!(
        "parse: {} messages parsed in {:.3} ms ({:.0} msgs/sec)",
        parsed, elapsed_ms, per_sec
    );
    println!("{},{:.3},{:.0}", parsed, elapsed_ms, per_sec);
}

fn synthetic_email(seed: usize) -> Vec<u8> {
    // Small, realistic-looking multipart with text + tiny html part.
    let body_text = format!(
        "Hello {seed},\r\n\r\nThis is a synthetic test message #{seed}.\r\nUsed for parser throughput measurements.\r\n\r\n-- \r\npostblox bench\r\n"
    );
    let body_html = format!(
        "<html><body><p>Hello <b>{seed}</b></p><p>Synthetic test message #{seed}.</p></body></html>"
    );
    let raw = format!(
        "From: bench-{seed}@example.com\r\n\
         To: dest-{seed}@example.com\r\n\
         Cc: cc-{seed}@example.com\r\n\
         Subject: bench message #{seed}\r\n\
         Date: Thu, 09 May 2026 12:00:{:02} +0000\r\n\
         Message-ID: <bench-{seed}@example.com>\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: multipart/alternative; boundary=\"BOUNDARY42\"\r\n\
         \r\n\
         --BOUNDARY42\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         {body_text}\r\n\
         --BOUNDARY42\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         \r\n\
         {body_html}\r\n\
         --BOUNDARY42--\r\n",
        seed % 60
    );
    raw.into_bytes()
}

fn print_latency_summary(label: &str, samples_us: &mut [u64]) {
    samples_us.sort_unstable();
    let n = samples_us.len();
    let min = samples_us[0];
    let p50 = samples_us[n / 2];
    let p95 = samples_us[(n * 95 / 100).min(n - 1)];
    let max = samples_us[n - 1];
    eprintln!("{label}: n={n} min={min}us p50={p50}us p95={p95}us max={max}us",);
    println!("{},{},{},{}", min, p50, p95, max);
}
