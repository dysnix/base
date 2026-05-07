pub use IB20::{IB20Errors as B20Error, IB20Events as B20Event};
pub use IRolesAuth::{IRolesAuthErrors as RolesAuthError, IRolesAuthEvents as RolesAuthEvent};
use alloy_primitives::{Address, U256};
use alloy_sol_types::{SolCall, SolType};

crate::sol! {
    #[derive(Debug, PartialEq, Eq)]
    #[sol(abi)]
    interface IRolesAuth {
        function hasRole(address account, bytes32 role) external view returns (bool);
        function getRoleAdmin(bytes32 role) external view returns (bytes32);
        function grantRole(bytes32 role, address account) external;
        function revokeRole(bytes32 role, address account) external;
        function renounceRole(bytes32 role) external;
        function setRoleAdmin(bytes32 role, bytes32 adminRole) external;

        event RoleMembershipUpdated(bytes32 indexed role, address indexed account, address indexed sender, bool hasRole);
        event RoleAdminUpdated(bytes32 indexed role, bytes32 indexed newAdminRole, address indexed sender);

        error Unauthorized();
    }
}

crate::sol! {
    /// B20 token interface providing standard ERC20 functionality with Base-specific extensions.
    ///
    /// B20 tokens extend the ERC20 standard with:
    /// - Currency denomination support for real-world asset backing
    /// - Transfer policy enforcement for compliance
    /// - Supply caps for controlled token issuance
    /// - Pause/unpause functionality for emergency controls
    /// - Memo support for transaction context
    /// The interface supports both standard token operations and administrative functions
    /// for managing token behavior and compliance requirements.
    #[derive(Debug, PartialEq, Eq)]
    #[sol(abi)]
    #[allow(clippy::too_many_arguments)]
    interface IB20 {
        // Standard token functions
        function name() external view returns (string memory);
        function symbol() external view returns (string memory);
        function decimals() external view returns (uint8);
        function totalSupply() external view returns (uint256);
        function balanceOf(address account) external view returns (uint256);
        function transfer(address to, uint256 amount) external returns (bool);
        function approve(address spender, uint256 amount) external returns (bool);
        function allowance(address owner, address spender) external view returns (uint256);
        function transferFrom(address from, address to, uint256 amount) external returns (bool);
        function mint(address to, uint256 amount) external;
        function burn(uint256 amount) external;

        // B20 Extension
        function currency() external view returns (string memory);
        function supplyCap() external view returns (uint256);
        function paused() external view returns (bool);
        function transferPolicyId() external view returns (uint64);
        function burnBlocked(address from, uint256 amount) external;
        function mintWithMemo(address to, uint256 amount, bytes32 memo) external;
        function burnWithMemo(uint256 amount, bytes32 memo) external;
        function transferWithMemo(address to, uint256 amount, bytes32 memo) external;
        function transferFromWithMemo(address from, address to, uint256 amount, bytes32 memo) external returns (bool);

        // Admin Functions
        function changeTransferPolicyId(uint64 newPolicyId) external;
        function setSupplyCap(uint256 newSupplyCap) external;
        function pause() external;
        function unpause() external;

        /// @notice Returns the role identifier for pausing the contract
        /// @return The pause role identifier
        function PAUSE_ROLE() external view returns (bytes32);

        /// @notice Returns the role identifier for unpausing the contract
        /// @return The unpause role identifier
        function UNPAUSE_ROLE() external view returns (bytes32);

        /// @notice Returns the role identifier for issuing tokens
        /// @return The issuer role identifier
        function ISSUER_ROLE() external view returns (bytes32);

        /// @notice Returns the role identifier for burning tokens from blocked accounts
        /// @return The burn blocked role identifier
        function BURN_BLOCKED_ROLE() external view returns (bytes32);

        // EIP-2612 Permit Functions
        function permit(address owner, address spender, uint256 value, uint256 deadline, uint8 v, bytes32 r, bytes32 s) external;
        function nonces(address owner) external view returns (uint256);
        function DOMAIN_SEPARATOR() external view returns (bytes32);

        // Events
        event Transfer(address indexed from, address indexed to, uint256 amount);
        event Approval(address indexed owner, address indexed spender, uint256 amount);
        event Mint(address indexed to, uint256 amount);
        event Burn(address indexed from, uint256 amount);
        event BurnBlocked(address indexed from, uint256 amount);
        event TransferWithMemo(address indexed from, address indexed to, uint256 amount, bytes32 indexed memo);
        event TransferPolicyUpdate(address indexed updater, uint64 indexed newPolicyId);
        event SupplyCapUpdate(address indexed updater, uint256 indexed newSupplyCap);
        event PauseStateUpdate(address indexed updater, bool isPaused);

        // Errors
        error InsufficientBalance(uint256 available, uint256 required, address token);
        error InsufficientAllowance();
        error SupplyCapExceeded();
        error InvalidSupplyCap();
        error InvalidPayload();
        error StringTooLong();
        error PolicyForbids();
        error InvalidRecipient();
        error ContractPaused();
        error InvalidCurrency();
        error TransfersDisabled();
        error InvalidAmount();
        error Unauthorized();
        error ProtectedAddress();
        error InvalidToken();
        error Uninitialized();
        error InvalidTransferPolicyId();
        error PermitExpired();
        error InvalidSignature();
    }
}

impl IB20::IB20Calls {
    /// Returns `true` if `input` matches one of the recognized [B20 payment] selectors:
    /// - `transfer` / `transferWithMemo`
    /// - `transferFrom` / `transferFromWithMemo`
    /// - `mint` / `mintWithMemo`
    /// - `burn` / `burnWithMemo`
    ///
    /// # NOTES
    /// - Only validates calldata; the caller must check the B20 address prefix on `to`.
    /// - Only selector and exact ABI-encoded length match, no decoding (better performance).
    ///
    /// [B20 payment]: <https://docs.base.xyz/protocol/b20/overview#get-predictable-payment-fees>
    pub fn is_payment(input: &[u8]) -> bool {
        fn is_call<C: SolCall>(input: &[u8]) -> bool {
            input.first_chunk::<4>() == Some(&C::SELECTOR)
                && input.len()
                    == 4 + <C::Parameters<'_> as SolType>::ENCODED_SIZE.unwrap_or_default()
        }

        is_call::<IB20::transferCall>(input)
            || is_call::<IB20::transferWithMemoCall>(input)
            || is_call::<IB20::transferFromCall>(input)
            || is_call::<IB20::transferFromWithMemoCall>(input)
            || is_call::<IB20::approveCall>(input)
            || is_call::<IB20::mintCall>(input)
            || is_call::<IB20::mintWithMemoCall>(input)
            || is_call::<IB20::burnCall>(input)
            || is_call::<IB20::burnWithMemoCall>(input)
    }
}

impl RolesAuthError {
    /// Creates an error for unauthorized access.
    pub const fn unauthorized() -> Self {
        Self::Unauthorized(IRolesAuth::Unauthorized {})
    }
}

impl B20Error {
    /// Creates an error for insufficient token balance.
    pub const fn insufficient_balance(available: U256, required: U256, token: Address) -> Self {
        Self::InsufficientBalance(IB20::InsufficientBalance { available, required, token })
    }

    /// Creates an error for insufficient spending allowance.
    pub const fn insufficient_allowance() -> Self {
        Self::InsufficientAllowance(IB20::InsufficientAllowance {})
    }

    /// Creates an error for unauthorized callers
    pub const fn unauthorized() -> Self {
        Self::Unauthorized(IB20::Unauthorized {})
    }

    /// Creates an error when minting would set a supply cap that is too large, or invalid.
    pub const fn invalid_supply_cap() -> Self {
        Self::InvalidSupplyCap(IB20::InvalidSupplyCap {})
    }

    /// Creates an error when minting would exceed supply cap.
    pub const fn supply_cap_exceeded() -> Self {
        Self::SupplyCapExceeded(IB20::SupplyCapExceeded {})
    }

    /// Creates an error for invalid payload data.
    pub const fn invalid_payload() -> Self {
        Self::InvalidPayload(IB20::InvalidPayload {})
    }

    /// Creates an error when string parameter exceeds maximum length.
    pub const fn string_too_long() -> Self {
        Self::StringTooLong(IB20::StringTooLong {})
    }

    /// Creates an error when transfer is forbidden by policy.
    pub const fn policy_forbids() -> Self {
        Self::PolicyForbids(IB20::PolicyForbids {})
    }

    /// Creates an error for invalid recipient address.
    pub const fn invalid_recipient() -> Self {
        Self::InvalidRecipient(IB20::InvalidRecipient {})
    }

    /// Creates an error when contract is paused.
    pub const fn contract_paused() -> Self {
        Self::ContractPaused(IB20::ContractPaused {})
    }

    /// Creates an error for invalid currency.
    pub const fn invalid_currency() -> Self {
        Self::InvalidCurrency(IB20::InvalidCurrency {})
    }

    /// Creates an error for transfers being disabled.
    pub const fn transfers_disabled() -> Self {
        Self::TransfersDisabled(IB20::TransfersDisabled {})
    }

    /// Creates an error for invalid amount.
    pub const fn invalid_amount() -> Self {
        Self::InvalidAmount(IB20::InvalidAmount {})
    }

    /// Error for operations on protected addresses (like burning `FeeManager` tokens)
    pub const fn protected_address() -> Self {
        Self::ProtectedAddress(IB20::ProtectedAddress {})
    }

    /// Error when an address is not a valid B20 token
    pub const fn invalid_token() -> Self {
        Self::InvalidToken(IB20::InvalidToken {})
    }

    /// Error when transfer policy ID does not exist
    pub const fn invalid_transfer_policy_id() -> Self {
        Self::InvalidTransferPolicyId(IB20::InvalidTransferPolicyId {})
    }

    /// Error when token is uninitialized (has no bytecode)
    pub const fn uninitialized() -> Self {
        Self::Uninitialized(IB20::Uninitialized {})
    }

    /// Error when permit signature has expired (block.timestamp > deadline)
    pub const fn permit_expired() -> Self {
        Self::PermitExpired(IB20::PermitExpired {})
    }

    /// Error when permit signature is invalid
    pub const fn invalid_signature() -> Self {
        Self::InvalidSignature(IB20::InvalidSignature {})
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use alloy_primitives::{Address, B256, U256};
    use std::vec::Vec;

    #[rustfmt::skip]
    /// Returns valid ABI-encoded calldata for every recognized B20 payment selector.
    fn payment_calldatas() -> [Vec<u8>; 9] {
        let (to, from, amount, memo) = (Address::with_last_byte(1), Address::with_last_byte(2), U256::from(3), B256::repeat_byte(4));

        [
            IB20::transferCall { to, amount }.abi_encode(),
            IB20::transferWithMemoCall { to, amount, memo }.abi_encode(),
            IB20::transferFromCall { from, to, amount }.abi_encode(),
            IB20::transferFromWithMemoCall { from, to, amount, memo }.abi_encode(),
            IB20::approveCall { spender: to, amount }.abi_encode(),
            IB20::mintCall { to, amount }.abi_encode(),
            IB20::mintWithMemoCall { to, amount, memo }.abi_encode(),
            IB20::burnCall { amount }.abi_encode(),
            IB20::burnWithMemoCall { amount, memo }.abi_encode(),
        ]
    }

    #[rustfmt::skip]
    /// Returns ABI-encoded calldata for B20 selectors NOT recognized as payments.
    fn non_payment_calldatas() -> [Vec<u8>; 3] {
        let mut data = IB20::transferCall { to: Address::with_last_byte(1), amount: U256::from(3) }.abi_encode();
        data[..4].copy_from_slice(&[0xde, 0xad, 0xbe, 0xef]);

        [
            // non-payment B20 calls with known selectors
            IB20::pauseCall {}.abi_encode(),
            IB20::permitCall {
                owner: Address::with_last_byte(1), spender: Address::with_last_byte(2), value: U256::from(3), deadline: U256::from(4),
                v: u8::MAX, r: B256::repeat_byte(5), s: B256::repeat_byte(6) }.abi_encode(),
            // non-payment B20 calls with unknown selectors
            data,
        ]
    }

    #[test]
    fn test_is_payment() {
        for calldata in payment_calldatas() {
            assert!(IB20::IB20Calls::is_payment(&calldata))
        }

        for calldata in non_payment_calldatas() {
            assert!(!IB20::IB20Calls::is_payment(&calldata))
        }
    }
}
