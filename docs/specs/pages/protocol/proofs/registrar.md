# Registrar

The registrar is responsible for maintaining the onchain registry of accepted TEE signer identities.

The default registrar configuration discovers and registers the Nitro prover fleet. TDX registration
is additive: a TDX fleet is processed only when explicit `--tdx-*` discovery and proving flags are
present. A single registrar process can poll both fleets, validate each endpoint's
`enclave_attestationKind`, submit Nitro `registerSigner(output, proofBytes)` calldata, and submit TDX
`registerTDXSigner(output, proofBytes)` calldata.

Orphan cleanup is computed from the union of active Nitro and TDX signers. A signer is deregistered
only when it is absent from every configured healthy prover fleet.

For TDX static discovery, collateral policy, canary registration, and rollback commands, see
[TDX Deployment](./tdx-deployment).
