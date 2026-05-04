use alloy_primitives::{B256, keccak256};

use crate::{Result, TdxRuntimeError};

/// Length of `TDREPORT.REPORTDATA`.
pub const TDX_REPORT_DATA_LEN: usize = 64;

/// Domain separator used in the app-specific report-data suffix.
pub const TDX_REPORT_DATA_SUFFIX_CONTEXT: &[u8] = b"base-tdx-tee-prover-v1";

/// Helper for constructing TDX `TDREPORT.REPORTDATA`.
#[derive(Debug)]
pub struct TdxReportData;

impl TdxReportData {
    /// Builds the report-data bytes expected by the TDX verifier.
    ///
    /// The first 32 bytes are `keccak256(public_key[1..65])`. The last 32
    /// bytes bind the app context and quote collection timestamp.
    pub fn for_public_key(public_key: &[u8], quote_timestamp_millis: u64) -> Result<[u8; 64]> {
        let mut report_data = [0u8; TDX_REPORT_DATA_LEN];
        report_data[..32].copy_from_slice(Self::public_key_prefix(public_key)?.as_slice());
        report_data[32..]
            .copy_from_slice(Self::timestamped_app_binding(quote_timestamp_millis).as_slice());
        Ok(report_data)
    }

    /// Computes `keccak256(public_key[1..65])`.
    pub fn public_key_prefix(public_key: &[u8]) -> Result<B256> {
        if public_key.len() != 65 || public_key.first() != Some(&0x04) {
            return Err(TdxRuntimeError::InvalidPublicKey);
        }
        Ok(keccak256(&public_key[1..65]))
    }

    /// Computes the timestamp-bound app-specific suffix.
    pub fn timestamped_app_binding(quote_timestamp_millis: u64) -> B256 {
        const CONTEXT_LEN: usize = TDX_REPORT_DATA_SUFFIX_CONTEXT.len();
        let mut buf = [0u8; CONTEXT_LEN + 8];
        buf[..CONTEXT_LEN].copy_from_slice(TDX_REPORT_DATA_SUFFIX_CONTEXT);
        buf[CONTEXT_LEN..].copy_from_slice(&quote_timestamp_millis.to_le_bytes());
        keccak256(buf)
    }

    /// Validates a runtime-supplied report-data buffer.
    pub const fn validate(report_data: &[u8]) -> Result<()> {
        if report_data.len() != TDX_REPORT_DATA_LEN {
            return Err(TdxRuntimeError::InvalidReportDataLength(report_data.len()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use alloy_primitives::keccak256;

    use super::*;
    use crate::TdxSigner;

    const TEST_KEY: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
    const TIMESTAMP_MILLIS: u64 = 1_711_111_111_000;

    #[test]
    fn report_data_binds_public_key_prefix_and_app_suffix() {
        let signer = TdxSigner::from_hex(TEST_KEY).unwrap();
        let public_key = signer.public_key();
        let report_data = TdxReportData::for_public_key(&public_key, TIMESTAMP_MILLIS).unwrap();

        assert_eq!(&report_data[..32], keccak256(&public_key[1..65]).as_slice());
        assert_eq!(
            &report_data[32..],
            TdxReportData::timestamped_app_binding(TIMESTAMP_MILLIS).as_slice()
        );
    }

    #[test]
    fn report_data_rejects_invalid_lengths() {
        assert!(matches!(
            TdxReportData::validate(&[0u8; 63]),
            Err(TdxRuntimeError::InvalidReportDataLength(63))
        ));
        assert!(matches!(
            TdxReportData::validate(&[0u8; 65]),
            Err(TdxRuntimeError::InvalidReportDataLength(65))
        ));
    }

    #[test]
    fn public_key_prefix_rejects_malformed_keys() {
        assert!(matches!(
            TdxReportData::public_key_prefix(&[0u8; 64]),
            Err(TdxRuntimeError::InvalidPublicKey)
        ));
    }
}
