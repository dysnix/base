//! ABI bindings and constants for Base B precompiles.

use alloy_primitives::{Address, address};

mod common_errors;
pub use common_errors::*;
mod b20;
pub use b20::*;
mod base_dex;
pub use base_dex::*;
mod b20_factory;
pub use b20_factory::*;
mod b403_registry;
pub use b403_registry::*;

/// Base POC B403 registry precompile address.
pub const B403_REGISTRY_ADDRESS: Address = address!("0x8453000000000000000000000000000000000403");
/// Base POC B20 factory precompile address.
pub const B20_FACTORY_ADDRESS: Address = address!("0x8453000000000000000000000000000000000001");
