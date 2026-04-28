#!/usr/bin/env bash
# Compare latest/safe/finalized block numbers between builder (sequencer) and
# validator nodes on the local devnet. Refreshes every 2 seconds.
#
# Usage: ./etc/scripts/devnet/compare-heads.sh

set -uo pipefail

source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

BUILDER="${L2_BUILDER_RPC_URL}"
CLIENT="${L2_CLIENT_RPC_URL}"
VALIDATOR="${L2_VALIDATOR_RPC_URL}"

while true; do
    clear
    echo "=== builder (sequencer, no delay) ==="
    for label in latest safe finalized; do
        num=$(cast block "$label" --rpc-url "$BUILDER" 2>/dev/null | grep "^number" | awk '{print $2}')
        printf "  %-12s number %s\n" "$label" "${num:-N/A}"
    done

    echo
    echo "=== client (validator, with delay) ==="
    for label in latest safe finalized; do
        num=$(cast block "$label" --rpc-url "$CLIENT" 2>/dev/null | grep "^number" | awk '{print $2}')
        printf "  %-12s number %s\n" "$label" "${num:-N/A}"
    done

    echo
    echo "=== unified validator (single process) ==="
    for label in latest safe finalized; do
        num=$(cast block "$label" --rpc-url "$VALIDATOR" 2>/dev/null | grep "^number" | awk '{print $2}')
        printf "  %-12s number %s\n" "$label" "${num:-N/A}"
    done

    echo
    echo "(refreshing every 2s — Ctrl-C to stop)"
    sleep 2
done
