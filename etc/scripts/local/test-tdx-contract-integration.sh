#!/usr/bin/env bash
# Runs the TDX verifier + registry Forge integration test against the contracts
# tree referenced by tdx-plan.md without switching the user's existing checkout.
#
# Environment:
#   CONTRACTS_DIR  Contracts repository path.
#                  Default: /Users/jackchuma/projects/active/base-chain/check-contracts-claim/contracts
#   CONTRACTS_REF  Git ref containing the TDX contracts.
#                  Default: feat/tdx-verifier

set -euo pipefail

CONTRACTS_DIR="${CONTRACTS_DIR:-/Users/jackchuma/projects/active/base-chain/check-contracts-claim/contracts}"
CONTRACTS_REF="${CONTRACTS_REF:-feat/tdx-verifier}"
FIXTURE_PATH="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)/contracts/TDXEndToEndRegistration.t.sol"
WORKTREE_PARENT="$(mktemp -d "${TMPDIR:-/tmp}/base-tdx-contracts.XXXXXX")"
WORKTREE_PATH="$WORKTREE_PARENT/contracts"

cleanup() {
    if [ -e "$WORKTREE_PATH/.git" ]; then
        git -C "$CONTRACTS_DIR" worktree remove --force "$WORKTREE_PATH" >/dev/null 2>&1 || true
    fi
    rm -rf "$WORKTREE_PARENT"
}
trap cleanup EXIT

command -v forge >/dev/null || {
    echo "forge is required to run the TDX contract integration test" >&2
    exit 1
}

test -d "$CONTRACTS_DIR/.git" || {
    echo "CONTRACTS_DIR must point at the contracts git repository: $CONTRACTS_DIR" >&2
    exit 1
}

test -f "$FIXTURE_PATH" || {
    echo "missing Forge fixture: $FIXTURE_PATH" >&2
    exit 1
}

git -C "$CONTRACTS_DIR" cat-file -e "$CONTRACTS_REF:src/multiproof/tee/TDXVerifier.sol" || {
    echo "CONTRACTS_REF does not contain src/multiproof/tee/TDXVerifier.sol: $CONTRACTS_REF" >&2
    exit 1
}

git -C "$CONTRACTS_DIR" worktree add --detach "$WORKTREE_PATH" "$CONTRACTS_REF"
mkdir -p "$WORKTREE_PATH/lib"
for dep in "$CONTRACTS_DIR"/lib/*; do
    if [ -e "$dep" ]; then
        dep_name="$(basename "$dep")"
        rm -rf "$WORKTREE_PATH/lib/$dep_name"
        ln -s "$dep" "$WORKTREE_PATH/lib/$dep_name"
    fi
done
cp "$FIXTURE_PATH" "$WORKTREE_PATH/test/multiproof/TDXEndToEndRegistration.t.sol"

forge test \
    --root "$WORKTREE_PATH" \
    --match-contract TDXEndToEndRegistrationTest \
    -vvv
