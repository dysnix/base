//! Unified error handling for Base precompiles.
//!
//! Provides [`BasePrecompileError`] — the top-level error enum — along with an
//! ABI-selector-based decoder registry for mapping raw revert bytes back to
//! typed error variants.

use std::{
    collections::HashMap,
    sync::{Arc, LazyLock},
};

use crate::b20::B20Error;
use alloy::{
    primitives::{Selector, U256},
    sol_types::{Panic, PanicKind, SolError, SolInterface},
};
use alloy_evm::EvmInternalsError;
use base_precompiles_contracts::{
    B20FactoryError, B403RegistryError, RolesAuthError, UnknownFunctionSelector,
};
use revm::{
    context::journaled_state::JournalLoadError,
    precompile::{PrecompileError, PrecompileOutput, PrecompileResult},
};

/// Top-level error type for all Base precompile operations
#[derive(
    Debug, Clone, PartialEq, Eq, thiserror::Error, derive_more::From, derive_more::TryInto,
)]
pub enum BasePrecompileError {
    /// Error from B20 token
    #[error("B20 token error: {0:?}")]
    B20(B20Error),

    /// Error from B20 factory
    #[error("B20 factory error: {0:?}")]
    B20Factory(B20FactoryError),

    /// Error from roles auth
    #[error("Roles auth error: {0:?}")]
    RolesAuthError(RolesAuthError),

    /// Error from 403 registry
    #[error("B403 registry error: {0:?}")]
    B403RegistryError(B403RegistryError),

    /// EVM panic (i.e. arithmetic under/overflow, out-of-bounds access).
    #[error("Panic({0:?})")]
    Panic(PanicKind),

    /// Gas limit exceeded during precompile execution.
    #[error("Gas limit exceeded")]
    OutOfGas,

    /// The calldata's 4-byte selector does not match any known precompile function.
    #[error("Unknown function selector: {0:?}")]
    UnknownFunctionSelector([u8; 4]),

    /// Unrecoverable internal error (e.g. database failure).
    #[error("Fatal precompile error: {0:?}")]
    #[from(skip)]
    Fatal(String),
}

impl From<EvmInternalsError> for BasePrecompileError {
    fn from(value: EvmInternalsError) -> Self {
        match value {
            EvmInternalsError::Database(e) => Self::Fatal(e.to_string()),
        }
    }
}

impl From<JournalLoadError<EvmInternalsError>> for BasePrecompileError {
    fn from(value: JournalLoadError<EvmInternalsError>) -> Self {
        match value {
            JournalLoadError::DBError(e) => Self::from(e),
            JournalLoadError::ColdLoadSkipped => Self::OutOfGas,
        }
    }
}

impl From<JournalLoadError<revm::context::ErasedError>> for BasePrecompileError {
    fn from(value: JournalLoadError<revm::context::ErasedError>) -> Self {
        match value {
            JournalLoadError::DBError(e) => Self::Fatal(e.to_string()),
            JournalLoadError::ColdLoadSkipped => Self::OutOfGas,
        }
    }
}

/// Result type alias for Base precompile operations
pub type Result<T> = std::result::Result<T, BasePrecompileError>;

impl BasePrecompileError {
    /// Returns true if this error represents a system-level failure that must be propagated
    /// rather than swallowed, because state may be inconsistent.
    pub fn is_system_error(&self) -> bool {
        match self {
            Self::OutOfGas | Self::Fatal(_) | Self::Panic(_) => true,
            Self::B20(_)
            | Self::B20Factory(_)
            | Self::RolesAuthError(_)
            | Self::B403RegistryError(_)
            | Self::UnknownFunctionSelector(_) => false,
        }
    }

    /// Creates an arithmetic under/overflow panic error.
    pub fn under_overflow() -> Self {
        Self::Panic(PanicKind::UnderOverflow)
    }

    /// Creates an enum conversion error panic (Solidity Panic `0x21`).
    pub fn enum_conversion_error() -> Self {
        Self::Panic(PanicKind::EnumConversionError)
    }

    /// Creates an array out-of-bounds panic error.
    pub fn array_oob() -> Self {
        Self::Panic(PanicKind::ArrayOutOfBounds)
    }

    /// ABI-encodes this error and wraps it as a reverted [`PrecompileResult`].
    pub fn into_precompile_result(self, gas: u64, _reservoir: u64) -> PrecompileResult {
        let bytes = match self {
            Self::B20(e) => e.abi_encode().into(),
            Self::B20Factory(e) => e.abi_encode().into(),
            Self::RolesAuthError(e) => e.abi_encode().into(),
            Self::B403RegistryError(e) => e.abi_encode().into(),
            Self::Panic(kind) => {
                let panic = Panic { code: U256::from(kind as u32) };

                panic.abi_encode().into()
            }
            Self::OutOfGas => {
                return Err(PrecompileError::OutOfGas);
            }
            Self::UnknownFunctionSelector(selector) => {
                UnknownFunctionSelector { selector: selector.into() }.abi_encode().into()
            }
            Self::Fatal(msg) => {
                return Err(PrecompileError::Fatal(msg));
            }
        };
        Ok(PrecompileOutput::new_reverted(gas, bytes))
    }
}

/// Registers all ABI error selectors for a [`SolInterface`] type into the decoder registry.
pub fn add_errors_to_registry<T: SolInterface>(
    registry: &mut BasePrecompileErrorRegistry,
    converter: impl Fn(T) -> BasePrecompileError + 'static + Send + Sync,
) {
    let converter = Arc::new(converter);
    for selector in T::selectors() {
        let converter = Arc::clone(&converter);
        registry.insert(
            selector.into(),
            Box::new(move |data: &[u8]| {
                T::abi_decode(data).ok().map(|error| DecodedBasePrecompileError {
                    error: converter(error),
                    revert_bytes: data,
                })
            }),
        );
    }
}

/// A decoded precompile error together with the raw revert bytes.
pub struct DecodedBasePrecompileError<'a> {
    pub error: BasePrecompileError,
    pub revert_bytes: &'a [u8],
}

/// Maps ABI error selectors to their decoder functions.
pub type BasePrecompileErrorRegistry = HashMap<
    Selector,
    Box<dyn for<'a> Fn(&'a [u8]) -> Option<DecodedBasePrecompileError<'a>> + Send + Sync>,
>;

/// Builds a [`BasePrecompileErrorRegistry`] mapping every known error selector to its decoder.
pub fn error_decoder_registry() -> BasePrecompileErrorRegistry {
    let mut registry: BasePrecompileErrorRegistry = HashMap::new();

    add_errors_to_registry(&mut registry, BasePrecompileError::B20);
    add_errors_to_registry(&mut registry, BasePrecompileError::B20Factory);
    add_errors_to_registry(&mut registry, BasePrecompileError::RolesAuthError);
    add_errors_to_registry(&mut registry, BasePrecompileError::B403RegistryError);

    registry
}

/// Global lazily-initialized registry of all Base precompile error decoders.
pub static ERROR_REGISTRY: LazyLock<BasePrecompileErrorRegistry> =
    LazyLock::new(error_decoder_registry);

/// Decodes raw revert bytes into a typed [`DecodedBasePrecompileError`] using the global
/// [`ERROR_REGISTRY`], returning `None` if the data is shorter than 4 bytes or the selector
/// is unrecognized.
pub fn decode_error<'a>(data: &'a [u8]) -> Option<DecodedBasePrecompileError<'a>> {
    if data.len() < 4 {
        return None;
    }

    let selector: [u8; 4] = data[0..4].try_into().ok()?;
    ERROR_REGISTRY.get(&selector).and_then(|decoder| decoder(data))
}

/// Extension trait to convert `Result<T, BasePrecompileError>` into a [`PrecompileResult`].
pub trait IntoPrecompileResult<T> {
    /// Converts `self` into a [`PrecompileResult`], using `encode_ok` for the success path.
    fn into_precompile_result(
        self,
        gas: u64,
        _reservoir: u64,
        encode_ok: impl FnOnce(T) -> alloy::primitives::Bytes,
    ) -> PrecompileResult;
}

impl<T> IntoPrecompileResult<T> for Result<T> {
    fn into_precompile_result(
        self,
        gas: u64,
        reservoir: u64,
        encode_ok: impl FnOnce(T) -> alloy::primitives::Bytes,
    ) -> PrecompileResult {
        match self {
            Ok(res) => Ok(PrecompileOutput::new(gas, encode_ok(res))),
            Err(err) => err.into_precompile_result(gas, reservoir),
        }
    }
}
