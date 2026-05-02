//! TEE proof source types for platform-specific prover fleets.

use std::{error::Error as StdError, fmt, sync::Arc};

use base_proof_primitives::{ProofRequest, ProofResult, Proposal, ProverClient};
use futures::future;
use thiserror::Error;

use crate::error::ProposerError;

/// TEE prover platform expected by the proposer.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum TeeProofPlatform {
    /// AWS Nitro Enclave TEE prover fleet.
    Nitro,
    /// Intel TDX TEE prover fleet.
    Tdx,
}

impl TeeProofPlatform {
    /// Returns the stable metrics and log label for the platform.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Nitro => "nitro",
            Self::Tdx => "tdx",
        }
    }
}

impl fmt::Display for TeeProofPlatform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// A proof returned by one platform-specific TEE prover fleet.
#[derive(Debug, Clone)]
pub struct PlatformProof {
    /// Platform that produced the proof.
    pub platform: TeeProofPlatform,
    /// Raw proof result returned by the platform prover.
    pub result: ProofResult,
}

impl PlatformProof {
    /// Creates a platform proof and rejects non-TEE prover responses.
    pub fn new(platform: TeeProofPlatform, result: ProofResult) -> Result<Self, ProposerError> {
        match &result {
            ProofResult::Tee { .. } => Ok(Self { platform, result }),
            ProofResult::Zk { .. } => Err(ProposerError::Prover(format!(
                "{platform} prover returned unexpected ZK proof result"
            ))),
        }
    }

    /// Returns the aggregate and per-block proposals for this TEE proof.
    pub fn proposals(&self) -> (&Proposal, &[Proposal]) {
        match &self.result {
            ProofResult::Tee { aggregate_proposal, proposals } => (aggregate_proposal, proposals),
            ProofResult::Zk { .. } => unreachable!("PlatformProof rejects ZK results"),
        }
    }
}

/// Paired Nitro and TDX proofs for one proposal input.
#[derive(Debug, Clone)]
pub struct DualPlatformProof {
    /// Proof returned by the Nitro prover fleet.
    pub nitro: PlatformProof,
    /// Proof returned by the TDX prover fleet.
    pub tdx: PlatformProof,
}

impl DualPlatformProof {
    /// Creates a paired proof after verifying both platforms signed the same proposal data.
    pub fn new(nitro: PlatformProof, tdx: PlatformProof) -> Result<Self, ProposerError> {
        let proof = Self { nitro, tdx };
        proof.validate_matching_payloads()?;
        Ok(proof)
    }

    /// Returns the Nitro proof used by the existing single-proof submission path.
    pub const fn submission_proof(&self) -> &PlatformProof {
        &self.nitro
    }

    /// Returns both platform proofs.
    pub const fn platform_proofs(&self) -> [&PlatformProof; 2] {
        [&self.nitro, &self.tdx]
    }

    /// Ensures both platform proofs are for the same proposal input.
    pub fn validate_matching_payloads(&self) -> Result<(), ProposerError> {
        let (nitro_aggregate, nitro_proposals) = self.nitro.proposals();
        let (tdx_aggregate, tdx_proposals) = self.tdx.proposals();

        Self::validate_matching_proposal_payload(
            nitro_aggregate,
            tdx_aggregate,
            "aggregate proposal",
        )?;

        if nitro_proposals.len() != tdx_proposals.len() {
            return Err(ProposerError::Prover(format!(
                "nitro and tdx proof proposal counts differ: nitro={}, tdx={}",
                nitro_proposals.len(),
                tdx_proposals.len()
            )));
        }

        for (index, (nitro_proposal, tdx_proposal)) in
            nitro_proposals.iter().zip(tdx_proposals.iter()).enumerate()
        {
            Self::validate_matching_proposal_payload(
                nitro_proposal,
                tdx_proposal,
                &format!("proposal {index}"),
            )?;
        }

        Ok(())
    }

    fn validate_matching_proposal_payload(
        nitro: &Proposal,
        tdx: &Proposal,
        label: &str,
    ) -> Result<(), ProposerError> {
        if nitro.output_root != tdx.output_root
            || nitro.l1_origin_hash != tdx.l1_origin_hash
            || nitro.l1_origin_number != tdx.l1_origin_number
            || nitro.l2_block_number != tdx.l2_block_number
            || nitro.prev_output_root != tdx.prev_output_root
            || nitro.config_hash != tdx.config_hash
        {
            return Err(ProposerError::Prover(format!(
                "nitro and tdx proofs do not match for {label}"
            )));
        }

        Ok(())
    }
}

/// Error returned while building a dual-platform proof.
#[derive(Debug, Error)]
pub enum TeeProofError {
    /// One platform failed while the other returned a usable TEE proof.
    #[error("{platform} prover failed: {error}")]
    Platform {
        /// Platform whose proof request failed.
        platform: TeeProofPlatform,
        /// Underlying proposer error.
        error: ProposerError,
    },
    /// Both platform proof requests failed.
    #[error("nitro and tdx provers failed: nitro={nitro}; tdx={tdx}")]
    BothPlatforms {
        /// Nitro prover error.
        nitro: ProposerError,
        /// TDX prover error.
        tdx: ProposerError,
    },
    /// Both platforms returned proofs, but the proof payloads did not match.
    #[error("{error}")]
    PayloadMismatch {
        /// Underlying mismatch error.
        error: ProposerError,
    },
    /// Non-platform-specific proof task failure.
    #[error("{error}")]
    Other {
        /// Underlying proposer error.
        error: ProposerError,
    },
}

impl TeeProofError {
    /// Returns the platform readiness implied by this error.
    pub const fn platform_readiness(&self) -> [(TeeProofPlatform, bool); 2] {
        match self {
            Self::Platform { platform: TeeProofPlatform::Nitro, .. } => {
                [(TeeProofPlatform::Nitro, false), (TeeProofPlatform::Tdx, true)]
            }
            Self::Platform { platform: TeeProofPlatform::Tdx, .. } => {
                [(TeeProofPlatform::Nitro, true), (TeeProofPlatform::Tdx, false)]
            }
            Self::BothPlatforms { .. } | Self::PayloadMismatch { .. } | Self::Other { .. } => {
                [(TeeProofPlatform::Nitro, false), (TeeProofPlatform::Tdx, false)]
            }
        }
    }

    /// Returns the metrics label for this error.
    pub const fn metric_label(&self) -> &'static str {
        match self {
            Self::Platform { error, .. }
            | Self::PayloadMismatch { error }
            | Self::Other { error } => error.metric_label(),
            Self::BothPlatforms { .. } => ProposerError::ERROR_TYPE_PROVER,
        }
    }
}

/// Configured Nitro and TDX prover clients.
#[derive(Debug, Clone)]
pub struct TeeProofSources {
    /// Nitro prover client.
    pub nitro: Arc<dyn ProverClient>,
    /// TDX prover client.
    pub tdx: Arc<dyn ProverClient>,
}

impl TeeProofSources {
    /// Creates paired proof sources.
    pub const fn new(nitro: Arc<dyn ProverClient>, tdx: Arc<dyn ProverClient>) -> Self {
        Self { nitro, tdx }
    }

    /// Requests a proof from both platform fleets for the same request.
    pub async fn prove(&self, request: ProofRequest) -> Result<DualPlatformProof, TeeProofError> {
        let nitro_request = request.clone();
        let tdx_request = request;

        let (nitro_result, tdx_result) =
            future::join(self.nitro.prove(nitro_request), self.tdx.prove(tdx_request)).await;

        let nitro = Self::platform_result(TeeProofPlatform::Nitro, nitro_result);
        let tdx = Self::platform_result(TeeProofPlatform::Tdx, tdx_result);

        match (nitro, tdx) {
            (Ok(nitro), Ok(tdx)) => DualPlatformProof::new(nitro, tdx)
                .map_err(|error| TeeProofError::PayloadMismatch { error }),
            (Err(error), Ok(_)) => {
                Err(TeeProofError::Platform { platform: TeeProofPlatform::Nitro, error })
            }
            (Ok(_), Err(error)) => {
                Err(TeeProofError::Platform { platform: TeeProofPlatform::Tdx, error })
            }
            (Err(nitro), Err(tdx)) => Err(TeeProofError::BothPlatforms { nitro, tdx }),
        }
    }

    fn platform_result(
        platform: TeeProofPlatform,
        result: Result<ProofResult, Box<dyn StdError + Send + Sync>>,
    ) -> Result<PlatformProof, ProposerError> {
        result
            .map_err(|e| ProposerError::Prover(format!("{platform} prover error: {e}")))
            .and_then(|result| PlatformProof::new(platform, result))
    }
}
