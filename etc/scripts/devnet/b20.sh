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
BASE_DEX_ADDRESS="${BASE_DEX_ADDRESS:-0x0000000000000000000000000000000000000dE7}"
BASE_USD_ADDRESS="${BASE_USD_ADDRESS:-$BUSD_ADDRESS}"
BUSD_ADMIN="${BUSD_ADMIN:-${SEQUENCER_ADDR:-${ANVIL_ACCOUNT_5_ADDR:-0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc}}}"
BUSD_ADMIN_KEY="${BUSD_ADMIN_KEY:-${SEQUENCER_KEY:-${ANVIL_ACCOUNT_5_KEY:-0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba}}}"
TOKEN_NAME="${TOKEN_NAME:-Dev USD}"
TOKEN_SYMBOL="${TOKEN_SYMBOL:-DUSD}"
TOKEN_CURRENCY="${TOKEN_CURRENCY:-USD}"
SALT="${SALT:-$(cast keccak "base-b20-$(date +%s)-$$")}"
TOKEN_B_NAME="${TOKEN_B_NAME:-Dev Token B}"
TOKEN_B_SYMBOL="${TOKEN_B_SYMBOL:-DTB}"
TOKEN_B_SALT="${TOKEN_B_SALT:-$(cast keccak "base-b20-b-$(date +%s)-$$")}"
MINT_AMOUNT="${MINT_AMOUNT:-1000000000}"
BUSD_MINT_AMOUNT="${BUSD_MINT_AMOUNT:-5000000000}"
BUSD_RECIPIENT_MINT_AMOUNT="${BUSD_RECIPIENT_MINT_AMOUNT:-100000000}"
BERYL_BLOCK="${BERYL_BLOCK:-${L2_BASE_BERYL_BLOCK:-3}}"
BERYL_WAIT_SECONDS="${BERYL_WAIT_SECONDS:-120}"
LIQUIDITY_TOKEN_AMOUNT="${LIQUIDITY_TOKEN_AMOUNT:-500000000}"
LIQUIDITY_BASE_AMOUNT="${LIQUIDITY_BASE_AMOUNT:-500000000}"
DEX_TRANSFER_AMOUNT="${DEX_TRANSFER_AMOUNT:-100000000}"
DEX_SWAP_AMOUNT="${DEX_SWAP_AMOUNT:-10000000}"
DEX_DIRECT_SWAP_AMOUNT="${DEX_DIRECT_SWAP_AMOUNT:-5000000}"
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

normalize_address() {
    echo "$1" | tr '[:upper:]' '[:lower:]'
}

assert_address_equals() {
    local label="$1"
    local actual="$2"
    local expected="$3"

    if [[ "$(normalize_address "$actual")" != "$(normalize_address "$expected")" ]]; then
        echo "expected $label to be $expected, got $actual" >&2
        exit 1
    fi
    echo "$label: $actual"
}

assert_dec_equals() {
    local label="$1"
    local actual="$2"
    local expected="$3"

    actual="$(to_dec "$actual")"
    if [[ "$actual" != "$expected" ]]; then
        echo "expected $label to be $expected, got $actual" >&2
        exit 1
    fi
    echo "$label: $actual"
}

b20_balance_of() {
    local account="$1"
    cast call --rpc-url "$RPC_URL" "$TOKEN_ADDRESS" "balanceOf(address)(uint256)" "$account"
}

to_dec() {
    local value="$1"
    local first

    first="${value%% *}"
    if [[ "$first" == 0x* ]]; then
        cast to-dec "$first"
    else
        echo "$first"
    fi
}

wait_for_base_precompiles() {
    local attempts="${BASE_PRECOMPILE_WAIT_ATTEMPTS:-90}"
    local attempt

    for ((attempt = 1; attempt <= attempts; attempt++)); do
        if cast call --rpc-url "$RPC_URL" "$BASE_DEX_ADDRESS" "BASE_TOKEN()(address)" >/dev/null 2>&1; then
            return
        fi
        sleep 1
    done

    echo "base precompiles were not active after ${attempts}s" >&2
    exit 1
}

balance_of_token() {
    local token="$1"
    local account="$2"
    local value

    value="$(cast call --rpc-url "$RPC_URL" "$token" "balanceOf(address)(uint256)" "$account")"
    to_dec "$value"
}

assert_eq() {
    local label="$1"
    local actual="$2"
    local expected="$3"

    if [[ "$actual" != "$expected" ]]; then
        echo "assertion failed: $label actual=$actual expected=$expected" >&2
        exit 1
    fi
}

assert_delta() {
    local label="$1"
    local before="$2"
    local after="$3"
    local expected_delta="$4"
    local actual_delta=$((after - before))

    assert_eq "$label" "$actual_delta" "$expected_delta"
}

require_cmd cast
require_cmd jq

if [[ ! "$BERYL_BLOCK" =~ ^[0-9]+$ ]]; then
    echo "BERYL_BLOCK must be a non-negative integer, got: $BERYL_BLOCK" >&2
    exit 1
fi

echo "RPC: $RPC_URL"
echo "factory: $B20_FACTORY_ADDRESS"
echo "base_dex: $BASE_DEX_ADDRESS"
echo "base_usd: $BASE_USD_ADDRESS"
echo "admin: $ADMIN"
echo "busd: $BUSD_ADDRESS"
echo "busd_admin: $BUSD_ADMIN"
echo "beryl_block: $BERYL_BLOCK"
echo "salt: $SALT"

echo "waiting for Beryl activation"
wait_for_block "$BERYL_BLOCK" "Beryl"
wait_for_base_precompiles

echo "checking hardfork-deployed BUSD"
wait_for_code "$BUSD_ADDRESS" "BUSD"
assert_call_equals "$BUSD_ADDRESS" "name()(string)" '"Base USD"'
assert_call_equals "$BUSD_ADDRESS" "symbol()(string)" '"BUSD"'
assert_call_equals "$BUSD_ADDRESS" "currency()(string)" '"USD"'

echo "checking Base DEX"
DEX_BASE_TOKEN="$(cast call --rpc-url "$RPC_URL" "$BASE_DEX_ADDRESS" "BASE_TOKEN()(address)")"
assert_address_equals "dex base token" "$DEX_BASE_TOKEN" "$BASE_USD_ADDRESS"
assert_dec_equals "dex fee numerator" "$(cast call --rpc-url "$RPC_URL" "$BASE_DEX_ADDRESS" "FEE_NUMERATOR()(uint256)")" "997"
assert_dec_equals "dex fee denominator" "$(cast call --rpc-url "$RPC_URL" "$BASE_DEX_ADDRESS" "FEE_DENOMINATOR()(uint256)")" "1000"
assert_dec_equals "dex minimum liquidity" "$(cast call --rpc-url "$RPC_URL" "$BASE_DEX_ADDRESS" "MINIMUM_LIQUIDITY()(uint256)")" "1000"

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
echo "base_dex: $BASE_DEX_ADDRESS"
echo "base_usd: $BASE_USD_ADDRESS"

echo "creating B20"
send_tx "$PRIVATE_KEY" \
    "$B20_FACTORY_ADDRESS" \
    "createToken(string,string,string,address,bytes32)" \
    "$TOKEN_NAME" \
    "$TOKEN_SYMBOL" \
    "$TOKEN_CURRENCY" \
    "$ADMIN" \
    "$SALT"

TOKEN_B_ADDRESS="$(
    cast call \
        --rpc-url "$RPC_URL" \
        "$B20_FACTORY_ADDRESS" \
        "getTokenAddress(address,bytes32)(address)" \
        "$ADMIN" \
        "$TOKEN_B_SALT"
)"

echo "creating second B20"
send_tx "$PRIVATE_KEY" \
    "$B20_FACTORY_ADDRESS" \
    "createToken(string,string,string,address,bytes32)" \
    "$TOKEN_B_NAME" \
    "$TOKEN_B_SYMBOL" \
    "$TOKEN_CURRENCY" \
    "$ADMIN" \
    "$TOKEN_B_SALT"

echo "second token salt: $TOKEN_B_SALT"
echo "second b20_token: $TOKEN_B_ADDRESS"

ISSUER_ROLE="$(cast call --rpc-url "$RPC_URL" "$TOKEN_ADDRESS" "ISSUER_ROLE()(bytes32)")"
BUSD_ISSUER_ROLE="$(cast call --rpc-url "$RPC_URL" "$BASE_USD_ADDRESS" "ISSUER_ROLE()(bytes32)")"
TOKEN_B_ISSUER_ROLE="$(cast call --rpc-url "$RPC_URL" "$TOKEN_B_ADDRESS" "ISSUER_ROLE()(bytes32)")"

echo "granting issuer role"
send_tx "$PRIVATE_KEY" "$TOKEN_ADDRESS" "grantRole(bytes32,address)" "$ISSUER_ROLE" "$ADMIN"
send_tx "$PRIVATE_KEY" "$TOKEN_B_ADDRESS" "grantRole(bytes32,address)" "$TOKEN_B_ISSUER_ROLE" "$ADMIN"
send_tx "$BUSD_ADMIN_KEY" "$BASE_USD_ADDRESS" "grantRole(bytes32,address)" "$BUSD_ISSUER_ROLE" "$ADMIN"

echo "minting"
send_tx "$PRIVATE_KEY" "$TOKEN_ADDRESS" "mint(address,uint256)" "$ADMIN" "$MINT_AMOUNT"
send_tx "$PRIVATE_KEY" "$TOKEN_B_ADDRESS" "mint(address,uint256)" "$ADMIN" "$MINT_AMOUNT"
send_tx "$PRIVATE_KEY" "$BASE_USD_ADDRESS" "mint(address,uint256)" "$ADMIN" "$BUSD_MINT_AMOUNT"
send_tx "$PRIVATE_KEY" "$BASE_USD_ADDRESS" "mint(address,uint256)" "$RECIPIENT_ONE" "$BUSD_RECIPIENT_MINT_AMOUNT"
send_tx "$PRIVATE_KEY" "$BASE_USD_ADDRESS" "mint(address,uint256)" "$RECIPIENT_TWO" "$BUSD_RECIPIENT_MINT_AMOUNT"

echo "transferring admin -> recipient one"
send_tx "$PRIVATE_KEY" "$TOKEN_ADDRESS" "transfer(address,uint256)" "$RECIPIENT_ONE" "$TRANSFER_ONE"

echo "transferring recipient one -> recipient two"
send_tx "$RECIPIENT_ONE_KEY" "$TOKEN_ADDRESS" "transfer(address,uint256)" "$RECIPIENT_TWO" "$TRANSFER_TWO"

echo "transferring recipient two -> admin"
send_tx "$RECIPIENT_TWO_KEY" "$TOKEN_ADDRESS" "transfer(address,uint256)" "$ADMIN" "$TRANSFER_THREE"

echo "adding Base DEX liquidity"
send_tx "$PRIVATE_KEY" \
    "$BASE_DEX_ADDRESS" \
    "addLiquidity(address,uint256,uint256,address)" \
    "$TOKEN_ADDRESS" \
    "$LIQUIDITY_TOKEN_AMOUNT" \
    "$LIQUIDITY_BASE_AMOUNT" \
    "$ADMIN"
send_tx "$PRIVATE_KEY" \
    "$BASE_DEX_ADDRESS" \
    "addLiquidity(address,uint256,uint256,address)" \
    "$TOKEN_B_ADDRESS" \
    "$LIQUIDITY_TOKEN_AMOUNT" \
    "$LIQUIDITY_BASE_AMOUNT" \
    "$ADMIN"

echo "funding recipient one for DEX swap"
send_tx "$PRIVATE_KEY" "$TOKEN_ADDRESS" "transfer(address,uint256)" "$RECIPIENT_ONE" "$DEX_TRANSFER_AMOUNT"

TOKEN_A_BEFORE="$(balance_of_token "$TOKEN_ADDRESS" "$RECIPIENT_ONE")"
TOKEN_B_BEFORE="$(balance_of_token "$TOKEN_B_ADDRESS" "$RECIPIENT_ONE")"
EXPECTED_TOKEN_B="$(
    to_dec "$(cast call \
        --rpc-url "$RPC_URL" \
        "$BASE_DEX_ADDRESS" \
        "quoteExactInput(address,address,uint256)(uint256)" \
        "$TOKEN_ADDRESS" \
        "$TOKEN_B_ADDRESS" \
        "$DEX_SWAP_AMOUNT")"
)"
echo "swapping token A -> token B via Base USD, expected_out=$EXPECTED_TOKEN_B"
send_tx "$RECIPIENT_ONE_KEY" \
    "$BASE_DEX_ADDRESS" \
    "swapExactTokensForTokens(address,address,uint256,uint256,address)" \
    "$TOKEN_ADDRESS" \
    "$TOKEN_B_ADDRESS" \
    "$DEX_SWAP_AMOUNT" \
    "0" \
    "$RECIPIENT_ONE"
TOKEN_A_AFTER="$(balance_of_token "$TOKEN_ADDRESS" "$RECIPIENT_ONE")"
TOKEN_B_AFTER="$(balance_of_token "$TOKEN_B_ADDRESS" "$RECIPIENT_ONE")"
assert_delta "recipient_one token A after A->B swap" "$TOKEN_A_BEFORE" "$TOKEN_A_AFTER" "-$DEX_SWAP_AMOUNT"
assert_delta "recipient_one token B after A->B swap" "$TOKEN_B_BEFORE" "$TOKEN_B_AFTER" "$EXPECTED_TOKEN_B"

BUSD_BEFORE="$(balance_of_token "$BASE_USD_ADDRESS" "$RECIPIENT_ONE")"
TOKEN_B_BEFORE_DIRECT="$(balance_of_token "$TOKEN_B_ADDRESS" "$RECIPIENT_ONE")"
EXPECTED_BUSD="$(
    to_dec "$(cast call \
        --rpc-url "$RPC_URL" \
        "$BASE_DEX_ADDRESS" \
        "quoteExactInput(address,address,uint256)(uint256)" \
        "$TOKEN_B_ADDRESS" \
        "$BASE_USD_ADDRESS" \
        "$DEX_DIRECT_SWAP_AMOUNT")"
)"
echo "swapping token B -> Base USD, expected_out=$EXPECTED_BUSD"
send_tx "$RECIPIENT_ONE_KEY" \
    "$BASE_DEX_ADDRESS" \
    "swapExactTokensForTokens(address,address,uint256,uint256,address)" \
    "$TOKEN_B_ADDRESS" \
    "$BASE_USD_ADDRESS" \
    "$DEX_DIRECT_SWAP_AMOUNT" \
    "0" \
    "$RECIPIENT_ONE"
BUSD_AFTER="$(balance_of_token "$BASE_USD_ADDRESS" "$RECIPIENT_ONE")"
TOKEN_B_AFTER_DIRECT="$(balance_of_token "$TOKEN_B_ADDRESS" "$RECIPIENT_ONE")"
assert_delta "recipient_one token B after B->BUSD swap" "$TOKEN_B_BEFORE_DIRECT" "$TOKEN_B_AFTER_DIRECT" "-$DEX_DIRECT_SWAP_AMOUNT"
assert_delta "recipient_one BUSD after B->BUSD swap" "$BUSD_BEFORE" "$BUSD_AFTER" "$EXPECTED_BUSD"

echo "balances"
echo "b20_token: $TOKEN_ADDRESS"
echo "admin: $(b20_balance_of "$ADMIN")"
echo "recipient_one: $(b20_balance_of "$RECIPIENT_ONE")"
echo "recipient_two: $(b20_balance_of "$RECIPIENT_TWO")"
echo "b20_token_b: $TOKEN_B_ADDRESS"
echo "base_usd: $BASE_USD_ADDRESS"
echo "recipient_one_token_b: $(balance_of_token "$TOKEN_B_ADDRESS" "$RECIPIENT_ONE")"
echo "recipient_one_busd: $(balance_of_token "$BASE_USD_ADDRESS" "$RECIPIENT_ONE")"
