//! Default load-test command execution.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use alloy_primitives::{U256, utils::format_ether};
use alloy_signer_local::PrivateKeySigner;
use eyre::{Result, bail};
use indicatif::MultiProgress;

use crate::{
    LoadRunner, LoadTestDisplay, MetricsSummary, Result as LoadResult, RpcClient, TestConfig,
};

/// Options for the default load-test command.
#[derive(Debug)]
pub struct LoadTestOptions {
    /// Path to the YAML config file.
    pub config_path: Option<PathBuf>,
    /// Run indefinitely until interrupted.
    pub continuous: bool,
    /// Drain accounts without running a load test.
    pub drain_only: bool,
}

/// Runs the default load-test command.
#[derive(Debug)]
pub struct LoadTest;

impl LoadTest {
    /// Runs the default load-test command.
    pub async fn run(options: LoadTestOptions) -> Result<()> {
        let mp = LoadTestDisplay::init_tracing();

        let config_path = options
            .config_path
            .or_else(|| {
                option_env!("CARGO_MANIFEST_DIR")
                    .map(|dir| PathBuf::from(dir).join("examples/devnet.yaml"))
            })
            .ok_or_else(|| {
                eyre::eyre!("usage: base-load-tests [--continuous] [--drain-only] <config.yaml>")
            })?;

        if !config_path.exists() {
            bail!("config file not found: {}", config_path.display());
        }

        let test_config = TestConfig::load(&config_path)?;

        let client = RpcClient::new(test_config.rpc.clone());
        let rpc_chain_id =
            if test_config.chain_id.is_none() { Some(client.chain_id().await?) } else { None };

        let load_config = {
            let cfg = test_config.to_load_config(rpc_chain_id)?;
            if options.continuous { cfg.with_continuous() } else { cfg }
        };

        let funding_key = TestConfig::funder_key()?;

        if options.drain_only {
            println!("=== Drain-Only Mode ===");
            println!(
                "Re-deriving {} accounts from config and draining to funder...",
                load_config.account_count
            );
            let runner = LoadRunner::new(load_config)?;
            match runner.drain_accounts(funding_key).await {
                Ok(drained) => println!("Drained {} ETH back to funder.", format_ether(drained)),
                Err(e) => bail!("drain failed: {e}"),
            }
            return Ok(());
        }

        println!("=== Base Load Test Runner ===");

        println!("Set RPCs to internal endpoints to avoid rate limiting");
        println!(
            "Config: {} | RPC: {} | Chain: {}",
            config_path.display(),
            test_config.rpc,
            load_config.chain_id
        );
        let duration_display =
            load_config.duration.map_or_else(|| "continuous".to_string(), |d| format!("{d:?}"));
        println!(
            "Target: {} GPS | Duration: {} | Accounts: {}",
            load_config.target_gps, duration_display, load_config.account_count
        );
        println!();

        let funding_amount = test_config.parse_funding_amount()?;
        let swap_token_amount = test_config.parse_swap_token_amount()?;

        let config_summary = test_config.to_summary();
        let mut runner = LoadRunner::new(load_config.clone())?;
        runner.set_config_summary(config_summary.clone());

        // Install signal handling before long-running work so the runner drains
        // funds on the first shutdown signal and force-exits on the second.
        let stop_flag = runner.stop_flag();
        Self::install_signal_handler(stop_flag);

        let run_result = Self::run_test_phases(
            &mut runner,
            &funding_key,
            funding_amount,
            swap_token_amount,
            &mp,
            load_config.duration,
        )
        .await;

        let (summary, run_err) = match run_result {
            Ok(summary) => (summary, None),
            Err(e) => {
                let summary = MetricsSummary {
                    config: Some(config_summary),
                    error: Some(e.to_string()),
                    ..Default::default()
                };
                (summary, Some(e))
            }
        };

        Self::print_summary(&summary);
        Self::write_summary_output(&summary);

        // Brief cooldown so in-flight load-test transactions can land and
        // mempool state settles before balances are queried for the drain.
        tokio::time::sleep(Duration::from_secs(2)).await;

        println!();
        println!("Draining accounts back to funder...");
        match runner.drain_accounts(funding_key).await {
            Ok(drained) => println!("Drained {} ETH back to funder.", format_ether(drained)),
            Err(e) => eprintln!("Warning: drain failed: {e}"),
        }

        if let Some(e) = run_err {
            return Err(e.into());
        }

        Ok(())
    }

    async fn run_test_phases(
        runner: &mut LoadRunner,
        funding_key: &PrivateKeySigner,
        funding_amount: U256,
        swap_token_amount: U256,
        mp: &MultiProgress,
        duration: Option<Duration>,
    ) -> LoadResult<MetricsSummary> {
        println!("Funding test accounts...");
        runner.fund_accounts(funding_key.clone(), funding_amount).await?;
        println!("Accounts funded.");

        if !runner.collect_swap_tokens().is_empty() {
            println!("Distributing swap tokens...");
            runner.setup_swap_tokens(funding_key.clone(), swap_token_amount).await?;
            println!("Swap tokens distributed.");
        }
        println!();

        println!("Running load test...");

        let display = LoadTestDisplay::new(mp, duration);
        runner.set_display(display);

        runner.run().await
    }

    fn print_summary(summary: &MetricsSummary) {
        if summary.error.is_none() || summary.throughput.total_submitted > 0 {
            println!();
            println!("=== Results ===");
            if let Some(ref err) = summary.error {
                println!("Error: {err}");
            }
            println!(
                "Submitted: {} | Confirmed: {} | Failed: {}",
                summary.throughput.total_submitted,
                summary.throughput.total_confirmed,
                summary.throughput.total_failed
            );
            println!(
                "TPS: {:.2} | GPS: {:.0} | Success: {:.1}%",
                summary.throughput.tps,
                summary.throughput.gps,
                summary.throughput.success_rate()
            );
            let tp = &summary.throughput_percentiles;
            println!(
                "TPS Rolling:   p50={:.0}  p90={:.0}  p99={:.0}  max={:.0}",
                tp.tps_p50, tp.tps_p90, tp.tps_p99, tp.tps_max
            );
            println!(
                "GPS Rolling:   p50={:.0}  p90={:.0}  p99={:.0}  max={:.0}",
                tp.gps_p50, tp.gps_p90, tp.gps_p99, tp.gps_max
            );
            let bl = &summary.block_latency;
            println!(
                "Block Latency: min={:.1?}  p50={:.1?}  mean={:.1?}  p99={:.1?}  max={:.1?}",
                bl.min, bl.p50, bl.mean, bl.p99, bl.max
            );
            let fb = &summary.flashblocks_latency;
            println!(
                "FB Latency:    min={:.1?}  p50={:.1?}  mean={:.1?}  p99={:.1?}  max={:.1?}  (n={})",
                fb.min, fb.p50, fb.mean, fb.p99, fb.max, fb.count
            );
            println!("Gas: total={}  avg/tx={}", summary.gas.total_gas, summary.gas.avg_gas);
            let br = &summary.block_range;
            match (br.first_block, br.last_block) {
                (Some(first), Some(last)) => {
                    println!("Blocks: first={first}  last={last}  span={} block(s)", br.block_count)
                }
                _ => println!("Blocks: no confirmed transactions"),
            }
            if !summary.top_failure_reasons.is_empty() {
                println!("Top failures:");
                for (reason, count) in &summary.top_failure_reasons {
                    println!("  {count:>6}x  {reason}");
                }
            }
        } else if let Some(ref err) = summary.error {
            println!();
            println!("=== Error ===");
            println!("{err}");
        }
    }

    fn write_summary_output(summary: &MetricsSummary) {
        if let Ok(output_path) = std::env::var("LOAD_TEST_OUTPUT") {
            match summary.to_json() {
                Ok(json) => match std::fs::write(&output_path, &json) {
                    Ok(()) => println!("Results written to {output_path}"),
                    Err(e) => eprintln!("Warning: failed to write results to {output_path}: {e}"),
                },
                Err(e) => eprintln!("Warning: failed to serialize results: {e}"),
            }
        }
    }

    fn install_signal_handler(stop_flag: Arc<AtomicBool>) {
        tokio::spawn(async move {
            Self::wait_for_shutdown_signal().await;
            eprintln!("\nReceived signal, stopping gracefully. Send again to force exit.");
            stop_flag.store(true, Ordering::SeqCst);

            Self::wait_for_shutdown_signal().await;
            eprintln!("\nForcing exit. Funds may remain in test accounts.");
            std::process::exit(1);
        });
    }

    async fn wait_for_shutdown_signal() {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};

            let mut sigterm =
                signal(SignalKind::terminate()).expect("failed to register SIGTERM handler");
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = sigterm.recv() => {}
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
    }
}
