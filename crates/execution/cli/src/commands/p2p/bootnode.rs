//! Bootnode command with discv5 NAT fix.

use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use base_execution_chainspec::BaseChainSpec;
use base_node_core::BaseDiscoveryFilter;
use clap::Parser;
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_util::{get_secret_key, load_secret_key::rng_secret_key};
use reth_discv4::{DiscoveryUpdate, Discv4, Discv4Config};
use reth_discv5::{
    Config, Discv5,
    discv5::{ConfigBuilder as Discv5ConfigBuilder, Event, ListenConfig},
};
use reth_net_nat::{NatResolver, external_addr_with};
use reth_network_peers::NodeRecord;
use secp256k1::SecretKey;
use tokio::select;
use tokio_stream::StreamExt;
use tracing::{info, warn};

/// Start a discovery-only bootnode.
#[derive(Parser, Debug)]
pub struct Command<C: ChainSpecParser = crate::chainspec::BaseChainSpecParser> {
    /// Listen address for the bootnode for discv4
    #[arg(long, default_value = "0.0.0.0:30301")]
    pub v4_addr: SocketAddr,

    /// Listen address for the bootnode for discv5
    #[arg(long, default_value = "0.0.0.0:9200")]
    pub v5_addr: SocketAddr,

    /// Secret key for the bootnode. Deterministically sets the peer ID.
    /// If the path exists, the key is loaded; otherwise a new key is generated and saved there.
    /// If omitted, an ephemeral key is used.
    #[arg(long, value_name = "PATH")]
    pub p2p_secret_key: Option<PathBuf>,

    /// NAT resolution method (any|none|upnp|publicip|extip:\<IP\>)
    #[arg(long, default_value = "any")]
    pub nat: NatResolver,

    /// Run a discv5 topic discovery bootnode in addition to discv4.
    #[arg(long)]
    pub v5: bool,

    /// The chain this node is running.
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        long_help = C::help_message(),
        default_value = C::default_value(),
        value_parser = C::parser(),
        global = true,
    )]
    pub chain: Arc<C::ChainSpec>,
}

impl<C: ChainSpecParser<ChainSpec = BaseChainSpec>> Command<C> {
    /// Execute the bootnode command.
    pub async fn execute(self) -> eyre::Result<()> {
        info!(v4_addr = %self.v4_addr, v5_addr = %self.v5_addr, nat = %self.nat, v5 = self.v5, "Bootnode starting");

        let v4_addr = self.v4_addr;
        let v5_addr = self.v5_addr;
        let sk = self.network_secret()?;
        let v4_node_record = self.discv4_node_record(&sk);
        let discv4_config = self.discv4_config();
        let discv5_config = self.v5.then(|| {
            BaseDiscoveryFilter::init_for_chain_spec(self.chain.as_ref());
            self.discv5_config()
        });
        let nat = self.nat;
        let (_discv4, mut discv4_service) =
            Discv4::bind(v4_addr, v4_node_record, sk, discv4_config).await?;
        info!(v4_node_record = ?v4_node_record, enode = %v4_node_record, "Started discv4");
        let mut discv4_updates = discv4_service.update_stream();
        discv4_service.spawn();

        let mut discv5_updates = None;
        let mut _discv5 = None;

        if let Some(discv5_config) = discv5_config {
            info!("Initializing discv5");
            let (discv5, updates) = Discv5::start(&sk, discv5_config).await?;

            // The upstream reth bootnode skips NAT resolution for discv5, leaving the ENR with
            // no IP address. Peers receiving the ENR cannot send WHOAREYOU back because they
            // have no address to target. Resolve the external IP and update the ENR here.
            match external_addr_with(nat).await {
                Some(external_ip) => {
                    let socket = SocketAddr::new(external_ip, v5_addr.port());
                    discv5.with_discv5(|d| d.update_local_enr_socket(socket, false));
                }
                None => {
                    warn!(
                        addr = %v5_addr,
                        "Could not resolve external IP via NAT; discv5 ENR has no IP and may not be reachable"
                    );
                }
            }

            info!(enr = %discv5.local_enr(), "Started discv5");

            discv5_updates = Some(updates);
            _discv5 = Some(discv5);
        }

        loop {
            select! {
                update = discv4_updates.next() => {
                    match update {
                        Some(DiscoveryUpdate::Added(record)) => {
                            info!(peer_id = ?record.id, "discv4 peer added");
                        }
                        Some(DiscoveryUpdate::Removed(peer_id)) => {
                            info!(peer_id = ?peer_id, "discv4 peer removed");
                        }
                        Some(_) => {}
                        None => {
                            info!("discv4 update stream ended");
                            break;
                        }
                    }
                }
                update = async {
                    if let Some(updates) = &mut discv5_updates {
                        updates.recv().await
                    } else {
                        futures::future::pending().await
                    }
                } => {
                    match update {
                        Some(Event::SessionEstablished(enr, _)) => {
                            info!(peer_id = ?enr.id(), "discv5 session established");
                        }
                        Some(_) => {}
                        None => {
                            info!("discv5 update stream ended");
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Returns the discv4 node record that the bootnode advertises.
    pub fn discv4_node_record(&self, secret_key: &SecretKey) -> NodeRecord {
        NodeRecord::from_secret_key(self.v4_addr, secret_key)
    }

    /// Returns the discv4 configuration for the bootnode.
    pub fn discv4_config(&self) -> Discv4Config {
        Discv4Config::builder().external_ip_resolver(Some(self.nat.clone())).build()
    }

    /// Returns the discv5 configuration for the bootnode.
    pub fn discv5_config(&self) -> Config {
        Config::builder(self.v5_addr)
            .add_enr_kv_pair(BaseDiscoveryFilter::ENR_KEY, BaseDiscoveryFilter::version_enr_value())
            .discv5_config(
                Discv5ConfigBuilder::new(ListenConfig::from_ip(
                    self.v5_addr.ip(),
                    self.v5_addr.port(),
                ))
                .table_filter(BaseDiscoveryFilter::table_filter)
                .build(),
            )
            .build()
    }

    /// Returns the chain spec for the bootnode command.
    pub const fn chain_spec(&self) -> Option<&Arc<BaseChainSpec>> {
        Some(&self.chain)
    }
}

impl<C: ChainSpecParser> Command<C> {
    fn network_secret(&self) -> eyre::Result<SecretKey> {
        match &self.p2p_secret_key {
            Some(path) => Ok(get_secret_key(path)?),
            None => Ok(rng_secret_key()),
        }
    }
}

#[cfg(test)]
mod tests {
    use base_execution_chainspec::BASE_MAINNET;
    use reth_discv5::build_local_enr;

    use super::*;

    fn command(v4_addr: &str, v5_addr: &str) -> Command {
        Command {
            v4_addr: v4_addr.parse().expect("valid v4 socket"),
            v5_addr: v5_addr.parse().expect("valid v5 socket"),
            p2p_secret_key: None,
            nat: NatResolver::None,
            v5: true,
            chain: BASE_MAINNET.clone(),
        }
    }

    #[test]
    fn discv4_node_record_uses_v4_addr() {
        let command = command("127.0.0.1:30301", "127.0.0.1:9200");
        let secret_key = SecretKey::from_slice(&[3; 32]).expect("valid secret key");
        let record = command.discv4_node_record(&secret_key);

        assert_eq!(record.address, command.v4_addr.ip());
        assert_eq!(record.udp_port, command.v4_addr.port());
        assert_eq!(record.tcp_port, command.v4_addr.port());
    }

    #[test]
    fn discv5_config_uses_v5_addr() {
        let command = command("127.0.0.1:30301", "127.0.0.1:9200");
        let config = command.discv5_config();

        assert_eq!(config.discovery_socket(), command.v5_addr);
    }

    #[test]
    fn discv5_local_enr_has_base_key_and_v5_udp_port() {
        let command = command("127.0.0.1:30301", "127.0.0.1:9200");
        let secret_key = SecretKey::from_slice(&[5; 32]).expect("valid secret key");
        let (enr, _, _, _) = build_local_enr(&secret_key, &command.discv5_config());

        assert_eq!(enr.ip4(), Some("127.0.0.1".parse().expect("valid ipv4")));
        assert_eq!(enr.udp4(), Some(command.v5_addr.port()));
        assert!(enr.get_raw_rlp(BaseDiscoveryFilter::ENR_KEY).is_some());
    }
}
