use std::time::SystemTime;

use alloy_primitives::{Address, B256};
use base_proof_tee_attestation::TeeAttestationKind;
use url::Url;

/// JSON-RPC attestation kind name returned by Nitro prover servers.
pub const NITRO_ATTESTATION_KIND: &str = "nitro";

/// JSON-RPC attestation kind name returned by TDX prover servers.
pub const TDX_ATTESTATION_KIND: &str = "tdx";

/// TEE attestation family exposed by a prover instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignerAttestationKind {
    /// AWS Nitro Enclave attestation documents.
    Nitro,
    /// Intel TDX signer quote attestations.
    Tdx,
}

impl SignerAttestationKind {
    /// Parses the JSON-RPC attestation kind returned by a prover server.
    pub fn from_rpc_name(name: &str) -> std::result::Result<Self, String> {
        match name {
            NITRO_ATTESTATION_KIND => Ok(Self::Nitro),
            TDX_ATTESTATION_KIND => Ok(Self::Tdx),
            other => Err(format!("unsupported attestation kind: {other}")),
        }
    }

    /// Returns the JSON-RPC attestation kind name for this family.
    pub const fn rpc_name(&self) -> &'static str {
        match self {
            Self::Nitro => NITRO_ATTESTATION_KIND,
            Self::Tdx => TDX_ATTESTATION_KIND,
        }
    }

    /// Returns whether this RPC-advertised kind matches a generated proof kind.
    pub const fn matches_proof_kind(&self, proof_kind: &TeeAttestationKind) -> bool {
        matches!(
            (self, proof_kind),
            (Self::Nitro, TeeAttestationKind::Nitro) | (Self::Tdx, TeeAttestationKind::Tdx)
        )
    }
}

/// A prover instance discovered from the infrastructure layer.
#[derive(Debug, Clone)]
pub struct ProverInstance {
    /// EC2 instance ID (e.g. `i-0abc123def456`).
    pub instance_id: String,
    /// HTTP endpoint URL for the prover (e.g. `http://10.0.1.5:8000/`).
    pub endpoint: Url,
    /// Current health status of the instance.
    pub health_status: InstanceHealthStatus,
    /// EC2 launch time of the instance. Used to determine if recently-launched
    /// unhealthy instances should still be eligible for registration.
    pub launch_time: Option<SystemTime>,
}

/// Health status of a discovered prover instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceHealthStatus {
    /// ALB health checks are in progress — instance just started.
    Initial,
    /// Instance is reachable and passing health checks.
    Healthy,
    /// Instance did not respond to the poll or is failing health checks.
    Unhealthy,
    /// ALB is draining connections from this instance.
    Draining,
}

impl InstanceHealthStatus {
    /// Returns `true` if the instance should be registered on-chain.
    ///
    /// Both `Initial` (AWS warm-up) and `Healthy` instances are candidates for
    /// registration. `Unhealthy` and `Draining` instances are not.
    pub const fn should_register(&self) -> bool {
        matches!(self, Self::Initial | Self::Healthy)
    }

    /// Maps an AWS ELB target health state string to [`InstanceHealthStatus`].
    ///
    /// Used by `AwsTargetGroupDiscovery` to convert `describe_target_health` responses.
    pub fn from_aws_state(state: &str) -> Self {
        match state {
            "initial" => Self::Initial,
            "healthy" => Self::Healthy,
            "draining" => Self::Draining,
            _ => Self::Unhealthy,
        }
    }
}

/// A signer currently registered on-chain via `TEEProverRegistry`.
#[derive(Debug, Clone)]
pub struct RegisteredSigner {
    /// The signer's Ethereum address.
    pub address: Address,
    /// The `keccak256(PCR0)` measurement hash the signer was registered under.
    pub pcr0: B256,
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::initial(InstanceHealthStatus::Initial, true)]
    #[case::healthy(InstanceHealthStatus::Healthy, true)]
    #[case::unhealthy(InstanceHealthStatus::Unhealthy, false)]
    #[case::draining(InstanceHealthStatus::Draining, false)]
    fn should_register(#[case] status: InstanceHealthStatus, #[case] expected: bool) {
        assert_eq!(status.should_register(), expected);
    }

    #[rstest]
    #[case::initial("initial", InstanceHealthStatus::Initial)]
    #[case::healthy("healthy", InstanceHealthStatus::Healthy)]
    #[case::draining("draining", InstanceHealthStatus::Draining)]
    #[case::unhealthy("unhealthy", InstanceHealthStatus::Unhealthy)]
    #[case::unavailable("unavailable", InstanceHealthStatus::Unhealthy)]
    #[case::empty("", InstanceHealthStatus::Unhealthy)]
    #[case::bogus("bogus", InstanceHealthStatus::Unhealthy)]
    fn from_aws_state(#[case] input: &str, #[case] expected: InstanceHealthStatus) {
        assert_eq!(InstanceHealthStatus::from_aws_state(input), expected);
    }

    #[rstest]
    #[case::nitro(NITRO_ATTESTATION_KIND, SignerAttestationKind::Nitro)]
    #[case::tdx(TDX_ATTESTATION_KIND, SignerAttestationKind::Tdx)]
    fn signer_attestation_kind_from_rpc_name(
        #[case] input: &str,
        #[case] expected: SignerAttestationKind,
    ) {
        assert_eq!(SignerAttestationKind::from_rpc_name(input).unwrap(), expected);
        assert_eq!(expected.rpc_name(), input);
    }

    #[test]
    fn signer_attestation_kind_rejects_unknown_rpc_name() {
        let error = SignerAttestationKind::from_rpc_name("sev").unwrap_err();

        assert!(error.contains("unsupported attestation kind"));
    }
}
