#!/usr/bin/env bash
# Hardware smoke test for registering a real Intel TDX prover signer.
#
# Run this from a host that can reach:
#   - a TDX prover endpoint running on a guest with QGS/PCCS configured,
#   - L1 RPC,
#   - the TDXTEEProverRegistry contract.
#
# Required environment:
#   TDX_PROVER_ENDPOINT            TDX prover JSON-RPC URL.
#   L1_RPC_URL                     L1 execution RPC URL.
#   L1_CHAIN_ID                    L1 chain ID.
#   TEE_PROVER_REGISTRY_ADDRESS    TDXTEEProverRegistry address.
#   REGISTRAR_PRIVATE_KEY          Registrar owner/manager private key.
#   NITRO_REGISTRAR_ARGS           Nitro fleet/proving args required by the dual-fleet registrar.
#   TDX_ELF_PATH                   TDX verifier guest ELF path when TDX_PROVING_MODE=risc-zero.
#
# Optional environment:
#   TDX_PROVING_MODE               direct|risc-zero|boundless (default: risc-zero).
#   EXTRA_REGISTRAR_ARGS           Additional registrar flags, for example Boundless args.
#   SMOKE_TIMEOUT_SECS             Time to wait for isValidSigner=true (default: 1800).
#   POLL_INTERVAL_SECS             Registry polling interval (default: 10).
#   REGISTRAR_BIN                  Prebuilt registrar binary path; defaults to cargo run.
#   TDX_IMAGE_HASH_BIN             Prebuilt image-hash binary path; defaults to cargo run.

set -euo pipefail

require_env() {
    local name="$1"
    if [ -z "${!name:-}" ]; then
        echo "missing required environment variable: $name" >&2
        exit 1
    fi
}

require_env TDX_PROVER_ENDPOINT
require_env L1_RPC_URL
require_env L1_CHAIN_ID
require_env TEE_PROVER_REGISTRY_ADDRESS
require_env REGISTRAR_PRIVATE_KEY
require_env NITRO_REGISTRAR_ARGS

TDX_PROVING_MODE="${TDX_PROVING_MODE:-risc-zero}"
SMOKE_TIMEOUT_SECS="${SMOKE_TIMEOUT_SECS:-1800}"
POLL_INTERVAL_SECS="${POLL_INTERVAL_SECS:-10}"

case "$TDX_PROVING_MODE" in
    risc-zero)
        require_env TDX_ELF_PATH
        ;;
    direct | boundless)
        ;;
    *)
        echo "invalid TDX_PROVING_MODE: $TDX_PROVING_MODE (expected direct, risc-zero, or boundless)" >&2
        exit 1
        ;;
esac

if [ -n "${REGISTRAR_BIN:-}" ]; then
    REGISTRAR_CMD=("$REGISTRAR_BIN")
else
    REGISTRAR_CMD=(cargo run -p base-proof-tee-registrar-bin --)
fi

if [ -n "${TDX_IMAGE_HASH_BIN:-}" ]; then
    TDX_IMAGE_HASH_CMD=("$TDX_IMAGE_HASH_BIN")
else
    TDX_IMAGE_HASH_CMD=(cargo run -p base-proof-tee-tdx-image-hash --)
fi

read -r -a NITRO_ARGS <<< "$NITRO_REGISTRAR_ARGS"

WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/base-tdx-smoke.XXXXXX")"
REGISTRAR_LOG="$WORKDIR/registrar.log"
REPORT_PATH="$WORKDIR/tdx-image-hash-report.txt"
REGISTRAR_PID=""

cleanup() {
    if [ -n "$REGISTRAR_PID" ] && kill -0 "$REGISTRAR_PID" >/dev/null 2>&1; then
        kill "$REGISTRAR_PID" >/dev/null 2>&1 || true
        wait "$REGISTRAR_PID" >/dev/null 2>&1 || true
    fi
    rm -rf "$WORKDIR"
}
trap cleanup EXIT

echo "Inspecting TDX prover quote before registration..."
"${TDX_IMAGE_HASH_CMD[@]}" \
    --endpoint "$TDX_PROVER_ENDPOINT" \
    --verify-quote

echo "Starting registrar with static TDX discovery..."
REGISTRAR_ARGS=(
    --l1-rpc-url "$L1_RPC_URL"
    --l1-chain-id "$L1_CHAIN_ID"
    --tee-prover-registry-address "$TEE_PROVER_REGISTRY_ADDRESS"
    --private-key "$REGISTRAR_PRIVATE_KEY"
    --tdx-discovery-mode static
    --tdx-prover-endpoint "$TDX_PROVER_ENDPOINT"
    --tdx-proving-mode "$TDX_PROVING_MODE"
    --poll-interval-secs "$POLL_INTERVAL_SECS"
)
if [ "$TDX_PROVING_MODE" = "risc-zero" ]; then
    REGISTRAR_ARGS+=(--tdx-elf-path "$TDX_ELF_PATH")
fi
REGISTRAR_ARGS+=("${NITRO_ARGS[@]}")
if [ -n "${EXTRA_REGISTRAR_ARGS:-}" ]; then
    read -r -a EXTRA_ARGS <<< "$EXTRA_REGISTRAR_ARGS"
    if [ "${#EXTRA_ARGS[@]}" -gt 0 ]; then
        REGISTRAR_ARGS+=("${EXTRA_ARGS[@]}")
    fi
fi

"${REGISTRAR_CMD[@]}" "${REGISTRAR_ARGS[@]}" >"$REGISTRAR_LOG" 2>&1 &
REGISTRAR_PID="$!"

deadline=$((SECONDS + SMOKE_TIMEOUT_SECS))
while [ "$SECONDS" -lt "$deadline" ]; do
    if ! kill -0 "$REGISTRAR_PID" >/dev/null 2>&1; then
        echo "registrar exited before registration completed" >&2
        cat "$REGISTRAR_LOG" >&2
        exit 1
    fi

    if "${TDX_IMAGE_HASH_CMD[@]}" \
        --endpoint "$TDX_PROVER_ENDPOINT" \
        --verify-quote \
        --l1-rpc-url "$L1_RPC_URL" \
        --registry-address "$TEE_PROVER_REGISTRY_ADDRESS" \
        >"$REPORT_PATH" 2>&1
    then
        if grep -q "Registry isValidSigner: true" "$REPORT_PATH"; then
            cat "$REPORT_PATH"
            echo "TDX hardware smoke succeeded: isValidSigner(real_signer) == true"
            exit 0
        fi
    fi

    sleep "$POLL_INTERVAL_SECS"
done

echo "timed out waiting for TDX signer registration" >&2
echo "--- last image-hash report ---" >&2
cat "$REPORT_PATH" >&2 || true
echo "--- registrar log ---" >&2
cat "$REGISTRAR_LOG" >&2
exit 1
