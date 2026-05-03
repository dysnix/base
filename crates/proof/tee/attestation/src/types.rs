//! Shared TEE attestation proof types and provider trait.

use std::{error::Error, fmt};

use alloy_primitives::{Address, Bytes};
use async_trait::async_trait;

/// Boxed error type used by platform-neutral attestation providers.
pub type BoxError = Box<dyn Error + Send + Sync>;

/// Convenience result alias for platform-neutral attestation providers.
pub type Result<T, E = BoxError> = std::result::Result<T, E>;

/// Supported TEE attestation proof families.
#[derive(Clone, Copy)]
pub enum TeeAttestationKind {
    /// AWS Nitro Enclave attestation proof.
    Nitro,
    /// Intel TDX attestation proof.
    Tdx,
}

impl fmt::Debug for TeeAttestationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Nitro => f.write_str("Nitro"),
            Self::Tdx => f.write_str("Tdx"),
        }
    }
}

impl PartialEq for TeeAttestationKind {
    fn eq(&self, other: &Self) -> bool {
        matches!((self, other), (Self::Nitro, Self::Nitro) | (Self::Tdx, Self::Tdx))
    }
}

impl Eq for TeeAttestationKind {}

/// A generated TEE attestation proof ready for on-chain signer registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeeAttestationProof {
    /// TEE platform and verifier flavor for this proof.
    pub kind: TeeAttestationKind,
    /// ABI-encoded verifier journal containing the verified attestation data.
    pub output: Bytes,
    /// ZK proof bytes for on-chain verification.
    pub proof_bytes: Bytes,
}

/// Trait for generating TEE attestation proofs for signer registration.
#[async_trait]
pub trait TeeAttestationProofProvider: Send + Sync {
    /// Generates a ZK proof for the given raw attestation bytes and signer.
    async fn generate_proof_for_signer(
        &self,
        attestation_bytes: &[u8],
        signer_address: Address,
    ) -> Result<TeeAttestationProof>;

    /// Marks a signer's recovered proof as failed on-chain.
    ///
    /// Implementations that support proof recovery should skip recovery for
    /// this signer on subsequent calls and generate a fresh proof instead.
    /// Implementations without recovery support can use the default no-op.
    fn block_recovery_for_signer(&self, _signer: Address) {}
}

#[async_trait]
impl TeeAttestationProofProvider for Box<dyn TeeAttestationProofProvider> {
    async fn generate_proof_for_signer(
        &self,
        attestation_bytes: &[u8],
        signer_address: Address,
    ) -> Result<TeeAttestationProof> {
        (**self).generate_proof_for_signer(attestation_bytes, signer_address).await
    }

    fn block_recovery_for_signer(&self, signer: Address) {
        (**self).block_recovery_for_signer(signer);
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use alloy_primitives::Address;
    use async_trait::async_trait;
    use rstest::rstest;

    use super::*;

    /// Synthetic signer addresses for trait-delegation tests.
    const SIGNER_A: Address = Address::repeat_byte(0xAA);
    const SIGNER_B: Address = Address::repeat_byte(0xBB);

    /// Stub attestation input bytes.
    const STUB_ATTESTATION: &[u8] = b"attestation-data";

    /// Stub seal returned by test providers.
    const STUB_SEAL: &[u8] = b"stub-seal";

    /// Stub provider whose output includes the attestation bytes and signer
    /// address so callers can verify dynamic dispatch preserved both inputs.
    struct StubProvider;

    #[async_trait]
    impl TeeAttestationProofProvider for StubProvider {
        async fn generate_proof_for_signer(
            &self,
            attestation_bytes: &[u8],
            signer_address: Address,
        ) -> Result<TeeAttestationProof> {
            let mut output = attestation_bytes.to_vec();
            output.extend_from_slice(signer_address.as_slice());
            Ok(TeeAttestationProof {
                kind: TeeAttestationKind::Nitro,
                output: Bytes::from(output),
                proof_bytes: Bytes::from_static(STUB_SEAL),
            })
        }
    }

    /// Provider that records recovery blocks for forwarding tests.
    struct RecoveryAwareProvider {
        signer: std::sync::Mutex<Option<Address>>,
    }

    impl RecoveryAwareProvider {
        /// Creates a provider with no blocked signer.
        const fn new() -> Self {
            Self { signer: std::sync::Mutex::new(None) }
        }

        /// Returns the last signer passed to `block_recovery_for_signer`.
        fn blocked_signer(&self) -> Option<Address> {
            *self.signer.lock().unwrap_or_else(|e| e.into_inner())
        }
    }

    #[async_trait]
    impl TeeAttestationProofProvider for RecoveryAwareProvider {
        async fn generate_proof_for_signer(
            &self,
            _attestation_bytes: &[u8],
            _signer_address: Address,
        ) -> Result<TeeAttestationProof> {
            Err(Box::new(io::Error::other("unused")))
        }

        fn block_recovery_for_signer(&self, signer: Address) {
            *self.signer.lock().unwrap_or_else(|e| e.into_inner()) = Some(signer);
        }
    }

    #[rstest]
    fn proof_fields_accessible() {
        let output = Bytes::from_static(b"journal-data");
        let proof_bytes = Bytes::from_static(b"seal-data");

        let proof = TeeAttestationProof {
            kind: TeeAttestationKind::Nitro,
            output: output.clone(),
            proof_bytes: proof_bytes.clone(),
        };

        assert_eq!(proof.kind, TeeAttestationKind::Nitro);
        assert_eq!(proof.output, output);
        assert_eq!(proof.proof_bytes, proof_bytes);
    }

    #[rstest]
    fn tdx_kind_is_distinct() {
        let kind = TeeAttestationKind::Tdx;

        assert_eq!(kind, TeeAttestationKind::Tdx);
    }

    #[rstest]
    fn proof_clone() {
        let proof = TeeAttestationProof {
            kind: TeeAttestationKind::Nitro,
            output: Bytes::from_static(b"j"),
            proof_bytes: Bytes::from_static(b"s"),
        };
        let cloned = proof.clone();

        assert_eq!(proof, cloned);
    }

    #[rstest]
    fn proof_debug_format() {
        let proof = TeeAttestationProof {
            kind: TeeAttestationKind::Nitro,
            output: Bytes::new(),
            proof_bytes: Bytes::new(),
        };
        let debug = format!("{proof:?}");

        assert!(debug.contains("TeeAttestationProof"));
        assert!(debug.contains("Nitro"));
    }

    #[rstest]
    #[case::signer_a(SIGNER_A)]
    #[case::signer_b(SIGNER_B)]
    #[case::zero_address(Address::ZERO)]
    #[tokio::test]
    async fn provider_receives_attestation_and_signer(#[case] signer: Address) {
        let provider = StubProvider;
        let proof = provider.generate_proof_for_signer(STUB_ATTESTATION, signer).await.unwrap();

        let mut expected = STUB_ATTESTATION.to_vec();
        expected.extend_from_slice(signer.as_slice());
        assert_eq!(proof.output, Bytes::from(expected));
    }

    #[rstest]
    #[tokio::test]
    async fn boxed_provider_delegates_generate_proof_for_signer() {
        let provider: Box<dyn TeeAttestationProofProvider> = Box::new(StubProvider);
        let proof = provider.generate_proof_for_signer(STUB_ATTESTATION, SIGNER_A).await.unwrap();

        let mut expected = STUB_ATTESTATION.to_vec();
        expected.extend_from_slice(SIGNER_A.as_slice());
        assert_eq!(proof.output, Bytes::from(expected));
    }

    #[rstest]
    fn recovery_block_records_signer() {
        let provider = RecoveryAwareProvider::new();

        provider.block_recovery_for_signer(SIGNER_A);

        assert_eq!(provider.blocked_signer(), Some(SIGNER_A));
    }
}
