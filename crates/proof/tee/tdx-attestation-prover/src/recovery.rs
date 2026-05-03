//! Recovery freshness checks for long-running TDX proof backends.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alloy_sol_types::SolValue;
use base_proof_contracts::TDXVerifierJournal;
use base_proof_tee_attestation::TeeAttestationProof;

/// Policy for accepting recovered TDX proofs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveredProofPolicy {
    /// Maximum recovered quote age accepted before submitting on-chain.
    pub max_recovered_quote_age: Duration,
}

impl RecoveredProofPolicy {
    /// Creates a new recovered proof freshness policy.
    pub const fn new(max_recovered_quote_age: Duration) -> Self {
        Self { max_recovered_quote_age }
    }

    /// Returns true when the recovered proof's journal timestamp is fresh at wall-clock time.
    pub fn is_fresh(&self, proof: &TeeAttestationProof) -> bool {
        let Some(now_millis) = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        else {
            return false;
        };
        self.is_fresh_at(proof, now_millis)
    }

    /// Returns true when the recovered proof's journal timestamp is fresh at `now_millis`.
    pub fn is_fresh_at(&self, proof: &TeeAttestationProof, now_millis: u64) -> bool {
        let Ok(journal) = <TDXVerifierJournal as SolValue>::abi_decode_validate(&proof.output)
        else {
            return false;
        };
        self.journal_is_fresh_at(&journal, now_millis)
    }

    /// Returns true when the recovered journal timestamp is fresh at wall-clock time.
    pub fn journal_is_fresh(&self, journal: &TDXVerifierJournal) -> bool {
        let Some(now_millis) = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        else {
            return false;
        };
        self.journal_is_fresh_at(journal, now_millis)
    }

    /// Returns true when the recovered journal timestamp is fresh at `now_millis`.
    pub fn journal_is_fresh_at(&self, journal: &TDXVerifierJournal, now_millis: u64) -> bool {
        let age = Duration::from_millis(now_millis.saturating_sub(journal.timestamp));
        age <= self.max_recovered_quote_age && journal.timestamp <= now_millis
    }
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{Address, B256, Bytes};
    use base_proof_contracts::{TDXTcbStatus, TDXVerificationResult};
    use base_proof_tee_attestation::TeeAttestationKind;
    use rstest::rstest;

    use super::*;

    const NOW_MILLIS: u64 = 1_711_111_111_000;

    fn proof(timestamp: u64) -> TeeAttestationProof {
        let journal = TDXVerifierJournal {
            result: TDXVerificationResult::Success,
            tcbStatus: TDXTcbStatus::UpToDate,
            timestamp,
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
        };
        TeeAttestationProof {
            kind: TeeAttestationKind::Tdx,
            output: Bytes::from(SolValue::abi_encode(&journal)),
            proof_bytes: Bytes::from_static(b"proof"),
        }
    }

    #[rstest]
    fn recovered_proof_with_fresh_quote_is_accepted() {
        let policy = RecoveredProofPolicy::new(Duration::from_secs(300));

        assert!(policy.is_fresh_at(&proof(NOW_MILLIS - 299_000), NOW_MILLIS));
    }

    #[rstest]
    fn recovered_proof_with_old_quote_is_skipped() {
        let policy = RecoveredProofPolicy::new(Duration::from_secs(300));

        assert!(!policy.is_fresh_at(&proof(NOW_MILLIS - 301_000), NOW_MILLIS));
    }

    #[rstest]
    fn recovered_proof_with_future_quote_is_skipped() {
        let policy = RecoveredProofPolicy::new(Duration::from_secs(300));

        assert!(!policy.is_fresh_at(&proof(NOW_MILLIS + 1), NOW_MILLIS));
    }

    #[rstest]
    fn recovered_proof_with_malformed_journal_is_skipped() {
        let policy = RecoveredProofPolicy::new(Duration::from_secs(300));
        let proof = TeeAttestationProof {
            kind: TeeAttestationKind::Tdx,
            output: Bytes::from_static(b"not abi"),
            proof_bytes: Bytes::from_static(b"proof"),
        };

        assert!(!policy.is_fresh_at(&proof, NOW_MILLIS));
    }
}
