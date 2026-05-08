//! CLI argument parsing for the load-test binary.

use std::path::PathBuf;

use alloy_signer_local::PrivateKeySigner;
use base_load_tests::{LoadTest, LoadTestOptions, Rescue, RescueOptions};
use clap::{ArgGroup, Args, Parser, Subcommand};
use url::Url;

/// Load-test binary CLI.
#[derive(Debug, Parser)]
#[command(
    author,
    version = env!("CARGO_PKG_VERSION"),
    about = "Base network load test runner",
    long_about = None
)]
pub(crate) struct Cli {
    /// Default load-test arguments.
    #[command(flatten)]
    pub(crate) load: LoadArgs,

    /// Optional subcommand.
    #[command(subcommand)]
    pub(crate) command: Option<Commands>,
}

/// CLI arguments for the default load-test command.
#[derive(Clone, Debug, Args)]
pub(crate) struct LoadArgs {
    /// YAML config file to run.
    #[arg(value_name = "CONFIG")]
    pub(crate) config: Option<PathBuf>,

    /// Run indefinitely until interrupted.
    #[arg(long)]
    pub(crate) continuous: bool,

    /// Drain accounts from the config without running a load test.
    #[arg(long)]
    pub(crate) drain_only: bool,
}

/// Load-test subcommands.
#[derive(Clone, Debug, Subcommand)]
pub(crate) enum Commands {
    /// Rescue stranded funds by deriving accounts from a seed or mnemonic.
    Rescue(RescueArgs),
}

/// CLI arguments for the rescue subcommand.
#[derive(Clone, Debug, Args)]
#[command(group(ArgGroup::new("derivation").required(true).args(["seed", "mnemonic"])))]
pub(crate) struct RescueArgs {
    /// RPC endpoint.
    #[arg(long = "rpc-url", alias = "rpc")]
    pub(crate) rpc_url: Url,

    /// Seed used for account generation.
    #[arg(long)]
    pub(crate) seed: Option<u64>,

    /// Mnemonic used for account generation.
    #[arg(long)]
    pub(crate) mnemonic: Option<String>,

    /// Number of accounts to scan.
    #[arg(long = "count", default_value_t = RescueOptions::DEFAULT_SCAN_COUNT)]
    pub(crate) scan_count: usize,

    /// Starting account offset.
    #[arg(long, default_value_t = 0)]
    pub(crate) offset: usize,

    /// Private key of the funder account.
    #[arg(long = "funder-key", env = "FUNDER_KEY")]
    pub(crate) funder_key: PrivateKeySigner,
}

impl Cli {
    /// Runs the load-test CLI.
    pub(crate) async fn run(self) -> eyre::Result<()> {
        match self.command {
            Some(Commands::Rescue(args)) => Rescue::run(args.into()).await,
            None => LoadTest::run(self.load.into()).await,
        }
    }
}

impl From<LoadArgs> for LoadTestOptions {
    fn from(args: LoadArgs) -> Self {
        Self { config_path: args.config, continuous: args.continuous, drain_only: args.drain_only }
    }
}

impl From<RescueArgs> for RescueOptions {
    fn from(args: RescueArgs) -> Self {
        Self {
            rpc_url: args.rpc_url,
            seed: args.seed,
            scan_count: args.scan_count,
            offset: args.offset,
            funder_key: args.funder_key,
            mnemonic: args.mnemonic,
        }
    }
}
