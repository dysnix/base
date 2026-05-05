//! Address helpers for Base B precompiles.

use alloy::primitives::Address;

/// B20 token address prefix.
pub const B20_PREFIX_BYTES: [u8; 12] =
    [0x84, 0x53, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];

/// Returns `true` when an address has the B20 prefix.
pub fn is_b20_prefix(address: &Address) -> bool {
    address.as_slice()[..B20_PREFIX_BYTES.len()] == B20_PREFIX_BYTES
}

/// Local address extensions needed by the imported B precompile code.
pub trait BaseBAddressExt {
    /// Returns `true` when an address has the B20 prefix.
    fn is_b20(&self) -> bool;

    /// Returns `true` when an address is a virtual account address.
    fn is_virtual(&self) -> bool;
}

impl BaseBAddressExt for Address {
    fn is_b20(&self) -> bool {
        is_b20_prefix(self)
    }

    fn is_virtual(&self) -> bool {
        false
    }
}
