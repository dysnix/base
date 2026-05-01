//! End-to-end FCU latency measurement against the full devnet stack.
//!
//! Captures the `fcu_duration_us` field emitted by the consensus engine's
//! [`SynchronizeTask`] every time it issues an `engine_forkchoiceUpdatedV3`
//! call to reth. Both the L2 builder and L2 client consensus nodes run
//! in-process, so a single global tracing subscriber sees every FCU from both
//! and we get an aggregated end-to-end view that includes the gossip → mpsc →
//! engine task queue → newPayload → FCU path that the in-process microbench
//! cannot exercise.
//!
//! On completion the test asserts a generous mean bound (regression guard)
//! and writes a JSON summary to `${FCU_LATENCY_OUTPUT}` (default
//! `target/fcu_latency.json`) for CI artifact upload and trend visibility.

use std::{
    env,
    fs::File,
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use alloy_provider::Provider;
use devnet::DevnetBuilder;
use eyre::{Result, WrapErr};
use serde::Serialize;
use tokio::time::{sleep, timeout};
use tracing::{Event, Subscriber, field::Visit};
use tracing_subscriber::{EnvFilter, Layer, layer::Context, prelude::*, registry::LookupSpan};

const L1_CHAIN_ID: u64 = 1337;
const L2_CHAIN_ID: u64 = 84538453;

/// Number of L2 blocks to observe before snapshotting samples.
///
/// With both consensus nodes running in-process we capture roughly two FCU
/// samples per produced block (sequencer + validator), so 25 blocks yields ~50
/// samples — enough for a stable mean and p50, marginal for p95.
const TARGET_BLOCKS: u64 = 25;

const BLOCK_PRODUCTION_TIMEOUT: Duration = Duration::from_secs(180);
const BLOCK_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Loose mean bound. Devnet runs both nodes locally with IPC engine transport,
/// so real FCU latency is sub-millisecond. 50 ms is generous enough to absorb
/// CI noise while still catching gross regressions (e.g. accidental synchronous
/// I/O on the FCU path).
const MAX_MEAN_US: u64 = 50_000;

/// Minimum FCU samples required for the assertions to be meaningful.
const MIN_SAMPLES: usize = 20;

#[tokio::test]
async fn fcu_latency_under_devnet_block_production() -> Result<()> {
    let samples = install_fcu_capture_subscriber();

    let devnet = DevnetBuilder::new()
        .with_l1_chain_id(L1_CHAIN_ID)
        .with_l2_chain_id(L2_CHAIN_ID)
        .build()
        .await?;

    let l2_builder = devnet.l2_builder_provider()?;

    timeout(BLOCK_PRODUCTION_TIMEOUT, async {
        loop {
            let block = l2_builder.get_block_number().await?;
            if block >= TARGET_BLOCKS {
                return Ok::<_, eyre::Error>(block);
            }
            sleep(BLOCK_POLL_INTERVAL).await;
        }
    })
    .await
    .wrap_err("L2 builder did not reach target block count in time")??;

    let collected = samples.lock().expect("samples lock not poisoned").clone();
    assert!(
        collected.len() >= MIN_SAMPLES,
        "expected at least {MIN_SAMPLES} FCU samples after {TARGET_BLOCKS} blocks, got {}",
        collected.len(),
    );

    let stats = LatencyStats::from_samples(&collected);
    eprintln!(
        "fcu_latency: samples={} mean={}µs p50={}µs p95={}µs max={}µs",
        stats.samples, stats.mean_us, stats.p50_us, stats.p95_us, stats.max_us
    );

    write_summary(&stats).wrap_err("failed to write fcu_latency summary")?;

    assert!(
        stats.mean_us <= MAX_MEAN_US,
        "FCU mean latency {}µs exceeds budget of {}µs (samples={}, p95={}µs, max={}µs)",
        stats.mean_us,
        MAX_MEAN_US,
        stats.samples,
        stats.p95_us,
        stats.max_us,
    );

    Ok(())
}

/// Installs a global tracing subscriber that records `fcu_duration_us` u64
/// fields from `target = "engine"` events into the returned shared buffer.
///
/// Must be called before any in-process node spawns its tasks so the
/// subscriber is the global default they inherit. Safe to call exactly once
/// per test process — repeated installs no-op.
fn install_fcu_capture_subscriber() -> Arc<Mutex<Vec<u64>>> {
    let samples = Arc::new(Mutex::new(Vec::<u64>::new()));
    let layer = FcuCaptureLayer { samples: Arc::clone(&samples) };

    let env_filter = EnvFilter::builder()
        .with_default_directive(tracing::level_filters::LevelFilter::WARN.into())
        .parse_lossy("warn,engine=debug,reth_tasks=off,reth_node_builder::launch::common=off");

    let _ = tracing_subscriber::registry().with(env_filter).with(layer).try_init();

    samples
}

struct FcuCaptureLayer {
    samples: Arc<Mutex<Vec<u64>>>,
}

impl<S> Layer<S> for FcuCaptureLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() != "engine" {
            return;
        }
        let mut visitor = FcuVisitor { duration_us: None };
        event.record(&mut visitor);
        if let Some(d) = visitor.duration_us
            && let Ok(mut guard) = self.samples.lock()
        {
            guard.push(d);
        }
    }
}

struct FcuVisitor {
    duration_us: Option<u64>,
}

impl Visit for FcuVisitor {
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        if field.name() == "fcu_duration_us" {
            self.duration_us = Some(value);
        }
    }

    fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
}

#[derive(Debug, Clone, Serialize)]
struct LatencyStats {
    samples: usize,
    mean_us: u64,
    p50_us: u64,
    p95_us: u64,
    max_us: u64,
}

impl LatencyStats {
    fn from_samples(samples: &[u64]) -> Self {
        let mut sorted = samples.to_vec();
        sorted.sort_unstable();
        let mean_us = sorted.iter().sum::<u64>() / sorted.len() as u64;
        Self {
            samples: sorted.len(),
            mean_us,
            p50_us: percentile(&sorted, 50),
            p95_us: percentile(&sorted, 95),
            max_us: *sorted.last().expect("non-empty after MIN_SAMPLES check"),
        }
    }
}

/// Nearest-rank percentile on a pre-sorted slice. `rank` is 1..=99.
fn percentile(sorted: &[u64], rank: u8) -> u64 {
    debug_assert!(!sorted.is_empty());
    let idx = ((rank as usize * sorted.len()) / 100).min(sorted.len() - 1);
    sorted[idx]
}

fn write_summary(stats: &LatencyStats) -> Result<()> {
    let path = env::var_os("FCU_LATENCY_OUTPUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/fcu_latency.json"));

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).wrap_err("failed to create summary dir")?;
    }

    let mut file = File::create(&path).wrap_err_with(|| format!("create {}", path.display()))?;
    file.write_all(&serde_json::to_vec_pretty(stats)?)?;
    file.write_all(b"\n")?;
    eprintln!("fcu_latency: wrote summary to {}", path.display());
    Ok(())
}
