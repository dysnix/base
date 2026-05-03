//! CLI definition for the Intel TDX TEE prover binary.

use std::{fmt, net::SocketAddr, sync::Arc, time::Duration};

use base_cli_utils::{LogConfig, RuntimeManager};
use base_common_chains::Registry;
use base_proof_host::ProverConfig;
use base_proof_tee_tdx_prover::{MeasuredMockTdxQuoteProvider, TdxProverServer};
use base_proof_tee_tdx_runtime::{
    ConfigfsTdxQuoteProvider, TdxQuoteProvider, TdxRuntime, TdxSigner,
};
use clap::{Parser, Subcommand};
use eyre::eyre;
use tracing::info;

base_cli_utils::define_log_args!("BASE_PROVER_TDX");
base_cli_utils::define_metrics_args!("BASE_PROVER_TDX", 7310);

/// Intel TDX TEE prover binary.
#[derive(Parser)]
#[command(author, version)]
pub(crate) struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Logging arguments.
    #[command(flatten)]
    logging: LogArgs,

    /// Metrics arguments.
    #[command(flatten)]
    metrics: MetricsArgs,
}

/// TDX prover subcommands.
#[derive(Subcommand)]
enum Command {
    /// Run the JSON-RPC server using Linux TSM/configfs quote collection.
    Server(ServerArgs),

    /// Run the JSON-RPC server with deterministic local TDX quote fixtures.
    Local(LocalArgs),
}

/// Shared arguments for TDX prover server modes.
#[derive(Parser)]
struct ProverServerArgs {
    /// L1 execution layer RPC URL.
    #[arg(long, env = "L1_ETH_URL")]
    l1_eth_url: String,

    /// L2 execution layer RPC URL.
    #[arg(long, env = "L2_ETH_URL")]
    l2_eth_url: String,

    /// L1 beacon API URL.
    #[arg(long, env = "L1_BEACON_URL")]
    l1_beacon_url: String,

    /// L2 chain ID.
    #[arg(long, env = "L2_CHAIN_ID")]
    l2_chain_id: u64,

    /// Socket address to listen on for JSON-RPC.
    #[arg(long, env = "LISTEN_ADDR")]
    listen_addr: SocketAddr,

    /// Enable experimental `debug_executePayload` witness endpoint.
    #[arg(long, env = "ENABLE_EXPERIMENTAL_WITNESS_ENDPOINT")]
    enable_experimental_witness_endpoint: bool,

    /// Maximum seconds for a single proof request before it is aborted.
    #[arg(long, env = "PROOF_REQUEST_TIMEOUT_SECS", default_value = "1740", value_parser = clap::value_parser!(u64).range(1..))]
    proof_request_timeout_secs: u64,

    /// Optional secp256k1 signer private key. Generates an ephemeral key when omitted.
    #[arg(long, env = "BASE_TDX_SIGNER_KEY")]
    signer_key: Option<String>,
}

impl ProverServerArgs {
    fn into_prover_config(self) -> eyre::Result<ProverConfig> {
        let rollup_config = Registry::rollup_config(self.l2_chain_id)
            .ok_or_else(|| eyre!("unknown L2 chain ID: {}", self.l2_chain_id))?
            .clone();

        let l1_config = base_common_chains::L1_CONFIGS
            .get(&rollup_config.l1_chain_id)
            .ok_or_else(|| eyre!("unknown L1 chain ID: {}", rollup_config.l1_chain_id))?
            .clone();

        Ok(ProverConfig {
            l1_eth_url: self.l1_eth_url,
            l2_eth_url: self.l2_eth_url,
            l1_beacon_url: self.l1_beacon_url,
            l2_chain_id: self.l2_chain_id,
            rollup_config,
            l1_config,
            enable_experimental_witness_endpoint: self.enable_experimental_witness_endpoint,
        })
    }

    fn signer(&self) -> eyre::Result<TdxSigner> {
        self.signer_key.as_deref().map_or_else(
            || Ok(TdxSigner::generate(&mut rand_08::rngs::OsRng)),
            |key| {
                TdxSigner::from_hex(key)
                    .map_err(|error| eyre!("failed to load TDX signer key: {error}"))
            },
        )
    }

    async fn run<P>(self, provider: P) -> eyre::Result<()>
    where
        P: TdxQuoteProvider + fmt::Debug + 'static,
    {
        let signer = self.signer()?;
        let listen_addr = self.listen_addr;
        let timeout = Duration::from_secs(self.proof_request_timeout_secs);
        let config = self.into_prover_config()?;
        let runtime = Arc::new(TdxRuntime::new(signer, provider));
        let server = TdxProverServer::new(config, runtime, timeout);

        let handle = server.run(listen_addr).await?;
        handle.stopped().await;
        Ok(())
    }
}

/// Arguments for the TDX configfs server mode.
#[derive(Parser)]
struct ServerArgs {
    #[command(flatten)]
    server: ProverServerArgs,

    /// Configfs report name below `/sys/kernel/config/tsm/report`.
    #[arg(long, env = "TDX_REPORT_NAME", default_value = "base-tdx-prover")]
    report_name: String,
}

/// Arguments for local deterministic mock mode.
#[derive(Parser)]
struct LocalArgs {
    #[command(flatten)]
    server: ProverServerArgs,
}

impl Cli {
    /// Run the selected subcommand.
    pub(crate) fn run(self) -> eyre::Result<()> {
        let Self { command, logging, metrics } = self;
        LogConfig::from(logging).init_tracing_subscriber()?;
        base_cli_utils::MetricsConfig::from(metrics).init_with(|| {
            base_cli_utils::register_version_metrics!();
        })?;
        RuntimeManager::new().with_thread_stack_size(8 * 1024 * 1024).run_until_ctrl_c(async move {
            match command {
                Command::Server(args) => args.run().await,
                Command::Local(args) => args.run().await,
            }
        })
    }
}

impl ServerArgs {
    async fn run(self) -> eyre::Result<()> {
        let provider = ConfigfsTdxQuoteProvider::new(&self.report_name);
        info!(addr = %self.server.listen_addr, report_name = %self.report_name, "starting tdx prover server");
        self.server.run(provider).await
    }
}

impl LocalArgs {
    async fn run(self) -> eyre::Result<()> {
        let provider = MeasuredMockTdxQuoteProvider::local_mock();
        info!(addr = %self.server.listen_addr, "starting tdx prover server (local mock mode)");
        self.server.run(provider).await
    }
}
