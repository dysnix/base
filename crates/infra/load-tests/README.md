# Load Tests

Load testing and benchmarking framework for Base infrastructure.

## Crate

| Crate | Description |
|-------|-------------|
| `base-load-tests` | Core library with workload generation, transaction submission, and metrics collection |

## Goals

- Provide standardized transaction submission for network load testing
- Centralize workload generation, network orchestration, and metrics collection
- Enable reproducible test scenarios with deterministic configurations

## Quick Start

```bash
# Run load test against local devnet (uses Anvil Account #1)
just load-test devnet

# Run load test against sepolia (requires funded key)
FUNDER_KEY=0x... just load-test sepolia
```

Or run directly with cargo:

```bash
# Build the crate
cargo build -p base-load-tests

# Run tests
cargo test -p base-load-tests

# Run the load test binary with a config file
cargo run -p base-load-tests --bin base-load-test -- path/to/config.yaml
```

## Configuration

All configuration is done via YAML files. See `src/config/test_config.rs` for comprehensive field documentation, or `examples/devnet.yaml` for a working example.

Example minimal config:

```yaml
rpc: http://localhost:8545
block_watcher_url: "ws://localhost:8546"
flashblocks_ws_url: "ws://localhost:7111"
sender_count: 10
target_gps: 2100000
duration: "30s"
```

### Available Configs

| Config | Target | Notes |
|--------|--------|-------|
| `devnet.yaml` | Local devnet | Uses Anvil Account #1 |
| `sepolia.yaml` | Sepolia | Requires `FUNDER_KEY` |

### Environment Variables

- `FUNDER_KEY` - Private key (0x-prefixed hex) of a funded account to distribute test funds from

### Transaction Types

The config supports weighted transaction mixes:

```yaml
transactions:
  - weight: 70
    type: transfer
  - weight: 20
    type: calldata
    max_size: 256
    repeat_count: 1  # Optional: repeat for compressible data
  - weight: 10
    type: precompile
    target: sha256
```

#### Precompile Testing

All EVM precompiles are supported for load testing:

**Cryptographic**: `ecrecover`, `sha256`, `ripemd160`, `blake2f`
**Elliptic Curve**: `bn254_add`, `bn254_mul`, `bn254_pairing`
**Other**: `identity`, `modexp`, `kzg_point_evaluation`

```yaml
# Simple precompile call
- type: precompile
  target: sha256

# Blake2f with custom rounds
- type: precompile
  target: blake2f
  rounds: 50000

# Multiple calls per transaction (requires looper_contract)
- type: precompile
  target: ecrecover
  iterations: 50

# When using iterations > 1, specify looper contract address:
looper_contract: "0x..."  # Deployed PrecompileLooper contract
```

The `PrecompileLooper` contract enables batch testing by calling a precompile multiple times in a single transaction, useful for scenarios like multi-signature verification or repeated hash operations.

### Fresh-recipient mode

By default, recipients are picked from the bounded sender pool, so a long run keeps targeting the same `sender_count` addresses and produces no account-trie fan-out. Set `fresh_recipients: true` to derive a fresh recipient *signing key* per transaction from a seeded RNG, which makes `transfer` (and `calldata`) workloads create one new account-trie entry per send. Required for the `dust` performance-baseline scenario.

```yaml
sender_count: 350
fresh_recipients: true
transactions:
  - weight: 100
    type: transfer
```

Recipient keys are derived from whatever the senders use (mnemonic if `mnemonic` is set, otherwise `seed`), advanced past the sender keys to avoid collision. The runner prints `recipient_offset` at startup; recover with:

```rust
// Mnemonic path
let recipients = AccountPool::from_mnemonic(mnemonic, n_recipients, recipient_offset)?;

// Seed path
let recipients = AccountPool::with_offset(seed, n_recipients, recipient_offset)?;
```

Notes:

- Recipients receive at most a few wei (1 wei from the default `transfer`); they are **not** reclaimed because the gas to sweep them back exceeds the value. The seed-based derivation just means you *can* recover them later if you ever want to.
- Same `seed` reproduces the same recipient sequence across runs.
- Only affects payloads that honor the runner-supplied `to`: `transfer` and `calldata`. `erc20`, `precompile`, `osaka`, and `uniswap_v2`/`uniswap_v3` use their own target addresses (contract / router / target field) and ignore this flag. The Uniswap payloads do still pass the runner-supplied `to` as the swap *output recipient*, so swap output goes to fresh addresses when this flag is on — harmless, but worth noting.
