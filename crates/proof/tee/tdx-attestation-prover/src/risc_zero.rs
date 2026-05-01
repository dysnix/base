//! RISC Zero TDX attestation proving backend.

use std::{fmt, sync::Arc};

use alloy_primitives::{Address, Bytes};
use async_trait::async_trait;
use base_proof_contracts::ZkCoProcessorType;
use base_proof_tee_attestation::{
    TeeAttestationKind, TeeAttestationProof, TeeAttestationProofProvider,
};
use base_proof_tee_tdx_verifier::TdxVerifierInput;
use risc0_zkvm::{ExecutorEnv, ProverOpts, compute_image_id, default_prover};

use crate::{DirectProver, ProverError, Result, TdxAttestationProverInput};

/// Attestation prover using the RISC Zero default prover.
pub struct RiscZeroProver {
    elf: Arc<[u8]>,
    image_id: [u32; 8],
}

impl RiscZeroProver {
    /// Creates a new RISC Zero prover from raw guest ELF bytes.
    pub fn new(elf: Vec<u8>) -> Result<Self> {
        let digest = compute_image_id(&elf)
            .map_err(|e| ProverError::ImageId(format!("failed to compute image ID: {e}")))?;
        let image_id: [u32; 8] = digest.into();

        Ok(Self { elf: Arc::from(elf), image_id })
    }

    /// Returns the computed image ID for this guest ELF.
    pub const fn image_id(&self) -> &[u32; 8] {
        &self.image_id
    }

    /// Generates a RISC Zero Groth16 TDX attestation proof from an explicit verifier input.
    pub async fn generate_proof(&self, input: &TdxVerifierInput) -> Result<TeeAttestationProof> {
        let elf = Arc::clone(&self.elf);
        let input_bytes = TdxAttestationProverInput::new(input.clone()).encode();

        let (journal_bytes, seal) = tokio::task::spawn_blocking(move || {
            let env = ExecutorEnv::builder()
                .write_slice(&input_bytes)
                .build()
                .map_err(|e| ProverError::Risc0(format!("failed to build executor env: {e}")))?;

            let prover = default_prover();
            let prove_info = prover
                .prove_with_opts(env, &elf, &ProverOpts::groth16())
                .map_err(|e| ProverError::Risc0(format!("proving failed: {e}")))?;

            let journal = prove_info.receipt.journal.bytes.clone();
            let seal = risc0_ethereum_contracts::encode_seal(&prove_info.receipt)
                .map_err(|e| ProverError::Risc0(format!("failed to encode seal: {e}")))?;

            Ok::<_, ProverError>((journal, seal))
        })
        .await
        .map_err(|e| ProverError::Risc0(format!("proving task panicked: {e}")))??;

        Ok(TeeAttestationProof {
            kind: TeeAttestationKind::Tdx { zk_coprocessor: ZkCoProcessorType::RiscZero },
            output: Bytes::from(journal_bytes),
            proof_bytes: Bytes::from(seal),
        })
    }

    /// Decodes a provider payload and verifies it targets `signer_address`.
    pub fn decode_input_for_signer(
        attestation_bytes: &[u8],
        signer_address: Address,
    ) -> Result<TdxAttestationProverInput> {
        let input = TdxAttestationProverInput::decode(attestation_bytes)?;
        if input.expected_signer() != signer_address {
            return Err(ProverError::SignerMismatch {
                expected: signer_address,
                actual: input.expected_signer(),
            });
        }
        Ok(input)
    }
}

impl fmt::Debug for RiscZeroProver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RiscZeroProver")
            .field("image_id", &self.image_id)
            .field("elf_len", &self.elf.len())
            .finish()
    }
}

#[async_trait]
impl TeeAttestationProofProvider for RiscZeroProver {
    async fn generate_proof_for_signer(
        &self,
        attestation_bytes: &[u8],
        signer_address: Address,
    ) -> base_proof_tee_attestation::Result<TeeAttestationProof> {
        DirectProver::validate_zk_coprocessor(ZkCoProcessorType::RiscZero)?;
        let input = Self::decode_input_for_signer(attestation_bytes, signer_address)?;
        Ok(self.generate_proof(input.verifier_input()).await?)
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn new_with_invalid_elf_returns_image_id_error() {
        let result = RiscZeroProver::new(vec![0xDE, 0xAD, 0xBE, 0xEF]);

        assert!(matches!(result, Err(ProverError::ImageId(_))));
    }
}
