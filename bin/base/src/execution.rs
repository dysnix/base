//! Execution-layer launcher and runtime wiring for the unified `base` binary.

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use base_bundle_extension::BundleExtension;
use base_cli_utils::MetricsConfig;
use base_execution_chainspec::BaseChainSpec;
use base_execution_cli::chainspec::chain_value_parser;
use base_flashblocks::FlashblocksConfig;
use base_flashblocks_node::FlashblocksExtension;
use base_node_core::args::RollupArgs;
use base_node_runner::{BaseNode, BaseNodeRunner};
use base_txpool_rpc::{TxPoolRpcConfig, TxPoolRpcExtension};
use base_txpool_tracing::{TxPoolExtension, TxpoolConfig};
use eyre::Context;
use reth_db::{
    ClientVersion, DatabaseEnv, init_db,
    mdbx::{DatabaseArguments, MaxReadTransactionDuration},
};
use reth_node_builder::rpc::EngineShutdown;
use reth_node_builder::{NodeBuilder, NodeHandleFor};
use reth_node_core::{
    args::{DatadirArgs, DiscoveryArgs, MetricArgs, NetworkArgs, RpcServerArgs},
    dirs::{DataDirPath, MaybePlatformPath},
    node_config::NodeConfig,
};
use reth_tasks::Runtime;
use url::Url;

use crate::config::{BaseDatadir, ResolvedChainConfig};

pub(crate) const DEFAULT_HTTP_PORT: u16 = 8545;
pub(crate) const DEFAULT_WS_PORT: u16 = 8546;

/// Configuration for launching the execution node.
#[derive(Debug, Clone)]
pub(crate) struct ExecutionLaunchConfig {
    /// Resolved chain inputs.
    pub(crate) chain: ResolvedChainConfig,
    /// Datadir layout.
    pub(crate) datadir: BaseDatadir,
    /// Shared Tokio-aware reth runtime.
    pub(crate) runtime: Runtime,
    /// Process-wide metrics configuration.
    pub(crate) metrics: MetricsConfig,
    /// Optional sequencer HTTP endpoint.
    pub(crate) sequencer_url: Option<String>,
    /// Optional flashblocks websocket endpoint.
    pub(crate) flashblocks_url: Option<Url>,
    /// Optional trusted peers for execution P2P.
    pub(crate) trusted_peers: Vec<String>,
    /// Optional HTTP RPC bind address.
    pub(crate) http_addr: Option<IpAddr>,
    /// HTTP RPC port for the embedded execution node.
    pub(crate) http_port: u16,
    /// Optional WebSocket RPC bind address.
    pub(crate) ws_addr: Option<IpAddr>,
    /// WebSocket RPC port for the embedded execution node.
    pub(crate) ws_port: u16,
    /// Optional execution P2P port.
    pub(crate) p2p_port: Option<u16>,
    /// Whether to disable execution discovery.
    pub(crate) discovery_disabled: bool,
}

/// A running EL instance.
#[derive(Debug)]
pub(crate) struct ExecutionInstance {
    /// Authenticated Engine API URL used by the CL.
    pub(crate) engine_url: Url,
    /// HTTP RPC address.
    pub(crate) http_addr: SocketAddr,
    /// WebSocket RPC address.
    pub(crate) ws_addr: SocketAddr,
    /// Graceful engine shutdown handle.
    pub(crate) engine_shutdown: EngineShutdown,
    /// Running node handle.
    pub(crate) node_handle: NodeHandleFor<BaseNode>,
}

/// Launches the execution node for the unified binary.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct ExecutionLauncher;

impl ExecutionLauncher {
    /// Launches the execution node and returns its handles.
    pub(crate) async fn launch(config: ExecutionLaunchConfig) -> eyre::Result<ExecutionInstance> {
        config.datadir.ensure()?;
        let datadir = config.datadir.canonicalized()?;
        datadir.ensure()?;
        datadir.ensure_jwt_secret()?;

        let chain_input = config.chain.prepare_execution_chain_input(&datadir)?;
        let chain_spec = Arc::clone(
            &chain_value_parser(&chain_input).wrap_err("failed to resolve execution chain spec")?,
        );
        let database = Self::open_database(datadir.db_path())?;
        let node_config = Self::node_config(chain_spec, &datadir, &config)?;
        let builder = NodeBuilder::new(node_config)
            .with_database(database)
            .with_launch_context(config.runtime);

        let rollup_args =
            RollupArgs { sequencer: config.sequencer_url.clone(), ..Default::default() };
        let flashblocks_config =
            config.flashblocks_url.clone().map(|url| FlashblocksConfig::new(url, 3));
        let mut runner = BaseNodeRunner::new(rollup_args.clone());
        runner.install_ext::<TxPoolRpcExtension>(TxPoolRpcConfig {
            sequencer_rpc: rollup_args.sequencer.clone(),
        });
        runner.install_ext::<BundleExtension>(());
        runner.install_ext::<TxPoolExtension>(TxpoolConfig {
            tracing_enabled: false,
            tracing_logs_enabled: false,
            flashblocks_config: flashblocks_config.clone(),
        });
        runner.install_ext::<FlashblocksExtension>(flashblocks_config);
        runner.add_started_callback(|| {
            base_cli_utils::register_version_metrics!();
            Ok(())
        });

        let node_handle =
            runner.launch(builder).await.wrap_err("failed to launch execution node")?;
        let http_addr =
            node_handle.node.rpc_server_handle().http_local_addr().ok_or_else(|| {
                eyre::eyre!("execution HTTP RPC failed to bind to a local address")
            })?;
        let ws_addr = node_handle.node.rpc_server_handle().ws_local_addr().ok_or_else(|| {
            eyre::eyre!("execution WebSocket RPC failed to bind to a local address")
        })?;
        let engine_url = Url::parse(&node_handle.node.auth_server_handle().http_url())
            .wrap_err("failed to parse execution Engine API URL")?;
        let engine_shutdown = node_handle.node.add_ons_handle.engine_shutdown.clone();

        Ok(ExecutionInstance { engine_url, http_addr, ws_addr, engine_shutdown, node_handle })
    }

    fn node_config(
        chain_spec: Arc<BaseChainSpec>,
        datadir: &BaseDatadir,
        launch_config: &ExecutionLaunchConfig,
    ) -> eyre::Result<NodeConfig<BaseChainSpec>> {
        let mut rpc = RpcServerArgs::default().with_http().with_ws();
        rpc.http_addr = launch_config.http_addr.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        rpc.http_port = launch_config.http_port;
        rpc.ws_addr = launch_config.ws_addr.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        rpc.ws_port = launch_config.ws_port;
        rpc.auth_jwtsecret = Some(datadir.jwt_secret_path());
        rpc.ipcdisable = true;
        rpc.http_api = Some(
            "admin,eth,web3,net,rpc,debug,txpool,miner"
                .parse()
                .wrap_err("failed to parse execution HTTP RPC modules")?,
        );
        rpc.ws_api = Some(
            "eth,web3,net,txpool,debug"
                .parse()
                .wrap_err("failed to parse execution WebSocket RPC modules")?,
        );
        rpc.http_corsdomain = Some("*".to_owned());
        rpc.ws_allowed_origins = Some("*".to_owned());

        let trusted_peers = launch_config
            .trusted_peers
            .iter()
            .map(|peer| peer.parse())
            .collect::<Result<_, _>>()
            .wrap_err("failed to parse execution trusted peers")?;
        let default_network = NetworkArgs::default();
        let default_discovery = DiscoveryArgs::default();
        let network = NetworkArgs {
            trusted_peers,
            port: launch_config.p2p_port.unwrap_or(default_network.port),
            discovery: DiscoveryArgs {
                disable_discovery: launch_config.discovery_disabled,
                port: launch_config.p2p_port.unwrap_or(default_discovery.port),
                ..default_discovery
            },
            ..default_network
        };

        let datadir_path = MaybePlatformPath::<DataDirPath>::from(datadir.path.clone());
        let mut config = NodeConfig::new(chain_spec)
            .with_datadir_args(DatadirArgs { datadir: datadir_path, ..Default::default() })
            .with_network(network)
            .with_rpc(rpc);

        if launch_config.metrics.enabled {
            config = config.with_metrics(MetricArgs {
                prometheus: Some(SocketAddr::new(
                    launch_config.metrics.addr,
                    launch_config.metrics.port,
                )),
                push_gateway_url: None,
                push_gateway_interval: Duration::from_secs(5),
            });
        }

        Ok(config)
    }

    fn open_database(path: PathBuf) -> eyre::Result<DatabaseEnv> {
        std::fs::create_dir_all(&path)
            .wrap_err_with(|| format!("failed to create database directory {}", path.display()))?;
        init_db(
            &path,
            DatabaseArguments::new(ClientVersion::default())
                .with_max_read_transaction_duration(Some(MaxReadTransactionDuration::Unbounded)),
        )
        .wrap_err_with(|| format!("failed to open database {}", path.display()))
    }
}
