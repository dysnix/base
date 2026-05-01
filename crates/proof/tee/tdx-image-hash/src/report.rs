//! Report types and display formatting for TDX image hash inspection.

use std::fmt;

use alloy_primitives::{Address, B256};

/// TDX measurement values that explain the image-hash derivation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TdxMeasurementsReport {
    /// Keccak256 hash of the MRTD measurement.
    pub mr_td_hash: B256,
    /// Raw RTMR0 measurement.
    pub rtmr0: [u8; 48],
    /// Raw RTMR1 measurement.
    pub rtmr1: [u8; 48],
    /// Raw RTMR2 measurement.
    pub rtmr2: [u8; 48],
    /// Raw RTMR3 measurement.
    pub rtmr3: [u8; 48],
    /// Contract-compatible TDX image hash.
    pub image_hash: B256,
    /// Report-data suffix committed by the prover quote.
    pub report_data_suffix: B256,
    /// Quote timestamp in milliseconds since Unix epoch.
    pub quote_timestamp_millis: u64,
}

/// Local quote verification result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuoteVerificationReport {
    /// Journal image hash emitted by local TDX verification.
    pub journal_image_hash: B256,
    /// Journal MRTD hash emitted by local TDX verification.
    pub journal_mr_td_hash: B256,
    /// Earliest accepted collateral expiration in seconds since Unix epoch.
    pub collateral_expiration: u64,
}

/// Optional on-chain registry comparison result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnchainRegistryReport {
    /// Queried registry contract address.
    pub registry_address: Address,
    /// Registered image hash stored for this signer.
    pub signer_image_hash: B256,
    /// Expected image hash read from the current `AggregateVerifier`.
    pub expected_image_hash: B256,
    /// Whether the registry reports the signer as registered.
    pub is_registered_signer: bool,
    /// Whether the registry preflight accepts the signer.
    pub is_valid_signer: bool,
}

impl OnchainRegistryReport {
    /// Validates registry state against the locally computed image hash.
    pub fn validate_against(&self, image_hash: B256) -> eyre::Result<()> {
        if self.is_registered_signer && self.signer_image_hash != image_hash {
            eyre::bail!(
                "registered signerImageHash {} does not match computed imageHash {}",
                self.signer_image_hash,
                image_hash
            );
        }

        let expected_valid = self.is_registered_signer && self.expected_image_hash == image_hash;
        if self.is_valid_signer != expected_valid {
            eyre::bail!(
                "isValidSigner returned {}, expected {} from registration and expected image hash state",
                self.is_valid_signer,
                expected_valid
            );
        }

        Ok(())
    }
}

/// Full TDX image hash inspection report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TdxImageHashReport {
    /// Signer address derived from the TDX public key.
    pub signer_address: Address,
    /// TDX quote measurement report.
    pub measurements: TdxMeasurementsReport,
    /// Optional local quote verification result.
    pub quote_verification: Option<QuoteVerificationReport>,
    /// Optional on-chain registry comparison result.
    pub registry: Option<OnchainRegistryReport>,
}

impl fmt::Display for TdxImageHashReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Signer address: {}", self.signer_address)?;
        writeln!(f, "MRTD hash: {}", self.measurements.mr_td_hash)?;
        writeln!(f, "RTMR0: 0x{}", hex::encode(self.measurements.rtmr0))?;
        writeln!(f, "RTMR1: 0x{}", hex::encode(self.measurements.rtmr1))?;
        writeln!(f, "RTMR2: 0x{}", hex::encode(self.measurements.rtmr2))?;
        writeln!(f, "RTMR3: 0x{}", hex::encode(self.measurements.rtmr3))?;
        writeln!(f, "imageHash: {}", self.measurements.image_hash)?;
        writeln!(f, "Report-data suffix: {}", self.measurements.report_data_suffix)?;
        writeln!(f, "Quote timestamp millis: {}", self.measurements.quote_timestamp_millis)?;
        writeln!(
            f,
            "AggregateVerifier.TEE_IMAGE_HASH for TDX must equal imageHash, not the raw MRTD hash."
        )?;

        if let Some(verification) = &self.quote_verification {
            writeln!(f, "Quote verification: success")?;
            writeln!(f, "Journal imageHash: {}", verification.journal_image_hash)?;
            writeln!(f, "Journal MRTD hash: {}", verification.journal_mr_td_hash)?;
            writeln!(f, "Collateral expiration: {}", verification.collateral_expiration)?;
        } else {
            writeln!(f, "Quote verification: not requested")?;
        }

        if let Some(registry) = &self.registry {
            writeln!(f, "Registry address: {}", registry.registry_address)?;
            writeln!(f, "Registry signerImageHash: {}", registry.signer_image_hash)?;
            writeln!(f, "Registry expected TEE_IMAGE_HASH: {}", registry.expected_image_hash)?;
            writeln!(f, "Registry isRegisteredSigner: {}", registry.is_registered_signer)?;
            writeln!(f, "Registry isValidSigner: {}", registry.is_valid_signer)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use alloy_primitives::B256;

    use super::*;

    #[test]
    fn registry_report_accepts_matching_registered_signer() {
        let image_hash = B256::repeat_byte(0x11);
        let report = OnchainRegistryReport {
            registry_address: Address::ZERO,
            signer_image_hash: image_hash,
            expected_image_hash: image_hash,
            is_registered_signer: true,
            is_valid_signer: true,
        };

        report.validate_against(image_hash).unwrap();
    }

    #[test]
    fn registry_report_rejects_registered_image_hash_mismatch() {
        let report = OnchainRegistryReport {
            registry_address: Address::ZERO,
            signer_image_hash: B256::repeat_byte(0x11),
            expected_image_hash: B256::repeat_byte(0x22),
            is_registered_signer: true,
            is_valid_signer: false,
        };

        let error = report.validate_against(B256::repeat_byte(0x33)).unwrap_err();

        assert!(error.to_string().contains("signerImageHash"));
    }

    #[test]
    fn display_documents_tdx_tee_image_hash_meaning() {
        let image_hash = B256::repeat_byte(0x11);
        let report = TdxImageHashReport {
            signer_address: Address::ZERO,
            measurements: TdxMeasurementsReport {
                mr_td_hash: B256::repeat_byte(0x22),
                rtmr0: [0x33; 48],
                rtmr1: [0x44; 48],
                rtmr2: [0x55; 48],
                rtmr3: [0x66; 48],
                image_hash,
                report_data_suffix: B256::repeat_byte(0x77),
                quote_timestamp_millis: 1,
            },
            quote_verification: None,
            registry: None,
        };

        let rendered = report.to_string();

        assert!(rendered.contains("AggregateVerifier.TEE_IMAGE_HASH"));
        assert!(rendered.contains("imageHash"));
        assert!(rendered.contains(&image_hash.to_string()));
    }
}
