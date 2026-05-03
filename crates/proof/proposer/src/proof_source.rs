//! TEE proof source types for platform-specific prover fleets.

use std::{error::Error as StdError, fmt, sync::Arc};

use alloy_primitives::Bytes;
use base_proof_primitives::{
    CryptoError, ProofEncoder, ProofRequest, ProofResult, Proposal, ProverClient,
};
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
    /// Aggregate proposal signed by the platform prover.
    pub aggregate_proposal: Proposal,
    /// Per-block proposals signed by the platform prover.
    pub proposals: Vec<Proposal>,
}

impl PlatformProof {
    /// Creates a platform proof and rejects non-TEE prover responses.
    pub fn new(platform: TeeProofPlatform, result: ProofResult) -> Result<Self, ProposerError> {
        match result {
            ProofResult::Tee { aggregate_proposal, proposals } => {
                Ok(Self { platform, aggregate_proposal, proposals })
            }
            ProofResult::Zk { .. } => Err(ProposerError::Prover(format!(
                "{platform} prover returned unexpected ZK proof result"
            ))),
        }
    }
}

/// TEE proofs for one proposal input.
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

    /// Returns the Nitro proof used for canonical proposal fields and intermediate roots.
    pub const fn submission_proof(&self) -> &PlatformProof {
        &self.nitro
    }

    /// Returns every platform proof that was actually sourced.
    pub const fn platform_proofs(&self) -> [&PlatformProof; 2] {
        [&self.nitro, &self.tdx]
    }

    /// Builds the proof bytes submitted to `AggregateVerifier.initializeWithInitData()`.
    ///
    /// The proof layout is:
    /// `proofType(1) || l1OriginHash(32) || l1OriginNumber(32) || nitroSignature(65) || tdxSignature(65)`.
    pub fn build_proof_data(&self) -> Result<Bytes, CryptoError> {
        let nitro_aggregate = &self.nitro.aggregate_proposal;
        let tdx_aggregate = &self.tdx.aggregate_proposal;

        ProofEncoder::encode_dual_tee_proof_bytes(
            &nitro_aggregate.signature,
            &tdx_aggregate.signature,
            nitro_aggregate.l1_origin_hash,
            nitro_aggregate.l1_origin_number,
        )
    }

    /// Ensures configured platform proofs are for the same proposal input.
    pub fn validate_matching_payloads(&self) -> Result<(), ProposerError> {
        Self::validate_matching_proposal_payload(
            &self.nitro.aggregate_proposal,
            &self.tdx.aggregate_proposal,
            ProposalLabel::Aggregate,
        )?;

        if self.nitro.proposals.len() != self.tdx.proposals.len() {
            return Err(ProposerError::Prover(format!(
                "nitro and tdx proof proposal counts differ: nitro={}, tdx={}",
                self.nitro.proposals.len(),
                self.tdx.proposals.len()
            )));
        }

        for (index, (nitro_proposal, tdx_proposal)) in
            self.nitro.proposals.iter().zip(self.tdx.proposals.iter()).enumerate()
        {
            Self::validate_matching_proposal_payload(
                nitro_proposal,
                tdx_proposal,
                ProposalLabel::Block(index),
            )?;
        }

        Ok(())
    }

    fn validate_matching_proposal_payload(
        nitro: &Proposal,
        tdx: &Proposal,
        label: ProposalLabel,
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

#[derive(Debug, Clone, Copy)]
enum ProposalLabel {
    Aggregate,
    Block(usize),
}

impl fmt::Display for ProposalLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Aggregate => f.write_str("aggregate proposal"),
            Self::Block(index) => write!(f, "proposal {index}"),
        }
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

    /// Requests proofs from configured platform fleets for the same request.
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

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use alloy_primitives::{Address, B256};
    use base_proof_primitives::{
        DUAL_TEE_SIGNATURE_LENGTH, PROOF_TYPE_TEE, ProofRequest, ProofResult, ProverClient,
    };

    use super::*;
    use crate::test_utils::test_proposal;

    #[derive(Debug)]
    struct CountingProver {
        calls: Arc<AtomicUsize>,
    }

    #[derive(Debug)]
    struct FailingProver {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl ProverClient for CountingProver {
        async fn prove(
            &self,
            request: ProofRequest,
        ) -> Result<ProofResult, Box<dyn StdError + Send + Sync>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let aggregate_proposal = test_proposal(request.claimed_l2_block_number);
            let proposals = vec![aggregate_proposal.clone()];
            Ok(ProofResult::Tee { aggregate_proposal, proposals })
        }
    }

    #[async_trait::async_trait]
    impl ProverClient for FailingProver {
        async fn prove(
            &self,
            _request: ProofRequest,
        ) -> Result<ProofResult, Box<dyn StdError + Send + Sync>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err("unavailable".into())
        }
    }

    fn proof_request() -> ProofRequest {
        ProofRequest {
            l1_head: B256::repeat_byte(0x11),
            agreed_l2_head_hash: B256::repeat_byte(0x22),
            agreed_l2_output_root: B256::repeat_byte(0x33),
            claimed_l2_output_root: B256::repeat_byte(0x44),
            claimed_l2_block_number: 1,
            proposer: Address::repeat_byte(0x55),
            intermediate_block_interval: 1,
            l1_head_number: 100,
            image_hash: B256::repeat_byte(0x66),
        }
    }

    #[test]
    fn dual_platform_proof_builds_concatenated_submission_proof_data() {
        let mut nitro_proposal = test_proposal(1);
        nitro_proposal.signature =
            Bytes::from([vec![0xAA; 64], vec![0]].into_iter().flatten().collect::<Vec<_>>());
        let mut tdx_proposal = nitro_proposal.clone();
        tdx_proposal.signature =
            Bytes::from([vec![0xBB; 64], vec![1]].into_iter().flatten().collect::<Vec<_>>());
        let proof = DualPlatformProof::new(
            PlatformProof::new(
                TeeProofPlatform::Nitro,
                ProofResult::Tee {
                    aggregate_proposal: nitro_proposal.clone(),
                    proposals: vec![nitro_proposal],
                },
            )
            .unwrap(),
            PlatformProof::new(
                TeeProofPlatform::Tdx,
                ProofResult::Tee {
                    aggregate_proposal: tdx_proposal.clone(),
                    proposals: vec![tdx_proposal],
                },
            )
            .unwrap(),
        )
        .unwrap();

        let proof_data = proof.build_proof_data().unwrap();

        assert_eq!(proof_data.len(), 1 + 32 + 32 + DUAL_TEE_SIGNATURE_LENGTH);
        assert_eq!(proof_data[0], PROOF_TYPE_TEE);
        assert_eq!(proof_data[129], 27);
        assert_eq!(proof_data[194], 28);
    }

    #[tokio::test]
    async fn dual_sources_request_both_platforms() {
        let nitro_calls = Arc::new(AtomicUsize::new(0));
        let tdx_calls = Arc::new(AtomicUsize::new(0));
        let nitro: Arc<dyn ProverClient> =
            Arc::new(CountingProver { calls: Arc::clone(&nitro_calls) });
        let tdx: Arc<dyn ProverClient> = Arc::new(CountingProver { calls: Arc::clone(&tdx_calls) });
        let sources = TeeProofSources::new(nitro, tdx);

        let proof = sources.prove(proof_request()).await.unwrap();

        assert_eq!(nitro_calls.load(Ordering::SeqCst), 1);
        assert_eq!(tdx_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            proof.platform_proofs().map(|proof| proof.platform),
            [TeeProofPlatform::Nitro, TeeProofPlatform::Tdx]
        );
    }

    #[tokio::test]
    async fn tdx_failure_reports_platform_readiness() {
        let nitro_calls = Arc::new(AtomicUsize::new(0));
        let tdx_calls = Arc::new(AtomicUsize::new(0));
        let nitro: Arc<dyn ProverClient> =
            Arc::new(CountingProver { calls: Arc::clone(&nitro_calls) });
        let tdx: Arc<dyn ProverClient> = Arc::new(FailingProver { calls: Arc::clone(&tdx_calls) });
        let sources = TeeProofSources::new(nitro, tdx);

        let error = sources.prove(proof_request()).await.unwrap_err();

        assert_eq!(nitro_calls.load(Ordering::SeqCst), 1);
        assert_eq!(tdx_calls.load(Ordering::SeqCst), 1);
        assert!(matches!(error, TeeProofError::Platform { platform: TeeProofPlatform::Tdx, .. }));
        assert_eq!(
            error.platform_readiness(),
            [(TeeProofPlatform::Nitro, true), (TeeProofPlatform::Tdx, false)]
        );
    }
}
