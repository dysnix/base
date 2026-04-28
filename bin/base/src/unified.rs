//! Unified EL+CL orchestration for the `base` validator process.

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    num::NonZeroUsize,
    time::Duration,
};

use alloy_rpc_types_engine::JwtSecret;
use base_cli_utils::MetricsConfig;
use base_client_cli::P2PArgs;
use base_common_chains::Registry;
use base_consensus_node::{EngineConfig, L1ConfigBuilder, NodeMode, RollupNodeBuilder};
use base_consensus_rpc::RpcBuilder;
use eyre::Context;
use metrics_process::Collector;
use reth_tasks::Runtime;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::info;
use url::Url;

use crate::config::{BaseDatadir, ResolvedChainConfig};
use crate::execution::{ExecutionLaunchConfig, ExecutionLauncher};

pub(crate) const DEFAULT_CONSENSUS_RPC_PORT: u16 = 9545;

/// Configuration for the unified validator node.
#[derive(Debug, Clone)]
pub(crate) struct UnifiedNodeConfig {
    /// Resolved chain inputs.
    pub(crate) chain: ResolvedChainConfig,
    /// Root datadir.
    pub(crate) datadir: BaseDatadir,
    /// L1 execution RPC endpoint.
    pub(crate) l1_eth_rpc: Url,
    /// L1 beacon endpoint.
    pub(crate) l1_beacon: Url,
    /// Whether to trust L1 RPC responses without hash validation.
    pub(crate) l1_trust_rpc: bool,
    /// Optional sequencer RPC URL for the EL.
    pub(crate) sequencer_url: Option<String>,
    /// Optional flashblocks websocket URL for the EL.
    pub(crate) flashblocks_url: Option<Url>,
    /// Optional trusted peers for the EL P2P stack.
    pub(crate) execution_trusted_peers: Vec<String>,
    /// Optional HTTP RPC bind address for the embedded execution node.
    pub(crate) execution_http_addr: Option<IpAddr>,
    /// HTTP RPC port for the embedded execution node.
    pub(crate) execution_http_port: u16,
    /// Optional WebSocket RPC bind address for the embedded execution node.
    pub(crate) execution_ws_addr: Option<IpAddr>,
    /// WebSocket RPC port for the embedded execution node.
    pub(crate) execution_ws_port: u16,
    /// Optional P2P port for the embedded execution node.
    pub(crate) execution_p2p_port: Option<u16>,
    /// Whether to disable execution discovery.
    pub(crate) execution_discovery_disabled: bool,
    /// Optional bootnodes for CL P2P.
    pub(crate) bootnodes: Vec<String>,
    /// Optional advertised hostname or IP for CL P2P.
    pub(crate) advertise_host: Option<String>,
    /// Optional advertised TCP port for CL P2P.
    pub(crate) advertise_tcp_port: Option<u16>,
    /// Optional advertised UDP port for CL P2P.
    pub(crate) advertise_udp_port: Option<u16>,
    /// Optional consensus RPC bind address.
    pub(crate) consensus_rpc_addr: Option<IpAddr>,
    /// Consensus RPC port.
    pub(crate) consensus_rpc_port: u16,
    /// Optional consensus P2P listen IP.
    pub(crate) consensus_listen_ip: Option<IpAddr>,
    /// Optional consensus P2P TCP listen port.
    pub(crate) consensus_listen_tcp_port: Option<u16>,
    /// Optional consensus P2P UDP listen port.
    pub(crate) consensus_listen_udp_port: Option<u16>,
    /// Whether to disable consensus discovery.
    pub(crate) consensus_no_discovery: bool,
    /// L1 confirmation depth for validator derivation.
    pub(crate) verifier_l1_confs: u64,
    /// Process-wide metrics configuration.
    pub(crate) metrics: MetricsConfig,
}

/// Unified EL + CL validator node runner.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct UnifiedNode;

#[derive(Debug)]
struct ConsensusTaskHandle {
    cancellation: CancellationToken,
    join_handle: JoinHandle<eyre::Result<()>>,
}

impl UnifiedNode {
    /// Runs the unified validator node until shutdown.
    pub(crate) async fn run(config: UnifiedNodeConfig) -> eyre::Result<()> {
        let runtime = Runtime::with_existing_handle(tokio::runtime::Handle::current())
            .wrap_err("failed to create reth runtime")?;
        let cancel = CancellationToken::new();
        let _signal_task = base_cli_utils::RuntimeManager::install_signal_handler(cancel.clone());

        let execution = ExecutionLauncher::launch(ExecutionLaunchConfig {
            chain: config.chain.clone(),
            datadir: config.datadir.clone(),
            runtime,
            metrics: config.metrics.clone(),
            sequencer_url: config.sequencer_url.clone(),
            flashblocks_url: config.flashblocks_url.clone(),
            trusted_peers: config.execution_trusted_peers.clone(),
            http_addr: config.execution_http_addr,
            http_port: config.execution_http_port,
            ws_addr: config.execution_ws_addr,
            ws_port: config.execution_ws_port,
            p2p_port: config.execution_p2p_port,
            discovery_disabled: config.execution_discovery_disabled,
        })
        .await?;
        let _metrics_collector = Self::spawn_metrics_collector(&config.metrics, cancel.clone());

        let engine_url = execution.engine_url.clone();
        let http_addr = execution.http_addr;
        let ws_addr = execution.ws_addr;
        let engine_shutdown = execution.engine_shutdown.clone();

        let mut consensus_handle = Self::launch_consensus_task(config, engine_url).await?;
        let mut execution_handle =
            tokio::spawn(async move { execution.node_handle.wait_for_node_exit().await });

        info!(http_addr = %http_addr, ws_addr = %ws_addr, "Started unified validator node");

        let exit = tokio::select! {
            _ = cancel.cancelled() => {
                ExitReason::Signal
            }
            execution_result = &mut execution_handle => {
                ExitReason::Execution(execution_result)
            }
            consensus_result = &mut consensus_handle.join_handle => {
                ExitReason::Consensus(consensus_result)
            }
        };
        cancel.cancel();

        match exit {
            ExitReason::Signal => {
                info!("Received shutdown request");
                Self::shutdown(engine_shutdown, execution_handle, Some(consensus_handle)).await?;
                Ok(())
            }
            ExitReason::Execution(execution_result) => {
                Self::shutdown_consensus(consensus_handle).await?;
                match execution_result {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(error)) => Err(error).wrap_err("execution node exited with an error"),
                    Err(error) => Err(eyre::eyre!("execution node join error: {error}")),
                }
            }
            ExitReason::Consensus(consensus_result) => {
                Self::shutdown(engine_shutdown, execution_handle, None).await?;
                match consensus_result {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(error)) => Err(error).wrap_err("consensus node exited with an error"),
                    Err(error) if error.is_cancelled() => Ok(()),
                    Err(error) => Err(eyre::eyre!("consensus node join error: {error}")),
                }
            }
        }
    }

    async fn launch_consensus_task(
        config: UnifiedNodeConfig,
        engine_url: Url,
    ) -> eyre::Result<ConsensusTaskHandle> {
        let rollup_config = config.chain.load_rollup_config()?;
        let l1_chain_config = config.chain.load_l1_config()?;

        let mut p2p_args = P2PArgs {
            priv_path: Some(config.datadir.p2p_key_path()),
            bootnodes: config.bootnodes.clone(),
            no_discovery: config.consensus_no_discovery,
            ..P2PArgs::defaults_without_env()
        };
        if let Some(host) = config.advertise_host.as_deref() {
            p2p_args.advertise_ip = Some(resolve_host(host).await?);
        }
        if let Some(port) = config.advertise_tcp_port {
            p2p_args.advertise_tcp_port = Some(port);
        }
        if let Some(port) = config.advertise_udp_port {
            p2p_args.advertise_udp_port = Some(port);
        }
        if let Some(ip) = config.consensus_listen_ip {
            p2p_args.listen_ip = ip;
        }
        if let Some(port) = config.consensus_listen_tcp_port {
            p2p_args.listen_tcp_port = port;
        }
        if let Some(port) = config.consensus_listen_udp_port {
            p2p_args.listen_udp_port = port;
        }
        p2p_args.check_ports()?;

        let genesis_signer = Registry::unsafe_block_signer(config.chain.l2_chain_id);
        let p2p_config = p2p_args
            .clone()
            .config(
                &rollup_config,
                config.chain.l2_chain_id,
                Some(config.l1_eth_rpc.clone()),
                genesis_signer,
            )
            .await
            .map_err(|error| eyre::eyre!("{error}"))
            .wrap_err("failed to build consensus P2P config")?;

        let engine_config = EngineConfig {
            config: std::sync::Arc::new(rollup_config.clone()),
            l2_url: engine_url,
            l2_jwt_secret: JwtSecret::from_file(&config.datadir.jwt_secret_path())
                .wrap_err("failed to load execution JWT secret")?,
            l1_url: config.l1_eth_rpc.clone(),
            mode: NodeMode::Validator,
        };
        let l1_config = L1ConfigBuilder {
            chain_config: l1_chain_config,
            trust_rpc: config.l1_trust_rpc,
            beacon: config.l1_beacon.clone(),
            rpc_url: config.l1_eth_rpc.clone(),
            slot_duration_override: config.chain.l1_slot_duration_override(),
            verifier_l1_confs: config.verifier_l1_confs,
        };
        let rpc_config = Some(RpcBuilder {
            no_restart: true,
            socket: SocketAddr::new(
                config.consensus_rpc_addr.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
                config.consensus_rpc_port,
            ),
            enable_admin: false,
            admin_persistence: None,
            ws_enabled: false,
            dev_enabled: false,
            http_timeout: std::time::Duration::from_secs(60),
            max_concurrent_requests: NonZeroUsize::new(1024).expect("nonzero"),
        });

        let rollup_node = RollupNodeBuilder::new(
            rollup_config,
            l1_config,
            true,
            engine_config,
            p2p_config,
            rpc_config,
        )
        .with_safedb_path(config.datadir.safedb_path())
        .build()
        .await
        .map_err(|error| eyre::eyre!("{error}"))
        .wrap_err("failed to build consensus node")?;

        let cancellation = CancellationToken::new();
        let task_cancellation = cancellation.clone();
        let join_handle = tokio::spawn(async move {
            rollup_node
                .start_with_cancellation(task_cancellation)
                .await
                .map_err(|error| eyre::eyre!("{error}"))
        });

        Ok(ConsensusTaskHandle { cancellation, join_handle })
    }

    fn spawn_metrics_collector(
        metrics: &MetricsConfig,
        cancel: CancellationToken,
    ) -> Option<JoinHandle<()>> {
        if !metrics.enabled {
            return None;
        }

        let interval = Duration::from_secs(metrics.interval.max(1));
        Some(tokio::spawn(async move {
            let collector = Collector::default();
            collector.describe();

            loop {
                collector.collect();

                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = tokio::time::sleep(interval) => {}
                }
            }
        }))
    }

    async fn shutdown(
        engine_shutdown: reth_node_builder::rpc::EngineShutdown,
        execution_handle: JoinHandle<eyre::Result<()>>,
        consensus_handle: Option<ConsensusTaskHandle>,
    ) -> eyre::Result<()> {
        if let Some(consensus_handle) = consensus_handle {
            Self::shutdown_consensus(consensus_handle).await?;
        }

        if let Some(done) = engine_shutdown.shutdown() {
            tokio::time::timeout(Duration::from_secs(10), done)
                .await
                .context("timed out waiting for execution shutdown")?
                .context("execution shutdown completion channel closed")?;
        }

        match tokio::time::timeout(Duration::from_secs(10), execution_handle)
            .await
            .context("timed out waiting for execution node exit")?
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                return Err(error).wrap_err("execution node exited with an error during shutdown");
            }
            Err(error) => {
                return Err(eyre::eyre!("execution node join error during shutdown: {error}"));
            }
        }

        Ok(())
    }

    async fn shutdown_consensus(consensus_handle: ConsensusTaskHandle) -> eyre::Result<()> {
        consensus_handle.cancellation.cancel();

        match tokio::time::timeout(Duration::from_secs(10), consensus_handle.join_handle)
            .await
            .context("timed out waiting for consensus node exit")?
        {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(error).wrap_err("consensus node exited with an error"),
            Err(error) if error.is_cancelled() => Ok(()),
            Err(error) => Err(eyre::eyre!("consensus node join error: {error}")),
        }
    }
}

#[derive(Debug)]
enum ExitReason {
    Signal,
    Execution(Result<eyre::Result<()>, tokio::task::JoinError>),
    Consensus(Result<eyre::Result<()>, tokio::task::JoinError>),
}

async fn resolve_host(host: &str) -> eyre::Result<IpAddr> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(ip);
    }

    tokio::net::lookup_host(format!("{host}:0"))
        .await
        .wrap_err_with(|| format!("failed to resolve host `{host}`"))?
        .next()
        .map(|addr| addr.ip())
        .ok_or_else(|| eyre::eyre!("no addresses resolved for host `{host}`"))
}

#[cfg(test)]
mod tests {
    use base_client_cli::P2PArgs;
    use figment::Jail;

    #[test]
    fn p2p_defaults_ignore_standalone_consensus_env() {
        Jail::expect_with(|jail| {
            jail.clear_env();
            jail.set_env("BASE_NODE_P2P_LISTEN_TCP_PORT", "19001");
            jail.set_env("BASE_NODE_P2P_LISTEN_UDP_PORT", "19002");

            let p2p_args = P2PArgs::defaults_without_env();

            assert_eq!(p2p_args.listen_tcp_port, 9222);
            assert_eq!(p2p_args.listen_udp_port, 9223);

            Ok(())
        });
    }
}
