# TDX TEE Prover Support Plan

## Scope

This plan covers the offchain work needed to support Intel TDX TEE provers with the contracts currently staged in:

```text
/Users/jackchuma/projects/active/base-chain/check-contracts-claim/contracts
```

The Solidity path adds:

- `TDXVerifier`, which verifies a RISC Zero or SP1 proof whose public values are an ABI-encoded `TDXVerifierJournal`.
- `TDXTEEProverRegistry`, which extends `TEEProverRegistry` with `registerTDXSigner(bytes output, ZkCoProcessorType zkCoprocessor, bytes proofBytes)`.
- The existing `TEEVerifier` proposal path remains unchanged. TDX changes signer registration and image-hash derivation, not proposal proof bytes.

The current offchain baseline is Nitro-only:

- Prover runtime: `crates/proof/tee/nitro-enclave`, `crates/proof/tee/nitro-host`, `bin/prover/nitro-*`.
- Attestation proving: `crates/proof/tee/nitro-attestation-prover`.
- Registration: `crates/proof/tee/registrar`, `bin/prover-registrar`.
- Contract bindings: `crates/proof/contracts`.

## Implementation Steps

### 1. Sync TDX Contract Bindings Into `base-proof-contracts`

Add Solidity-aligned Rust bindings for the TDX ABI surface from the contracts branch.

Actions:

- Add `ZkCoProcessorType` and `ZkCoProcessorConfig` to a shared binding module if they stay shared between Nitro and TDX.
- Add `TDXVerificationResult`, `TDXTcbStatus`, and `TDXVerifierJournal` with enum ordering exactly matching `interfaces/multiproof/tee/ITDXVerifier.sol`.
- Add `ITDXVerifier` bindings for `verify`, `getZkConfig`, and `allowedTcbStatuses`.
- Extend or add a registry binding for `TDXTEEProverRegistry.registerTDXSigner(bytes,uint8,bytes)`.
- Keep existing Nitro `ITEEProverRegistry.registerSigner(bytes,bytes)` bindings unchanged.

Success criteria:

- `cargo test -p base-proof-contracts` passes.
- Unit tests prove `TDXVerifierJournal` ABI encode/decode round-trips.
- Unit tests assert all TDX enum discriminants match Solidity ordering.
- Unit tests assert the generated `registerTDXSigner` selector matches the compiled Solidity ABI from the contracts branch.
- No Nitro selector, type, or API test changes are required outside expected binding additions.

### 2. Introduce a Common TEE Registration Proof Model

Remove the registrar's dependency on Nitro-specific proof output types so Nitro and TDX can share the registration loop.

Actions:

- Add a small shared crate, `crates/proof/tee/attestation`, named `base-proof-tee-attestation`.
- Move the generic proof output trait shape out of `base-proof-tee-nitro-attestation-prover` into the shared crate.
- Define:

```rust
pub enum TeeAttestationKind {
    Nitro,
    Tdx { zk_coprocessor: ZkCoProcessorType },
}

pub struct TeeAttestationProof {
    pub kind: TeeAttestationKind,
    pub output: Bytes,
    pub proof_bytes: Bytes,
}

#[async_trait]
pub trait TeeAttestationProofProvider: Send + Sync {
    async fn generate_proof_for_signer(
        &self,
        attestation_bytes: &[u8],
        signer_address: Address,
    ) -> Result<TeeAttestationProof>;

    fn block_recovery_for_signer(&self, signer: Address) {}
}
```

- Update `base-proof-tee-nitro-attestation-prover` to implement `TeeAttestationProofProvider` and return `TeeAttestationKind::Nitro`.
- Keep backwards-compatible type aliases in the Nitro prover crate only if needed to avoid a large mechanical change.
- Update `crates/proof/tee/registrar` to depend on the shared trait instead of the Nitro prover crate.

Success criteria:

- `cargo test -p base-proof-tee-attestation` passes.
- `cargo test -p base-proof-tee-nitro-attestation-prover` passes.
- `cargo test -p base-proof-tee-registrar` passes without importing Nitro prover types in registrar core modules.
- The registrar can still encode Nitro `registerSigner(output, proofBytes)` calldata from a `TeeAttestationKind::Nitro` proof.

### 3. Add TDX Quote and Journal Verification Logic

Create pure verification logic that can be compiled into a ZK guest and can also be tested natively.

Actions:

- Add `crates/proof/tee/tdx-verifier`, named `base-proof-tee-tdx-verifier`.
- Define a verifier input type containing:
  - Raw TDX quote bytes.
  - PCK certificate chain.
  - TCB info collateral and signing chain.
  - QE identity collateral and signing chain.
  - CRLs or equivalent revocation evidence.
  - Trusted Intel root CA hash.
  - Expected public key binding inputs.
  - Verification time.
- Parse the quote body and extract MRTD, RTMR0, RTMR1, RTMR2, RTMR3, report data, quote timestamp, and attestation key data.
- Verify the TDX quote signature, PCK certificate chain, TCB info, QE identity, collateral expiration, and revocation evidence.
- Map Intel TCB status values into the contract's `TDXTcbStatus` enum.
- Compute the contract-compatible image hash:

```text
keccak256(MRTD || RTMR0 || RTMR1 || RTMR2 || RTMR3)
```

- Compute `mrTdHash = keccak256(MRTD)`.
- Verify `TDREPORT.REPORTDATA[0..32] == keccak256(public_key[1..65])`.
- Emit a `TDXVerifierJournal` matching `ITDXVerifier.sol`.

Success criteria:

- Native tests pass for at least one known-good TDX quote fixture and collateral bundle.
- Native tests cover failures for bad quote signature, wrong root CA hash, expired collateral, revoked collateral, timestamp outside policy, unsupported TCB status, malformed public key, signer mismatch, and report-data mismatch.
- The encoded journal produced by Rust decodes correctly with the Solidity ABI types added in step 1.
- The crate is usable from a ZK guest without network or filesystem access; all quote collateral is provided as explicit input.
- The crate has no dependency on the registrar, host server, or transaction manager crates.

### 4. Add TDX Attestation Collection Runtime

Add runtime code that runs inside a TDX guest and returns signer identity plus a fresh TDX quote.

Actions:

- Add `crates/proof/tee/tdx-runtime`, named `base-proof-tee-tdx-runtime`.
- Generate or load the secp256k1 signer key inside the TDX guest.
- Derive the uncompressed 65-byte public key and Ethereum signer address the same way Nitro does.
- Build TDREPORT report data as:

```text
first 32 bytes  = keccak256(public_key[1..65])
last 32 bytes   = app-specific binding data, initially keccak256("base-tdx-tee-prover-v1")
```

- Generate a TDX quote behind a narrow `TdxQuoteProvider` trait. The initial implementation target is the Linux guest TSM/configfs quote path; any DCAP/QGS FFI needed by the deployment must be implemented as a second provider behind the same trait.
- Return the raw quote and any local quote-generation metadata needed by the verifier input builder.
- Add a deterministic mock quote provider for local tests and CI.

Success criteria:

- On a real TDX guest, a smoke test can return a quote for the generated signer public key.
- The quote's report-data prefix matches `keccak256(public_key[1..65])`.
- The runtime rejects quote generation when report data is not exactly 64 bytes.
- Local tests can run without TDX hardware by using fixture quote bytes.
- Signer key material is never logged and is not exposed through any RPC endpoint.

### 5. Add a TDX Prover Server and Binary

Expose the existing prover RPC surface for a TDX backend.

Actions:

- Add `crates/proof/tee/tdx-prover`, named `base-proof-tee-tdx-prover`.
- Add `bin/prover/tdx`, with binary glue only.
- Reuse the existing JSON-RPC namespaces where possible:
  - `prover_prove` for proof requests.
  - `enclave_signerPublicKey` for signer public key.
  - `enclave_signerAttestation` for raw TDX quote bytes when the binary is configured for TDX.
- Add an explicit `enclave_attestationKind` or equivalent version method so operators and the registrar can fail fast if a TDX registrar points at a Nitro prover or vice versa.
- Reuse the existing proof pipeline and signature format.
- Set `ProofJournal.tee_image_hash` to the TDX image hash computed from the current quote measurements.
- Keep proposal proof bytes unchanged: `proposer(20) || signature(65)`.

Success criteria:

- `cargo run -p base-prover-tdx -- --help` works.
- Local mock mode serves `enclave_signerPublicKey`, `enclave_attestationKind`, `enclave_signerAttestation`, and `prover_prove`.
- The TDX server signs the same `ProofJournal` bytes that `TEEVerifier` expects.
- A unit test proves TDX `ProofJournal.tee_image_hash` equals the journal image hash emitted by the TDX attestation verifier for the same quote.
- Existing Nitro host/enclave tests continue to pass.

### 6. Add TDX ZK Attestation Proving

Build the offchain prover that transforms a TDX quote and collateral bundle into `TDXVerifier.registerTDXSigner` inputs.

Actions:

- Add `crates/proof/tee/tdx-attestation-prover`, named `base-proof-tee-tdx-attestation-prover`.
- Add a ZK guest that:
  - Reads the explicit TDX verifier input.
  - Calls `base-proof-tee-tdx-verifier`.
  - Commits the ABI-encoded `TDXVerifierJournal` as public output.
- Add a direct prover path for local/dev mode.
- Add a production RISC Zero prover path first, reusing the existing Boundless-style flow where possible.
- Treat SP1 support as a follow-up implementation unless launch configuration explicitly selects `ZkCoProcessorType.Succinct` before this step starts.
- Return `TeeAttestationProof { kind: TeeAttestationKind::Tdx { zk_coprocessor }, output, proof_bytes }`.
- Add recovery logic equivalent to the Nitro Boundless provider if the proving backend has long-running requests.

Success criteria:

- Dev-mode proving returns a proof and ABI-encoded `TDXVerifierJournal`.
- The returned proof kind includes the exact `ZkCoProcessorType` that the deployed `TDXVerifier` is configured to accept.
- A local Solidity test or Anvil script accepts the generated `(output, zkCoprocessor, proofBytes)` against a mock or real verifier configured with the same verifier ID.
- Recovered in-flight proofs are skipped if their quote timestamp is too old for the verifier's `maxTimeDiff`.
- `cargo test -p base-proof-tee-tdx-attestation-prover` passes without requiring TDX hardware.

### 7. Update the Registrar for TDX

Make the registrar platform-aware while preserving Nitro behavior.

Actions:

- Add `--tee-platform nitro|tdx`.
- Add `--tdx-zk-coprocessor risc-zero|succinct`.
- Add TDX-specific proving config:
  - TDX verifier guest ELF/program URL.
  - Verifier image ID/program ID.
  - Collateral fetch/config source.
  - Maximum recovered quote age.
- Rename generic prover-program arguments only if the old Nitro names can remain backwards-compatible.
- Add static endpoint discovery in addition to AWS target group discovery:
  - `--discovery-mode aws-target-group|static`.
  - `--prover-endpoint` repeatable for static mode.
- Keep AWS target group discovery for current Nitro deployments.
- When `TeeAttestationKind::Nitro`, submit `registerSigner(output, proofBytes)`.
- When `TeeAttestationKind::Tdx`, submit `registerTDXSigner(output, zkCoprocessor, proofBytes)`.
- Disable Nitro CRL revocation transactions for TDX. TDX collateral and revocation checks must be proven in the TDX verifier guest and represented in `TDXVerifierJournal`.
- Fail fast when `enclave_attestationKind` does not match `--tee-platform`.

Success criteria:

- Registrar CLI parsing tests cover Nitro, TDX/RISC Zero, TDX/SP1, AWS discovery, and static discovery.
- Registrar driver tests assert the exact calldata selector for Nitro and TDX registration.
- Registrar driver tests prove TDX deregistration still uses the shared `deregisterSigner(address)` path.
- Existing Nitro registrar tests pass unchanged or with only expected constructor/config updates.
- Running `base-proof-tee-registrar --tee-platform tdx --help` shows all required TDX configuration.

### 8. Add TDX Collateral Retrieval and Caching

Provide the TDX prover with all collateral needed by the ZK guest without allowing network access inside the guest.

Actions:

- Add a host-side `TdxCollateralProvider` that fetches and caches:
  - PCK certificate chain.
  - TCB info.
  - QE identity.
  - CRLs or revocation-equivalent data.
- Key cache entries by issuer, FMSPC, CA, collateral version, and expiration.
- Validate collateral freshness before submitting a proof request.
- Record the earliest accepted collateral expiration into `TDXVerifierJournal.collateralExpiration`.
- Add explicit configuration for trusted Intel root CA hash and collateral endpoint/PCCS base URL.
- Treat unavailable collateral as a registration failure, not as a fail-open path.

Success criteria:

- Unit tests cover cache hit, cache miss, expired collateral, malformed collateral, and root mismatch.
- The prover never performs network access inside the ZK guest.
- A stale collateral bundle cannot produce a journal with `result == Success`.
- Metrics expose collateral fetch failures and earliest collateral expiration.

### 9. Add TDX Image Hash Tooling

Make the TDX image hash observable before using it in `AggregateVerifier.TEE_IMAGE_HASH`.

Actions:

- Add a `base-proof-tee-tdx-image-hash` binary that queries a TDX prover endpoint and prints:
  - Signer address.
  - MRTD hash.
  - RTMR0-RTMR3 values or hashes.
  - Computed `imageHash`.
  - Report-data suffix.
  - Quote timestamp.
- Add an option to verify the quote locally with the same collateral provider used by the registrar.
- Document that the `AggregateVerifier.TEE_IMAGE_HASH` for TDX must equal the journal `imageHash`, not the raw MRTD hash.

Success criteria:

- Operators can run one command against a TDX prover and obtain the exact `TEE_IMAGE_HASH` value to deploy/configure.
- The tool exits non-zero if quote verification fails.
- The printed `imageHash` matches the value stored in `signerImageHash` after registration.
- The proposer preflight `isValidSigner` check succeeds only when the deployed `AggregateVerifier.TEE_IMAGE_HASH` matches this value.

### 10. Update Proposal and Health Paths

Verify that the existing proposal flow works for both Nitro and TDX signers.

Actions:

- Keep proposer config unchanged unless a clearer TDX-specific label is needed in docs; it already checks `TEEProverRegistry.isValidSigner`.
- Keep `TEEVerifier` proof bytes unchanged.
- Ensure TDX prover health can be gated on `isValidSigner` the same way Nitro health is gated.
- Add platform labels to health and metrics output.
- Add failure messages that distinguish "registered but wrong image hash" from "not registered".

Success criteria:

- Existing proposer tests pass.
- A local integration test can register a TDX signer in a mock registry and then pass health gating.
- A signer registered under one image hash is rejected when the configured game type expects a different image hash.
- Health output identifies the active TEE platform without exposing keys or quote bytes.

### 11. Add End-to-End Tests

Cover the full TDX offchain flow before attempting hardware deployment.

Actions:

- Add a pure Rust test that runs:
  - Fixture quote and collateral.
  - TDX verifier.
  - ABI journal encoding.
  - TDX registration calldata encoding.
- Add an Anvil/Forge integration test against the contracts branch that runs:
  - Deploy `TDXVerifier`.
  - Deploy `TDXTEEProverRegistry`.
  - Configure `proofSubmitter`.
  - Register a TDX signer.
  - Assert `isRegisteredSigner`, `signerImageHash`, and `isValidSigner`.
- Add a local mock prover/registrar integration test using static discovery.
- Add a hardware smoke test script for a TDX guest with QGS/PCCS configured.

Success criteria:

- `cargo test -p base-proof-contracts -p base-proof-tee-attestation -p base-proof-tee-tdx-verifier -p base-proof-tee-tdx-attestation-prover -p base-proof-tee-registrar` passes.
- Contract integration tests pass against the exact contracts tree referenced in this plan.
- The local mock prover/registrar test submits `registerTDXSigner` calldata.
- The hardware smoke test registers a real TDX signer and `isValidSigner(real_signer) == true`.

### 12. Roll Out Behind Explicit Configuration

Ship TDX support without changing default Nitro behavior.

Actions:

- Keep Nitro as the default platform where a default is required.
- Require `--tee-platform tdx` for all TDX-specific behavior.
- Gate new TDX crates and heavy proving dependencies behind features where possible.
- Add deployment docs for:
  - TDX prover runtime.
  - Registrar static discovery.
  - TDX verifier program IDs.
  - Intel root CA hash.
  - Allowed TCB statuses.
  - Collateral/PCCS configuration.
  - Image hash extraction.
- Add a canary runbook:
  - Register one TDX signer.
  - Verify `isValidSigner`.
  - Submit a single TEE proposal.
  - Confirm challenge/proposer monitoring sees the same game metadata as Nitro.

Success criteria:

- Existing Nitro deployment commands still work.
- TDX deployment commands are documented with every required address and secret.
- Canary registration and one proposal succeed before any production rollout.
- Rollback is documented as deregistering TDX signers and switching the proposer back to Nitro endpoints.

## Open Decisions

These decisions must be closed before starting implementation steps 3-8. If a decision is not closed, the default listed here is the implementation target for the first pass.

- Quote collection ABI. Default: Linux TSM/configfs provider first, DCAP/QGS FFI provider only if the target kernels require it.
- ZK coprocessor. Default: RISC Zero first, SP1 after RISC Zero unless launch requires `ZkCoProcessorType.Succinct`.
- Signer key lifecycle. Default: ephemeral signer keys matching the current Nitro behavior; sealed or KMS-backed keys require an explicit follow-up design.
- Accepted TCB statuses. Default: `UpToDate` only for first pass; add other statuses only with a deployment policy decision.
- Discovery backend. Default: static endpoint discovery for TDX plus existing AWS target group discovery for Nitro.

## Reference Files

- `crates/proof/tee/registrar/src/driver.rs`
- `crates/proof/tee/registrar/src/prover.rs`
- `crates/proof/tee/nitro-attestation-prover/src/types.rs`
- `crates/proof/tee/nitro-enclave/src/server.rs`
- `crates/proof/tee/nitro-host/src/server.rs`
- `crates/proof/contracts/src/tee_prover_registry.rs`
- `/Users/jackchuma/projects/active/base-chain/check-contracts-claim/contracts/src/multiproof/tee/TDXVerifier.sol`
- `/Users/jackchuma/projects/active/base-chain/check-contracts-claim/contracts/src/multiproof/tee/TDXTEEProverRegistry.sol`
- `/Users/jackchuma/projects/active/base-chain/check-contracts-claim/contracts/interfaces/multiproof/tee/ITDXVerifier.sol`
- `/Users/jackchuma/projects/active/base-chain/check-contracts-claim/contracts/scripts/multiproof/README.md`
