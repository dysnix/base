pub use IBaseDex::{IBaseDexErrors as BaseDexError, IBaseDexEvents as BaseDexEvent};
use alloy_primitives::{Address, address};

/// Base DEX singleton precompile address.
pub const BASE_DEX_ADDRESS: Address = address!("0x0000000000000000000000000000000000000dE7");
/// Reserved B20 address for the Beryl-deployed Base USD token.
pub const BASE_USD_ADDRESS: Address = address!("0x8453000000000000000000000000000000000000");

crate::sol! {
    /// Singleton Base DEX interface.
    ///
    /// All non-base pools are quoted against Base USD, and non-base to non-base swaps route
    /// through the Base USD reserves.
    #[derive(Debug, PartialEq, Eq)]
    #[sol(abi)]
    interface IBaseDex {
        struct Pool {
            uint128 reserveToken;
            uint128 reserveBase;
        }

        function BASE_TOKEN() external view returns (address);
        function FEE_NUMERATOR() external view returns (uint256);
        function FEE_DENOMINATOR() external view returns (uint256);
        function MINIMUM_LIQUIDITY() external view returns (uint256);

        function initializeBaseToken() external returns (address);

        function getPool(address token) external view returns (Pool memory);
        function pools(address token) external view returns (Pool memory);
        function totalSupply(address token) external view returns (uint256);
        function liquidityBalances(address token, address user) external view returns (uint256);
        function quoteExactInput(address tokenIn, address tokenOut, uint256 amountIn) external view returns (uint256);

        function addLiquidity(
            address token,
            uint256 amountToken,
            uint256 amountBase,
            address to
        ) external returns (uint256 liquidity);

        function removeLiquidity(
            address token,
            uint256 liquidity,
            address to
        ) external returns (uint256 amountToken, uint256 amountBase);

        function swapExactTokensForTokens(
            address tokenIn,
            address tokenOut,
            uint256 amountIn,
            uint256 minAmountOut,
            address to
        ) external returns (uint256 amountOut);

        event Mint(address indexed sender, address indexed token, uint256 amountToken, uint256 amountBase, uint256 liquidity, address indexed to);
        event Burn(address indexed sender, address indexed token, uint256 amountToken, uint256 amountBase, uint256 liquidity, address to);
        event Swap(address indexed sender, address indexed tokenIn, address indexed tokenOut, uint256 amountIn, uint256 amountOut, address to);

        error IdenticalTokens();
        error InvalidToken();
        error InvalidAmount();
        error InsufficientLiquidity();
        error InsufficientOutputAmount();
        error InvalidSwapPath();
    }
}

impl BaseDexError {
    /// Creates an error for identical token addresses.
    pub const fn identical_tokens() -> Self {
        Self::IdenticalTokens(IBaseDex::IdenticalTokens {})
    }

    /// Creates an error for invalid token addresses.
    pub const fn invalid_token() -> Self {
        Self::InvalidToken(IBaseDex::InvalidToken {})
    }

    /// Creates an error for invalid amounts.
    pub const fn invalid_amount() -> Self {
        Self::InvalidAmount(IBaseDex::InvalidAmount {})
    }

    /// Creates an error for insufficient pool liquidity.
    pub const fn insufficient_liquidity() -> Self {
        Self::InsufficientLiquidity(IBaseDex::InsufficientLiquidity {})
    }

    /// Creates an error when output is below the requested minimum.
    pub const fn insufficient_output_amount() -> Self {
        Self::InsufficientOutputAmount(IBaseDex::InsufficientOutputAmount {})
    }

    /// Creates an error for unsupported swap paths.
    pub const fn invalid_swap_path() -> Self {
        Self::InvalidSwapPath(IBaseDex::InvalidSwapPath {})
    }
}
