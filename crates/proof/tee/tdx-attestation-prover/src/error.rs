//! Error types for TDX attestation proof generation.

use thiserror::Error;

/// Errors that can occur during TDX attestation proof generation.
#[derive(Debug, Error)]
pub enum ProverError {
    /// The encoded TDX prover input is malformed.
    #[error("input decode error: {0}")]
    InputDecode(String),
    /// The underlying TDX verifier rejected the input.
    #[error("verifier error: {0}")]
    Verifier(#[from] base_proof_tee_tdx_verifier::TdxVerifierError),
    /// The decoded input signer does not match the signer being registered.
    #[error("signer mismatch: expected {expected}, got {actual}")]
    SignerMismatch {
        /// Signer supplied by the registrar.
        expected: alloy_primitives::Address,
        /// Signer committed by the TDX verifier input.
        actual: alloy_primitives::Address,
    },
    /// RISC Zero proving failed.
    #[error("risc0 error: {0}")]
    Risc0(String),
    /// Boundless marketplace interaction failed.
    #[error("boundless error: {0}")]
    Boundless(String),
    /// The guest ELF or image ID is invalid.
    #[error("image ID error: {0}")]
    ImageId(String),
}

/// Convenience result alias for TDX attestation prover operations.
pub type Result<T, E = ProverError> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use alloy_primitives::Address;
    use base_proof_tee_tdx_verifier::TdxVerifierError;
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn from_verifier_error() {
        let prover_err = ProverError::from(TdxVerifierError::MalformedPublicKey);

        assert!(matches!(prover_err, ProverError::Verifier(_)));
        assert!(prover_err.to_string().contains("malformed"));
    }

    #[rstest]
    fn signer_mismatch_display_includes_addresses() {
        let expected = Address::repeat_byte(0x11);
        let actual = Address::repeat_byte(0x22);
        let error = ProverError::SignerMismatch { expected, actual };
        let message = error.to_string();

        assert!(message.contains(&expected.to_string()));
        assert!(message.contains(&actual.to_string()));
    }
}
