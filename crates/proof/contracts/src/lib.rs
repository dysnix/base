#![doc = include_str!("../README.md")]
#![doc(
    html_logo_url = "https://avatars.githubusercontent.com/u/16627100?s=200&v=4",
    html_favicon_url = "https://avatars.githubusercontent.com/u/16627100?s=200&v=4",
    issue_tracker_base_url = "https://github.com/base/base/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

#[cfg(feature = "clients")]
mod aggregate_verifier;
#[cfg(feature = "clients")]
pub use aggregate_verifier::{
    AggregateVerifierClient, AggregateVerifierContractClient, GameInfo, encode_challenge_calldata,
    encode_claim_credit_calldata, encode_nullify_calldata, encode_resolve_calldata,
};

#[cfg(feature = "clients")]
mod delayed_weth;
#[cfg(feature = "clients")]
pub use delayed_weth::{DelayedWETHClient, DelayedWETHContractClient};

#[cfg(feature = "clients")]
mod anchor_state_registry;
#[cfg(feature = "clients")]
pub use anchor_state_registry::{
    AnchorPreflight, AnchorRoot, AnchorStateRegistryClient, AnchorStateRegistryContractClient,
    encode_set_anchor_state_calldata,
};

#[cfg(feature = "clients")]
mod dispute_game_factory;
#[cfg(feature = "clients")]
pub use dispute_game_factory::{
    DisputeGameFactoryClient, DisputeGameFactoryContractClient, GameAtIndex,
    encode_create_calldata, encode_extra_data, game_already_exists_selector,
};

#[cfg(feature = "clients")]
mod tee_prover_registry;
#[cfg(feature = "clients")]
pub use tee_prover_registry::{
    ITEEProverRegistry, TEEProverRegistryClient, TEEProverRegistryContractClient,
};

#[cfg(feature = "clients")]
mod nitro_enclave_verifier;
#[cfg(feature = "clients")]
pub use nitro_enclave_verifier::INitroEnclaveVerifier;

mod tdx_verifier;
pub use tdx_verifier::{
    ITDXTEEProverRegistry, ITDXVerifier, TDXTcbStatus, TDXVerificationResult, TDXVerifierJournal,
    ZkCoProcessorConfig, ZkCoProcessorType,
};

#[cfg(feature = "clients")]
mod error;
#[cfg(feature = "clients")]
pub use error::ContractError;
