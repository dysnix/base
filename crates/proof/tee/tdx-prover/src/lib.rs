#![doc = include_str!("../README.md")]

pub use base_proof_tee_tdx_verifier::{
    TDX_SIGNER_ATTESTATION_HEADER_LEN, TDX_SIGNER_ATTESTATION_MAGIC, TdxSignerAttestation,
    TdxSignerAttestationDecodeError,
};

mod backend;
pub use backend::{AggregateProposalInput, CONFIG_HASHES, TdxBackend};

mod error;
pub use error::{Result, TdxProverError};

mod image;
pub use image::{MeasuredMockTdxQuoteProvider, TdxMeasurements};

mod oracle;
pub use oracle::Oracle;

mod server;
pub use server::{
    TDX_ATTESTATION_KIND, TdxEnclaveService, TdxProverHandler, TdxProverServer, TdxSignerRpc,
};
