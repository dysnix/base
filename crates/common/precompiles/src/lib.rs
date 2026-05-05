//! Base precompile implementations for B20 and B403.
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]

mod address;
pub use address::{B20_PREFIX_BYTES, BaseBAddressExt, is_b20_prefix};

pub mod error;
pub use error::{IntoPrecompileResult, Result};

pub mod storage;

pub mod b20;
pub mod b20_factory;
pub mod b403_registry;

#[cfg(test)]
use alloy::sol_types::SolInterface;
use alloy::{
    primitives::{Address, Bytes, address},
    sol,
    sol_types::{SolCall, SolError},
};
use alloy_evm::precompiles::{DynPrecompile, PrecompilesMap};
use revm::{
    context::CfgEnv,
    context_interface::cfg::GasParams,
    precompile::{PrecompileError, PrecompileId, PrecompileOutput, PrecompileResult},
    primitives::hardfork::SpecId,
};

use crate::{
    b20::B20Token, b20_factory::B20Factory, b403_registry::B403Registry, storage::StorageCtx,
};

pub use base_precompiles_contracts::{B20_FACTORY_ADDRESS, B403_REGISTRY_ADDRESS};

/// B precompile activation marker.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub enum BaseBSpec {
    /// B precompiles are not active yet.
    #[default]
    PreAzul,
    /// B precompiles are active under the Base Azul hardfork.
    Azul,
}

impl BaseBSpec {
    /// Checks if this spec enables behavior introduced in `other`.
    pub const fn is_enabled_in(self, other: Self) -> bool {
        other as u8 <= self as u8
    }
}

impl From<BaseBSpec> for SpecId {
    fn from(spec: BaseBSpec) -> Self {
        match spec {
            BaseBSpec::PreAzul => Self::PRAGUE,
            BaseBSpec::Azul => Self::OSAKA,
        }
    }
}

/// Base path USD B20 address under the `0x8453` B20 prefix.
pub const PATH_USD_ADDRESS: Address = address!("0x8453000000000000000000000000000000000000");

/// Input per word cost. It covers abi decoding and cloning of input into call data.
///
/// Being careful and pricing it twice as COPY_COST to mitigate different abi decodings.
pub const INPUT_PER_WORD_COST: u64 = 6;

/// Gas cost for `ecrecover` signature verification (used by KeyAuthorization and Permit).
pub const ECRECOVER_GAS: u64 = 3_000;

/// Returns the gas cost for decoding calldata of the given length, rounded up to word boundaries.
#[inline]
pub fn input_cost(calldata_len: usize) -> u64 {
    calldata_len.div_ceil(32).saturating_mul(INPUT_PER_WORD_COST as usize) as u64
}

/// Trait implemented by all Base precompile contract types.
///
/// Precompiles must provide a dispatcher that decodes the 4-byte function selector from calldata,
/// ABI-decodes the arguments, and routes to the corresponding method.
pub trait Precompile {
    /// Dispatches an EVM call to this precompile.
    ///
    /// Implementations should deduct calldata gas upfront via [`input_cost`], then decode the
    /// 4-byte function selector from `calldata` and route to the matching method using
    /// `dispatch_call` combined with the `view`, `mutate`, or `mutate_void` helpers.
    ///
    /// Business-logic errors are returned as reverted [`PrecompileOutput`]s with ABI-encoded
    /// error data, while fatal failures (e.g. out-of-gas) are returned as
    /// [`PrecompileError`](revm::precompile::PrecompileError).
    fn call(&mut self, calldata: &[u8], msg_sender: Address) -> PrecompileResult;
}

/// Returns the full Base B precompiles for the given config.
///
/// Base-specific B precompiles are registered via [`extend_base_b_precompiles`].
pub fn base_b_precompiles(cfg: &CfgEnv<BaseBSpec>) -> PrecompilesMap {
    let mut precompiles = PrecompilesMap::from_static(
        revm::handler::EthPrecompiles::new(cfg.spec.into()).precompiles,
    );
    extend_base_b_precompiles(&mut precompiles, cfg.spec, cfg.gas_params.clone());
    precompiles
}

/// Registers Base B precompiles into an existing [`PrecompilesMap`].
pub fn extend_base_b_precompiles(
    precompiles: &mut PrecompilesMap,
    spec: BaseBSpec,
    gas_params: GasParams,
) {
    precompiles.set_precompile_lookup(move |address: &Address| {
        if !spec.is_enabled_in(BaseBSpec::Azul) {
            None
        } else if *address == B20_FACTORY_ADDRESS {
            tracing::info!(address = %address, "base B20 factory precompile lookup");
            Some(B20Factory::create_precompile(spec, gas_params.clone()))
        } else if *address == B403_REGISTRY_ADDRESS {
            tracing::info!(address = %address, "base B403 registry precompile lookup");
            Some(B403Registry::create_precompile(spec, gas_params.clone()))
        } else if address.is_b20() {
            tracing::info!(address = %address, "base B20 precompile lookup");
            Some(B20Token::create_precompile(*address, spec, gas_params.clone()))
        } else {
            None
        }
    });
}

sol! {
    error DelegateCallNotAllowed();
    error StaticCallNotAllowed();
}

macro_rules! base_precompile {
    ($id:expr, $spec:expr, $gas_params:expr, |$input:ident| $impl:expr) => {{
        let spec = $spec;
        let gas_params = $gas_params.clone();
        DynPrecompile::new_stateful(PrecompileId::Custom($id.into()), move |$input| {
            if !$input.is_direct_call() {
                return Ok(PrecompileOutput::new_reverted(
                    0,
                    DelegateCallNotAllowed {}.abi_encode().into(),
                ));
            }
            let mut storage = crate::storage::evm::EvmPrecompileStorageProvider::new(
                $input.internals,
                $input.gas,
                spec,
                $input.is_static,
                gas_params.clone(),
            );
            crate::storage::StorageCtx::enter(&mut storage, || {
                $impl.call($input.data, $input.caller)
            })
        })
    }};
}

impl B403Registry {
    /// Creates the EVM precompile for this type.
    pub fn create_precompile(spec: BaseBSpec, gas_params: GasParams) -> DynPrecompile {
        base_precompile!("B403Registry", spec, gas_params, |input| { Self::new() })
    }
}

impl B20Factory {
    /// Creates the EVM precompile for this type.
    pub fn create_precompile(spec: BaseBSpec, gas_params: GasParams) -> DynPrecompile {
        base_precompile!("B20Factory", spec, gas_params, |input| { Self::new() })
    }
}

impl B20Token {
    /// Creates the EVM precompile for this type.
    pub fn create_precompile(
        address: Address,
        spec: BaseBSpec,
        gas_params: GasParams,
    ) -> DynPrecompile {
        base_precompile!("B20Token", spec, gas_params, |input| {
            Self::from_address(address).expect("B20 prefix already verified")
        })
    }
}

/// Dispatches a parameterless view call, encoding the return via `T`.
#[inline]
fn metadata<T: SolCall>(f: impl FnOnce() -> Result<T::Return>) -> PrecompileResult {
    f().into_precompile_result(0, 0, |ret| T::abi_encode_returns(&ret).into())
}

/// Dispatches a read-only call with decoded arguments, encoding the return via `T`.
#[inline]
fn view<T: SolCall>(call: T, f: impl FnOnce(T) -> Result<T::Return>) -> PrecompileResult {
    f(call).into_precompile_result(0, 0, |ret| T::abi_encode_returns(&ret).into())
}

/// Dispatches a state-mutating call that returns ABI-encoded data.
///
/// Rejects static calls with [`StaticCallNotAllowed`].
#[inline]
fn mutate<T: SolCall>(
    call: T,
    sender: Address,
    f: impl FnOnce(Address, T) -> Result<T::Return>,
) -> PrecompileResult {
    if StorageCtx.is_static() {
        return Ok(PrecompileOutput::new_reverted(0, StaticCallNotAllowed {}.abi_encode().into()));
    }
    f(sender, call).into_precompile_result(0, 0, |ret| T::abi_encode_returns(&ret).into())
}

/// Dispatches a state-mutating call that returns no data (e.g. `approve`, `transfer`).
///
/// Rejects static calls with [`StaticCallNotAllowed`].
#[inline]
fn mutate_void<T: SolCall>(
    call: T,
    sender: Address,
    f: impl FnOnce(Address, T) -> Result<()>,
) -> PrecompileResult {
    if StorageCtx.is_static() {
        return Ok(PrecompileOutput::new_reverted(0, StaticCallNotAllowed {}.abi_encode().into()));
    }
    f(sender, call).into_precompile_result(0, 0, |()| Bytes::new())
}

/// Deducts the calldata input cost, returning an OOG halt result if insufficient gas.
#[inline]
pub(crate) fn charge_input_cost(
    storage: &mut StorageCtx,
    calldata: &[u8],
) -> Option<PrecompileResult> {
    if storage.deduct_gas(input_cost(calldata.len())).is_err() {
        return Some(Err(PrecompileError::OutOfGas));
    }
    None
}

/// Fills state gas accounting on a [`PrecompileOutput`] from the storage context.
///
/// State gas / reservoir tracking is only set when B1016 (EIP-8037) is enabled.
/// When disabled, `state_gas_used` must remain 0 to avoid leaking into revm's reservoir
/// accounting and corrupting `tx_gas_used()` via `handle_reservoir_remaining_gas`.
///
/// SSTORE refund propagation is activated unconditionally at T4 so the
/// `BasePrecompileProvider` wrapper can apply refunds with `record_refund`. Pre-T4
/// blocks were executed without refund propagation, so we cannot change their gas
/// accounting.
#[inline]
fn fill_state_gas(output: &mut PrecompileOutput, storage: &StorageCtx) {
    if storage.spec().is_enabled_in(crate::BaseBSpec::Azul) && !output.reverted {
        output.gas_refunded = storage.gas_refunded();
    }
}

/// A selector schedule at a given hardfork boundary.
///
/// Before the hardfork activates, selectors in `added` are treated as unknown.
/// After it activates, selectors in `dropped` are treated as unknown.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SelectorSchedule<'a> {
    hardfork: BaseBSpec,
    added: &'a [[u8; 4]],
    dropped: &'a [[u8; 4]],
}

impl<'a> SelectorSchedule<'a> {
    /// Creates a new schedule anchored at `hardfork` with no selectors registered yet.
    pub(crate) const fn new(hardfork: BaseBSpec) -> Self {
        Self { hardfork, added: &[], dropped: &[] }
    }

    /// Registers selectors that are introduced at this hardfork boundary.
    ///
    /// These selectors are treated as unknown BEFORE `hardfork` activates.
    pub(crate) const fn with_added(mut self, selectors: &'a [[u8; 4]]) -> Self {
        self.added = selectors;
        self
    }

    /// Registers selectors that are removed at this hardfork boundary.
    ///
    /// These selectors are treated as unknown ONCE `hardfork` activates.
    pub(crate) const fn with_dropped(mut self, selectors: &'a [[u8; 4]]) -> Self {
        self.dropped = selectors;
        self
    }

    /// Returns `true` if this schedule gates out `selector` under the `active` hardfork.
    #[inline]
    fn rejects(self, selector: [u8; 4], active: BaseBSpec) -> bool {
        if self.hardfork <= active { self.dropped } else { self.added }.contains(&selector)
    }
}

/// Applies hardfork selector schedules, decodes calldata via `decode`, then dispatches to `f`.
///
/// Handles missing selectors (revert on T1+, error on earlier forks), hardfork-gated selectors,
/// unknown selectors (ABI-encoded `UnknownFunctionSelector`), and malformed ABI data (empty
/// revert).
#[inline]
pub(crate) fn dispatch_call<T>(
    calldata: &[u8],
    hardforks: &[SelectorSchedule<'_>],
    decode: impl FnOnce(&[u8]) -> core::result::Result<T, alloy::sol_types::Error>,
    f: impl FnOnce(T) -> PrecompileResult,
) -> PrecompileResult {
    let storage = StorageCtx::default();

    if calldata.len() < 4 {
        return Ok(storage.revert_output(Bytes::new()));
    }

    let selector: [u8; 4] = calldata[..4].try_into().expect("calldata len >= 4");
    if hardforks.iter().any(|schedule| schedule.rejects(selector, storage.spec())) {
        return storage.error_result(error::BasePrecompileError::UnknownFunctionSelector(selector));
    }

    let result = decode(calldata);

    match result {
        Ok(call) => f(call).map(|mut res| {
            // TODO: fix this, each precompile handler should either return output with proper gas values or don't return any gas values at all.
            res.gas_used = storage.gas_used();
            fill_state_gas(&mut res, &storage);
            res
        }),
        Err(alloy::sol_types::Error::UnknownSelector { selector, .. }) => {
            storage.error_result(error::BasePrecompileError::UnknownFunctionSelector(*selector))
        }
        Err(_) => Ok(storage.revert_output(Bytes::new())),
    }
}

/// Asserts that `result` is a reverted output whose bytes decode to `expected_error`.
#[cfg(test)]
pub fn expect_precompile_revert<E>(result: &PrecompileResult, expected_error: E)
where
    E: SolInterface + PartialEq + std::fmt::Debug,
{
    match result {
        Ok(result) => {
            assert!(result.reverted);
            let decoded = E::abi_decode(&result.bytes).unwrap();
            assert_eq!(decoded, expected_error);
        }
        Err(other) => {
            panic!("expected reverted output, got: {other:?}");
        }
    }
}
