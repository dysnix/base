use base_cli_utils::{LogConfig, MetricsConfig, RuntimeManager};
use eyre::WrapErr;

use crate::cli::BaseCli;
use crate::config::ChainResolver;

/// Runs the `base` binary.
#[derive(Debug, Clone)]
pub(crate) struct BaseApp {
    /// Parsed CLI input.
    cli: BaseCli,
}

impl BaseApp {
    /// Creates a new app from parsed CLI input.
    pub(crate) const fn new(cli: BaseCli) -> Self {
        Self { cli }
    }

    /// Runs the requested command.
    pub(crate) fn run(self) -> eyre::Result<()> {
        let BaseCli { chain, logging, metrics, command } = self.cli;

        LogConfig::from(logging)
            .init_tracing_subscriber()
            .wrap_err("failed to initialize tracing")?;

        let resolved_chain = ChainResolver::new(chain).resolve()?;
        let metrics_config = MetricsConfig::from(metrics);
        let runtime =
            RuntimeManager::new().tokio_runtime().wrap_err("failed to create Tokio runtime")?;

        runtime.block_on(command.run(resolved_chain, metrics_config))
    }
}
