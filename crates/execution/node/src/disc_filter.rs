//! Discovery v5 table filter for Base peer validation.
//!
//! Peers are accepted into the discv5 routing table if they advertise either:
//!
//! 1. A `"base"` ENR key (any value) — the canonical Base network identifier.
//! 2. An `"opel"` ENR key whose [`ForkId`] matches the expected Azul (Base V1)
//!    fork ID — backward compatibility with peers that have not yet adopted the
//!    `"base"` tag.
//!
//! When Azul is not yet scheduled on the current chain (i.e. [`hardfork_fork_id`]
//! returns `None`), any `"opel"` entry is accepted regardless of value so that
//! existing peers remain reachable.
//!
//! # Lifecycle
//!
//! After the next required network upgrade where **all** peers are expected to
//! advertise the `"base"` tag, the `"opel"` fallback logic in
//! [`base_table_filter`] can be removed entirely, simplifying the filter to a
//! single `"base"` key-presence check.
//!
//! [`hardfork_fork_id`]: reth_chainspec::EthChainSpec::hardfork_fork_id

use std::sync::OnceLock;

use discv5::Enr;
use reth_discv5::NetworkStackId;
use reth_ethereum_forks::{EnrForkIdEntry, ForkId};
use tracing::trace;

/// Holds the expected Azul (Base V1) fork ID for the active chain.
///
/// Initialized once via [`init_azul_fork_id`] before discovery starts.
/// - `Some(fork_id)` — Azul is scheduled; `"opel"` entries must match.
/// - `None` — Azul is not scheduled; any `"opel"` entry is accepted.
static AZUL_FORK_ID: OnceLock<Option<ForkId>> = OnceLock::new();

/// The ENR key used to identify Base network peers.
pub const BASE_ENR_KEY: &[u8] = b"base";

/// Stores the expected Azul fork ID for later use by [`base_table_filter`].
///
/// Must be called exactly once before the discv5 service starts. Subsequent
/// calls are silently ignored (the first value wins).
pub fn init_azul_fork_id(fork_id: Option<ForkId>) {
    let _ = AZUL_FORK_ID.set(fork_id);
}

/// discv5 table filter for Base peers.
///
/// This is a **function pointer** (`fn(&Enr) -> bool`) — it cannot capture
/// state. Chain-specific context is read from the [`AZUL_FORK_ID`] static
/// that must be initialized via [`init_azul_fork_id`] before discovery
/// starts.
///
/// # Acceptance criteria
///
/// A peer is accepted if **any** of the following hold:
///
/// 1. The ENR contains a `"base"` key (value is ignored).
/// 2. Azul is scheduled **and** the ENR's `"opel"` [`ForkId`] matches the
///    expected Azul fork ID.
/// 3. Azul is **not** scheduled **and** the ENR contains an `"opel"` key
///    (value is ignored — backward compat until Azul activates everywhere).
///
/// # TODO
///
/// Once Azul is scheduled on all networks, the `None` arm (case 3) can be
/// tightened to require an exact fork ID match. After the subsequent required
/// update, the entire `"opel"` fallback can be removed in favor of `"base"`
/// only.
pub fn base_table_filter(enr: &Enr) -> bool {
    let azul = AZUL_FORK_ID.get().copied().flatten();
    filter_enr(enr, azul)
}

/// Core filter logic, separated from the static for testability.
fn filter_enr(enr: &Enr, azul_fork_id: Option<ForkId>) -> bool {
    // Case 1: peer advertises the "base" tag — always accepted.
    if enr.get_raw_rlp(BASE_ENR_KEY).is_some() {
        return true;
    }

    // Case 2: Azul is scheduled — accept only if opel matches the expected
    // fork ID. Peers with a missing or non-decodable opel entry are rejected.
    //
    // Case 3: Azul is NOT yet scheduled on this chain — accept any peer
    // that has an opel entry, regardless of its value.
    //
    // TODO(azul): Once Azul is scheduled on all networks, the None branch
    // can be tightened to require an exact fork ID match. After the subsequent
    // required update, the entire opel fallback can be removed in favor of the
    // "base" key only.
    azul_fork_id.map_or_else(
        || {
            let has_opel = enr.get_raw_rlp(NetworkStackId::OPEL).is_some();
            if !has_opel {
                trace!("rejecting peer: no base tag and no opel entry");
            }
            has_opel
        },
        |expected| {
            let matched = matches!(
                enr.get_decodable::<EnrForkIdEntry>(NetworkStackId::OPEL),
                Some(Ok(entry)) if ForkId::from(entry.clone()) == expected
            );
            if !matched {
                trace!("rejecting peer: opel fork ID does not match Azul");
            }
            matched
        },
    )
}

#[cfg(test)]
mod tests {
    use alloy_rlp::Encodable;
    use bytes::BytesMut;
    use discv5::enr::{CombinedKey, Enr as EnrBuilder};
    use reth_ethereum_forks::ForkHash;

    use super::*;

    fn build_enr(pairs: &[(&[u8], Vec<u8>)]) -> Enr {
        let key = CombinedKey::generate_secp256k1();
        let mut builder = EnrBuilder::builder();
        for (k, v) in pairs {
            builder.add_value_rlp(k, v.clone().into());
        }
        builder.build(&key).unwrap()
    }

    fn encode_fork_id(fork_id: &ForkId) -> Vec<u8> {
        let entry = EnrForkIdEntry::from(*fork_id);
        let mut buf = BytesMut::new();
        entry.encode(&mut buf);
        buf.to_vec()
    }

    fn encode_str(s: &str) -> Vec<u8> {
        let mut buf = BytesMut::new();
        s.encode(&mut buf);
        buf.to_vec()
    }

    const AZUL_MAINNET: ForkId = ForkId { hash: ForkHash([0x86, 0x72, 0x8b, 0x4e]), next: 0 };
    const WRONG_FORK: ForkId = ForkId { hash: ForkHash([0xde, 0xad, 0xbe, 0xef]), next: 0 };

    #[test]
    fn accepts_enr_with_base_tag() {
        let enr = build_enr(&[(BASE_ENR_KEY, encode_str("1.0.0"))]);
        assert!(filter_enr(&enr, Some(AZUL_MAINNET)));
    }

    #[test]
    fn accepts_enr_with_matching_opel_azul() {
        let enr = build_enr(&[(NetworkStackId::OPEL, encode_fork_id(&AZUL_MAINNET))]);
        assert!(filter_enr(&enr, Some(AZUL_MAINNET)));
    }

    #[test]
    fn rejects_enr_with_no_base_no_opel() {
        let enr = build_enr(&[]);
        assert!(!filter_enr(&enr, Some(AZUL_MAINNET)));
    }

    #[test]
    fn rejects_enr_with_wrong_opel_no_base() {
        let enr = build_enr(&[(NetworkStackId::OPEL, encode_fork_id(&WRONG_FORK))]);
        assert!(!filter_enr(&enr, Some(AZUL_MAINNET)));
    }

    #[test]
    fn accepts_enr_with_wrong_opel_but_has_base() {
        let enr = build_enr(&[
            (NetworkStackId::OPEL, encode_fork_id(&WRONG_FORK)),
            (BASE_ENR_KEY, encode_str("0.8.0")),
        ]);
        assert!(filter_enr(&enr, Some(AZUL_MAINNET)));
    }

    #[test]
    fn accepts_any_opel_when_azul_not_scheduled() {
        let enr = build_enr(&[(NetworkStackId::OPEL, encode_fork_id(&WRONG_FORK))]);
        assert!(filter_enr(&enr, None));
    }

    #[test]
    fn rejects_no_opel_no_base_when_azul_not_scheduled() {
        let enr = build_enr(&[]);
        assert!(!filter_enr(&enr, None));
    }

    #[test]
    fn accepts_base_tag_when_azul_not_scheduled() {
        let enr = build_enr(&[(BASE_ENR_KEY, encode_str("0.9.0"))]);
        assert!(filter_enr(&enr, None));
    }
}
