//! Base discv5 ENR tagging and peer validation helpers.

use std::sync::OnceLock;

use alloy_primitives::Bytes;
use base_common_chains::BaseUpgrade;
use base_execution_chainspec::BaseChainSpec;
use reth_discv5::{NetworkStackId, discv5::Enr};
use reth_ethereum_forks::{EnrForkIdEntry, ForkId};
use reth_node_core::version::version_metadata;
use tracing::trace;

/// Discovery helpers shared by the execution node and bootnode.
#[derive(Debug, Clone, Copy, Default)]
pub struct BaseDiscoveryFilter;

static AZUL_FORK_ID: OnceLock<Option<ForkId>> = OnceLock::new();

impl BaseDiscoveryFilter {
    /// The ENR key that marks a peer as part of the Base discovery network.
    pub const ENR_KEY: &'static [u8] = b"base";

    /// Stores the expected Azul fork ID for the active chain.
    ///
    /// The first initialized value wins for the process lifetime.
    pub fn init_for_chain_spec(chain_spec: &BaseChainSpec) {
        let _ = AZUL_FORK_ID.set(chain_spec.hardfork_fork_id(BaseUpgrade::V1));
    }

    /// Returns the RLP-encoded crate version used for the Base ENR entry.
    pub fn version_enr_value() -> Bytes {
        alloy_rlp::encode(version_metadata().cargo_pkg_version.as_ref()).into()
    }

    /// Returns `true` if the peer should be accepted into the Base discovery table.
    pub fn table_filter(enr: &Enr) -> bool {
        Self::matches_enr(enr, AZUL_FORK_ID.get().copied().flatten())
    }

    /// Returns `true` if the given ENR matches the Base discovery policy.
    pub fn matches_enr(enr: &Enr, azul_fork_id: Option<ForkId>) -> bool {
        if enr.get_raw_rlp(Self::ENR_KEY).is_some() {
            return true;
        }

        azul_fork_id.map_or_else(
            || {
                let has_opel = enr.get_raw_rlp(NetworkStackId::OPEL).is_some();
                if !has_opel {
                    trace!("rejecting peer without base or opel ENR key");
                }
                has_opel
            },
            |expected| {
                let matched = matches!(
                    enr.get_decodable::<EnrForkIdEntry>(NetworkStackId::OPEL),
                    Some(Ok(entry)) if ForkId::from(entry.clone()) == expected
                );
                if !matched {
                    trace!("rejecting peer with mismatched opel fork id");
                }
                matched
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use alloy_rlp::{Decodable, Encodable};
    use reth_discv5::{Config, build_local_enr, discv5::enr::CombinedKey};
    use reth_ethereum_forks::ForkHash;
    use reth_network::config::SecretKey;

    use super::*;

    fn build_enr(pairs: &[(&'static [u8], Vec<u8>)]) -> Enr {
        let key = CombinedKey::generate_secp256k1();
        let mut builder = Enr::builder();
        for (entry_key, entry_value) in pairs {
            builder.add_value_rlp(*entry_key, entry_value.clone().into());
        }
        builder.build(&key).expect("test ENR should build")
    }

    fn encode_fork_id(fork_id: ForkId) -> Vec<u8> {
        alloy_rlp::encode(EnrForkIdEntry::from(fork_id)).to_vec()
    }

    fn encode_string(value: &str) -> Vec<u8> {
        let mut buf = Vec::new();
        value.encode(&mut buf);
        buf
    }

    const AZUL_MAINNET: ForkId = ForkId { hash: ForkHash([0x86, 0x72, 0x8b, 0x4e]), next: 0 };
    const WRONG_FORK: ForkId = ForkId { hash: ForkHash([0xde, 0xad, 0xbe, 0xef]), next: 0 };

    #[test]
    fn version_enr_value_round_trips() {
        let key = SecretKey::from_slice(&[7; 32]).expect("valid secret key");
        let config = Config::builder("127.0.0.1:30303".parse().expect("valid socket"))
            .add_enr_kv_pair(BaseDiscoveryFilter::ENR_KEY, BaseDiscoveryFilter::version_enr_value())
            .build();
        let (enr, _, _, _) = build_local_enr(&key, &config);
        let value =
            enr.get_raw_rlp(BaseDiscoveryFilter::ENR_KEY).expect("base ENR key should exist");
        let decoded = String::decode(&mut value.as_ref()).expect("version should decode");

        assert_eq!(decoded, version_metadata().cargo_pkg_version.as_ref());
    }

    #[test]
    fn accepts_base_key_with_any_value() {
        let enr = build_enr(&[(BaseDiscoveryFilter::ENR_KEY, encode_string("0.8.0"))]);
        assert!(BaseDiscoveryFilter::matches_enr(&enr, Some(AZUL_MAINNET)));
    }

    #[test]
    fn accepts_matching_opel_when_azul_is_scheduled() {
        let enr = build_enr(&[(NetworkStackId::OPEL, encode_fork_id(AZUL_MAINNET))]);
        assert!(BaseDiscoveryFilter::matches_enr(&enr, Some(AZUL_MAINNET)));
    }

    #[test]
    fn rejects_wrong_opel_when_azul_is_scheduled() {
        let enr = build_enr(&[(NetworkStackId::OPEL, encode_fork_id(WRONG_FORK))]);
        assert!(!BaseDiscoveryFilter::matches_enr(&enr, Some(AZUL_MAINNET)));
    }

    #[test]
    fn accepts_any_opel_before_azul_is_scheduled() {
        let enr = build_enr(&[(NetworkStackId::OPEL, encode_fork_id(WRONG_FORK))]);
        assert!(BaseDiscoveryFilter::matches_enr(&enr, None));
    }

    #[test]
    fn rejects_enr_without_base_or_opel() {
        let enr = build_enr(&[]);
        assert!(!BaseDiscoveryFilter::matches_enr(&enr, Some(AZUL_MAINNET)));
        assert!(!BaseDiscoveryFilter::matches_enr(&enr, None));
    }
}
