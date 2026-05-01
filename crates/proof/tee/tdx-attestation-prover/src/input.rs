//! ABI-compatible host and guest input encoding for TDX attestation proving.

use std::fmt;

use alloy_primitives::Address;
use alloy_sol_types::{SolValue, sol};
use base_proof_contracts::TDXTcbStatus;
use base_proof_tee_tdx_verifier::{
    IntelTcbStatus, TdxCertificate, TdxCertificateRevocationList, TdxCollateral, TdxQuotePolicy,
    TdxRevocationEvidence, TdxSignedCollateral, TdxVerifierInput,
};

use crate::{ProverError, Result};

sol! {
    /// ABI mirror of `TdxCertificate` for deterministic host/guest input encoding.
    struct TdxCertificateInput {
        /// Raw DER certificate bytes.
        bytes raw;
        /// DER certificate serial bytes.
        bytes serial;
        /// Uncompressed P-256 subject public key.
        bytes subjectPublicKey;
        /// Uncompressed P-256 issuer public key.
        bytes issuerPublicKey;
        /// Certificate validity start time in seconds since Unix epoch.
        uint64 notBefore;
        /// Certificate validity end time in seconds since Unix epoch.
        uint64 notAfter;
        /// Whether this certificate can issue child certificates.
        bool isCa;
        /// DER-encoded `TBSCertificate` bytes.
        bytes tbsCertificate;
        /// P-256 ECDSA signature over the TBS certificate bytes.
        bytes signature;
    }

    /// ABI mirror of `TdxSignedCollateral`.
    struct TdxSignedCollateralInput {
        /// Raw collateral document bytes.
        bytes raw;
        /// Root-to-leaf signing certificate chain.
        TdxCertificateInput[] signingChain;
        /// P-256 ECDSA signature over the signed collateral body.
        bytes signature;
        /// Collateral issue time in seconds since Unix epoch.
        uint64 issueTime;
        /// Collateral expiration time in seconds since Unix epoch.
        uint64 nextUpdate;
    }

    /// ABI mirror of `TdxCollateral`.
    struct TdxCollateralInput {
        /// TCB info collateral and signing chain.
        TdxSignedCollateralInput tcbInfo;
        /// QE identity collateral and signing chain.
        TdxSignedCollateralInput qeIdentity;
        /// Intel TCB status hint retained for lossless host-side round trips.
        uint8 tcbStatus;
    }

    /// ABI mirror of `TdxCertificateRevocationList`.
    struct TdxCertificateRevocationListInput {
        /// Raw DER CRL bytes.
        bytes raw;
    }

    /// ABI mirror of `TdxRevocationEvidence`.
    struct TdxRevocationEvidenceInput {
        /// DER X.509 CRLs for all non-root certificate issuers.
        TdxCertificateRevocationListInput[] certificateCrls;
    }

    /// ABI mirror of `TdxQuotePolicy`.
    struct TdxQuotePolicyInput {
        /// Maximum accepted quote age in seconds.
        uint64 maxQuoteAgeSeconds;
    }

    /// Complete explicit TDX verifier input encoded for a RISC Zero guest.
    struct TdxVerifierInputAbi {
        /// Raw Intel TDX quote bytes.
        bytes quote;
        /// Root-to-leaf PCK certificate chain.
        TdxCertificateInput[] pckCertificateChain;
        /// TCB info and QE identity collateral.
        TdxCollateralInput collateral;
        /// Certificate revocation evidence.
        TdxRevocationEvidenceInput revocation;
        /// Trusted Intel root CA hash.
        bytes32 trustedRootCaHash;
        /// Expected uncompressed secp256k1 signer public key.
        bytes expectedPublicKey;
        /// Expected Ethereum signer address.
        address expectedSigner;
        /// Quote collection timestamp in milliseconds since Unix epoch.
        uint64 quoteTimestampMillis;
        /// Verification time in seconds since Unix epoch.
        uint64 verificationTime;
        /// Quote timestamp policy.
        TdxQuotePolicyInput policy;
        /// Contract TCB statuses accepted by verifier policy.
        uint8[] allowedTcbStatuses;
    }
}

/// Explicit TDX attestation prover input.
#[derive(Clone)]
pub struct TdxAttestationProverInput {
    /// Complete input consumed by `base-proof-tee-tdx-verifier`.
    pub verifier_input: TdxVerifierInput,
}

impl TdxAttestationProverInput {
    /// Creates a prover input from a verifier input.
    pub const fn new(verifier_input: TdxVerifierInput) -> Self {
        Self { verifier_input }
    }

    /// Returns the signer committed by the verifier input.
    pub const fn expected_signer(&self) -> Address {
        self.verifier_input.expected_signer
    }

    /// Returns the quote timestamp committed by the verifier input.
    pub const fn quote_timestamp_millis(&self) -> u64 {
        self.verifier_input.quote_timestamp_millis
    }

    /// Returns a shared reference to the verifier input.
    pub const fn verifier_input(&self) -> &TdxVerifierInput {
        &self.verifier_input
    }

    /// Consumes this wrapper and returns the verifier input.
    pub fn into_verifier_input(self) -> TdxVerifierInput {
        self.verifier_input
    }

    /// ABI-encodes this input for host-to-guest transport.
    pub fn encode(&self) -> Vec<u8> {
        let abi = TdxVerifierInputAbi::from(&self.verifier_input);
        SolValue::abi_encode(&abi)
    }

    /// ABI-decodes a host-to-guest TDX verifier input.
    pub fn decode(buf: &[u8]) -> Result<Self> {
        let abi = <TdxVerifierInputAbi as SolValue>::abi_decode_validate(buf)
            .map_err(|e| ProverError::InputDecode(e.to_string()))?;
        Ok(Self { verifier_input: TdxVerifierInput::try_from(abi)? })
    }
}

impl fmt::Debug for TdxAttestationProverInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TdxAttestationProverInput")
            .field("verifier_input", &self.verifier_input)
            .finish()
    }
}

impl From<&TdxVerifierInput> for TdxVerifierInputAbi {
    fn from(input: &TdxVerifierInput) -> Self {
        Self {
            quote: input.quote.clone(),
            pckCertificateChain: input.pck_certificate_chain.iter().map(Into::into).collect(),
            collateral: (&input.collateral).into(),
            revocation: (&input.revocation).into(),
            trustedRootCaHash: input.trusted_root_ca_hash,
            expectedPublicKey: input.expected_public_key.clone(),
            expectedSigner: input.expected_signer,
            quoteTimestampMillis: input.quote_timestamp_millis,
            verificationTime: input.verification_time,
            policy: (&input.policy).into(),
            allowedTcbStatuses: input
                .allowed_tcb_statuses
                .iter()
                .map(|status| *status as u8)
                .collect(),
        }
    }
}

impl TryFrom<TdxVerifierInputAbi> for TdxVerifierInput {
    type Error = ProverError;

    fn try_from(input: TdxVerifierInputAbi) -> Result<Self> {
        Ok(Self {
            quote: input.quote,
            pck_certificate_chain: input
                .pckCertificateChain
                .into_iter()
                .map(TdxCertificate::from)
                .collect(),
            collateral: TdxCollateral::try_from(input.collateral)?,
            revocation: TdxRevocationEvidence::from(input.revocation),
            trusted_root_ca_hash: input.trustedRootCaHash,
            expected_public_key: input.expectedPublicKey,
            expected_signer: input.expectedSigner,
            quote_timestamp_millis: input.quoteTimestampMillis,
            verification_time: input.verificationTime,
            policy: TdxQuotePolicy::from(input.policy),
            allowed_tcb_statuses: input
                .allowedTcbStatuses
                .into_iter()
                .map(tdx_tcb_status_from_u8)
                .collect::<Result<Vec<_>>>()?,
        })
    }
}

impl From<&TdxCertificate> for TdxCertificateInput {
    fn from(certificate: &TdxCertificate) -> Self {
        Self {
            raw: certificate.raw.clone(),
            serial: certificate.serial.clone(),
            subjectPublicKey: certificate.subject_public_key.clone(),
            issuerPublicKey: certificate.issuer_public_key.clone(),
            notBefore: certificate.not_before,
            notAfter: certificate.not_after,
            isCa: certificate.is_ca,
            tbsCertificate: certificate.tbs_certificate.clone(),
            signature: certificate.signature.clone(),
        }
    }
}

impl From<TdxCertificateInput> for TdxCertificate {
    fn from(certificate: TdxCertificateInput) -> Self {
        Self {
            raw: certificate.raw,
            serial: certificate.serial,
            subject_public_key: certificate.subjectPublicKey,
            issuer_public_key: certificate.issuerPublicKey,
            not_before: certificate.notBefore,
            not_after: certificate.notAfter,
            is_ca: certificate.isCa,
            tbs_certificate: certificate.tbsCertificate,
            signature: certificate.signature,
        }
    }
}

impl From<&TdxSignedCollateral> for TdxSignedCollateralInput {
    fn from(collateral: &TdxSignedCollateral) -> Self {
        Self {
            raw: collateral.raw.clone(),
            signingChain: collateral.signing_chain.iter().map(Into::into).collect(),
            signature: collateral.signature.clone(),
            issueTime: collateral.issue_time,
            nextUpdate: collateral.next_update,
        }
    }
}

impl From<TdxSignedCollateralInput> for TdxSignedCollateral {
    fn from(collateral: TdxSignedCollateralInput) -> Self {
        Self {
            raw: collateral.raw,
            signing_chain: collateral.signingChain.into_iter().map(TdxCertificate::from).collect(),
            signature: collateral.signature,
            issue_time: collateral.issueTime,
            next_update: collateral.nextUpdate,
        }
    }
}

impl From<&TdxCollateral> for TdxCollateralInput {
    fn from(collateral: &TdxCollateral) -> Self {
        Self {
            tcbInfo: (&collateral.tcb_info).into(),
            qeIdentity: (&collateral.qe_identity).into(),
            tcbStatus: intel_tcb_status_to_u8(collateral.tcb_status),
        }
    }
}

impl TryFrom<TdxCollateralInput> for TdxCollateral {
    type Error = ProverError;

    fn try_from(collateral: TdxCollateralInput) -> Result<Self> {
        Ok(Self {
            tcb_info: collateral.tcbInfo.into(),
            qe_identity: collateral.qeIdentity.into(),
            tcb_status: intel_tcb_status_from_u8(collateral.tcbStatus)?,
        })
    }
}

impl From<&TdxCertificateRevocationList> for TdxCertificateRevocationListInput {
    fn from(crl: &TdxCertificateRevocationList) -> Self {
        Self { raw: crl.raw.clone() }
    }
}

impl From<TdxCertificateRevocationListInput> for TdxCertificateRevocationList {
    fn from(crl: TdxCertificateRevocationListInput) -> Self {
        Self { raw: crl.raw }
    }
}

impl From<&TdxRevocationEvidence> for TdxRevocationEvidenceInput {
    fn from(evidence: &TdxRevocationEvidence) -> Self {
        Self { certificateCrls: evidence.certificate_crls.iter().map(Into::into).collect() }
    }
}

impl From<TdxRevocationEvidenceInput> for TdxRevocationEvidence {
    fn from(evidence: TdxRevocationEvidenceInput) -> Self {
        Self {
            certificate_crls: evidence
                .certificateCrls
                .into_iter()
                .map(TdxCertificateRevocationList::from)
                .collect(),
        }
    }
}

impl From<&TdxQuotePolicy> for TdxQuotePolicyInput {
    fn from(policy: &TdxQuotePolicy) -> Self {
        Self { maxQuoteAgeSeconds: policy.max_quote_age_seconds }
    }
}

impl From<TdxQuotePolicyInput> for TdxQuotePolicy {
    fn from(policy: TdxQuotePolicyInput) -> Self {
        Self { max_quote_age_seconds: policy.maxQuoteAgeSeconds }
    }
}

/// Converts a contract TDX TCB status discriminant into a typed status.
pub fn tdx_tcb_status_from_u8(status: u8) -> Result<TDXTcbStatus> {
    match status {
        value if value == TDXTcbStatus::Unknown as u8 => Ok(TDXTcbStatus::Unknown),
        value if value == TDXTcbStatus::UpToDate as u8 => Ok(TDXTcbStatus::UpToDate),
        value if value == TDXTcbStatus::SwHardeningNeeded as u8 => {
            Ok(TDXTcbStatus::SwHardeningNeeded)
        }
        value if value == TDXTcbStatus::ConfigurationNeeded as u8 => {
            Ok(TDXTcbStatus::ConfigurationNeeded)
        }
        value if value == TDXTcbStatus::ConfigurationAndSwHardeningNeeded as u8 => {
            Ok(TDXTcbStatus::ConfigurationAndSwHardeningNeeded)
        }
        value if value == TDXTcbStatus::OutOfDate as u8 => Ok(TDXTcbStatus::OutOfDate),
        value if value == TDXTcbStatus::OutOfDateConfigurationNeeded as u8 => {
            Ok(TDXTcbStatus::OutOfDateConfigurationNeeded)
        }
        value if value == TDXTcbStatus::Revoked as u8 => Ok(TDXTcbStatus::Revoked),
        value => {
            Err(ProverError::InputDecode(format!("invalid TDX TCB status discriminant: {value}")))
        }
    }
}

/// Converts an Intel TCB status into a stable input discriminant.
pub const fn intel_tcb_status_to_u8(status: IntelTcbStatus) -> u8 {
    match status {
        IntelTcbStatus::UpToDate => 1,
        IntelTcbStatus::SwHardeningNeeded => 2,
        IntelTcbStatus::ConfigurationNeeded => 3,
        IntelTcbStatus::ConfigurationAndSwHardeningNeeded => 4,
        IntelTcbStatus::OutOfDate => 5,
        IntelTcbStatus::OutOfDateConfigurationNeeded => 6,
        IntelTcbStatus::Revoked => 7,
        IntelTcbStatus::Unsupported => 255,
    }
}

/// Converts an input discriminant into an Intel TCB status.
pub fn intel_tcb_status_from_u8(status: u8) -> Result<IntelTcbStatus> {
    match status {
        1 => Ok(IntelTcbStatus::UpToDate),
        2 => Ok(IntelTcbStatus::SwHardeningNeeded),
        3 => Ok(IntelTcbStatus::ConfigurationNeeded),
        4 => Ok(IntelTcbStatus::ConfigurationAndSwHardeningNeeded),
        5 => Ok(IntelTcbStatus::OutOfDate),
        6 => Ok(IntelTcbStatus::OutOfDateConfigurationNeeded),
        7 => Ok(IntelTcbStatus::Revoked),
        255 => Ok(IntelTcbStatus::Unsupported),
        value => {
            Err(ProverError::InputDecode(format!("invalid Intel TCB status discriminant: {value}")))
        }
    }
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{B256, Bytes};
    use base_proof_contracts::TDXTcbStatus;
    use rstest::rstest;

    use super::*;

    const SIGNER: Address = Address::repeat_byte(0x44);

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
            expected_signer: SIGNER,
            quote_timestamp_millis: 1_711_111_111_000,
            verification_time: 1_711_111_222,
            policy: TdxQuotePolicy { max_quote_age_seconds: 300 },
            allowed_tcb_statuses: vec![TDXTcbStatus::UpToDate, TDXTcbStatus::SwHardeningNeeded],
        }
    }

    #[rstest]
    fn prover_input_abi_round_trips() {
        let input = TdxAttestationProverInput::new(verifier_input());
        let decoded = TdxAttestationProverInput::decode(&input.encode()).unwrap();

        assert_eq!(decoded.verifier_input.quote, input.verifier_input.quote);
        assert_eq!(
            decoded.verifier_input.pck_certificate_chain,
            input.verifier_input.pck_certificate_chain
        );
        assert_eq!(decoded.verifier_input.collateral, input.verifier_input.collateral);
        assert_eq!(decoded.verifier_input.revocation, input.verifier_input.revocation);
        assert_eq!(
            decoded.verifier_input.trusted_root_ca_hash,
            input.verifier_input.trusted_root_ca_hash
        );
        assert_eq!(
            decoded.verifier_input.expected_public_key,
            input.verifier_input.expected_public_key
        );
        assert_eq!(decoded.verifier_input.expected_signer, input.verifier_input.expected_signer);
        assert_eq!(
            decoded.verifier_input.quote_timestamp_millis,
            input.verifier_input.quote_timestamp_millis
        );
        assert_eq!(
            decoded.verifier_input.verification_time,
            input.verifier_input.verification_time
        );
        assert_eq!(decoded.verifier_input.policy, input.verifier_input.policy);
        assert_eq!(
            decoded
                .verifier_input
                .allowed_tcb_statuses
                .iter()
                .map(|status| *status as u8)
                .collect::<Vec<_>>(),
            input
                .verifier_input
                .allowed_tcb_statuses
                .iter()
                .map(|status| *status as u8)
                .collect::<Vec<_>>()
        );
    }

    #[rstest]
    fn decode_rejects_invalid_status() {
        let mut abi = TdxVerifierInputAbi::from(&verifier_input());
        abi.allowedTcbStatuses = vec![200];
        let encoded = SolValue::abi_encode(&abi);

        assert!(matches!(
            TdxAttestationProverInput::decode(&encoded),
            Err(ProverError::InputDecode(_))
        ));
    }

    #[rstest]
    #[case(IntelTcbStatus::UpToDate, 1)]
    #[case(IntelTcbStatus::Revoked, 7)]
    #[case(IntelTcbStatus::Unsupported, 255)]
    fn intel_status_discriminants_round_trip(#[case] status: IntelTcbStatus, #[case] expected: u8) {
        assert_eq!(intel_tcb_status_to_u8(status), expected);
        assert_eq!(intel_tcb_status_from_u8(expected).unwrap(), status);
    }
}
