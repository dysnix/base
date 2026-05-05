pub use IB20Factory::{IB20FactoryErrors as B20FactoryError, IB20FactoryEvents as B20FactoryEvent};
use alloy_primitives::Address;

crate::sol! {
  #[derive(Debug, PartialEq, Eq)]
    #[sol(abi)]
    interface IB20Factory {
        error AddressReserved();
        error AddressNotReserved();
        error InvalidQuoteToken();
        error TokenAlreadyExists(address token);

        event TokenCreated(address indexed token, string name, string symbol, string currency, address quoteToken, address admin, bytes32 salt);

        function createToken(
            string memory name,
            string memory symbol,
            string memory currency,
            address quoteToken,
            address admin,
            bytes32 salt
        ) external returns (address);

        function isB20(address token) public view returns (bool);

        function getTokenAddress(address sender, bytes32 salt) public view returns (address);
    }
}

impl B20FactoryError {
    /// Creates an error when attempting to use a reserved address.
    pub const fn address_reserved() -> Self {
        Self::AddressReserved(IB20Factory::AddressReserved {})
    }

    /// Creates an error when address is not in the reserved range.
    pub const fn address_not_reserved() -> Self {
        Self::AddressNotReserved(IB20Factory::AddressNotReserved {})
    }

    /// Creates an error for invalid quote token.
    pub const fn invalid_quote_token() -> Self {
        Self::InvalidQuoteToken(IB20Factory::InvalidQuoteToken {})
    }

    /// Creates an error when token already exists at the given address.
    pub const fn token_already_exists(token: Address) -> Self {
        Self::TokenAlreadyExists(IB20Factory::TokenAlreadyExists { token })
    }
}
