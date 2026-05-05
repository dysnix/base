//! [B403] transfer policy registry precompile.
//!
//! Manages whitelist, blacklist, and compound transfer policies that B20
//! tokens reference to gate sender/recipient authorization.
//!
//! [B403]: <https://docs.base.xyz/protocol/b403>

pub mod dispatch;

use crate::StorageCtx;
pub use base_precompiles_contracts::{
    B403RegistryError, B403RegistryEvent,
    IB403Registry::{self, PolicyType},
};
use base_precompiles_macros::{Storable, contract};

use crate::{
    B403_REGISTRY_ADDRESS, BaseBAddressExt,
    error::{BasePrecompileError, Result},
    storage::{Handler, Mapping},
};
use alloy::primitives::Address;

/// Built-in policy ID that always rejects authorization.
pub const REJECT_ALL_POLICY_ID: u64 = 0;

/// Built-in policy ID that always allows authorization.
pub const ALLOW_ALL_POLICY_ID: u64 = 1;

/// Registry for [B403] transfer policies. B20 tokens reference an ID from this registry
/// to police transfers between sender and receiver addresses.
///
/// [B403]: <https://docs.base.xyz/protocol/b403>
///
/// The struct fields define the on-chain storage layout; the `#[contract]` macro generates the
/// storage handlers which provide an ergonomic way to interact with the EVM state.
#[contract(addr = B403_REGISTRY_ADDRESS)]
pub struct B403Registry {
    /// Monotonically increasing counter for policy IDs. Starts at `2` because IDs `0`
    /// ([`REJECT_ALL_POLICY_ID`]) and `1` ([`ALLOW_ALL_POLICY_ID`]) are reserved special
    /// policies.
    policy_id_counter: u64,
    /// Maps a policy ID to its [`PolicyRecord`], which stores the base [`PolicyData`] and, for
    /// compound policies, the [`CompoundPolicyData`] sub-policy references.
    policy_records: Mapping<u64, PolicyRecord>,
    /// Per-policy address set used by simple (non-compound) policies. For whitelists the
    /// value is `true` when the address is allowed; for blacklists it is `true` when the
    /// address is restricted.
    policy_set: Mapping<u64, Mapping<Address, bool>>,
}

/// Policy record containing base data and optional data for compound policies ([B1015])
///
/// [B1015]: <https://docs.base.xyz/protocol/b/1015>
#[derive(Debug, Clone, Storable)]
pub struct PolicyRecord {
    /// Base policy data
    pub base: PolicyData,
    /// Compound policy data. Only relevant when `base.policy_type == COMPOUND`
    pub compound: CompoundPolicyData,
}

/// Data for compound policies ([B1015])
///
/// [B1015]: <https://docs.base.xyz/protocol/b/1015>
#[derive(Debug, Clone, Default, Storable)]
pub struct CompoundPolicyData {
    /// Sub-policy ID used to authorize the sender.
    pub sender_policy_id: u64,
    /// Sub-policy ID used to authorize the recipient.
    pub recipient_policy_id: u64,
    /// Sub-policy ID used to authorize mint recipients.
    pub mint_recipient_policy_id: u64,
}

/// Authorization role for policy checks.
///
/// - `Transfer` (symmetric sender/recipient) available since `Genesis`.
/// - Directional roles (`Sender`, `Recipient`, `MintRecipient`) for compound policies available since `T2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthRole {
    /// Check both sender AND recipient. Used for `isAuthorized` calls (spec: pre T2).
    Transfer,
    /// Check sender authorization only (spec: +T2).
    Sender,
    /// Check recipient authorization only (spec: +T2).
    Recipient,
    /// Check mint recipient authorization only (spec: +T2).
    MintRecipient,
}

/// Base policy metadata. Packed into a single storage slot.
#[derive(Debug, Clone, Storable)]
pub struct PolicyData {
    // NOTE: enums are defined as u8, and leverage the sol! macro's `TryInto<u8>` impl
    /// Discriminant of the [`PolicyType`] enum, stored as `u8` for slot packing.
    pub policy_type: u8,
    /// Address authorized to modify this policy.
    pub admin: Address,
}

impl PolicyData {
    /// Decodes the raw `policy_type` u8 to a `PolicyType` enum.
    fn policy_type(&self) -> Result<PolicyType> {
        let is_t2 = StorageCtx.spec().is_enabled_in(crate::BaseBSpec::Azul);

        match self.policy_type.try_into() {
            Ok(ty) if is_t2 || ty != PolicyType::COMPOUND => Ok(ty),
            _ => Err(if is_t2 {
                B403RegistryError::invalid_policy_type().into()
            } else {
                BasePrecompileError::under_overflow()
            }),
        }
    }

    /// Returns `true` if the policy type is a simple policy (WHITELIST or BLACKLIST).
    fn is_simple(&self) -> bool {
        self.policy_type == PolicyType::WHITELIST as u8
            || self.policy_type == PolicyType::BLACKLIST as u8
    }

    /// Returns `true` if the policy data indicates a compound policy
    pub fn is_compound(&self) -> bool {
        self.policy_type == PolicyType::COMPOUND as u8
    }

    /// Returns `true` if the policy data is the default (uninitialized) value.
    fn is_default(&self) -> bool {
        self.policy_type == 0 && self.admin == Address::ZERO
    }
}

impl B403Registry {
    /// Initializes the B403 registry precompile.
    pub fn initialize(&mut self) -> Result<()> {
        self.__initialize()
    }

    /// Returns the next policy ID to be assigned (always ≥ 2, since IDs 0 and 1 are reserved).
    pub fn policy_id_counter(&self) -> Result<u64> {
        // Skips the built-in policy IDs, when initializing the counter for the first time.
        self.policy_id_counter.read().map(|counter| counter.max(2))
    }

    /// Returns `true` if the given policy ID exists (built-in or user-created).
    pub fn policy_exists(&self, call: IB403Registry::policyExistsCall) -> Result<bool> {
        // Built-in policies (0 and 1) always exist
        if self.builtin_authorization(call.policyId).is_some() {
            return Ok(true);
        }

        // Check if policy ID is within the range of created policies
        let counter = self.policy_id_counter()?;
        Ok(call.policyId < counter)
    }

    /// Returns the type and admin of a policy. Reverts if the policy does not exist or has an
    /// invalid type.
    ///
    /// # Errors
    /// - `PolicyNotFound` — the policy ID does not exist
    /// - `InvalidPolicyType` — stored type cannot be decoded (e.g. pre-T1 `COMPOUND` on T2+)
    pub fn policy_data(
        &self,
        call: IB403Registry::policyDataCall,
    ) -> Result<IB403Registry::policyDataReturn> {
        if self.storage.spec().is_enabled_in(crate::BaseBSpec::Azul) {
            // Built-in policies are virtual (not stored), and match the `PolicyType`:
            //  - 0: REJECT_ALL_POLICY_ID → WHITELIST
            //  - 1: ALLOW_ALL_POLICY_ID  → BLACKLIST
            if self.builtin_authorization(call.policyId).is_some() {
                return Ok(IB403Registry::policyDataReturn {
                    policyType: (call.policyId as u8)
                        .try_into()
                        .map_err(|_| B403RegistryError::invalid_policy_type())?,
                    admin: Address::ZERO,
                });
            }
        } else {
            // Check if policy exists before reading the data (spec: pre-T2)
            if !self.policy_exists(IB403Registry::policyExistsCall { policyId: call.policyId })? {
                return Err(B403RegistryError::policy_not_found().into());
            }
        }

        // Get policy data and verify that the policy id exists (spec: +T2)
        let data = self.get_policy_data(call.policyId)?;

        Ok(IB403Registry::policyDataReturn { policyType: data.policy_type()?, admin: data.admin })
    }

    /// Returns the sub-policy IDs of a compound policy ([B1015]).
    ///
    /// [B1015]: <https://docs.base.xyz/protocol/b/1015>
    ///
    /// # Errors
    /// - `IncompatiblePolicyType` — the policy exists but is not compound
    /// - `PolicyNotFound` — the policy ID does not exist
    pub fn compound_policy_data(
        &self,
        call: IB403Registry::compoundPolicyDataCall,
    ) -> Result<IB403Registry::compoundPolicyDataReturn> {
        let data = self.get_policy_data(call.policyId)?;

        // Only compound policies have compound data
        if !data.is_compound() {
            // Check if the policy exists for error clarity
            let err = if self
                .policy_exists(IB403Registry::policyExistsCall { policyId: call.policyId })?
            {
                B403RegistryError::incompatible_policy_type()
            } else {
                B403RegistryError::policy_not_found()
            };
            return Err(err.into());
        }

        let compound = self.policy_records[call.policyId].compound.read()?;
        Ok(IB403Registry::compoundPolicyDataReturn {
            senderPolicyId: compound.sender_policy_id,
            recipientPolicyId: compound.recipient_policy_id,
            mintRecipientPolicyId: compound.mint_recipient_policy_id,
        })
    }

    /// Creates a new simple (whitelist or blacklist) policy and returns its ID.
    ///
    /// # Errors
    /// - `IncompatiblePolicyType` — `policyType` is not `WHITELIST` or `BLACKLIST` (T2+)
    /// - `UnderOverflow` — policy ID counter overflows
    pub fn create_policy(
        &mut self,
        msg_sender: Address,
        call: IB403Registry::createPolicyCall,
    ) -> Result<u64> {
        let policy_type = call.policyType.ensure_is_simple()?;

        let new_policy_id = self.policy_id_counter()?;

        // Increment counter
        self.policy_id_counter
            .write(new_policy_id.checked_add(1).ok_or(BasePrecompileError::under_overflow())?)?;

        // Store policy data
        self.policy_records[new_policy_id]
            .base
            .write(PolicyData { policy_type, admin: call.admin })?;

        self.emit_event(B403RegistryEvent::PolicyCreated(IB403Registry::PolicyCreated {
            policyId: new_policy_id,
            updater: msg_sender,
            policyType: policy_type.try_into().unwrap_or(PolicyType::__Invalid),
        }))?;

        self.emit_event(B403RegistryEvent::PolicyAdminUpdated(
            IB403Registry::PolicyAdminUpdated {
                policyId: new_policy_id,
                updater: msg_sender,
                admin: call.admin,
            },
        ))?;

        Ok(new_policy_id)
    }

    /// Creates a simple policy and pre-populates it with an initial set of accounts.
    ///
    /// # Errors
    /// - `UnderOverflow` — policy ID counter overflows
    /// - `IncompatiblePolicyType` — `policyType` is not `WHITELIST` or `BLACKLIST` (T2+), or
    ///   accounts are non-empty for compound/invalid types (pre-T2)
    /// - `VirtualAddressNotAllowed` — virtual addresses are forbidden (T3+)
    pub fn create_policy_with_accounts(
        &mut self,
        msg_sender: Address,
        call: IB403Registry::createPolicyWithAccountsCall,
    ) -> Result<u64> {
        let admin = call.admin;
        let policy_type = call.policyType.ensure_is_simple()?;

        // B1022: reject virtual addresses in initial account set (spec T3+)
        if self.storage.spec().is_enabled_in(crate::BaseBSpec::Azul) {
            for account in call.accounts.iter() {
                if account.is_virtual() {
                    return Err(B403RegistryError::virtual_address_not_allowed().into());
                }
            }
        }

        let new_policy_id = self.policy_id_counter()?;

        // Increment counter
        self.policy_id_counter
            .write(new_policy_id.checked_add(1).ok_or(BasePrecompileError::under_overflow())?)?;

        // Store policy data
        self.set_policy_data(new_policy_id, PolicyData { policy_type, admin })?;

        // Set initial accounts - only emit events for valid policy types
        // Pre-T2 with invalid types: accounts are added but no events emitted (matches original)
        for account in call.accounts.iter() {
            self.set_policy_set(new_policy_id, *account, true)?;

            match call.policyType {
                PolicyType::WHITELIST => {
                    self.emit_event(B403RegistryEvent::WhitelistUpdated(
                        IB403Registry::WhitelistUpdated {
                            policyId: new_policy_id,
                            updater: msg_sender,
                            account: *account,
                            allowed: true,
                        },
                    ))?;
                }
                PolicyType::BLACKLIST => {
                    self.emit_event(B403RegistryEvent::BlacklistUpdated(
                        IB403Registry::BlacklistUpdated {
                            policyId: new_policy_id,
                            updater: msg_sender,
                            account: *account,
                            restricted: true,
                        },
                    ))?;
                }
                IB403Registry::PolicyType::COMPOUND | IB403Registry::PolicyType::__Invalid => {
                    // T2+: unreachable since `ensure_is_simple` already rejected
                    return Err(B403RegistryError::incompatible_policy_type().into());
                }
            }
        }

        self.emit_event(B403RegistryEvent::PolicyCreated(IB403Registry::PolicyCreated {
            policyId: new_policy_id,
            updater: msg_sender,
            policyType: policy_type.try_into().unwrap_or(PolicyType::__Invalid),
        }))?;

        self.emit_event(B403RegistryEvent::PolicyAdminUpdated(
            IB403Registry::PolicyAdminUpdated {
                policyId: new_policy_id,
                updater: msg_sender,
                admin,
            },
        ))?;

        Ok(new_policy_id)
    }

    /// Transfers admin control of a policy. Only callable by the current admin.
    ///
    /// # Errors
    /// - `Unauthorized` — `msg_sender` is not the current admin
    /// - `PolicyNotFound` — the policy ID does not exist (T2+)
    pub fn set_policy_admin(
        &mut self,
        msg_sender: Address,
        call: IB403Registry::setPolicyAdminCall,
    ) -> Result<()> {
        let data = self.get_policy_data(call.policyId)?;

        // Check authorization
        if data.admin != msg_sender {
            return Err(B403RegistryError::unauthorized().into());
        }

        // Update admin policy ID
        self.set_policy_data(call.policyId, PolicyData { admin: call.admin, ..data })?;

        self.emit_event(B403RegistryEvent::PolicyAdminUpdated(IB403Registry::PolicyAdminUpdated {
            policyId: call.policyId,
            updater: msg_sender,
            admin: call.admin,
        }))
    }

    /// Adds or removes an account from a whitelist policy. Admin-only.
    ///
    /// # Errors
    /// - `Unauthorized` — `msg_sender` is not the policy admin
    /// - `IncompatiblePolicyType` — the policy is not a whitelist
    /// - `PolicyNotFound` — the policy ID does not exist (T2+)
    /// - `VirtualAddressNotAllowed` — virtual addresses are forbidden (T3+)
    pub fn modify_policy_whitelist(
        &mut self,
        msg_sender: Address,
        call: IB403Registry::modifyPolicyWhitelistCall,
    ) -> Result<()> {
        // B1022: virtual addresses are forwarding aliases, not valid policy members (spec: T3+)
        if self.storage.spec().is_enabled_in(crate::BaseBSpec::Azul) && call.account.is_virtual() {
            return Err(B403RegistryError::virtual_address_not_allowed().into());
        }

        let data = self.get_policy_data(call.policyId)?;

        // Check authorization
        if data.admin != msg_sender {
            return Err(B403RegistryError::unauthorized().into());
        }

        // Check policy type
        if !matches!(data.policy_type()?, PolicyType::WHITELIST) {
            return Err(B403RegistryError::incompatible_policy_type().into());
        }

        self.set_policy_set(call.policyId, call.account, call.allowed)?;

        self.emit_event(B403RegistryEvent::WhitelistUpdated(IB403Registry::WhitelistUpdated {
            policyId: call.policyId,
            updater: msg_sender,
            account: call.account,
            allowed: call.allowed,
        }))
    }

    /// Adds or removes an account from a blacklist policy. Admin-only.
    ///
    /// # Errors
    /// - `Unauthorized` — `msg_sender` is not the policy admin
    /// - `IncompatiblePolicyType` — the policy is not a blacklist
    /// - `PolicyNotFound` — the policy ID does not exist (T2+)
    /// - `VirtualAddressNotAllowed` — virtual addresses are forbidden (T3+)
    pub fn modify_policy_blacklist(
        &mut self,
        msg_sender: Address,
        call: IB403Registry::modifyPolicyBlacklistCall,
    ) -> Result<()> {
        // B1022: virtual addresses are forwarding aliases, not valid policy members (spec: T3+)
        if self.storage.spec().is_enabled_in(crate::BaseBSpec::Azul) && call.account.is_virtual() {
            return Err(B403RegistryError::virtual_address_not_allowed().into());
        }

        let data = self.get_policy_data(call.policyId)?;

        // Check authorization
        if data.admin != msg_sender {
            return Err(B403RegistryError::unauthorized().into());
        }

        // Check policy type
        if !matches!(data.policy_type()?, PolicyType::BLACKLIST) {
            return Err(B403RegistryError::incompatible_policy_type().into());
        }

        self.set_policy_set(call.policyId, call.account, call.restricted)?;

        self.emit_event(B403RegistryEvent::BlacklistUpdated(IB403Registry::BlacklistUpdated {
            policyId: call.policyId,
            updater: msg_sender,
            account: call.account,
            restricted: call.restricted,
        }))
    }

    /// Creates a new compound policy that references three simple sub-policies ([B1015]).
    /// Compound policies have no admin and cannot be modified after creation.
    ///
    /// [B1015]: <https://docs.base.xyz/protocol/b/1015>
    ///
    /// # Errors
    /// - `PolicyNotFound` — a referenced sub-policy ID does not exist
    /// - `PolicyNotSimple` — a referenced sub-policy is itself compound
    /// - `UnderOverflow` — policy ID counter overflows
    pub fn create_compound_policy(
        &mut self,
        msg_sender: Address,
        call: IB403Registry::createCompoundPolicyCall,
    ) -> Result<u64> {
        // Validate all referenced policies exist and are simple (not compound)
        self.validate_simple_policy(call.senderPolicyId)?;
        self.validate_simple_policy(call.recipientPolicyId)?;
        self.validate_simple_policy(call.mintRecipientPolicyId)?;

        let new_policy_id = self.policy_id_counter()?;

        // Increment counter
        self.policy_id_counter
            .write(new_policy_id.checked_add(1).ok_or(BasePrecompileError::under_overflow())?)?;

        // Store policy record with COMPOUND type and compound data
        self.policy_records[new_policy_id].write(PolicyRecord {
            base: PolicyData { policy_type: PolicyType::COMPOUND as u8, admin: Address::ZERO },
            compound: CompoundPolicyData {
                sender_policy_id: call.senderPolicyId,
                recipient_policy_id: call.recipientPolicyId,
                mint_recipient_policy_id: call.mintRecipientPolicyId,
            },
        })?;

        // Emit event
        self.emit_event(B403RegistryEvent::CompoundPolicyCreated(
            IB403Registry::CompoundPolicyCreated {
                policyId: new_policy_id,
                creator: msg_sender,
                senderPolicyId: call.senderPolicyId,
                recipientPolicyId: call.recipientPolicyId,
                mintRecipientPolicyId: call.mintRecipientPolicyId,
            },
        ))?;

        Ok(new_policy_id)
    }

    /// Core role-based authorization check ([B1015]). Resolves built-in policies (0 = reject,
    /// 1 = allow) immediately, delegates compound policies to their sub-policies, and evaluates
    /// simple policies via `is_simple`.
    ///
    /// [B1015]: <https://docs.base.xyz/protocol/b/1015>
    ///
    /// # Errors
    /// - `PolicyNotFound` — the policy ID does not exist (T2+)
    /// - `InvalidPolicyType` — stored type cannot be decoded
    /// - `IncompatiblePolicyType` — a compound policy was passed where a simple one is required
    pub fn is_authorized_as(&self, policy_id: u64, user: Address, role: AuthRole) -> Result<bool> {
        if let Some(auth) = self.builtin_authorization(policy_id) {
            return Ok(auth);
        }

        let data = self.get_policy_data(policy_id)?;

        if data.is_compound() {
            let compound = self.policy_records[policy_id].compound.read()?;
            return match role {
                AuthRole::Sender => self.is_authorized_simple(compound.sender_policy_id, user),
                AuthRole::Recipient => {
                    self.is_authorized_simple(compound.recipient_policy_id, user)
                }
                AuthRole::MintRecipient => {
                    self.is_authorized_simple(compound.mint_recipient_policy_id, user)
                }
                AuthRole::Transfer => {
                    // (spec: +T2) short-circuit and skip recipient check if sender fails
                    let sender_auth = self.is_authorized_simple(compound.sender_policy_id, user)?;
                    if self.storage.spec().is_enabled_in(crate::BaseBSpec::Azul) && !sender_auth {
                        return Ok(false);
                    }
                    let recipient_auth =
                        self.is_authorized_simple(compound.recipient_policy_id, user)?;
                    Ok(sender_auth && recipient_auth)
                }
            };
        }

        self.is_simple(policy_id, user, &data)
    }

    /// Returns authorization result for built-in policies ([`REJECT_ALL_POLICY_ID`] / [`ALLOW_ALL_POLICY_ID`]).
    /// Returns None for user-created policies.
    #[inline]
    fn builtin_authorization(&self, policy_id: u64) -> Option<bool> {
        match policy_id {
            ALLOW_ALL_POLICY_ID => Some(true),
            REJECT_ALL_POLICY_ID => Some(false),
            _ => None,
        }
    }

    /// Authorization for simple (non-compound) policies only.
    ///
    /// **WARNING:** skips compound check - caller must guarantee policy is simple.
    fn is_authorized_simple(&self, policy_id: u64, user: Address) -> Result<bool> {
        if let Some(auth) = self.builtin_authorization(policy_id) {
            return Ok(auth);
        }
        let data = self.get_policy_data(policy_id)?;
        self.is_simple(policy_id, user, &data)
    }

    /// Authorization check for simple (non-compound) policies
    fn is_simple(&self, policy_id: u64, user: Address, data: &PolicyData) -> Result<bool> {
        // NOTE: read `policy_set` BEFORE checking policy type to match original gas consumption.
        // Pre-T1: the old code read policy_set first, then failed on invalid policy types.
        // This order must be preserved for block re-execution compatibility.
        let is_in_set = self.policy_set[policy_id][user].read()?;

        match data.policy_type()? {
            PolicyType::WHITELIST => Ok(is_in_set),
            PolicyType::BLACKLIST => Ok(!is_in_set),
            PolicyType::COMPOUND => Err(B403RegistryError::incompatible_policy_type().into()),
            PolicyType::__Invalid => unreachable!(),
        }
    }

    /// Validates that a policy ID references an existing simple policy (not compound)
    fn validate_simple_policy(&self, policy_id: u64) -> Result<()> {
        // Built-in policies (0 and 1) are always valid simple policies
        if self.builtin_authorization(policy_id).is_some() {
            return Ok(());
        }

        // Check if policy exists
        if policy_id >= self.policy_id_counter()? {
            return Err(B403RegistryError::policy_not_found().into());
        }

        // Check if policy is simple (WHITELIST or BLACKLIST only)
        let data = self.get_policy_data(policy_id)?;
        if !data.is_simple() {
            return Err(B403RegistryError::policy_not_simple().into());
        }

        Ok(())
    }

    // Internal helper functions

    /// Returns policy data for the given policy ID.
    /// Errors with `PolicyNotFound` for invalid policy ids.
    fn get_policy_data(&self, policy_id: u64) -> Result<PolicyData> {
        let data = self.policy_records[policy_id].base.read()?;

        // Verify that the policy id exists (spec: +T2).
        // Skip the counter read (extra SLOAD) when policy data is non-default.
        if self.storage.spec().is_enabled_in(crate::BaseBSpec::Azul)
            && data.is_default()
            && policy_id >= self.policy_id_counter()?
        {
            return Err(B403RegistryError::policy_not_found().into());
        }

        Ok(data)
    }

    fn set_policy_data(&mut self, policy_id: u64, data: PolicyData) -> Result<()> {
        self.policy_records[policy_id].base.write(data)
    }

    fn set_policy_set(&mut self, policy_id: u64, account: Address, value: bool) -> Result<()> {
        self.policy_set[policy_id][account].write(value)
    }
}

impl AuthRole {
    #[inline]
    fn transfer_or(t2_variant: Self) -> Self {
        if StorageCtx.spec().is_enabled_in(crate::BaseBSpec::Azul) {
            t2_variant
        } else {
            Self::Transfer
        }
    }

    /// Hardfork-aware: always returns `Transfer`.
    pub fn transfer() -> Self {
        Self::Transfer
    }

    /// Hardfork-aware: returns `Sender` for T2+, `Transfer` for pre-T2.
    pub fn sender() -> Self {
        Self::transfer_or(Self::Sender)
    }

    /// Hardfork-aware: returns `Recipient` for T2+, `Transfer` for pre-T2.
    pub fn recipient() -> Self {
        Self::transfer_or(Self::Recipient)
    }

    /// Hardfork-aware: returns `MintRecipient` for T2+, `Transfer` for pre-T2.
    pub fn mint_recipient() -> Self {
        Self::transfer_or(Self::MintRecipient)
    }
}

/// Returns `true` if the error indicates a failed policy lookup — the policy type is invalid
/// or the policy doesn't exist.
pub fn is_policy_lookup_error(e: &BasePrecompileError) -> bool {
    if StorageCtx.spec().is_enabled_in(crate::BaseBSpec::Azul) {
        // T2+: typed B403 errors
        *e == B403RegistryError::invalid_policy_type().into()
            || *e == B403RegistryError::policy_not_found().into()
    } else {
        // Pre-T2: legacy Panic(UnderOverflow) sentinel
        *e == BasePrecompileError::under_overflow()
    }
}

/// Extension trait for [`PolicyType`] validation.
trait PolicyTypeExt {
    /// Validates that this is a simple policy type and returns its `u8` discriminant.
    fn ensure_is_simple(&self) -> Result<u8>;
}

impl PolicyTypeExt for PolicyType {
    /// Validates and returns the policy type to store, handling backward compatibility.
    ///
    /// Pre-T2: Converts `COMPOUND` and `__Invalid` to 255 to match original ABI decoding behavior.
    /// T2+: Only allows `WHITELIST` and `BLACKLIST`.
    fn ensure_is_simple(&self) -> Result<u8> {
        match self {
            Self::WHITELIST | Self::BLACKLIST => Ok(*self as u8),
            Self::COMPOUND | Self::__Invalid => {
                if StorageCtx.spec().is_enabled_in(crate::BaseBSpec::Azul) {
                    Err(B403RegistryError::incompatible_policy_type().into())
                } else {
                    Ok(Self::__Invalid as u8)
                }
            }
        }
    }
}
