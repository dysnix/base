//! Native direct TDX attestation proof generation for development and tests.

use std::{fmt, sync::Arc};

use alloy_primitives::{Address, Bytes};
use async_trait::async_trait;
use base_proof_contracts::TDXVerifierJournal;
use base_proof_tee_attestation::{
    TeeAttestationKind, TeeAttestationProof, TeeAttestationProofProvider,
};
use base_proof_tee_tdx_verifier::{TdxVerifier, TdxVerifierInput};

use crate::{ProverError, Result, TdxAttestationProverInput};

/// Default proof bytes used by the native direct prover.
pub const DIRECT_DEV_PROOF_BYTES: &[u8] = b"base-tdx-direct-dev-proof-v1";

/// Verifies a TDX attestation input into a `TDXVerifierJournal`.
pub trait TdxJournalVerifier: fmt::Debug + Send + Sync {
    /// Verifies the explicit TDX verifier input and returns the journal.
    fn verify(&self, input: &TdxVerifierInput) -> Result<TDXVerifierJournal>;
}

/// Journal verifier backed by `base-proof-tee-tdx-verifier`.
#[derive(Debug)]
pub struct NativeTdxJournalVerifier;

impl TdxJournalVerifier for NativeTdxJournalVerifier {
    fn verify(&self, input: &TdxVerifierInput) -> Result<TDXVerifierJournal> {
        Ok(TdxVerifier::verify(input)?)
    }
}

/// Native direct prover for local development.
///
/// This path runs the TDX verifier in-process and returns the ABI-encoded
/// journal with deterministic development proof bytes. It is intended for
/// local/mock verifier configurations and does not require TDX hardware.
pub struct DirectProver {
    proof_bytes: Bytes,
    verifier: Arc<dyn TdxJournalVerifier>,
}

impl DirectProver {
    /// Creates a direct prover using the native TDX journal verifier.
    pub fn new() -> Self {
        Self {
            proof_bytes: Bytes::from_static(DIRECT_DEV_PROOF_BYTES),
            verifier: Arc::new(NativeTdxJournalVerifier),
        }
    }

    /// Creates a direct prover with a custom verifier and proof bytes.
    pub fn with_verifier(proof_bytes: Bytes, verifier: Arc<dyn TdxJournalVerifier>) -> Self {
        Self { proof_bytes, verifier }
    }

    /// Generates a TDX attestation proof from an explicit verifier input.
    pub async fn generate_proof(&self, input: &TdxVerifierInput) -> Result<TeeAttestationProof> {
        let journal = self.verifier.verify(input)?;
        Ok(TeeAttestationProof {
            kind: TeeAttestationKind::Tdx,
            output: Bytes::from(TdxVerifier::encode_journal(&journal)),
            proof_bytes: self.proof_bytes.clone(),
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

impl Default for DirectProver {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for DirectProver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DirectProver")
            .field("proof_bytes_len", &self.proof_bytes.len())
            .field("verifier", &self.verifier)
            .finish()
    }
}

#[async_trait]
impl TeeAttestationProofProvider for DirectProver {
    async fn generate_proof_for_signer(
        &self,
        attestation_bytes: &[u8],
        signer_address: Address,
    ) -> base_proof_tee_attestation::Result<TeeAttestationProof> {
        let input = Self::decode_input_for_signer(attestation_bytes, signer_address)?;
        Ok(self.generate_proof(input.verifier_input()).await?)
    }
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{B256, Bytes};
    use alloy_sol_types::SolValue;
    use base_proof_contracts::{TDXTcbStatus, TDXVerificationResult};
    use base_proof_tee_tdx_verifier::{
        IntelTcbStatus, TdxCertificate, TdxCertificateRevocationList, TdxCollateral,
        TdxQuotePolicy, TdxRevocationEvidence, TdxSignedCollateral,
    };
    use rstest::rstest;

    use super::*;

    struct StaticJournalVerifier {
        journal: TDXVerifierJournal,
    }

    impl fmt::Debug for StaticJournalVerifier {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("StaticJournalVerifier").finish_non_exhaustive()
        }
    }

    impl TdxJournalVerifier for StaticJournalVerifier {
        fn verify(&self, _input: &TdxVerifierInput) -> Result<TDXVerifierJournal> {
            Ok(self.journal.clone())
        }
    }

    struct MockTdxVerifierContract;

    fn certificate(byte: u8) -> TdxCertificate {
        TdxCertificate {
            raw: Bytes::from(vec![byte; 3]),
            serial: Bytes::from(vec![byte; 2]),
            subject_public_key: Bytes::from(vec![0x04, byte]),
            issuer_public_key: Bytes::from(vec![0x04, byte.wrapping_add(1)]),
            not_before: 1_700_000_000,
            not_after: 1_800_000_000,
            is_ca: true,
            tbs_certificate: Bytes::from(vec![byte; 4]),
            signature: Bytes::from(vec![byte; 64]),
        }
    }

    fn signed_collateral(byte: u8) -> TdxSignedCollateral {
        TdxSignedCollateral {
            raw: Bytes::from(vec![byte; 5]),
            signing_chain: vec![certificate(byte)],
            signature: Bytes::from(vec![byte; 64]),
            issue_time: 1_700_000_000,
            next_update: 1_800_000_000,
        }
    }

    fn verifier_input() -> TdxVerifierInput {
        TdxVerifierInput {
            quote: Bytes::from_static(b"quote"),
            pck_certificate_chain: vec![certificate(0x11), certificate(0x22)],
            collateral: TdxCollateral {
                tcb_info: signed_collateral(0x33),
                qe_identity: signed_collateral(0x44),
                tcb_status: IntelTcbStatus::UpToDate,
            },
            revocation: TdxRevocationEvidence {
                certificate_crls: vec![TdxCertificateRevocationList {
                    raw: Bytes::from_static(b"crl"),
                }],
            },
            trusted_root_ca_hash: B256::repeat_byte(0x55),
            expected_public_key: Bytes::from(vec![0x04; 65]),
            expected_signer: Address::repeat_byte(0x44),
            quote_timestamp_millis: 1_711_111_111_000,
            verification_time: 1_711_111_222,
            policy: TdxQuotePolicy { max_quote_age_seconds: 300 },
            allowed_tcb_statuses: vec![TDXTcbStatus::UpToDate, TDXTcbStatus::SwHardeningNeeded],
        }
    }

    impl MockTdxVerifierContract {
        fn verify(&self, output: &[u8], proof_bytes: &[u8]) -> TDXVerifierJournal {
            assert!(!proof_bytes.is_empty());
            <TDXVerifierJournal as SolValue>::abi_decode_validate(output)
                .expect("mock verifier must decode ABI journal output")
        }
    }

    fn journal() -> TDXVerifierJournal {
        TDXVerifierJournal {
            result: TDXVerificationResult::Success,
            tcbStatus: TDXTcbStatus::UpToDate,
            timestamp: 1_711_111_111_000,
            collateralExpiration: 1_711_222_222,
            rootCaHash: B256::repeat_byte(0x11),
            pckCertHash: B256::repeat_byte(0x22),
            tcbInfoHash: B256::repeat_byte(0x33),
            qeIdentityHash: B256::repeat_byte(0x44),
            publicKey: Bytes::from(vec![0x04; 65]),
            signer: Address::repeat_byte(0x44),
            imageHash: B256::repeat_byte(0x55),
            mrTdHash: B256::repeat_byte(0x66),
            reportDataPrefix: B256::repeat_byte(0x77),
            reportDataSuffix: B256::repeat_byte(0x88),
        }
    }

    fn prover() -> DirectProver {
        DirectProver::with_verifier(
            Bytes::from_static(b"proof"),
            Arc::new(StaticJournalVerifier { journal: journal() }),
        )
    }

    #[rstest]
    #[tokio::test]
    async fn dev_mode_proving_returns_proof_and_abi_encoded_journal() {
        let input = TdxAttestationProverInput::new(verifier_input());
        let prover = prover();

        let proof = prover
            .generate_proof_for_signer(&input.encode(), input.expected_signer())
            .await
            .unwrap();

        assert_eq!(proof.kind, TeeAttestationKind::Tdx);
        assert_eq!(proof.proof_bytes, Bytes::from_static(b"proof"));

        let decoded = <TDXVerifierJournal as SolValue>::abi_decode_validate(&proof.output)
            .expect("direct prover output must be ABI-encoded journal");
        assert_eq!(decoded.result as u8, TDXVerificationResult::Success as u8);
        assert_eq!(decoded.signer, input.expected_signer());
    }

    #[tokio::test]
    async fn mock_solidity_verifier_accepts_generated_tuple() {
        let input = TdxAttestationProverInput::new(verifier_input());
        let prover = prover();
        let proof = prover
            .generate_proof_for_signer(&input.encode(), input.expected_signer())
            .await
            .unwrap();
        let mock = MockTdxVerifierContract;

        assert_eq!(proof.kind, TeeAttestationKind::Tdx);
        let decoded = mock.verify(&proof.output, &proof.proof_bytes);

        assert_eq!(decoded.signer, input.expected_signer());
    }

    #[rstest]
    #[tokio::test]
    async fn provider_rejects_mismatched_signer() {
        let input = TdxAttestationProverInput::new(verifier_input());
        let prover = prover();

        let error = prover
            .generate_proof_for_signer(&input.encode(), Address::repeat_byte(0x99))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("signer mismatch"));
    }
}
