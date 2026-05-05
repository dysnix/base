//! Opt-in staking [rewards system] for B20 tokens.
//!
//! Token holders opt in by setting a reward recipient via [`B20Token::set_reward_recipient`].
//! Rewards are distributed pro-rata across the opted-in supply and tracked via a global
//! reward-per-token accumulator scaled by [`ACC_PRECISION`].
//!
//! [Reward system]: <https://docs.base.xyz/protocol/b20-rewards/overview>

use crate::BaseBAddressExt;
use crate::{
    b20::{B20Token, Recipient},
    error::{BasePrecompileError, Result},
    storage::Handler,
};
use alloy::primitives::{Address, U256, uint};
use base_precompiles_contracts::{B20Error, B20Event, IB20};
use base_precompiles_macros::Storable;

/// Precision multiplier for reward-per-token accumulator (1e18).
pub const ACC_PRECISION: U256 = uint!(1000000000000000000_U256);

impl B20Token {
    /// Distributes `amount` of reward tokens from the caller into the opted-in reward pool.
    /// Transfers tokens to the contract and increases the global reward-per-token accumulator
    /// proportionally to the opted-in supply.
    ///
    /// # Errors
    /// - `Paused` — token transfers are currently paused
    /// - `InvalidAmount` — `amount` is zero
    /// - `PolicyForbids` — B403 policy rejects the transfer
    /// - `SpendingLimitExceeded` — access key spending limit exceeded
    /// - `InsufficientBalance` — caller balance lower than `amount`
    /// - `NoOptedInSupply` — no tokens are currently opted into rewards
    pub fn distribute_reward(
        &mut self,
        msg_sender: Address,
        call: IB20::distributeRewardCall,
    ) -> Result<()> {
        self.check_not_paused()?;
        let token_address = self.address;

        if call.amount == U256::ZERO {
            return Err(B20Error::invalid_amount().into());
        }

        self.ensure_transfer_authorized(msg_sender, token_address)?;
        self.check_and_update_spending_limit(msg_sender, call.amount)?;

        self._transfer(msg_sender, &Recipient::direct(token_address), call.amount)?;

        let opted_in_supply = U256::from(self.get_opted_in_supply()?);
        if opted_in_supply.is_zero() {
            return Err(B20Error::no_opted_in_supply().into());
        }

        let delta_rpt = call
            .amount
            .checked_mul(ACC_PRECISION)
            .and_then(|v| v.checked_div(opted_in_supply))
            .ok_or(BasePrecompileError::under_overflow())?;
        let current_rpt = self.get_global_reward_per_token()?;
        let new_rpt =
            current_rpt.checked_add(delta_rpt).ok_or(BasePrecompileError::under_overflow())?;
        self.set_global_reward_per_token(new_rpt)?;

        // Emit distributed reward event (recipients claim accrued rewards separately)
        self.emit_event(B20Event::RewardDistributed(IB20::RewardDistributed {
            funder: msg_sender,
            amount: call.amount,
        }))?;

        Ok(())
    }

    /// Updates and accumulates accrued rewards for a specific token holder.
    ///
    /// This function calculates the rewards earned by a holder based on their
    /// balance and the reward per token difference since their last update.
    /// Rewards are accumulated in the delegated recipient's rewardBalance.
    /// Returns the holder's delegated recipient address.
    pub fn update_rewards(&mut self, holder: Address) -> Result<Address> {
        let mut info = self.user_reward_info[holder].read()?;

        let cached_delegate = info.reward_recipient;

        let global_reward_per_token = self.get_global_reward_per_token()?;
        let reward_per_token_delta = global_reward_per_token
            .checked_sub(info.reward_per_token)
            .ok_or(BasePrecompileError::under_overflow())?;

        if reward_per_token_delta != U256::ZERO {
            if cached_delegate != Address::ZERO {
                let holder_balance = self.get_balance(holder)?;
                let reward = holder_balance
                    .checked_mul(reward_per_token_delta)
                    .and_then(|v| v.checked_div(ACC_PRECISION))
                    .ok_or(BasePrecompileError::under_overflow())?;

                // Add reward to delegate's balance (or holder's own balance if self-delegated)
                if cached_delegate == holder {
                    info.reward_balance = info
                        .reward_balance
                        .checked_add(reward)
                        .ok_or(BasePrecompileError::under_overflow())?;
                } else {
                    let mut delegate_info = self.user_reward_info[cached_delegate].read()?;
                    delegate_info.reward_balance = delegate_info
                        .reward_balance
                        .checked_add(reward)
                        .ok_or(BasePrecompileError::under_overflow())?;
                    self.user_reward_info[cached_delegate].write(delegate_info)?;
                }
            }
            info.reward_per_token = global_reward_per_token;
            self.user_reward_info[holder].write(info)?;
        }

        Ok(cached_delegate)
    }

    /// Sets or changes the reward recipient for a token holder.
    ///
    /// This function allows a token holder to designate who should receive their
    /// share of rewards. Setting to zero address opts out of rewards.
    ///
    /// # Errors
    /// - `Paused` — token transfers are currently paused
    /// - `PolicyForbids` — B403 policy rejects the sender→recipient transfer authorization
    /// - `InvalidRecipient` — B1022 virtual addresses are rejected
    pub fn set_reward_recipient(
        &mut self,
        msg_sender: Address,
        call: IB20::setRewardRecipientCall,
    ) -> Result<()> {
        self.check_not_paused()?;

        // B1022: reject virtual addresses as reward recipients
        if self.storage.spec().is_enabled_in(crate::BaseBSpec::Azul) && call.recipient.is_virtual()
        {
            return Err(B20Error::invalid_recipient().into());
        }

        if call.recipient != Address::ZERO {
            self.ensure_transfer_authorized(msg_sender, call.recipient)?;
        }

        let from_delegate = self.update_rewards(msg_sender)?;

        let holder_balance = self.get_balance(msg_sender)?;

        if from_delegate != Address::ZERO {
            if call.recipient == Address::ZERO {
                let opted_in_supply = U256::from(self.get_opted_in_supply()?)
                    .checked_sub(holder_balance)
                    .ok_or(BasePrecompileError::under_overflow())?;
                self.set_opted_in_supply(
                    opted_in_supply
                        .try_into()
                        .map_err(|_| BasePrecompileError::under_overflow())?,
                )?;
            }
        } else if call.recipient != Address::ZERO {
            let opted_in_supply = U256::from(self.get_opted_in_supply()?)
                .checked_add(holder_balance)
                .ok_or(BasePrecompileError::under_overflow())?;
            self.set_opted_in_supply(
                opted_in_supply.try_into().map_err(|_| BasePrecompileError::under_overflow())?,
            )?;
        }

        let mut info = self.user_reward_info[msg_sender].read()?;
        info.reward_recipient = call.recipient;
        self.user_reward_info[msg_sender].write(info)?;

        // Emit reward recipient set event
        self.emit_event(B20Event::RewardRecipientSet(IB20::RewardRecipientSet {
            holder: msg_sender,
            recipient: call.recipient,
        }))?;

        Ok(())
    }

    /// Claims accumulated rewards for a recipient.
    ///
    /// Pays out the lesser of the accrued reward balance and the contract's token
    /// balance. Any remainder stays stored for future claims.
    ///
    /// # Errors
    /// - `Paused` — token transfers are currently paused
    /// - `PolicyForbids` — B403 policy rejects the contract→caller transfer authorization
    pub fn claim_rewards(&mut self, msg_sender: Address) -> Result<U256> {
        self.check_not_paused()?;
        self.ensure_transfer_authorized(self.address, msg_sender)?;

        self.update_rewards(msg_sender)?;

        let mut info = self.user_reward_info[msg_sender].read()?;
        let amount = info.reward_balance;
        let contract_address = self.address;
        let contract_balance = self.get_balance(contract_address)?;
        let max_amount = amount.min(contract_balance);

        let reward_recipient = info.reward_recipient;
        info.reward_balance =
            amount.checked_sub(max_amount).ok_or(BasePrecompileError::under_overflow())?;
        self.user_reward_info[msg_sender].write(info)?;

        if max_amount > U256::ZERO {
            let new_contract_balance = contract_balance
                .checked_sub(max_amount)
                .ok_or(BasePrecompileError::under_overflow())?;
            self.set_balance(contract_address, new_contract_balance)?;

            let recipient_balance = self
                .get_balance(msg_sender)?
                .checked_add(max_amount)
                .ok_or(BasePrecompileError::under_overflow())?;
            self.set_balance(msg_sender, recipient_balance)?;

            if reward_recipient != Address::ZERO {
                let opted_in_supply = U256::from(self.get_opted_in_supply()?)
                    .checked_add(max_amount)
                    .ok_or(BasePrecompileError::under_overflow())?;
                self.set_opted_in_supply(
                    opted_in_supply
                        .try_into()
                        .map_err(|_| BasePrecompileError::under_overflow())?,
                )?;
            }

            self.emit_event(B20Event::Transfer(IB20::Transfer {
                from: contract_address,
                to: msg_sender,
                amount: max_amount,
            }))?;
        }

        Ok(max_amount)
    }

    /// Gets the accumulated global reward per token.
    pub fn get_global_reward_per_token(&self) -> Result<U256> {
        self.global_reward_per_token.read()
    }

    /// Sets the accumulated global reward per token in storage.
    fn set_global_reward_per_token(&mut self, value: U256) -> Result<()> {
        self.global_reward_per_token.write(value)
    }

    /// Gets the total supply of tokens opted into rewards from storage.
    pub fn get_opted_in_supply(&self) -> Result<u128> {
        self.opted_in_supply.read()
    }

    /// Sets the total supply of tokens opted into rewards.
    pub fn set_opted_in_supply(&mut self, value: u128) -> Result<()> {
        self.opted_in_supply.write(value)
    }

    /// Handles reward accounting for both sender and receiver during token transfers.
    pub fn handle_rewards_on_transfer(
        &mut self,
        from: Address,
        to: Address,
        amount: U256,
    ) -> Result<()> {
        let from_delegate = self.update_rewards(from)?;
        let to_delegate = self.update_rewards(to)?;

        if !from_delegate.is_zero() {
            if to_delegate.is_zero() {
                let opted_in_supply = U256::from(self.get_opted_in_supply()?)
                    .checked_sub(amount)
                    .ok_or(BasePrecompileError::under_overflow())?;
                self.set_opted_in_supply(
                    opted_in_supply
                        .try_into()
                        .map_err(|_| BasePrecompileError::under_overflow())?,
                )?;
            }
        } else if !to_delegate.is_zero() {
            let opted_in_supply = U256::from(self.get_opted_in_supply()?)
                .checked_add(amount)
                .ok_or(BasePrecompileError::under_overflow())?;
            self.set_opted_in_supply(
                opted_in_supply.try_into().map_err(|_| BasePrecompileError::under_overflow())?,
            )?;
        }

        Ok(())
    }

    /// Handles reward accounting when tokens are minted to an address.
    pub fn handle_rewards_on_mint(&mut self, to: Address, amount: U256) -> Result<()> {
        let to_delegate = self.update_rewards(to)?;

        if !to_delegate.is_zero() {
            let opted_in_supply = U256::from(self.get_opted_in_supply()?)
                .checked_add(amount)
                .ok_or(BasePrecompileError::under_overflow())?;
            self.set_opted_in_supply(
                opted_in_supply.try_into().map_err(|_| BasePrecompileError::under_overflow())?,
            )?;
        }

        Ok(())
    }

    /// Retrieves user reward information for a given account.
    pub fn get_user_reward_info(&self, account: Address) -> Result<UserRewardInfo> {
        self.user_reward_info[account].read()
    }

    /// Calculates the pending claimable rewards for an account without modifying state.
    ///
    /// This function returns the total pending claimable reward amount, which includes:
    /// 1. The stored reward balance from previous updates
    /// 2. Newly accrued rewards based on the current global reward per token
    ///
    /// For accounts that have delegated their rewards to another recipient, only the stored
    /// reward balance is returned (new accrual is skipped since it goes to the delegate).
    pub fn get_pending_rewards(&self, account: Address) -> Result<u128> {
        let info = self.user_reward_info[account].read()?;

        // Start with the stored reward balance
        let mut pending = info.reward_balance;

        // For the account's own accrued rewards (if self-delegated):
        if info.reward_recipient == account {
            let holder_balance = self.get_balance(account)?;
            if holder_balance > U256::ZERO {
                let global_reward_per_token = self.get_global_reward_per_token()?;
                let reward_per_token_delta = global_reward_per_token
                    .checked_sub(info.reward_per_token)
                    .ok_or(BasePrecompileError::under_overflow())?;

                if reward_per_token_delta > U256::ZERO {
                    let accrued = holder_balance
                        .checked_mul(reward_per_token_delta)
                        .and_then(|v| v.checked_div(ACC_PRECISION))
                        .ok_or(BasePrecompileError::under_overflow())?;
                    pending =
                        pending.checked_add(accrued).ok_or(BasePrecompileError::under_overflow())?;
                }
            }
        }

        pending.try_into().map_err(|_| BasePrecompileError::under_overflow())
    }
}

/// Per-user reward tracking state for the opt-in staking rewards system.
#[derive(Debug, Clone, Storable)]
pub struct UserRewardInfo {
    /// Address that receives this user's accrued rewards (`Address::ZERO` = opted out).
    pub reward_recipient: Address,
    /// Snapshot of the global reward-per-token at the user's last update.
    pub reward_per_token: U256,
    /// Accumulated but unclaimed reward balance.
    pub reward_balance: U256,
}

impl From<UserRewardInfo> for IB20::UserRewardInfo {
    fn from(value: UserRewardInfo) -> Self {
        Self {
            rewardRecipient: value.reward_recipient,
            rewardPerToken: value.reward_per_token,
            rewardBalance: value.reward_balance,
        }
    }
}
