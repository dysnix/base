use std::{net::IpAddr, path::PathBuf};

use base_cli_utils::MetricsConfig;
use clap::{Args, Parser, Subcommand, ValueEnum};
use tracing::info;
use url::Url;

use crate::config::{BaseDatadir, ChainArg, ResolvedChainConfig};
use crate::execution::{DEFAULT_HTTP_PORT, DEFAULT_WS_PORT};
use crate::unified::{DEFAULT_CONSENSUS_RPC_PORT, UnifiedNode, UnifiedNodeConfig};

base_cli_utils::define_log_args!("BASE_NODE");
base_cli_utils::define_metrics_args!("BASE_NODE", 9090);

/// The `base` CLI.
#[derive(Parser, Clone, Debug)]
#[command(
    author,
    version = env!("CARGO_PKG_VERSION"),
    styles = base_cli_utils::CliStyles::init(),
    about,
    long_about = None
)]
pub(crate) struct BaseCli {
    /// Chain selection.
    #[arg(long, short = 'c', global = true, default_value = "mainnet", env = "BASE_CHAIN")]
    pub(crate) chain: ChainArg,

    /// Logging configuration.
    #[command(flatten)]
    pub(crate) logging: LogArgs,

    /// Metrics configuration.
    #[command(flatten)]
    pub(crate) metrics: MetricsArgs,

    /// The command to run.
    #[command(subcommand)]
    pub(crate) command: BaseCommand,
}

/// Top-level commands for `base`.
#[derive(Subcommand, Clone, Debug)]
#[non_exhaustive]
pub(crate) enum BaseCommand {
    /// Start the integrated Base node.
    #[command(name = "node")]
    Node(NodeArgs),
}

impl BaseCommand {
    /// Runs the selected top-level command.
    pub(crate) async fn run(
        self,
        resolved_chain: ResolvedChainConfig,
        metrics: MetricsConfig,
    ) -> eyre::Result<()> {
        match self {
            Self::Node(node) => node.run(resolved_chain, metrics).await,
        }
    }
}

/// Arguments for `base node`.
#[derive(Args, Clone, Debug)]
pub(crate) struct NodeArgs {
    /// The node flavor to run.
    #[arg(long, value_enum, default_value_t = NodeFlavor::Rpc)]
    pub(crate) flavor: NodeFlavor,

    /// Arguments for the selected node flavor.
    #[command(flatten)]
    pub(crate) validator: ValidatorArgs,
}

impl NodeArgs {
    /// Runs the selected `node` flavor.
    pub(crate) async fn run(
        self,
        resolved_chain: ResolvedChainConfig,
        metrics: MetricsConfig,
    ) -> eyre::Result<()> {
        match self.flavor {
            NodeFlavor::Rpc => self.validator.run(resolved_chain, metrics).await,
        }
    }
}

/// Available `base node` flavors.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum NodeFlavor {
    /// Run the integrated node in RPC mode.
    Rpc,
}

/// Arguments for running the unified validator node.
#[derive(Args, Clone, Debug)]
pub(crate) struct ValidatorArgs {
    /// Root datadir. Defaults to `~/.base/<chain-name>`.
    #[arg(long, env = "BASE_DATADIR")]
    pub(crate) datadir: Option<PathBuf>,

    /// L1 execution RPC URL.
    #[arg(long = "l1-eth-rpc", env = "BASE_L1_ETH_RPC")]
    pub(crate) l1_eth_rpc: Url,

    /// L1 beacon RPC URL.
    #[arg(long = "l1-beacon", env = "BASE_L1_BEACON")]
    pub(crate) l1_beacon: Url,

    /// Whether to trust the L1 RPC responses without hash validation.
    #[arg(
        long = "l1.trust-rpc",
        env = "BASE_NODE_L1_TRUST_RPC",
        action = clap::ArgAction::Set,
        default_value_t = true
    )]
    pub(crate) l1_trust_rpc: bool,

    /// Optional trusted peers for the execution P2P stack.
    #[arg(
        long = "execution.trusted-peers",
        value_delimiter = ',',
        env = "BASE_EXECUTION_TRUSTED_PEERS"
    )]
    pub(crate) execution_trusted_peers: Vec<String>,

    /// Optional bootnodes for the consensus P2P stack.
    #[arg(long = "p2p.bootnodes", value_delimiter = ',', env = "BASE_P2P_BOOTNODES")]
    pub(crate) p2p_bootnodes: Vec<String>,

    /// Optional advertised hostname or IP for the consensus P2P stack.
    #[arg(long = "p2p.advertise.ip", env = "BASE_P2P_ADVERTISE_IP")]
    pub(crate) p2p_advertise_ip: Option<String>,

    /// Optional advertised TCP port for the consensus P2P stack.
    #[arg(long = "p2p.advertise.tcp-port", env = "BASE_P2P_ADVERTISE_TCP_PORT")]
    pub(crate) p2p_advertise_tcp_port: Option<u16>,

    /// Optional advertised UDP port for the consensus P2P stack.
    #[arg(long = "p2p.advertise.udp-port", env = "BASE_P2P_ADVERTISE_UDP_PORT")]
    pub(crate) p2p_advertise_udp_port: Option<u16>,

    /// Number of L1 blocks to keep distance from the L1 head for validator derivation.
    #[arg(long = "l1.verifier-confs", default_value_t = 0, env = "BASE_NODE_VERIFIER_L1_CONFS")]
    pub(crate) verifier_l1_confs: u64,

    /// Execution-layer HTTP RPC bind address.
    #[arg(long = "el.http-addr", env = "BASE_NODE_EL_HTTP_ADDR")]
    pub(crate) execution_http_addr: Option<IpAddr>,

    /// Execution-layer HTTP RPC port.
    #[arg(
        long = "el.http-port",
        env = "BASE_NODE_EL_HTTP_PORT",
        default_value_t = DEFAULT_HTTP_PORT
    )]
    pub(crate) execution_http_port: u16,

    /// Execution-layer WebSocket RPC bind address.
    #[arg(long = "el.ws-addr", env = "BASE_NODE_EL_WS_ADDR")]
    pub(crate) execution_ws_addr: Option<IpAddr>,

    /// Execution-layer WebSocket RPC port.
    #[arg(
        long = "el.ws-port",
        env = "BASE_NODE_EL_WS_PORT",
        default_value_t = DEFAULT_WS_PORT
    )]
    pub(crate) execution_ws_port: u16,

    /// Optional execution-layer P2P port.
    #[arg(long = "el.p2p-port", env = "BASE_NODE_EL_P2P_PORT")]
    pub(crate) execution_p2p_port: Option<u16>,

    /// Whether to disable execution-layer discovery.
    #[arg(
        long = "el.discovery-disabled",
        env = "BASE_NODE_EL_DISCOVERY_DISABLED",
        action = clap::ArgAction::Set,
        default_value_t = false
    )]
    pub(crate) execution_discovery_disabled: bool,

    /// Consensus-layer RPC bind address.
    #[arg(long = "cl.rpc-addr", env = "BASE_NODE_CL_RPC_ADDR")]
    pub(crate) consensus_rpc_addr: Option<IpAddr>,

    /// Consensus-layer RPC port.
    #[arg(
        long = "cl.rpc-port",
        env = "BASE_NODE_CL_RPC_PORT",
        default_value_t = DEFAULT_CONSENSUS_RPC_PORT
    )]
    pub(crate) consensus_rpc_port: u16,

    /// Optional consensus-layer P2P listen IP.
    #[arg(long = "cl.listen-ip", env = "BASE_NODE_CL_LISTEN_IP")]
    pub(crate) consensus_listen_ip: Option<IpAddr>,

    /// Optional consensus-layer P2P TCP listen port.
    #[arg(long = "cl.listen-tcp-port", env = "BASE_NODE_CL_LISTEN_TCP_PORT")]
    pub(crate) consensus_listen_tcp_port: Option<u16>,

    /// Optional consensus-layer P2P UDP listen port.
    #[arg(long = "cl.listen-udp-port", env = "BASE_NODE_CL_LISTEN_UDP_PORT")]
    pub(crate) consensus_listen_udp_port: Option<u16>,

    /// Whether to disable consensus-layer discovery.
    #[arg(
        long = "cl.no-discovery",
        env = "BASE_NODE_CL_NO_DISCOVERY",
        action = clap::ArgAction::Set,
        default_value_t = false
    )]
    pub(crate) consensus_no_discovery: bool,
}

impl ValidatorArgs {
    /// Runs the unified validator node.
    pub(crate) async fn run(
        self,
        resolved_chain: ResolvedChainConfig,
        metrics: MetricsConfig,
    ) -> eyre::Result<()> {
        let datadir = match self.datadir {
            Some(path) => BaseDatadir::new(path),
            None => BaseDatadir::default_for_chain(&resolved_chain.name)?,
        };
        let sequencer_url = resolved_chain.sequencer_url().map(ToOwned::to_owned);
        let flashblocks_url = resolved_chain.flashblocks_url()?;

        info!(
            chain = %resolved_chain.name,
            l1_chain_id = resolved_chain.l1_chain_id,
            l2_chain_id = resolved_chain.l2_chain_id,
            datadir = %datadir.path.display(),
            "Starting unified validator node"
        );

        UnifiedNode::run(UnifiedNodeConfig {
            chain: resolved_chain,
            datadir,
            l1_eth_rpc: self.l1_eth_rpc,
            l1_beacon: self.l1_beacon,
            l1_trust_rpc: self.l1_trust_rpc,
            sequencer_url,
            flashblocks_url,
            execution_trusted_peers: self.execution_trusted_peers,
            execution_http_addr: self.execution_http_addr,
            execution_http_port: self.execution_http_port,
            execution_ws_addr: self.execution_ws_addr,
            execution_ws_port: self.execution_ws_port,
            execution_p2p_port: self.execution_p2p_port,
            execution_discovery_disabled: self.execution_discovery_disabled,
            bootnodes: self.p2p_bootnodes,
            advertise_host: self.p2p_advertise_ip,
            advertise_tcp_port: self.p2p_advertise_tcp_port,
            advertise_udp_port: self.p2p_advertise_udp_port,
            consensus_rpc_addr: self.consensus_rpc_addr,
            consensus_rpc_port: self.consensus_rpc_port,
            consensus_listen_ip: self.consensus_listen_ip,
            consensus_listen_tcp_port: self.consensus_listen_tcp_port,
            consensus_listen_udp_port: self.consensus_listen_udp_port,
            consensus_no_discovery: self.consensus_no_discovery,
            verifier_l1_confs: self.verifier_l1_confs,
            metrics,
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::*;
    use crate::config::{BuiltInChain, ChainArg};

    #[test]
    fn parses_default_chain_for_node() {
        let cli = BaseCli::parse_from([
            "base",
            "node",
            "--l1-eth-rpc",
            "http://localhost:8545",
            "--l1-beacon",
            "http://localhost:5052",
        ]);

        assert!(matches!(cli.chain, ChainArg::BuiltIn(BuiltInChain::Mainnet)));
        assert!(matches!(cli.command, BaseCommand::Node(_)));
    }

    #[test]
    fn parses_named_chain_selector() {
        let cli = BaseCli::parse_from([
            "base",
            "-c",
            "sepolia",
            "node",
            "--l1-eth-rpc",
            "http://localhost:8545",
            "--l1-beacon",
            "http://localhost:5052",
        ]);

        assert!(matches!(cli.chain, ChainArg::BuiltIn(BuiltInChain::Sepolia)));
    }

    #[test]
    fn parses_path_chain_selector() {
        let cli = BaseCli::parse_from([
            "base",
            "--chain",
            "./chain.toml",
            "node",
            "--l1-eth-rpc",
            "http://localhost:8545",
            "--l1-beacon",
            "http://localhost:5052",
        ]);

        assert!(matches!(cli.chain, ChainArg::File(_)));
    }

    #[test]
    fn parses_validator_operator_flags() {
        let cli = BaseCli::parse_from([
            "base",
            "node",
            "--l1-eth-rpc",
            "http://localhost:8545",
            "--l1-beacon",
            "http://localhost:5052",
            "--l1.trust-rpc",
            "false",
            "--datadir",
            "/tmp/base",
            "--execution.trusted-peers",
            "enode://def@host:30303",
            "--p2p.bootnodes",
            "enode://abc@host:9000",
            "--p2p.advertise.ip",
            "127.0.0.1",
            "--p2p.advertise.tcp-port",
            "19003",
            "--p2p.advertise.udp-port",
            "19004",
            "--l1.verifier-confs",
            "15",
            "--el.http-addr",
            "127.0.0.1",
            "--el.http-port",
            "18545",
            "--el.ws-addr",
            "127.0.0.1",
            "--el.ws-port",
            "18546",
            "--el.p2p-port",
            "30303",
            "--el.discovery-disabled",
            "true",
            "--cl.rpc-addr",
            "127.0.0.1",
            "--cl.rpc-port",
            "19545",
            "--cl.listen-ip",
            "0.0.0.0",
            "--cl.listen-tcp-port",
            "19001",
            "--cl.listen-udp-port",
            "19002",
            "--cl.no-discovery",
            "true",
        ]);

        let BaseCommand::Node(node) = cli.command;
        let validator = node.validator;

        assert_eq!(validator.datadir, Some(PathBuf::from("/tmp/base")));
        assert_eq!(validator.l1_eth_rpc.as_str(), "http://localhost:8545/");
        assert_eq!(validator.l1_beacon.as_str(), "http://localhost:5052/");
        assert!(!validator.l1_trust_rpc);
        assert_eq!(validator.execution_trusted_peers, vec!["enode://def@host:30303".to_owned()]);
        assert_eq!(validator.p2p_bootnodes, vec!["enode://abc@host:9000".to_owned()]);
        assert_eq!(validator.p2p_advertise_ip.as_deref(), Some("127.0.0.1"));
        assert_eq!(validator.p2p_advertise_tcp_port, Some(19_003));
        assert_eq!(validator.p2p_advertise_udp_port, Some(19_004));
        assert_eq!(validator.verifier_l1_confs, 15);
        assert_eq!(validator.execution_http_addr, Some("127.0.0.1".parse().unwrap()));
        assert_eq!(validator.execution_http_port, 18_545);
        assert_eq!(validator.execution_ws_addr, Some("127.0.0.1".parse().unwrap()));
        assert_eq!(validator.execution_ws_port, 18_546);
        assert_eq!(validator.execution_p2p_port, Some(30_303));
        assert!(validator.execution_discovery_disabled);
        assert_eq!(validator.consensus_rpc_addr, Some("127.0.0.1".parse().unwrap()));
        assert_eq!(validator.consensus_rpc_port, 19_545);
        assert_eq!(validator.consensus_listen_ip, Some("0.0.0.0".parse().unwrap()));
        assert_eq!(validator.consensus_listen_tcp_port, Some(19_001));
        assert_eq!(validator.consensus_listen_udp_port, Some(19_002));
        assert!(validator.consensus_no_discovery);
    }

    #[test]
    fn node_rejects_config_backed_chain_flags() {
        let error = BaseCli::try_parse_from([
            "base",
            "node",
            "--l1-eth-rpc",
            "http://localhost:8545",
            "--l1-beacon",
            "http://localhost:5052",
            "--rollup.sequencer",
            "http://localhost:7545",
        ])
        .unwrap_err();

        assert!(error.to_string().contains("unexpected argument"));

        let error = BaseCli::try_parse_from([
            "base",
            "node",
            "--l1-eth-rpc",
            "http://localhost:8545",
            "--l1-beacon",
            "http://localhost:5052",
            "--flashblocks-url",
            "ws://localhost:7111",
        ])
        .unwrap_err();

        assert!(error.to_string().contains("unexpected argument"));
    }

    #[test]
    fn chain_arg_uses_base_chain_env_var() {
        let command = BaseCli::command();
        let chain_arg =
            command.get_arguments().find(|arg| arg.get_long() == Some("chain")).unwrap();

        assert_eq!(chain_arg.get_env().and_then(|value| value.to_str()), Some("BASE_CHAIN"));
    }

    #[test]
    fn rejects_multiple_chain_selectors() {
        let err = BaseCli::try_parse_from([
            "base",
            "-c",
            "mainnet",
            "--chain",
            "sepolia",
            "node",
            "--l1-eth-rpc",
            "http://localhost:8545",
            "--l1-beacon",
            "http://localhost:5052",
        ])
        .unwrap_err();

        let rendered = err.to_string();
        assert!(rendered.contains("cannot be used multiple times"));
    }

    #[test]
    fn parses_explicit_rpc_flavor() {
        let cli = BaseCli::parse_from([
            "base",
            "node",
            "--flavor",
            "rpc",
            "--l1-eth-rpc",
            "http://localhost:8545",
            "--l1-beacon",
            "http://localhost:5052",
        ]);

        let BaseCommand::Node(node) = cli.command;
        assert_eq!(node.flavor, NodeFlavor::Rpc);
    }
}
