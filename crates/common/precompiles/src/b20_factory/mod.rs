//! [B20] token factory precompile — deploys new [B20] tokens at deterministic addresses.
//!
//! [B20]: <https://docs.base.xyz/protocol/b20>

pub mod dispatch;

pub use base_precompiles_contracts::{B20FactoryError, B20FactoryEvent, IB20Factory};
use base_precompiles_macros::contract;

use crate::BaseBAddressExt;
use crate::{
    B20_FACTORY_ADDRESS, B20_PREFIX_BYTES, PATH_USD_ADDRESS,
    b20::{B20Error, B20Token, USD_CURRENCY},
    error::{BasePrecompileError, Result},
};
use alloy::{
    primitives::{Address, B256, keccak256},
    sol_types::SolValue,
};
use tracing::trace;

/// Number of reserved addresses (0 to RESERVED_SIZE-1) that cannot be deployed via factory
const RESERVED_SIZE: u64 = 1024;

/// Factory contract for deploying new B20 tokens at deterministic addresses.
///
/// Tokens are deployed at `B20_PREFIX || keccak256(sender, salt)[..8]`.
/// The first 1024 addresses are reserved for protocol-deployed tokens.
///
/// The struct fields define the on-chain storage layout; the `#[contract]` macro generates the
/// storage handlers which provide an ergonomic way to interact with the EVM state.
#[contract(addr = B20_FACTORY_ADDRESS)]
pub struct B20Factory {}

/// Computes the deterministic B20 address from sender and salt.
/// Returns the address and the lower bytes used for derivation.
#[cfg_attr(test, allow(dead_code))]
pub(crate) fn compute_b20_address(sender: Address, salt: B256) -> (Address, u64) {
    let hash = keccak256((sender, salt).abi_encode());

    // Take first 8 bytes of hash as lower bytes
    let mut padded = [0u8; 8];
    padded.copy_from_slice(&hash[..8]);
    let lower_bytes = u64::from_be_bytes(padded);

    // Construct the address: B20_PREFIX (12 bytes) || hash[..8] (8 bytes)
    let mut address_bytes = [0u8; 20];
    address_bytes[..12].copy_from_slice(&B20_PREFIX_BYTES);
    address_bytes[12..].copy_from_slice(&hash[..8]);

    (Address::from(address_bytes), lower_bytes)
}

// Precompile functions
impl B20Factory {
    /// Initializes the B20 factory precompile.
    pub fn initialize(&mut self) -> Result<()> {
        self.__initialize()
    }

    /// Computes the deterministic address for a token given `sender` and `salt`. Reverts if the
    /// derived address falls within the reserved range (lower 8 bytes < `RESERVED_SIZE`).
    ///
    /// # Errors
    /// - `AddressReserved` — the derived address is in the reserved range
    pub fn get_token_address(&self, call: IB20Factory::getTokenAddressCall) -> Result<Address> {
        let (address, lower_bytes) = compute_b20_address(call.sender, call.salt);

        // Check if address would be in reserved range
        if lower_bytes < RESERVED_SIZE {
            return Err(BasePrecompileError::B20Factory(B20FactoryError::address_reserved()));
        }

        Ok(address)
    }

    /// Returns `true` if `token` has the correct B20 prefix and has code deployed.
    pub fn is_b20(&self, token: Address) -> Result<bool> {
        if !token.is_b20() {
            return Ok(false);
        }
        // Check if the token has code deployed (non-empty code hash)
        self.storage.with_account_info(token, |info| Ok(!info.is_empty_code_hash()))
    }

    /// Deploys a new B20 token at a deterministic address derived from `sender` and `salt`.
    ///
    /// Validates that the token does not already exist, the quote token is a deployed B20 of
    /// a compatible currency, and the derived address is outside the reserved range. Initializes
    /// the token via [`B20Token::initialize`].
    ///
    /// # Errors
    /// - `TokenAlreadyExists` — a B20 is already deployed at the derived address
    /// - `InvalidQuoteToken` — quote token is not a deployed B20 or has incompatible currency
    /// - `AddressReserved` — the derived address is in the reserved range
    pub fn create_token(
        &mut self,
        sender: Address,
        call: IB20Factory::createTokenCall,
    ) -> Result<Address> {
        trace!(%sender, ?call, "Create token");

        // Compute the deterministic address from sender and salt
        let (token_address, lower_bytes) = compute_b20_address(sender, call.salt);

        if self.is_b20(token_address)? {
            return Err(BasePrecompileError::B20Factory(B20FactoryError::token_already_exists(
                token_address,
            )));
        }

        let is_root_usd_token = call.currency == USD_CURRENCY && call.quoteToken.is_zero();

        // Ensure that the quote token is a valid B20 that is currently deployed.
        if !is_root_usd_token && !self.is_b20(call.quoteToken)? {
            return Err(B20Error::invalid_quote_token().into());
        }

        // If token is USD, its quote token must also be USD
        if !is_root_usd_token
            && call.currency == USD_CURRENCY
            && B20Token::from_address(call.quoteToken)?.currency()? != USD_CURRENCY
        {
            return Err(B20Error::invalid_quote_token().into());
        }

        // Check if address is in reserved range
        if lower_bytes < RESERVED_SIZE {
            return Err(BasePrecompileError::B20Factory(B20FactoryError::address_reserved()));
        }

        B20Token::from_address(token_address)?.initialize(
            sender,
            &call.name,
            &call.symbol,
            &call.currency,
            call.quoteToken,
            call.admin,
        )?;

        self.emit_event(B20FactoryEvent::TokenCreated(IB20Factory::TokenCreated {
            token: token_address,
            name: call.name,
            symbol: call.symbol,
            currency: call.currency,
            quoteToken: call.quoteToken,
            admin: call.admin,
            salt: call.salt,
        }))?;

        Ok(token_address)
    }

    /// Deploys a B20 token at a reserved address (lower 8 bytes < `RESERVED_SIZE`). Used
    /// during genesis or hardforks to bootstrap protocol tokens like pathUSD.
    ///
    /// # Errors
    /// - `InvalidToken` — `address` does not have the B20 prefix
    /// - `TokenAlreadyExists` — a B20 is already deployed at `address`
    /// - `InvalidQuoteToken` — quote token is invalid, not deployed, or has incompatible
    ///   currency; pathUSD must use `Address::ZERO` as quote token
    /// - `AddressNotReserved` — the address is outside the reserved range
    pub fn create_token_reserved_address(
        &mut self,
        address: Address,
        name: &str,
        symbol: &str,
        currency: &str,
        quote_token: Address,
        admin: Address,
    ) -> Result<Address> {
        // Validate that the address has a B20 prefix
        if !address.is_b20() {
            return Err(B20Error::invalid_token().into());
        }

        // Validate that the address is not already deployed
        if self.is_b20(address)? {
            return Err(BasePrecompileError::B20Factory(B20FactoryError::token_already_exists(
                address,
            )));
        }

        // quote_token must be address(0) or a valid B20
        if !quote_token.is_zero() {
            // pathUSD must set address(0) as the quote token
            // or the b20 must be a valid deployed token
            if address == PATH_USD_ADDRESS || !self.is_b20(quote_token)? {
                return Err(B20Error::invalid_quote_token().into());
            }
            // If token is USD, its quote token must also be USD
            if currency == USD_CURRENCY
                && B20Token::from_address(quote_token)?.currency()? != USD_CURRENCY
            {
                return Err(B20Error::invalid_quote_token().into());
            }
        }

        // Validate that the address is within the reserved range
        // Reserved addresses have their last 8 bytes represent a value < RESERVED_SIZE
        let mut padded = [0u8; 8];
        padded.copy_from_slice(&address.as_slice()[12..]);
        let lower_bytes = u64::from_be_bytes(padded);
        if lower_bytes >= RESERVED_SIZE {
            return Err(BasePrecompileError::B20Factory(B20FactoryError::address_not_reserved()));
        }

        let mut token = B20Token::from_address(address)?;
        token.initialize(admin, name, symbol, currency, quote_token, admin)?;

        self.emit_event(B20FactoryEvent::TokenCreated(IB20Factory::TokenCreated {
            token: address,
            name: name.into(),
            symbol: symbol.into(),
            currency: currency.into(),
            quoteToken: quote_token,
            admin,
            salt: B256::ZERO,
        }))?;

        Ok(address)
    }
}
