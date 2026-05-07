//! [B20] token standard — Base's native fungible token implementation.
//!
//! Provides ERC-20-like balances, allowances, and transfers with Base extensions:
//! role-based access control, pausability, supply caps, transfer policies ([B403]),
//! EIP-2612 permits (T2+), and virtual addresses ([B1022]).
//!
//! [B20]: <https://docs.base.xyz/protocol/b20>
//! [B403]: <https://docs.base.xyz/protocol/b403>
//! [B1022]: <https://docs.base.xyz/protocol/b1022>

pub mod dispatch;
pub mod roles;

use std::sync::LazyLock;

use alloy::{
    primitives::{Address, B256, U256, keccak256, uint},
    sol_types::SolValue,
};
pub use base_precompiles_contracts::{
    B20Error, B20Event, IB20, IRolesAuth, RolesAuthError, RolesAuthEvent,
};
use base_precompiles_macros::contract;
// Re-export the generated slots module for external access to storage slot constants
pub use slots as b20_slots;
use tracing::trace;

pub use crate::is_b20_prefix;
use crate::{
    BaseBAddressExt,
    b20::roles::DEFAULT_ADMIN_ROLE,
    b403_registry::{AuthRole, B403Registry, IB403Registry},
    error::{BasePrecompileError, Result},
    storage::{Handler, Mapping},
};

/// u128::MAX as U256
pub const U128_MAX: U256 = uint!(0xffffffffffffffffffffffffffffffff_U256);

/// B20 token contract — the native token standard on Base.
///
/// Implements ERC-20-like functionality (balances, allowances, transfers) with additional
/// features: role-based access control, pausability, supply caps, transfer policies ([B403]),
/// and virtual addresses ([B1022]).
///
/// [B403]: <https://docs.base.xyz/protocol/b403>
/// [B1022]: <https://docs.base.xyz/protocol/b1022>
///
/// Each token lives at a deterministic address with the `0x20C0` prefix.
///
/// The struct fields define the on-chain storage layout; the `#[contract]` macro generates the
/// storage handlers which provide an ergonomic way to interact with the EVM state.
#[contract]
pub struct B20Token {
    // RolesAuth
    roles: Mapping<Address, Mapping<B256, bool>>,
    role_admins: Mapping<B256, B256>,

    // B20 Metadata
    name: String,
    symbol: String,
    currency: String,
    // Unused slot, kept for storage layout compatibility
    _domain_separator: B256,
    transfer_policy_id: u64,

    // B20 Token
    total_supply: U256,
    balances: Mapping<Address, U256>,
    allowances: Mapping<Address, Mapping<Address, U256>>,
    permit_nonces: Mapping<Address, U256>,
    paused: bool,
    supply_cap: U256,
    // Unused slot, kept for storage layout compatibility
    _salts: Mapping<B256, bool>,
}

/// EIP-712 Permit typehash: keccak256("Permit(address owner,address spender,uint256 value,uint256 nonce,uint256 deadline)")
pub static PERMIT_TYPEHASH: LazyLock<B256> = LazyLock::new(|| {
    keccak256(b"Permit(address owner,address spender,uint256 value,uint256 nonce,uint256 deadline)")
});

/// EIP-712 domain separator typehash
pub static EIP712_DOMAIN_TYPEHASH: LazyLock<B256> = LazyLock::new(|| {
    keccak256(b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)")
});

/// EIP-712 version hash: keccak256("1")
pub static VERSION_HASH: LazyLock<B256> = LazyLock::new(|| keccak256(b"1"));

/// Role hash for pausing token transfers.
pub static PAUSE_ROLE: LazyLock<B256> = LazyLock::new(|| keccak256(b"PAUSE_ROLE"));
/// Role hash for unpausing token transfers.
pub static UNPAUSE_ROLE: LazyLock<B256> = LazyLock::new(|| keccak256(b"UNPAUSE_ROLE"));
/// Role hash for minting new tokens.
pub static ISSUER_ROLE: LazyLock<B256> = LazyLock::new(|| keccak256(b"ISSUER_ROLE"));
/// Role hash that authorizes burning tokens from blocked accounts.
pub static BURN_BLOCKED_ROLE: LazyLock<B256> = LazyLock::new(|| keccak256(b"BURN_BLOCKED_ROLE"));

impl B20Token {
    /// Returns the token name.
    pub fn name(&self) -> Result<String> {
        self.name.read()
    }

    /// Returns the token symbol.
    pub fn symbol(&self) -> Result<String> {
        self.symbol.read()
    }

    /// Returns the token decimals (always 6 for B20).
    pub fn decimals(&self) -> Result<u8> {
        Ok(6)
    }

    /// Returns the token's currency denomination (e.g. `"USD"`).
    pub fn currency(&self) -> Result<String> {
        self.currency.read()
    }

    /// Returns the current total supply.
    pub fn total_supply(&self) -> Result<U256> {
        self.total_supply.read()
    }

    /// Returns the maximum mintable supply.
    pub fn supply_cap(&self) -> Result<U256> {
        self.supply_cap.read()
    }

    /// Returns whether the token is currently paused.
    pub fn paused(&self) -> Result<bool> {
        self.paused.read()
    }

    /// Returns the B403 transfer policy ID governing this token's transfers.
    pub fn transfer_policy_id(&self) -> Result<u64> {
        self.transfer_policy_id.read()
    }

    /// Returns the PAUSE_ROLE constant
    ///
    /// This role identifier grants permission to pause the token contract.
    /// The role is computed as `keccak256("PAUSE_ROLE")`.
    pub fn pause_role() -> B256 {
        *PAUSE_ROLE
    }

    /// Returns the UNPAUSE_ROLE constant
    ///
    /// This role identifier grants permission to unpause the token contract.
    /// The role is computed as `keccak256("UNPAUSE_ROLE")`.
    pub fn unpause_role() -> B256 {
        *UNPAUSE_ROLE
    }

    /// Returns the ISSUER_ROLE constant
    ///
    /// This role identifier grants permission to mint and burn tokens.
    /// The role is computed as `keccak256("ISSUER_ROLE")`.
    pub fn issuer_role() -> B256 {
        *ISSUER_ROLE
    }

    /// Returns the BURN_BLOCKED_ROLE constant
    ///
    /// This role identifier grants permission to burn tokens from blocked accounts.
    /// The role is computed as `keccak256("BURN_BLOCKED_ROLE")`.
    pub fn burn_blocked_role() -> B256 {
        *BURN_BLOCKED_ROLE
    }

    /// Returns the token balance of `account`.
    pub fn balance_of(&self, call: IB20::balanceOfCall) -> Result<U256> {
        self.balances[call.account].read()
    }

    /// Returns the remaining allowance that `spender` can transfer on behalf of `owner`.
    pub fn allowance(&self, call: IB20::allowanceCall) -> Result<U256> {
        self.allowances[call.owner][call.spender].read()
    }

    /// Updates the [`B403Registry`] transfer policy governing this token's transfers.
    ///
    /// # Errors
    /// - `Unauthorized` — caller does not hold `DEFAULT_ADMIN_ROLE`
    /// - `InvalidTransferPolicyId` — policy does not exist in the [`B403Registry`]
    pub fn change_transfer_policy_id(
        &mut self,
        msg_sender: Address,
        call: IB20::changeTransferPolicyIdCall,
    ) -> Result<()> {
        self.check_role(msg_sender, DEFAULT_ADMIN_ROLE)?;

        // Validate that the policy exists
        if !B403Registry::new()
            .policy_exists(IB403Registry::policyExistsCall { policyId: call.newPolicyId })?
        {
            return Err(B20Error::invalid_transfer_policy_id().into());
        }

        self.transfer_policy_id.write(call.newPolicyId)?;

        self.emit_event(B20Event::TransferPolicyUpdate(IB20::TransferPolicyUpdate {
            updater: msg_sender,
            newPolicyId: call.newPolicyId,
        }))
    }

    /// Sets a new supply cap. Must be ≥ current total supply and ≤ [`U128_MAX`].
    ///
    /// # Errors
    /// - `Unauthorized` — caller does not hold `DEFAULT_ADMIN_ROLE`
    /// - `InvalidSupplyCap` — new cap is below current total supply
    /// - `SupplyCapExceeded` — new cap exceeds [`U128_MAX`]
    pub fn set_supply_cap(
        &mut self,
        msg_sender: Address,
        call: IB20::setSupplyCapCall,
    ) -> Result<()> {
        self.check_role(msg_sender, DEFAULT_ADMIN_ROLE)?;
        if call.newSupplyCap < self.total_supply()? {
            return Err(B20Error::invalid_supply_cap().into());
        }

        if call.newSupplyCap > U128_MAX {
            return Err(B20Error::supply_cap_exceeded().into());
        }

        self.supply_cap.write(call.newSupplyCap)?;

        self.emit_event(B20Event::SupplyCapUpdate(IB20::SupplyCapUpdate {
            updater: msg_sender,
            newSupplyCap: call.newSupplyCap,
        }))
    }

    /// Pauses all token transfers.
    ///
    /// # Errors
    /// - `Unauthorized` — caller does not hold `PAUSE_ROLE`
    pub fn pause(&mut self, msg_sender: Address, _call: IB20::pauseCall) -> Result<()> {
        self.check_role(msg_sender, *PAUSE_ROLE)?;
        self.paused.write(true)?;

        self.emit_event(B20Event::PauseStateUpdate(IB20::PauseStateUpdate {
            updater: msg_sender,
            isPaused: true,
        }))
    }

    /// Unpauses token transfers.
    ///
    /// # Errors
    /// - `Unauthorized` — caller does not hold `UNPAUSE_ROLE`
    pub fn unpause(&mut self, msg_sender: Address, _call: IB20::unpauseCall) -> Result<()> {
        self.check_role(msg_sender, *UNPAUSE_ROLE)?;
        self.paused.write(false)?;

        self.emit_event(B20Event::PauseStateUpdate(IB20::PauseStateUpdate {
            updater: msg_sender,
            isPaused: false,
        }))
    }

    // Token operations

    /// Mints `amount` tokens to the resolved target `to` address:
    /// - Enforces mint-recipient compliance via [`B403Registry`] and validates against supply cap
    /// - Resolves `to` via the [`AddressRegistry`]. If `to` is a virtual address, credits the
    ///   resolved master and emits a two-hop `Transfer` and `Mint(virtual, amount)` events
    ///
    /// # Errors
    /// - `Unauthorized` — caller does not hold the `ISSUER_ROLE` role
    /// - `ContractPaused` — (+T3) token is paused
    /// - `InvalidRecipient` — (+T3) recipient is zero or a B20 prefix address
    /// - `PolicyForbids` — B403 policy rejects the mint recipient
    /// - `SupplyCapExceeded` — minting would push total supply above the cap
    pub fn mint(&mut self, msg_sender: Address, call: IB20::mintCall) -> Result<()> {
        let to = Recipient::resolve(call.to)?;
        self._mint(msg_sender, &to, call.amount)?;

        self.emit_event(B20Event::Mint(IB20::Mint { to: call.to, amount: call.amount }))?;
        if let Some(hop) = to.build_virtual_transfer_event(call.amount) {
            self.emit_event(hop)?;
        }

        Ok(())
    }

    /// Like [`Self::mint`], but attaches a 32-byte memo.
    pub fn mint_with_memo(
        &mut self,
        msg_sender: Address,
        call: IB20::mintWithMemoCall,
    ) -> Result<()> {
        let to = Recipient::resolve(call.to)?;
        self._mint(msg_sender, &to, call.amount)?;

        self.emit_event(B20Event::TransferWithMemo(IB20::TransferWithMemo {
            from: Address::ZERO,
            to: call.to,
            amount: call.amount,
            memo: call.memo,
        }))?;
        self.emit_event(B20Event::Mint(IB20::Mint { to: call.to, amount: call.amount }))?;
        if let Some(hop) = to.build_virtual_transfer_event(call.amount) {
            self.emit_event(hop)?;
        }
        Ok(())
    }

    /// Internal helper to mint new tokens and update balances.
    fn _mint(&mut self, msg_sender: Address, to: &Recipient, amount: U256) -> Result<()> {
        self.check_role(msg_sender, *ISSUER_ROLE)?;
        let total_supply = self.total_supply()?;

        // Check if the resolved target address is authorized to receive minted tokens
        self.validate_mint(to)?;

        let new_supply =
            total_supply.checked_add(amount).ok_or(BasePrecompileError::under_overflow())?;

        let supply_cap = self.supply_cap()?;
        if new_supply > supply_cap {
            return Err(B20Error::supply_cap_exceeded().into());
        }

        self.set_total_supply(new_supply)?;
        let to_balance = self.get_balance(to.target)?;
        let new_to_balance: alloy::primitives::Uint<256, 4> =
            to_balance.checked_add(amount).ok_or(BasePrecompileError::under_overflow())?;
        self.set_balance(to.target, new_to_balance)?;

        self.emit_event(to.build_transfer_event(Address::ZERO, amount))
    }

    /// Burns `amount` from the caller's balance and reduces total supply.
    ///
    /// # Errors
    /// - `ContractPaused` — (+T3) token is paused
    /// - `Unauthorized` — caller does not hold the `ISSUER_ROLE` role
    /// - `InsufficientBalance` — caller balance lower than burn amount
    pub fn burn(&mut self, msg_sender: Address, call: IB20::burnCall) -> Result<()> {
        self._burn(msg_sender, call.amount)?;
        self.emit_event(B20Event::Burn(IB20::Burn { from: msg_sender, amount: call.amount }))
    }

    /// Like [`Self::burn`], but attaches a 32-byte memo.
    pub fn burn_with_memo(
        &mut self,
        msg_sender: Address,
        call: IB20::burnWithMemoCall,
    ) -> Result<()> {
        self._burn(msg_sender, call.amount)?;

        self.emit_event(B20Event::TransferWithMemo(IB20::TransferWithMemo {
            from: msg_sender,
            to: Address::ZERO,
            amount: call.amount,
            memo: call.memo,
        }))?;
        self.emit_event(B20Event::Burn(IB20::Burn { from: msg_sender, amount: call.amount }))
    }

    /// Burns tokens from addresses blocked by [`B403Registry`] policy.
    ///
    /// # Errors
    /// - `ContractPaused` — (+T3) token is paused
    /// - `Unauthorized` — caller does not hold `BURN_BLOCKED_ROLE`
    /// - `PolicyForbids` — target address is not blocked by policy
    pub fn burn_blocked(&mut self, msg_sender: Address, call: IB20::burnBlockedCall) -> Result<()> {
        // Validate burner role and (+T3) ensure token is not paused
        if self.storage.spec().is_enabled_in(crate::BaseBSpec::Beryl) {
            self.check_not_paused()?;
        }
        self.check_role(msg_sender, *BURN_BLOCKED_ROLE)?;

        // Check if the address is blocked from transferring (sender authorization)
        let policy_id = self.transfer_policy_id()?;
        if B403Registry::new().is_authorized_as(policy_id, call.from, AuthRole::sender())? {
            // Only allow burning from addresses that are blocked from transferring
            return Err(B20Error::policy_forbids().into());
        }

        self._transfer(call.from, &Recipient::direct(Address::ZERO), call.amount)?;

        let total_supply = self.total_supply()?;
        let new_supply = total_supply
            .checked_sub(call.amount)
            .ok_or(B20Error::insufficient_balance(total_supply, call.amount, self.address))?;
        self.set_total_supply(new_supply)?;

        self.emit_event(B20Event::BurnBlocked(IB20::BurnBlocked {
            from: call.from,
            amount: call.amount,
        }))
    }

    fn _burn(&mut self, msg_sender: Address, amount: U256) -> Result<()> {
        // Validate issuer role and (+T3) ensure token is not paused
        if self.storage.spec().is_enabled_in(crate::BaseBSpec::Beryl) {
            self.check_not_paused()?;
        }
        self.check_role(msg_sender, *ISSUER_ROLE)?;

        self._transfer(msg_sender, &Recipient::direct(Address::ZERO), amount)?;

        let total_supply = self.total_supply()?;
        let new_supply = total_supply.checked_sub(amount).ok_or(B20Error::insufficient_balance(
            total_supply,
            amount,
            self.address,
        ))?;
        self.set_total_supply(new_supply)
    }

    /// Sets `spender`'s allowance to `amount` for the caller's tokens.
    /// Deducts from the caller's [`AccountKeychain`] spending limit
    /// when the new allowance exceeds the previous one.
    ///
    /// # Errors
    /// - `SpendingLimitExceeded` — new allowance exceeds access key spending limit
    pub fn approve(&mut self, msg_sender: Address, call: IB20::approveCall) -> Result<bool> {
        // Set the new allowance
        self.set_allowance(msg_sender, call.spender, call.amount)?;

        self.emit_event(B20Event::Approval(IB20::Approval {
            owner: msg_sender,
            spender: call.spender,
            amount: call.amount,
        }))?;

        Ok(true)
    }

    // EIP-2612 Permit

    /// Returns the current nonce for an address (EIP-2612)
    pub fn nonces(&self, call: IB20::noncesCall) -> Result<U256> {
        self.permit_nonces[call.owner].read()
    }

    /// Returns the EIP-712 domain separator, computed dynamically from the token name and chain ID.
    pub fn domain_separator(&self) -> Result<B256> {
        let name = self.name()?;
        let name_hash = self.storage.keccak256(name.as_bytes())?;
        let chain_id = U256::from(self.storage.chain_id());

        let encoded = (*EIP712_DOMAIN_TYPEHASH, name_hash, *VERSION_HASH, chain_id, self.address)
            .abi_encode();

        self.storage.keccak256(&encoded)
    }

    /// Sets allowance via a signed [EIP-2612] permit. Validates the ECDSA signature, checks the
    /// deadline, and increments the nonce. Allowed even when the token is paused.
    ///
    /// [EIP-2612]: https://eips.ethereum.org/EIPS/eip-2612
    ///
    /// # Errors
    /// - `PermitExpired` — current timestamp exceeds permit deadline
    /// - `InvalidSignature` — ECDSA recovery failed or recovered signer ≠ owner
    pub fn permit(&mut self, call: IB20::permitCall) -> Result<()> {
        // 1. Check deadline
        if self.storage.timestamp() > call.deadline {
            return Err(B20Error::permit_expired().into());
        }

        // 2. Construct EIP-712 struct hash
        let nonce = self.permit_nonces[call.owner].read()?;
        let struct_hash = self.storage.keccak256(
            &(*PERMIT_TYPEHASH, call.owner, call.spender, call.value, nonce, call.deadline)
                .abi_encode(),
        )?;

        // 3. Construct EIP-712 digest
        let domain_separator = self.domain_separator()?;
        let digest = self.storage.keccak256(
            &[&[0x19, 0x01], domain_separator.as_slice(), struct_hash.as_slice()].concat(),
        )?;

        // 4. Validate ECDSA signature
        // Only v=27/28 is accepted; v=0/1 is intentionally NOT normalized (see B1004 spec).
        let recovered = self
            .storage
            .recover_signer(digest, call.v, call.r, call.s)?
            .ok_or(B20Error::invalid_signature())?;
        if recovered != call.owner {
            return Err(B20Error::invalid_signature().into());
        }

        // 5. Increment nonce
        self.permit_nonces[call.owner].write(
            nonce.checked_add(U256::from(1)).ok_or(BasePrecompileError::under_overflow())?,
        )?;

        // 6. Set allowance
        self.set_allowance(call.owner, call.spender, call.value)?;

        // 7. Emit Approval event
        self.emit_event(B20Event::Approval(IB20::Approval {
            owner: call.owner,
            spender: call.spender,
            amount: call.value,
        }))
    }

    /// Transfers `amount` tokens from the caller to `to`. Enforces compliance via the
    /// [`B403Registry`] and deducts from the caller's [`AccountKeychain`] spending limit.
    ///
    /// # Errors
    /// - `Paused` — token transfers are currently paused
    /// - `InvalidRecipient` — recipient address is zero
    /// - `PolicyForbids` — B403 policy rejects sender or recipient
    /// - `SpendingLimitExceeded` — access key spending limit exceeded
    /// - `InsufficientBalance` — sender balance lower than transfer amount
    pub fn transfer(&mut self, msg_sender: Address, call: IB20::transferCall) -> Result<bool> {
        trace!(%msg_sender, ?call, "transferring B20");
        let to = Recipient::resolve(call.to)?;
        self.validate_transfer(msg_sender, &to)?;
        self.check_and_update_spending_limit(msg_sender, call.amount)?;

        self._transfer(msg_sender, &to, call.amount)?;
        if let Some(hop) = to.build_virtual_transfer_event(call.amount) {
            self.emit_event(hop)?;
        }
        Ok(true)
    }

    /// Transfers `amount` on behalf of `from` using the caller's allowance.
    /// Enforces compliance via the [`B403Registry`].
    ///
    /// # Errors
    /// - `Paused` — token transfers are currently paused
    /// - `InvalidRecipient` — recipient address is zero
    /// - `PolicyForbids` — B403 policy rejects sender or recipient
    /// - `InsufficientAllowance` — caller allowance lower than transfer amount
    /// - `InsufficientBalance` — `from` balance lower than transfer amount
    pub fn transfer_from(
        &mut self,
        msg_sender: Address,
        call: IB20::transferFromCall,
    ) -> Result<bool> {
        let to = Recipient::resolve(call.to)?;
        self._transfer_from(msg_sender, call.from, &to, call.amount)?;
        if let Some(hop) = to.build_virtual_transfer_event(call.amount) {
            self.emit_event(hop)?;
        }
        Ok(true)
    }

    /// Like [`Self::transfer_from`], but attaches a 32-byte memo.
    pub fn transfer_from_with_memo(
        &mut self,
        msg_sender: Address,
        call: IB20::transferFromWithMemoCall,
    ) -> Result<bool> {
        let to = Recipient::resolve(call.to)?;
        self._transfer_from(msg_sender, call.from, &to, call.amount)?;

        self.emit_event(B20Event::TransferWithMemo(IB20::TransferWithMemo {
            from: call.from,
            to: call.to,
            amount: call.amount,
            memo: call.memo,
        }))?;
        if let Some(hop) = to.build_virtual_transfer_event(call.amount) {
            self.emit_event(hop)?;
        }
        Ok(true)
    }

    /// Transfers `amount` from `from` to `to` without approval, for use
    /// by other precompiles only (not exposed via ABI). Enforces
    /// compliance via the [`B403Registry`] and [`AccountKeychain`].
    ///
    /// # Errors
    /// - `Paused` — token transfers are currently paused
    /// - `InvalidRecipient` — recipient address is zero
    /// - `PolicyForbids` — B403 policy rejects sender or recipient
    /// - `SpendingLimitExceeded` — access key spending limit exceeded
    /// - `InsufficientBalance` — `from` balance lower than transfer amount
    pub fn system_transfer_from(
        &mut self,
        from: Address,
        to: Address,
        amount: U256,
    ) -> Result<bool> {
        let to = Recipient::resolve(to)?;
        self.validate_transfer(from, &to)?;
        self.check_and_update_spending_limit(from, amount)?;

        self._transfer(from, &to, amount)?;
        if let Some(hop) = to.build_virtual_transfer_event(amount) {
            self.emit_event(hop)?;
        }

        Ok(true)
    }

    fn _transfer_from(
        &mut self,
        msg_sender: Address,
        from: Address,
        to: &Recipient,
        amount: U256,
    ) -> Result<bool> {
        self.validate_transfer(from, to)?;

        let allowed = self.get_allowance(from, msg_sender)?;
        if amount > allowed {
            return Err(B20Error::insufficient_allowance().into());
        }

        if allowed != U256::MAX {
            let new_allowance =
                allowed.checked_sub(amount).ok_or(B20Error::insufficient_allowance())?;
            self.set_allowance(from, msg_sender, new_allowance)?;
        }

        self._transfer(from, to, amount)?;

        Ok(true)
    }

    /// Like [`Self::transfer`], but attaches a 32-byte memo.
    pub fn transfer_with_memo(
        &mut self,
        msg_sender: Address,
        call: IB20::transferWithMemoCall,
    ) -> Result<()> {
        let to = Recipient::resolve(call.to)?;
        self.validate_transfer(msg_sender, &to)?;
        self.check_and_update_spending_limit(msg_sender, call.amount)?;

        self._transfer(msg_sender, &to, call.amount)?;

        self.emit_event(B20Event::TransferWithMemo(IB20::TransferWithMemo {
            from: msg_sender,
            to: call.to,
            amount: call.amount,
            memo: call.memo,
        }))?;
        if let Some(hop) = to.build_virtual_transfer_event(call.amount) {
            self.emit_event(hop)?;
        }
        Ok(())
    }
}

// Utility functions
impl B20Token {
    /// Creates a `B20Token` handle from a raw address.
    ///
    /// # Errors
    /// - `InvalidToken` — address does not carry the `0x20C0` B20 prefix
    pub fn from_address(address: Address) -> Result<Self> {
        if !address.is_b20() {
            return Err(B20Error::invalid_token().into());
        }
        Ok(Self::__new(address))
    }

    /// Creates a B20Token without validating the prefix.
    ///
    /// # Safety
    /// Caller must ensure `is_b20_prefix(address)` returns true.
    #[inline]
    pub fn from_address_unchecked(address: Address) -> Self {
        debug_assert!(address.is_b20(), "address must have B20 prefix");
        Self::__new(address)
    }

    /// Initializes the B20 token precompile with metadata, supply cap, and default admin role.
    /// Called once by [`B20Factory`] during token creation.
    pub fn initialize(
        &mut self,
        msg_sender: Address,
        name: &str,
        symbol: &str,
        currency: &str,
        admin: Address,
    ) -> Result<()> {
        trace!(%name, address=%self.address, "Initializing token");

        // must ensure the account is not empty, by setting some code
        self.__initialize()?;

        self.name.write(name.to_string())?;
        self.symbol.write(symbol.to_string())?;
        self.currency.write(currency.to_string())?;

        // Set default values
        self.supply_cap.write(U128_MAX)?;
        self.transfer_policy_id.write(1)?;

        // Initialize roles system and grant admin role
        self.initialize_roles()?;
        self.grant_default_admin(msg_sender, admin)
    }

    fn get_balance(&self, account: Address) -> Result<U256> {
        self.balances[account].read()
    }

    fn set_balance(&mut self, account: Address, amount: U256) -> Result<()> {
        self.balances[account].write(amount)
    }

    fn get_allowance(&self, owner: Address, spender: Address) -> Result<U256> {
        self.allowances[owner][spender].read()
    }

    fn set_allowance(&mut self, owner: Address, spender: Address, amount: U256) -> Result<()> {
        self.allowances[owner][spender].write(amount)
    }

    fn set_total_supply(&mut self, amount: U256) -> Result<()> {
        self.total_supply.write(amount)
    }

    pub fn check_not_paused(&self) -> Result<()> {
        if self.paused()? {
            return Err(B20Error::contract_paused().into());
        }
        Ok(())
    }

    /// Checks pause state, validates the effective recipient, and ensures the transfer
    /// is authorized. Shared by public entrypoints that resolve a [`Recipient`] up front.
    fn validate_transfer(&self, from: Address, to: &Recipient) -> Result<()> {
        self.check_not_paused()?;
        to.validate()?;
        self.ensure_transfer_authorized(from, to.target)
    }

    /// Ensures that the recipient is authorized to receive mints.
    /// Additionally (+T3) checks pause state, validates the effective recipient.
    fn validate_mint(&self, to: &Recipient) -> Result<()> {
        if self.storage.spec().is_enabled_in(crate::BaseBSpec::Beryl) {
            self.check_not_paused()?;
            to.validate()?;
        }

        // Check if the resolved target address is authorized to receive minted tokens
        if !B403Registry::new().is_authorized_as(
            self.transfer_policy_id()?,
            to.target,
            AuthRole::mint_recipient(),
        )? {
            return Err(B20Error::policy_forbids().into());
        }

        Ok(())
    }

    /// Check whether a transfer is authorized by the token's [`B403Registry`] policy.
    /// [B1015]: For T2+, uses directional sender/recipient checks.
    ///
    /// [B1015]: <https://docs.base.xyz/protocol/b/1015>
    pub fn is_transfer_authorized(&self, from: Address, to: Address) -> Result<bool> {
        let policy_id = self.transfer_policy_id()?;
        let registry = B403Registry::new();

        // (spec: +T2) short-circuit and skip recipient check if sender fails
        let sender_auth = registry.is_authorized_as(policy_id, from, AuthRole::sender())?;
        if self.storage.spec().is_enabled_in(crate::BaseBSpec::Beryl) && !sender_auth {
            return Ok(false);
        }
        let recipient_auth = registry.is_authorized_as(policy_id, to, AuthRole::recipient())?;
        Ok(sender_auth && recipient_auth)
    }

    /// Ensures the transfer is authorized by the token's [`B403Registry`] policy.
    ///
    /// # Errors
    /// - `PolicyForbids` — sender or recipient is not authorized by the active transfer policy
    pub fn ensure_transfer_authorized(&self, from: Address, to: Address) -> Result<()> {
        if !self.is_transfer_authorized(from, to)? {
            return Err(B20Error::policy_forbids().into());
        }

        Ok(())
    }

    /// Checks and deducts `amount` from the caller's [`AccountKeychain`] spending limit.
    ///
    /// # Errors
    /// - `SpendingLimitExceeded` — access key spending limit exceeded
    pub fn check_and_update_spending_limit(&mut self, _from: Address, _amount: U256) -> Result<()> {
        Ok(())
    }

    /// Core transfer: debits `from`, credits `to.target`, emits `Transfer(from, event_addr, amount)`.
    ///
    /// For virtual recipients the event address is the virtual alias; the balance update always
    /// targets `to.target` (the resolved master).
    fn _transfer(&mut self, from: Address, to: &Recipient, amount: U256) -> Result<()> {
        let from_balance = self.get_balance(from)?;
        if amount > from_balance {
            return Err(B20Error::insufficient_balance(from_balance, amount, self.address).into());
        }

        // Adjust balances
        let new_from_balance =
            from_balance.checked_sub(amount).ok_or(BasePrecompileError::under_overflow())?;

        self.set_balance(from, new_from_balance)?;

        if to.target != Address::ZERO {
            let to_balance = self.get_balance(to.target)?;
            let new_to_balance =
                to_balance.checked_add(amount).ok_or(BasePrecompileError::under_overflow())?;

            self.set_balance(to.target, new_to_balance)?;
        }

        self.emit_event(to.build_transfer_event(from, amount))
    }
}

/// Resolved transfer recipient for [B1022] virtual address support.
///
/// `target` is always the effective (resolved) address where the balance is credited. For virtual
/// recipients, `virtual_addr` carries the original virtual address for event emission.
///
/// [B1022]: <https://docs.base.xyz/protocol/b1022>
#[derive(Debug, PartialEq)]
pub(crate) struct Recipient {
    /// The effective (resolved) address where the balance is credited.
    pub(crate) target: Address,
    /// The virtual address, if registered.
    pub(crate) virtual_addr: Option<Address>,
}

impl Recipient {
    /// Creates a [`Recipient`] with no virtual indirection.
    #[inline]
    pub(crate) fn direct(addr: Address) -> Self {
        Self { target: addr, virtual_addr: None }
    }

    /// Resolves a recipient via the [`AddressRegistry`].
    ///
    /// If `addr` is a virtual address its registered master is looked up and stored in `target`,
    /// with the original virtual address preserved in `virtual_addr`.
    pub(crate) fn resolve(addr: Address) -> Result<Self> {
        Ok(Self::direct(addr))
    }

    /// Validates that the recipient is not:
    /// - the zero address (preventing accidental burns)
    /// - an address with the B20 prefix (preventing transfers to token contracts)
    pub(crate) fn validate(&self) -> Result<()> {
        if self.target.is_zero() || self.target.is_b20() {
            return Err(B20Error::invalid_recipient().into());
        }
        Ok(())
    }

    /// Builds the primary `Transfer(from, to, amount)` event.
    ///
    /// For virtual recipients `to` is the virtual address (first hop); for regular
    /// recipients this is the only `Transfer` event needed.
    pub(crate) fn build_transfer_event(&self, from: Address, amount: U256) -> B20Event {
        B20Event::Transfer(IB20::Transfer {
            from,
            to: self.virtual_addr.unwrap_or(self.target),
            amount,
        })
    }

    /// Builds the forwarding `Transfer(virtual, master, amount)` event for virtual recipients.
    /// Returns `None` for non-virtual recipients.
    pub(crate) fn build_virtual_transfer_event(&self, amount: U256) -> Option<B20Event> {
        self.virtual_addr.map(|virtual_addr| {
            B20Event::Transfer(IB20::Transfer { from: virtual_addr, to: self.target, amount })
        })
    }
}
