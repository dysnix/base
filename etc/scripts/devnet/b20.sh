#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
COMMON_SH="$SCRIPT_DIR/common.sh"
if [[ -f "$COMMON_SH" ]]; then
    # shellcheck source=/dev/null
    source "$COMMON_SH"
fi

RPC_URL="${1:-${RPC_URL:-${L2_BUILDER_RPC_URL:-http://localhost:7545}}}"
PRIVATE_KEY="${PRIVATE_KEY:-${SEQUENCER_KEY:-${ANVIL_ACCOUNT_5_KEY:-0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba}}}"
ADMIN="${ADMIN:-${SEQUENCER_ADDR:-${ANVIL_ACCOUNT_5_ADDR:-0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc}}}"
RECIPIENT_ONE="${RECIPIENT_ONE:-${BATCHER_ADDR:-${ANVIL_ACCOUNT_6_ADDR:-0x976EA74026E726554dB657fA54763abd0C3a0aa9}}}"
RECIPIENT_ONE_KEY="${RECIPIENT_ONE_KEY:-${BATCHER_KEY:-${ANVIL_ACCOUNT_6_KEY:-0x92db14e403b83dfe3df233f83dfa3a0d7096f21ca9b0d6d6b8d88b2b4ec1564e}}}"
RECIPIENT_TWO="${RECIPIENT_TWO:-${PROPOSER_ADDR:-${ANVIL_ACCOUNT_7_ADDR:-0x14dC79964da2C08b23698B3D3cc7Ca32193d9955}}}"
RECIPIENT_TWO_KEY="${RECIPIENT_TWO_KEY:-${PROPOSER_KEY:-${ANVIL_ACCOUNT_7_KEY:-0x4bbbf85ce3377467afe5d46f804f221813b2bb87f24d81f60f1fcdbf7cbf4356}}}"

B20_FACTORY_ADDRESS="${B20_FACTORY_ADDRESS:-0x8453000000000000000000000000000000000001}"
BUSD_ADDRESS="${BUSD_ADDRESS:-0x8453000000000000000000000000000000000000}"
BUSD_ADMIN="${BUSD_ADMIN:-${SEQUENCER_ADDR:-${ANVIL_ACCOUNT_5_ADDR:-0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc}}}"
BUSD_ADMIN_KEY="${BUSD_ADMIN_KEY:-${SEQUENCER_KEY:-${ANVIL_ACCOUNT_5_KEY:-0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba}}}"
TOKEN_NAME="${TOKEN_NAME:-Dev USD}"
TOKEN_SYMBOL="${TOKEN_SYMBOL:-DUSD}"
TOKEN_CURRENCY="${TOKEN_CURRENCY:-USD}"
SALT="${SALT:-$(cast keccak "base-b20-$(date +%s)-$$")}"
BUSD_MINT_AMOUNT="${BUSD_MINT_AMOUNT:-1000000000}"
BUSD_RECIPIENT_MINT_AMOUNT="${BUSD_RECIPIENT_MINT_AMOUNT:-100000000}"
BERYL_BLOCK="${BERYL_BLOCK:-${L2_BASE_BERYL_BLOCK:-3}}"
BERYL_WAIT_SECONDS="${BERYL_WAIT_SECONDS:-120}"
MINT_AMOUNT="${MINT_AMOUNT:-1000000000}"
TRANSFER_ONE="${TRANSFER_ONE:-100000000}"
TRANSFER_TWO="${TRANSFER_TWO:-25000000}"
TRANSFER_THREE="${TRANSFER_THREE:-10000000}"
GAS_LIMIT="${GAS_LIMIT:-10000000}"

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "missing required command: $1" >&2
        exit 1
    }
}

send_tx() {
    local from_key="$1"
    local raw_tx
    local tx_hash
    shift

    raw_tx="$(
        cast mktx \
            --rpc-url "$RPC_URL" \
            --private-key "$from_key" \
            --gas-limit "$GAS_LIMIT" \
            "$@"
    )"

    tx_hash="$(cast rpc --rpc-url "$RPC_URL" eth_sendRawTransaction "$raw_tx" | jq -r .)"

    cast receipt \
        --rpc-url "$RPC_URL" \
        --json \
        "$tx_hash" |
        jq -r '"tx=\(.transactionHash) block=\(.blockNumber) status=\(.status)"'
}

balance_of() {
    local token="$1"
    local account="$2"
    cast call --rpc-url "$RPC_URL" "$token" "balanceOf(address)(uint256)" "$account"
}

wait_for_block() {
    local target_block="$1"
    local label="$2"
    local current_block

    for _ in $(seq 1 "$BERYL_WAIT_SECONDS"); do
        current_block="$(cast block-number --rpc-url "$RPC_URL" 2>/dev/null || true)"
        if [[ "$current_block" =~ ^[0-9]+$ && "$current_block" -ge "$target_block" ]]; then
            echo "$label active at block $current_block"
            return
        fi
        sleep 1
    done

    echo "timed out waiting for $label block $target_block; latest block: ${current_block:-<unknown>}" >&2
    exit 1
}

wait_for_code() {
    local address="$1"
    local label="$2"
    local code

    for _ in $(seq 1 60); do
        code="$(cast code --rpc-url "$RPC_URL" "$address" 2>/dev/null || true)"
        if [[ -n "$code" && "$code" != "0x" ]]; then
            echo "$label code: $code"
            return
        fi
        sleep 1
    done

    echo "$label has no deployed code at $address" >&2
    exit 1
}

assert_call_equals() {
    local address="$1"
    local call="$2"
    local expected="$3"
    local actual

    actual="$(cast call --rpc-url "$RPC_URL" "$address" "$call")"
    if [[ "$actual" != "$expected" ]]; then
        echo "expected $call to return $expected, got $actual" >&2
        exit 1
    fi
    echo "$call: $actual"
}

b20_balance_of() {
    local account="$1"
    cast call --rpc-url "$RPC_URL" "$TOKEN_ADDRESS" "balanceOf(address)(uint256)" "$account"
}

require_cmd cast
require_cmd jq

if [[ ! "$BERYL_BLOCK" =~ ^[0-9]+$ ]]; then
    echo "BERYL_BLOCK must be a non-negative integer, got: $BERYL_BLOCK" >&2
    exit 1
fi

echo "RPC: $RPC_URL"
echo "factory: $B20_FACTORY_ADDRESS"
echo "admin: $ADMIN"
echo "busd: $BUSD_ADDRESS"
echo "busd_admin: $BUSD_ADMIN"
echo "beryl_block: $BERYL_BLOCK"
echo "salt: $SALT"

echo "waiting for Beryl activation"
wait_for_block "$BERYL_BLOCK" "Beryl"

echo "checking hardfork-deployed BUSD"
wait_for_code "$BUSD_ADDRESS" "BUSD"
assert_call_equals "$BUSD_ADDRESS" "name()(string)" '"Base USD"'
assert_call_equals "$BUSD_ADDRESS" "symbol()(string)" '"BUSD"'
assert_call_equals "$BUSD_ADDRESS" "currency()(string)" '"USD"'

BUSD_ISSUER_ROLE="$(cast call --rpc-url "$RPC_URL" "$BUSD_ADDRESS" "ISSUER_ROLE()(bytes32)")"

echo "granting BUSD issuer role"
send_tx "$BUSD_ADMIN_KEY" "$BUSD_ADDRESS" "grantRole(bytes32,address)" "$BUSD_ISSUER_ROLE" "$BUSD_ADMIN"

echo "minting BUSD to devnet accounts"
send_tx "$BUSD_ADMIN_KEY" "$BUSD_ADDRESS" "mint(address,uint256)" "$BUSD_ADMIN" "$BUSD_MINT_AMOUNT"
send_tx "$BUSD_ADMIN_KEY" "$BUSD_ADDRESS" "mint(address,uint256)" "$RECIPIENT_ONE" "$BUSD_RECIPIENT_MINT_AMOUNT"
send_tx "$BUSD_ADMIN_KEY" "$BUSD_ADDRESS" "mint(address,uint256)" "$RECIPIENT_TWO" "$BUSD_RECIPIENT_MINT_AMOUNT"

echo "BUSD balances"
echo "busd: $BUSD_ADDRESS"
echo "busd_admin: $(balance_of "$BUSD_ADDRESS" "$BUSD_ADMIN")"
echo "recipient_one: $(balance_of "$BUSD_ADDRESS" "$RECIPIENT_ONE")"
echo "recipient_two: $(balance_of "$BUSD_ADDRESS" "$RECIPIENT_TWO")"

TOKEN_ADDRESS="$(
    cast call \
        --rpc-url "$RPC_URL" \
        "$B20_FACTORY_ADDRESS" \
        "getTokenAddress(address,bytes32)(address)" \
        "$ADMIN" \
        "$SALT"
)"

echo "addresses"
echo "b20_factory: $B20_FACTORY_ADDRESS"
echo "b20_token: $TOKEN_ADDRESS"

echo "creating B20"
send_tx "$PRIVATE_KEY" \
    "$B20_FACTORY_ADDRESS" \
    "createToken(string,string,string,address,bytes32)" \
    "$TOKEN_NAME" \
    "$TOKEN_SYMBOL" \
    "$TOKEN_CURRENCY" \
    "$ADMIN" \
    "$SALT"

ISSUER_ROLE="$(cast call --rpc-url "$RPC_URL" "$TOKEN_ADDRESS" "ISSUER_ROLE()(bytes32)")"

echo "granting issuer role"
send_tx "$PRIVATE_KEY" "$TOKEN_ADDRESS" "grantRole(bytes32,address)" "$ISSUER_ROLE" "$ADMIN"

echo "minting"
send_tx "$PRIVATE_KEY" "$TOKEN_ADDRESS" "mint(address,uint256)" "$ADMIN" "$MINT_AMOUNT"

echo "transferring admin -> recipient one"
send_tx "$PRIVATE_KEY" "$TOKEN_ADDRESS" "transfer(address,uint256)" "$RECIPIENT_ONE" "$TRANSFER_ONE"

echo "transferring recipient one -> recipient two"
send_tx "$RECIPIENT_ONE_KEY" "$TOKEN_ADDRESS" "transfer(address,uint256)" "$RECIPIENT_TWO" "$TRANSFER_TWO"

echo "transferring recipient two -> admin"
send_tx "$RECIPIENT_TWO_KEY" "$TOKEN_ADDRESS" "transfer(address,uint256)" "$ADMIN" "$TRANSFER_THREE"

echo "balances"
echo "b20_token: $TOKEN_ADDRESS"
echo "admin: $(b20_balance_of "$ADMIN")"
echo "recipient_one: $(b20_balance_of "$RECIPIENT_ONE")"
echo "recipient_two: $(b20_balance_of "$RECIPIENT_TWO")"
