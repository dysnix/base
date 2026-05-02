# TDX Prover GCP End-to-End Test Plan

## Goal

Deploy one Intel TDX prover on GCP, validate that it can produce a real TDX
quote, verify that quote from the local development machine, register the
signer against test contracts, and then integrate the prover into the existing
registrar, proposer, and Nitro prover development environment.

This plan intentionally stages the work so each phase proves one new boundary:

- Local TDX software path.
- GCP TDX VM and real quote collection.
- Off-host quote and collateral verification.
- Local contract registration.
- Testnet registration.
- Full dev environment integration with both Nitro and TDX fleets.

## Assumptions

- TDX is an additional prover fleet, not a replacement for Nitro.
- The first hardware deployment uses static TDX discovery.
- The first hardware smoke test runs the TDX prover bound to `127.0.0.1` on the
  VM and accesses it through an SSH tunnel.
- The first onchain registration should use local contracts before testnet.
- Use `BASE_TDX_SIGNER_KEY` for POC stability. Omitting it generates an
  ephemeral signer, which is useful for smoke tests but not for comparing
  registration state across restarts.
- The TDX image hash used for `AggregateVerifier.TEE_IMAGE_HASH` is the TDX
  verifier journal `imageHash`, computed as:

```text
keccak256(MRTD || RTMR0 || RTMR1 || RTMR2 || RTMR3)
```

It is not raw MRTD and not `keccak256(MRTD)`.

## Reference Surfaces In This Repo

- TDX prover binary:
  `bin/prover/tdx`
- TDX prover server:
  `crates/proof/tee/tdx-prover`
- TDX runtime and configfs quote collection:
  `crates/proof/tee/tdx-runtime`
- TDX image hash inspection tool:
  `crates/proof/tee/tdx-image-hash`
- TDX verifier:
  `crates/proof/tee/tdx-verifier`
- TDX attestation prover:
  `crates/proof/tee/tdx-attestation-prover`
- Registrar TDX discovery/proving/collateral configuration:
  `bin/prover-registrar`
- Proposer Nitro plus optional TDX proof source configuration:
  `crates/proof/proposer`

## Phase 0: Local Baseline

Goal: prove the local software path works before debugging GCP, quote
collateral, contracts, or proposer integration.

### Run Focused Tests

```sh
cargo test -p base-proof-contracts \
  -p base-proof-tee-attestation \
  -p base-proof-tee-tdx-runtime \
  -p base-proof-tee-tdx-prover \
  -p base-proof-tee-tdx-verifier \
  -p base-proof-tee-tdx-attestation-prover \
  -p base-proof-tee-registrar
```

Expected:

- All focused tests pass locally.
- Existing Nitro behavior is not regressed.

### Start The Deterministic Local TDX Prover

```sh
cargo run -p base-prover-tdx -- local \
  --l1-eth-url "$L1_ETH_URL" \
  --l2-eth-url "$L2_ETH_URL" \
  --l1-beacon-url "$L1_BEACON_URL" \
  --l2-chain-id "$L2_CHAIN_ID" \
  --listen-addr 127.0.0.1:7310
```

### Smoke The RPC Surface

```sh
curl -s localhost:7310 \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"enclave_attestationKind","params":[]}'
```

```sh
curl -s localhost:7310 \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":2,"method":"enclave_signerPublicKey","params":[]}'
```

```sh
curl -s localhost:7310 \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":3,"method":"enclave_signerAttestation","params":[null,null]}'
```

Expected:

- `enclave_attestationKind` returns `"tdx"`.
- `enclave_signerPublicKey` returns one 65-byte uncompressed secp256k1 public
  key.
- `enclave_signerAttestation` returns one encoded `TdxSignerAttestation`.

### Inspect The Local Image Hash

```sh
cargo run -p base-proof-tee-tdx-image-hash -- \
  --endpoint http://127.0.0.1:7310
```

Expected:

- The tool prints signer identity, MRTD/RTMR-derived measurements, and the TDX
  contract-compatible `imageHash`.

### Gate

Continue only when:

- Local mock TDX prover starts.
- Required TDX RPC methods respond.
- Image hash inspection works.
- Focused tests pass.

## Phase 1: GCP TDX VM Bring-Up

Goal: prove a GCP Confidential VM can boot with Intel TDX and collect a real
TDX quote.

### Pick A Supported GCP Configuration

Use a GCP Confidential VM configuration that supports Intel TDX. As of this
plan, GCP documents Intel TDX support on `c3-standard-*` machine types with
TDX-capable guest images.

Before creating the VM:

```sh
gcloud services enable compute.googleapis.com
gcloud compute images list \
  --filter='guestOsFeatures[].type:(TDX_CAPABLE)'
```

For Ubuntu-based POCs, prefer a TDX-capable Ubuntu 24.04 LTS image when one is
available:

```sh
gcloud compute images describe-from-family ubuntu-2404-lts-amd64 \
  --project ubuntu-os-cloud \
  --format='json(selfLink,name,family,guestOsFeatures)'
```

Set:

```sh
export PROJECT_ID="..."
export ZONE="..."
export TDX_IMAGE="..."
export TDX_VM_NAME="base-tdx-prover-poc"
```

Choose `ZONE` to match both TDX-capable machine availability and the project
networking. Do not assume the configured default zone is usable. If the project
does not have a default VPC, list existing networks and regional subnets before
creating the VM:

```sh
gcloud compute networks list --project "$PROJECT_ID"
gcloud compute networks subnets list --project "$PROJECT_ID"
gcloud compute machine-types list \
  --project "$PROJECT_ID" \
  --filter='name=c3-standard-8'
```

Create the VM:

```sh
gcloud compute instances create "$TDX_VM_NAME" \
  --project "$PROJECT_ID" \
  --zone "$ZONE" \
  --machine-type c3-standard-8 \
  --confidential-compute-type TDX \
  --maintenance-policy TERMINATE \
  --image "$TDX_IMAGE" \
  --boot-disk-size 200GB \
  --tags base-tdx-prover
```

If the project has no default VPC, pass the explicit network and regional
subnet:

```sh
gcloud compute instances create "$TDX_VM_NAME" \
  --project "$PROJECT_ID" \
  --zone "$ZONE" \
  --machine-type c3-standard-8 \
  --confidential-compute-type TDX \
  --maintenance-policy TERMINATE \
  --image "$TDX_IMAGE" \
  --boot-disk-size 200GB \
  --network "$TDX_NETWORK" \
  --subnet "$TDX_SUBNET" \
  --tags base-tdx-prover
```

Keep the first POC closed to public inbound traffic. Use SSH tunneling for RPC:

```sh
gcloud compute ssh "$TDX_VM_NAME" --project "$PROJECT_ID" --zone "$ZONE" -- \
  -L 7310:127.0.0.1:7310
```

SSH must still be reachable through an approved operational path. Prefer VPN or
IAP access. If IAP is used, the caller needs permission to create IAP TCP
tunnels and the VPC needs an ingress rule from `35.235.240.0/20` to TCP port
22 for the VM tag. If a temporary public SSH rule is used for a smoke test, make
it tag-scoped and source-restricted to the operator's `/32`, then remove it
after the test.

### Verify TDX And TSM/configfs Inside The VM

```sh
sudo dmesg | grep -i tdx
sudo mount -t configfs none /sys/kernel/config || true
ls -la /sys/kernel/config/tsm/report
```

Expected:

- The kernel indicates TDX guest support.
- `/sys/kernel/config/tsm/report` exists.

### Build And Run The Real TDX Prover

Inside the VM:

```sh
git clone "$BASE_REPO_URL"
cd base
sudo apt-get update
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y \
  build-essential \
  clang \
  cmake \
  curl \
  git \
  libssl-dev \
  lld \
  mold \
  pkg-config \
  protobuf-compiler
cargo build --release -p base-prover-tdx
```

Run the configfs-backed TDX prover:

```sh
sudo env BASE_TDX_SIGNER_KEY="$POC_TDX_SIGNER_KEY" \
  ./target/release/base-prover-tdx server \
  --l1-eth-url "$L1_ETH_URL" \
  --l2-eth-url "$L2_ETH_URL" \
  --l1-beacon-url "$L1_BEACON_URL" \
  --l2-chain-id "$L2_CHAIN_ID" \
  --listen-addr 127.0.0.1:7310 \
  --report-name base-tdx-prover
```

The prover may need root privileges to create and read entries below
`/sys/kernel/config/tsm/report`. A non-root run can fail with `Permission
denied` when `enclave_signerAttestation` tries to collect a quote. Keep the RPC
listener bound to `127.0.0.1` for the first POC and access it only through SSH
tunneling.

From the local machine, through the SSH tunnel:

```sh
curl -s localhost:7310 \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"enclave_attestationKind","params":[]}'
```

```sh
curl -s localhost:7310 \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":2,"method":"enclave_signerPublicKey","params":[]}'
```

```sh
curl -s localhost:7310 \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":3,"method":"enclave_signerAttestation","params":[null,null]}'
```

Expected:

- The prover returns `"tdx"`.
- The signer public key is stable across repeated calls.
- The attestation contains a real quote from the VM.

### Gate

Continue only when:

- The VM boots as a TDX guest.
- The prover can read/write the TSM configfs report path.
- `enclave_signerAttestation` returns a quote.
- Restarting the prover with the same `BASE_TDX_SIGNER_KEY` preserves the
  signer identity.

## Phase 2: Real Quote Verification From Local

Goal: prove quote, collateral, policy, and image-hash derivation from the local
machine against the real GCP TDX prover.

From local:

```sh
cargo run -p base-proof-tee-tdx-image-hash -- \
  --endpoint http://127.0.0.1:7310 \
  --verify-quote \
  --max-quote-age-secs 300 \
  --allowed-tcb-status up-to-date
```

The default Intel PCS configuration is:

- PCS base URL:
  `https://api.trustedservices.intel.com/tdx/certification/v4/`
- Trusted Intel SGX/TDX root hash:
  `0xa1acc73eb45794fa1734f14d882e91925b6006f79d3bb2460df9d01b333d7009`
- Max quote age: 300 seconds.
- Allowed TCB statuses: `up-to-date`.

### Negative Checks

Run with a bad root hash:

```sh
cargo run -p base-proof-tee-tdx-image-hash -- \
  --endpoint http://127.0.0.1:7310 \
  --verify-quote \
  --trusted-root-ca-hash 0x1111111111111111111111111111111111111111111111111111111111111111
```

Expected:

- Verification fails.

Call attestation with unsupported challenge fields:

```sh
curl -s localhost:7310 \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":4,"method":"enclave_signerAttestation","params":[[1,2,3],null]}'
```

Expected:

- The prover rejects `user_data`.

```sh
curl -s localhost:7310 \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":5,"method":"enclave_signerAttestation","params":[null,[1,2,3]]}'
```

Expected:

- The prover rejects `nonce`.

### Gate

Continue only when:

- Real quote verification succeeds with production policy.
- Wrong trust root fails closed.
- Unsupported challenge fields fail closed.
- Repeated quote verification produces a stable signer and image hash.

## Phase 3: Local Contract Registration

Goal: register the real GCP TDX signer against local contracts before touching
testnet.

Use Anvil and the contracts branch referenced by `tdx-plan.md`.

Deploy locally:

- `TDXVerifier`
- `TDXTEEProverRegistry`
- Any required mock verifier or dev verifier configuration for direct-mode
  proving.
- The proof submitter used by the registrar.

For the first contract registration test, use:

- TDX static discovery.
- TDX direct proving.
- The GCP TDX prover endpoint through the SSH tunnel.

Registrar command shape:

```sh
cargo run -p base-prover-registrar -- \
  --l1-rpc-url http://127.0.0.1:8545 \
  --tee-prover-registry-address "$LOCAL_TDX_TEE_PROVER_REGISTRY" \
  --l1-chain-id "$LOCAL_L1_CHAIN_ID" \
  --nitro-discovery-mode static \
  --nitro-prover-endpoint "$LOCAL_OR_DEV_NITRO_PROVER_ENDPOINT" \
  --nitro-proving-mode direct \
  --nitro-elf-path "$NITRO_ATTESTATION_ELF" \
  --tdx-discovery-mode static \
  --tdx-prover-endpoint http://127.0.0.1:7310 \
  --tdx-proving-mode direct \
  --tdx-zk-coprocessor risc-zero \
  --tdx-max-quote-age-secs 300 \
  --tdx-allowed-tcb-status up-to-date \
  --poll-interval-secs 30 \
  "$SIGNER_AND_TX_MANAGER_ARGS"
```

Expected:

- Registrar discovers the TDX endpoint.
- Registrar validates `enclave_attestationKind == "tdx"`.
- Registrar fetches TDX quote collateral.
- Registrar submits `registerTDXSigner(output, zkCoprocessor, proofBytes)`.
- Registrar does not submit Nitro `registerSigner` for the TDX endpoint.

### Local Contract Assertions

Query the local registry:

- `isRegisteredSigner(tdx_signer) == true`
- `signerImageHash(tdx_signer) == image_hash_from_tool`
- `isValidSigner(tdx_signer) == true` when `AggregateVerifier.TEE_IMAGE_HASH`
  matches the TDX image hash.
- `isValidSigner(tdx_signer) == false` when the configured image hash is wrong.

### Gate

Continue only when:

- A real GCP TDX signer registers locally.
- The local registry stores the expected signer image hash.
- `isValidSigner` behaves correctly for matching and mismatched image hashes.

## Phase 4: Testnet Registration

Goal: prove the production proving and contract path against deployed test
contracts.

### Deploy And Configure Testnet Contracts

Deploy or identify:

- `TDXVerifier`
- `TDXTEEProverRegistry`
- `AggregateVerifier`
- Proof submitter permissions.
- Accepted `ZkCoProcessorType`.
- Allowed TCB statuses.
- `AggregateVerifier.TEE_IMAGE_HASH` equal to the TDX journal `imageHash`.

### Select The Real TDX Proving Backend

For local RISC Zero proving:

```sh
--tdx-proving-mode risc-zero \
--tdx-elf-path "$TDX_ATTESTATION_VERIFIER_ELF"
```

For Boundless proving:

```sh
--tdx-proving-mode boundless \
--tdx-image-id "$TDX_IMAGE_ID" \
--tdx-boundless-rpc-url "$TDX_BOUNDLESS_RPC_URL" \
--tdx-boundless-private-key "$TDX_BOUNDLESS_PRIVATE_KEY" \
--tdx-boundless-verifier-program-url "$TDX_BOUNDLESS_VERIFIER_PROGRAM_URL"
```

Run registrar against testnet with static TDX discovery:

```sh
cargo run -p base-prover-registrar -- \
  --l1-rpc-url "$TESTNET_L1_RPC_URL" \
  --tee-prover-registry-address "$TESTNET_TDX_TEE_PROVER_REGISTRY" \
  --l1-chain-id "$TESTNET_L1_CHAIN_ID" \
  --nitro-discovery-mode static \
  --nitro-prover-endpoint "$TESTNET_OR_DEV_NITRO_PROVER_ENDPOINT" \
  --nitro-proving-mode "$NITRO_PROVING_MODE" \
  --tdx-discovery-mode static \
  --tdx-prover-endpoint http://127.0.0.1:7310 \
  --tdx-proving-mode "$TDX_PROVING_MODE" \
  --tdx-zk-coprocessor risc-zero \
  --tdx-max-quote-age-secs 300 \
  --tdx-allowed-tcb-status up-to-date \
  "$TDX_PROVING_ARGS" \
  "$SIGNER_AND_TX_MANAGER_ARGS"
```

After registration:

```sh
cargo run -p base-proof-tee-tdx-image-hash -- \
  --endpoint http://127.0.0.1:7310 \
  --verify-quote \
  --l1-rpc-url "$TESTNET_L1_RPC_URL" \
  --registry-address "$TESTNET_TDX_TEE_PROVER_REGISTRY"
```

Expected:

- The printed image hash equals the onchain `signerImageHash`.
- `isValidSigner` is true when the deployed aggregate verifier expects that
  image hash.

### Gate

Continue only when:

- Testnet registration succeeds.
- Testnet registry has the expected signer and image hash.
- Testnet `isValidSigner` succeeds.
- A wrong TDX image hash configuration is observed as invalid, not valid.

## Phase 5: Full Dev Environment Integration

Goal: run the same architecture as production: registrar plus proposer plus
Nitro prover fleet plus TDX prover fleet.

### Registrar Integration

Configure:

- Existing Nitro fleet exactly as currently used.
- TDX fleet with static discovery pointing at the GCP TDX prover endpoint.
- TDX collateral defaults unless policy requires otherwise.
- TDX proving mode matching deployed contracts.

Expected:

- Registrar registers the union of healthy Nitro and TDX signers.
- TDX endpoints are rejected if they do not report `"tdx"`.
- Nitro endpoints are rejected if discovered under the TDX fleet.
- Orphan cleanup considers both Nitro and TDX active signer sets.

### Proposer Integration

Configure proposer with:

- Nitro prover RPC.
- TDX prover RPC.
- Registry address for `isValidSigner` checks.
- The expected TEE image hash.

Run first in dry-run mode. Then submit a single testnet proposal.

Expected:

- Proposer readiness fails when either Nitro or TDX proof source is unavailable.
- Proposer obtains Nitro and TDX proofs for the same proposal input.
- Proposer rejects signatures from unregistered signers.
- Proposer distinguishes "registered but wrong image hash" from "not
  registered".
- One testnet proposal path can consume both platform proofs.

### Gate

The POC is complete when:

- The GCP TDX prover is reachable through the expected operational path.
- Real TDX quote verification succeeds.
- The TDX signer is registered on testnet.
- Testnet `isValidSigner` succeeds for the TDX signer.
- The existing dev environment can run with both Nitro and TDX configured.
- A single canary proposal can be driven through the dual-prover path.

## Useful Failure Triage

### VM Does Not Expose TSM/configfs

Check:

- The VM was created with `--confidential-compute-type TDX`.
- The machine type supports Intel TDX.
- The guest image is TDX-capable.
- `configfs` is mounted.
- The kernel has TDX guest support.

### Quote Collection Fails

Check:

- `/sys/kernel/config/tsm/report` exists.
- The prover has permission to create the configured report directory.
- The `provider` file, when present, reports `tdx_guest`.
- Writing exactly 64 bytes to `inblob` changes the generation counter.
- `outblob` is non-empty after writing report data.

### Quote Verification Fails

Check:

- The quote is fresh relative to `--tdx-max-quote-age-secs`.
- Intel PCS is reachable from the machine running the registrar or image-hash
  tool.
- The trusted root hash matches the production Intel SGX/TDX root.
- The TCB status is allowed by policy.
- The quote's report data binds the signer public key and timestamp.

### Registration Fails

Check:

- `enclave_attestationKind` returns `"tdx"`.
- Registrar TDX discovery mode is `static`.
- The registrar is using `registerTDXSigner`, not `registerSigner`, for TDX.
- The `ZkCoProcessorType` matches the deployed `TDXVerifier` configuration.
- The proof submitter has permission to register.
- The TDX verifier accepts the proof format generated by the selected proving
  mode.

### Proposer Fails After Registration

Check:

- The proposer is configured with the TDX prover RPC.
- The registry address is correct.
- `signerImageHash` equals the TDX image-hash tool output.
- `AggregateVerifier.TEE_IMAGE_HASH` equals the TDX journal `imageHash`.
- Both Nitro and TDX proof sources are healthy.

## Final Confidence Checklist

- [ ] Local TDX package tests pass.
- [ ] Local mock TDX prover RPC smoke test passes.
- [ ] GCP TDX VM boots with TSM/configfs quote support.
- [ ] Real TDX prover returns `enclave_attestationKind == "tdx"`.
- [ ] Real TDX prover returns a stable signer public key.
- [ ] Real TDX prover returns a parseable quote.
- [ ] Local image-hash tool verifies the real quote with Intel PCS collateral.
- [ ] Bad root hash fails verification.
- [ ] Unsupported nonce/user-data attestation requests fail.
- [ ] Local contract registration succeeds.
- [ ] Local `signerImageHash` equals image-hash tool output.
- [ ] Local `isValidSigner` passes for matching image hash and fails for wrong
  image hash.
- [ ] Testnet TDX registration succeeds.
- [ ] Testnet `isValidSigner` passes.
- [ ] Registrar can run with both Nitro and TDX fleets configured.
- [ ] Proposer dry-run works with both Nitro and TDX proof sources.
- [ ] One testnet canary proposal succeeds through the dual-prover path.
