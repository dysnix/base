use crate::BaseBSpec;
use alloy::primitives::{Address, Log, LogData, U256};
use alloy_evm::EvmInternals;
use revm::{
    context::{Block, CfgEnv, journaled_state::JournalCheckpoint},
    context_interface::cfg::{GasParams, gas},
    state::{AccountInfo, Bytecode},
};

use crate::{error::BasePrecompileError, storage::PrecompileStorageProvider};

/// Production [`PrecompileStorageProvider`] backed by the live EVM journal.
///
/// Wraps `EvmInternals` and tracks gas consumption for storage operations.
pub struct EvmPrecompileStorageProvider<'a> {
    internals: EvmInternals<'a>,
    gas_limit: u64,
    gas_remaining: u64,
    gas_refunded: i64,
    spec: BaseBSpec,
    is_static: bool,
    gas_params: GasParams,
    /// Debug-only LIFO checkpoint validator. See [`Self::assert_lifo`].
    #[cfg(debug_assertions)]
    checkpoint_stack: Vec<(usize, usize)>,
}

impl<'a> EvmPrecompileStorageProvider<'a> {
    /// Creates a new storage provider with the given gas limit, hardfork, and static flag.
    pub fn new(
        internals: EvmInternals<'a>,
        gas_limit: u64,
        spec: BaseBSpec,
        is_static: bool,
        gas_params: GasParams,
    ) -> Self {
        Self {
            internals,
            gas_limit,
            gas_remaining: gas_limit,
            gas_refunded: 0,
            spec,
            is_static,
            gas_params,
            #[cfg(debug_assertions)]
            checkpoint_stack: Vec::new(),
        }
    }

    /// Creates a new storage provider with maximum gas limit and non-static context.
    pub fn new_max_gas(internals: EvmInternals<'a>, cfg: &CfgEnv<BaseBSpec>) -> Self {
        Self::new(internals, u64::MAX, cfg.spec, false, cfg.gas_params.clone())
    }

    /// Creates a new storage provider with the given gas limit, deriving spec from `cfg`.
    pub fn new_with_gas_limit(
        internals: EvmInternals<'a>,
        cfg: &CfgEnv<BaseBSpec>,
        gas_limit: u64,
        _reservoir: u64,
    ) -> Self {
        Self::new(internals, gas_limit, cfg.spec, false, cfg.gas_params.clone())
    }

    #[inline]
    fn deduct_state_gas(&mut self, _gas: u64) -> Result<(), BasePrecompileError> {
        Ok(())
    }
}

impl<'a> PrecompileStorageProvider for EvmPrecompileStorageProvider<'a> {
    fn chain_id(&self) -> u64 {
        self.internals.chain_id()
    }

    fn timestamp(&self) -> U256 {
        self.internals.block_timestamp()
    }

    fn beneficiary(&self) -> Address {
        self.internals.block_env().beneficiary()
    }

    fn block_number(&self) -> u64 {
        self.internals.block_env().number().to::<u64>()
    }

    #[inline]
    fn set_code(&mut self, address: Address, code: Bytecode) -> Result<(), BasePrecompileError> {
        let code_len = code.len();
        self.deduct_gas(self.gas_params.code_deposit_cost(code_len))?;

        let was_empty = {
            let mut account = self.internals.load_account_mut(address)?;
            let was_empty = account.data.account().info.is_empty();
            account.set_code_and_hash_slow(code);
            was_empty
        };

        if self.spec.is_enabled_in(crate::BaseBSpec::Azul) && was_empty {
            self.deduct_gas(self.gas_params.create_cost())?;
            self.deduct_gas(self.gas_params.keccak256_cost(code_len.div_ceil(32)))?;
        }

        Ok(())
    }

    #[inline]
    fn with_account_info(
        &mut self,
        address: Address,
        f: &mut dyn FnMut(&AccountInfo),
    ) -> Result<(), BasePrecompileError> {
        let additional_cost = self.gas_params.cold_account_additional_cost();

        // T4+: pre-charge static gas to avoid cheap useless work.
        let insufficient_gas_for_cold_load = if self.spec.is_enabled_in(crate::BaseBSpec::Azul) {
            self.deduct_gas(self.gas_params.warm_storage_read_cost())?;
            self.gas_remaining < additional_cost
        } else {
            false
        };

        let is_cold = {
            let mut account = self
                .internals
                .load_account_mut_skip_cold_load(address, insufficient_gas_for_cold_load)?;
            account.load_code()?;
            f(&account.data.account().info);
            account.is_cold
        };

        if !self.spec.is_enabled_in(crate::BaseBSpec::Azul) {
            self.deduct_gas(self.gas_params.warm_storage_read_cost())?;
        }

        if is_cold {
            self.deduct_gas(additional_cost)?;
        }
        Ok(())
    }

    #[inline]
    fn sstore(
        &mut self,
        address: Address,
        key: U256,
        value: U256,
    ) -> Result<(), BasePrecompileError> {
        // T4+: pre-charge static gas before loading storage to avoid cheap useless work.
        let insufficient_gas_for_cold_load = if self.spec.is_enabled_in(crate::BaseBSpec::Azul) {
            self.deduct_gas(self.gas_params.sstore_static_gas())?;
            self.gas_remaining < self.gas_params.cold_storage_additional_cost()
        } else {
            false
        };

        let result = self.internals.load_account_mut(address)?.sstore(
            key,
            value,
            insufficient_gas_for_cold_load,
        )?;

        if !self.spec.is_enabled_in(crate::BaseBSpec::Azul) {
            self.deduct_gas(self.gas_params.sstore_static_gas())?;
        }

        // dynamic gas
        self.deduct_gas(self.gas_params.sstore_dynamic_gas(true, &result.data, result.is_cold))?;

        // refund gas.
        self.refund_gas(self.gas_params.sstore_refund(true, &result.data));

        Ok(())
    }

    #[inline]
    fn tstore(
        &mut self,
        address: Address,
        key: U256,
        value: U256,
    ) -> Result<(), BasePrecompileError> {
        self.deduct_gas(self.gas_params.warm_storage_read_cost())?;
        self.internals.tstore(address, key, value);
        Ok(())
    }

    #[inline]
    fn emit_event(&mut self, address: Address, event: LogData) -> Result<(), BasePrecompileError> {
        self.deduct_gas(
            gas::LOG
                + self.gas_params.log_cost(event.topics().len() as u8, event.data.len() as u64),
        )?;

        self.internals.log(Log { address, data: event });

        Ok(())
    }

    #[inline]
    fn sload(&mut self, address: Address, key: U256) -> Result<U256, BasePrecompileError> {
        let additional_cost = self.gas_params.cold_storage_additional_cost();

        // T4+: pre-charge static gas to avoid cheap useless work.
        let insufficient_gas_for_cold_load = if self.spec.is_enabled_in(crate::BaseBSpec::Azul) {
            self.deduct_gas(self.gas_params.warm_storage_read_cost())?;
            self.gas_remaining < additional_cost
        } else {
            false
        };

        let value;
        let is_cold;
        {
            let mut account = self.internals.load_account_mut(address)?;
            let val = account.sload(key, insufficient_gas_for_cold_load)?;

            value = val.present_value;
            is_cold = val.is_cold;
        };

        if !self.spec.is_enabled_in(crate::BaseBSpec::Azul) {
            self.deduct_gas(self.gas_params.warm_storage_read_cost())?;
        }

        // dynamic gas
        if is_cold {
            self.deduct_gas(additional_cost)?;
        }

        Ok(value)
    }

    #[inline]
    fn tload(&mut self, address: Address, key: U256) -> Result<U256, BasePrecompileError> {
        self.deduct_gas(self.gas_params.warm_storage_read_cost())?;

        Ok(self.internals.tload(address, key))
    }

    #[inline]
    fn deduct_gas(&mut self, gas: u64) -> Result<(), BasePrecompileError> {
        deduct_gas(&mut self.gas_remaining, gas)
    }

    #[inline]
    fn refund_gas(&mut self, gas: i64) {
        self.gas_refunded = self.gas_refunded.saturating_add(gas);
    }

    #[inline]
    fn gas_limit(&self) -> u64 {
        self.gas_limit
    }

    #[inline]
    fn gas_used(&self) -> u64 {
        self.gas_limit - self.gas_remaining
    }

    #[inline]
    fn state_gas_used(&self) -> u64 {
        0
    }

    #[inline]
    fn gas_refunded(&self) -> i64 {
        self.gas_refunded
    }

    #[inline]
    fn reservoir(&self) -> u64 {
        0
    }

    #[inline]
    fn spec(&self) -> BaseBSpec {
        self.spec
    }

    #[inline]
    fn amsterdam_eip8037_enabled(&self) -> bool {
        false
    }

    #[inline]
    fn is_static(&self) -> bool {
        self.is_static
    }

    #[inline]
    fn checkpoint(&mut self) -> JournalCheckpoint {
        let cp = self.internals.checkpoint();
        #[cfg(debug_assertions)]
        self.track_checkpoint(&cp);
        cp
    }

    #[inline]
    fn checkpoint_commit(&mut self, _checkpoint: JournalCheckpoint) {
        #[cfg(debug_assertions)]
        self.assert_lifo(&_checkpoint, "commit");
        self.internals.checkpoint_commit()
    }

    #[inline]
    fn checkpoint_revert(&mut self, checkpoint: JournalCheckpoint) {
        #[cfg(debug_assertions)]
        self.assert_lifo(&checkpoint, "revert");
        self.internals.checkpoint_revert(checkpoint)
    }
}

/// LIFO checkpoint validation (debug builds only).
///
/// Since `EvmInternals` doesn't expose revm's journal depth, we mirror it by
/// recording each checkpoint's (`journal_i`, `log_i`) on creation and asserting
/// that commits/reverts always resolve the most recent checkpoint first.
#[cfg(debug_assertions)]
impl EvmPrecompileStorageProvider<'_> {
    /// Records a newly created checkpoint for later LIFO validation.
    fn track_checkpoint(&mut self, cp: &JournalCheckpoint) {
        self.checkpoint_stack.push((cp.journal_i, cp.log_i));
    }

    /// Panics if `cp` is not the most recently created checkpoint.
    fn assert_lifo(&mut self, cp: &JournalCheckpoint, op: &str) {
        let top = self
            .checkpoint_stack
            .pop()
            .unwrap_or_else(|| panic!("checkpoint_{op}: no active checkpoint"));

        assert_eq!(
            (cp.journal_i, cp.log_i),
            top,
            "out-of-order checkpoint {op} (expected top of stack)"
        );
    }
}

/// Deducts gas from the remaining gas and returns an error if insufficient.
#[inline]
pub fn deduct_gas(
    gas_remaining: &mut u64,
    additional_cost: u64,
) -> Result<(), BasePrecompileError> {
    let Some(remaining) = gas_remaining.checked_sub(additional_cost) else {
        return Err(BasePrecompileError::OutOfGas);
    };
    *gas_remaining = remaining;
    Ok(())
}
