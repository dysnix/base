# TDX Deployment

TDX support is an additive rollout path. Nitro remains the default path unless
TDX endpoints and proving configuration are supplied explicitly.

## Required Inputs

Operators must collect these values before enabling TDX registration:

| Value | Used by | Notes |
|---|---|---|
| `TDX_PROVER_ENDPOINT` | registrar, image hash tool, proposer | JSON-RPC endpoint for the TDX prover. |
| `TEE_PROVER_REGISTRY_ADDRESS` | registrar, proposer, image hash tool | Address of the deployed `TDXTEEProverRegistry` or compatible `TEEProverRegistry`. |
| `L1_RPC_URL` / `L1_ETH_RPC` | registrar, proposer, image hash tool | L1 RPC endpoint used for registration and signer checks. |
| `L1_CHAIN_ID` | registrar | Chain ID for transaction signing. |
| registrar signer secret | registrar | `BASE_REGISTRAR_PRIVATE_KEY` or remote signer endpoint/address. |
| TDX prover signer secret | TDX prover | `BASE_TDX_SIGNER_KEY` when persistent signer identity is required; omit only for ephemeral test signers. |
| TDX verifier guest program | registrar | `TDX_ELF_PATH` for local RISC Zero proving or `TDX_BOUNDLESS_VERIFIER_PROGRAM_URL` plus `TDX_IMAGE_ID` for Boundless. |
| TDX Boundless signer secret | registrar | `TDX_BOUNDLESS_PRIVATE_KEY` when `--tdx-proving-mode boundless` is used. |
| Intel root CA hash | registrar, image hash tool | `TDX_TRUSTED_ROOT_CA_HASH`; production default is `0xa1acc73eb45794fa1734f14d882e91925b6006f79d3bb2460df9d01b333d7009`. |
| Intel PCS/PCCS URL | registrar, image hash tool | `TDX_PCS_TDX_BASE_URL`; production default is Intel PCS v4. |
| Allowed TCB statuses | registrar, image hash tool, contracts | Repeat `--tdx-allowed-tcb-status`; first rollout should use only `up-to-date` unless policy explicitly accepts more. |
| TDX image hash | contracts, proposer | Contract-compatible `imageHash`, not raw MRTD. |
| Nitro prover endpoint | proposer | Required while dual-platform proposal canarying is enabled. |
| `AnchorStateRegistry`, `DisputeGameFactory`, game type | proposer | Existing proposal deployment values. |

## TDX Prover Runtime

Run the prover in a real TDX guest with Linux TSM/configfs quote collection:

```sh
sudo env BASE_TDX_SIGNER_KEY="$BASE_TDX_SIGNER_KEY" base-prover-tdx server \
  --l1-eth-url "$L1_ETH_URL" \
  --l2-eth-url "$L2_ETH_URL" \
  --l1-beacon-url "$L1_BEACON_URL" \
  --l2-chain-id "$L2_CHAIN_ID" \
  --listen-addr "0.0.0.0:7310" \
  --report-name "base-tdx-prover"
```

The Linux TSM/configfs report path is root-owned on common guest images. Run
the configfs-backed prover under a service account that can create and read
entries below `/sys/kernel/config/tsm/report`; for an initial smoke test, root is
the simplest option. If the process lacks permission, quote collection fails
when `enclave_signerAttestation` reads the report path. For first bring-up, bind
the RPC listener to `127.0.0.1:7310` and expose it only with an SSH tunnel.

For local tests without TDX hardware:

```sh
base-prover-tdx local \
  --l1-eth-url "$L1_ETH_URL" \
  --l2-eth-url "$L2_ETH_URL" \
  --l1-beacon-url "$L1_BEACON_URL" \
  --l2-chain-id "$L2_CHAIN_ID" \
  --listen-addr "127.0.0.1:7310"
```

The local mode uses deterministic mock quote fixtures and is not acceptable for
production registration.

## Image Hash Extraction

Before deployment or registration, extract the exact TDX image hash that
`AggregateVerifier.TEE_IMAGE_HASH` must expect:

```sh
base-proof-tee-tdx-image-hash \
  --endpoint "$TDX_PROVER_ENDPOINT" \
  --verify-quote \
  --pcs-tdx-base-url "$TDX_PCS_TDX_BASE_URL" \
  --trusted-root-ca-hash "$TDX_TRUSTED_ROOT_CA_HASH" \
  --allowed-tcb-status up-to-date \
  --l1-rpc-url "$L1_RPC_URL" \
  --registry-address "$TEE_PROVER_REGISTRY_ADDRESS"
```

Use the printed `imageHash` for the TDX `TEE_IMAGE_HASH`. Do not use MRTD,
`mrTdHash`, or a PCR0/Nitro value for TDX.

## Registrar Static Discovery

TDX registration is enabled only when TDX fleet flags are present. Existing
Nitro registrar commands continue to work without these flags.

For a static TDX canary alongside the existing Nitro fleet:

```sh
base-proof-tee-registrar \
  --l1-rpc-url "$L1_RPC_URL" \
  --l1-chain-id "$L1_CHAIN_ID" \
  --tee-prover-registry-address "$TEE_PROVER_REGISTRY_ADDRESS" \
  --private-key "$BASE_REGISTRAR_PRIVATE_KEY" \
  --nitro-discovery-mode aws-target-group \
  --nitro-target-group-arn "$NITRO_TARGET_GROUP_ARN" \
  --nitro-aws-region "$NITRO_AWS_REGION" \
  --nitro-prover-port "$NITRO_PROVER_PORT" \
  --nitro-proving-mode boundless \
  --nitro-image-id "$NITRO_IMAGE_ID" \
  --boundless-rpc-url "$BOUNDLESS_RPC_URL" \
  --boundless-private-key "$BOUNDLESS_PRIVATE_KEY" \
  --boundless-verifier-program-url "$BOUNDLESS_VERIFIER_PROGRAM_URL" \
  --tdx-discovery-mode static \
  --tdx-prover-endpoint "$TDX_PROVER_ENDPOINT" \
  --tdx-proving-mode boundless \
  --tdx-image-id "$TDX_IMAGE_ID" \
  --tdx-boundless-rpc-url "$TDX_BOUNDLESS_RPC_URL" \
  --tdx-boundless-private-key "$TDX_BOUNDLESS_PRIVATE_KEY" \
  --tdx-boundless-verifier-program-url "$TDX_BOUNDLESS_VERIFIER_PROGRAM_URL" \
  --tdx-pcs-tdx-base-url "$TDX_PCS_TDX_BASE_URL" \
  --tdx-trusted-root-ca-hash "$TDX_TRUSTED_ROOT_CA_HASH" \
  --tdx-allowed-tcb-status up-to-date \
  --tdx-max-quote-age-secs 300 \
  --tdx-collateral-fetch-timeout-secs 30
```

Use `--tdx-proving-mode risc-zero --tdx-elf-path "$TDX_ELF_PATH"` for a local
RISC Zero prover, or `--tdx-proving-mode direct` only with a development
verifier contract that accepts the direct proof bytes.

## Canary Proposal

After one TDX signer is registered, run the proposer with both Nitro and TDX
proof sources for a canary:

```sh
base-proposer \
  --nitro-prover-rpc "$NITRO_PROVER_RPC" \
  --tdx-prover-rpc "$TDX_PROVER_ENDPOINT" \
  --l1-eth-rpc "$L1_ETH_RPC" \
  --l2-eth-rpc "$L2_ETH_RPC" \
  --rollup-rpc "$ROLLUP_RPC" \
  --anchor-state-registry-addr "$ANCHOR_STATE_REGISTRY_ADDRESS" \
  --dispute-game-factory-addr "$DISPUTE_GAME_FACTORY_ADDRESS" \
  --game-type "$GAME_TYPE" \
  --tee-image-hash "$TDX_IMAGE_HASH" \
  --tee-prover-registry-address "$TEE_PROVER_REGISTRY_ADDRESS" \
  --private-key "$BASE_PROPOSER_PRIVATE_KEY" \
  --max-parallel-proofs 1
```

Canary success requires:

- `isValidSigner(tdx_signer) == true` before the proposal.
- The proposer obtains Nitro and TDX proofs for the same proposal input.
- One TEE proposal transaction lands successfully.
- Proposer and challenger monitoring report the same game type, game address,
  parent game, L2 block range, output root, and `TEE_IMAGE_HASH` as the Nitro
  path for that canary range.

## Rollback

Rollback must remove TDX from the active rollout without disturbing Nitro:

1. Stop the registrar or restart it without every `--tdx-*` fleet flag.
2. Deregister TDX signer addresses with `deregisterSigner(address)` on
   `TEEProverRegistry`.
3. Restart the proposer with the Nitro endpoint configured by
   `--nitro-prover-rpc`.
4. Keep Nitro prover, registrar, and proposer settings unchanged.
5. Confirm health and metrics no longer expect TDX endpoints before resuming
   production rollout.
