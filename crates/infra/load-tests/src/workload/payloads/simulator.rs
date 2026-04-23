//! Generic resource-stress payload that calls `Simulator.run(SimulatorConfig)`
//! on the `Simulator` contract from `base/benchmark`.
//!
//! `Simulator` exposes per-call knobs for state-trie and account-trie work
//! (`create_storage`, `create_accounts`, `update_*`, `load_*`, `delete_*`)
//! plus a precompile mix. Pointing this payload at one of the already-deployed
//! Simulator instances avoids embedding any new contract bytecode in the load
//! tool — the whole workload is described by the `SimulatorConfig` struct.
//!
//! Deployed addresses (see `base/benchmark`):
//! - Sepolia Alpha: `0x596E578e5Db8B287208518Db6366194720958e35`
//! - Base Sepolia:  `0xee1dc3309A40a5645769bFCEF90f4131af626f19`
//! - Mainnet:       `0xF86d7753dc7778A5e30901c91F611611c93C07Ad`
//!
//! Per-chunk initialization (`initialize_storage_chunk`, `initialize_address_chunk`)
//! is only required if the workload uses `load_*` or `update_*` ops; pure
//! `create_*` workloads call `run` directly.

use alloy_network::TransactionBuilder;
use alloy_primitives::{Address, Bytes, U160, U256};
use alloy_rpc_types::TransactionRequest;
use alloy_sol_types::{SolCall, sol};

use super::Payload;
use crate::workload::SeededRng;

sol! {
    interface ISimulator {
        struct PrecompileConfig {
            address precompile_address;
            uint256 num_calls;
        }

        struct SimulatorConfig {
            uint160 load_accounts;
            uint160 update_accounts;
            uint160 create_accounts;
            uint256 load_storage;
            uint256 update_storage;
            uint256 delete_storage;
            uint256 create_storage;
            PrecompileConfig[] precompiles;
        }

        function run(SimulatorConfig calldata config) external;
    }
}

/// Per-precompile entry in the simulator config.
#[derive(Debug, Clone, Copy)]
pub struct SimulatorPrecompile {
    /// Precompile address (e.g. `0x09` for blake2f).
    pub address: Address,
    /// Number of calls to that precompile per `run`.
    pub num_calls: u64,
}

/// Op counts forwarded to `Simulator.run`. Each field maps 1:1 to the
/// corresponding field on the on-chain `SimulatorConfig` struct.
#[derive(Debug, Clone, Default)]
pub struct SimulatorOps {
    /// Number of `BALANCE`-load ops on existing accounts.
    pub load_accounts: u64,
    /// Number of `send(1)` updates to existing accounts.
    pub update_accounts: u64,
    /// Number of newly-created accounts (one new account-trie entry each).
    pub create_accounts: u64,
    /// Number of `SLOAD` ops on existing slots.
    pub load_storage: u64,
    /// Number of `SSTORE` ops on existing slots.
    pub update_storage: u64,
    /// Number of `SSTORE`-to-zero ops on existing slots.
    pub delete_storage: u64,
    /// Number of newly-written storage slots (one new storage-trie entry each).
    pub create_storage: u64,
    /// Optional precompile mix.
    pub precompiles: Vec<SimulatorPrecompile>,
}

/// Generates `Simulator.run(SimulatorConfig)` transactions.
///
/// The on-chain Simulator is offset-aware (each instance was constructed with
/// a starting offset) and bumps internal counters as it runs, so concurrent
/// senders pointing at the same Simulator are safe — they just race on the
/// shared counters and write to disjoint regions.
#[derive(Debug, Clone)]
pub struct SimulatorPayload {
    target: Address,
    ops: SimulatorOps,
    gas_limit: u64,
}

impl SimulatorPayload {
    /// Creates a new simulator payload targeting `target` with the given ops
    /// and gas limit.
    pub const fn new(target: Address, ops: SimulatorOps, gas_limit: u64) -> Self {
        Self { target, ops, gas_limit }
    }

    /// Returns the gas limit used per generated transaction.
    pub const fn gas_limit(&self) -> u64 {
        self.gas_limit
    }
}

impl Payload for SimulatorPayload {
    fn name(&self) -> &'static str {
        "simulator"
    }

    fn generate(&self, _rng: &mut SeededRng, _from: Address, _to: Address) -> TransactionRequest {
        let precompiles: Vec<ISimulator::PrecompileConfig> = self
            .ops
            .precompiles
            .iter()
            .map(|p| ISimulator::PrecompileConfig {
                precompile_address: p.address,
                num_calls: U256::from(p.num_calls),
            })
            .collect();

        let call = ISimulator::runCall {
            config: ISimulator::SimulatorConfig {
                load_accounts: U160::from(self.ops.load_accounts),
                update_accounts: U160::from(self.ops.update_accounts),
                create_accounts: U160::from(self.ops.create_accounts),
                load_storage: U256::from(self.ops.load_storage),
                update_storage: U256::from(self.ops.update_storage),
                delete_storage: U256::from(self.ops.delete_storage),
                create_storage: U256::from(self.ops.create_storage),
                precompiles,
            },
        };

        TransactionRequest::default()
            .with_to(self.target)
            .with_input(Bytes::from(call.abi_encode()))
            .with_gas_limit(self.gas_limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_produces_well_formed_calldata() {
        let target = Address::repeat_byte(0xab);
        let ops = SimulatorOps { create_accounts: 470, ..SimulatorOps::default() };
        let payload = SimulatorPayload::new(target, ops, 16_700_000);

        let mut rng = SeededRng::new(1);
        let tx = payload.generate(&mut rng, Address::ZERO, Address::ZERO);

        assert_eq!(tx.to.and_then(|k| k.to().copied()), Some(target));
        assert_eq!(tx.gas, Some(16_700_000));

        let input = tx.input.input().expect("calldata present").clone();
        let decoded = ISimulator::runCall::abi_decode(&input).expect("decodable");
        assert_eq!(decoded.config.create_accounts, U160::from(470u64));
        assert_eq!(decoded.config.create_storage, U256::ZERO);
        assert!(decoded.config.precompiles.is_empty());
    }

    #[test]
    fn generate_includes_precompiles() {
        let target = Address::repeat_byte(0xcd);
        let ops = SimulatorOps {
            create_storage: 10,
            precompiles: vec![SimulatorPrecompile {
                address: Address::with_last_byte(0x09),
                num_calls: 5,
            }],
            ..SimulatorOps::default()
        };
        let payload = SimulatorPayload::new(target, ops, 5_000_000);

        let mut rng = SeededRng::new(1);
        let tx = payload.generate(&mut rng, Address::ZERO, Address::ZERO);
        let input = tx.input.input().expect("calldata present").clone();
        let decoded = ISimulator::runCall::abi_decode(&input).expect("decodable");

        assert_eq!(decoded.config.create_storage, U256::from(10u64));
        assert_eq!(decoded.config.precompiles.len(), 1);
        assert_eq!(decoded.config.precompiles[0].precompile_address, Address::with_last_byte(0x09));
        assert_eq!(decoded.config.precompiles[0].num_calls, U256::from(5u64));
    }
}
