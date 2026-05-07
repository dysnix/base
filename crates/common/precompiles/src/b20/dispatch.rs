//! ABI dispatch for the [`B20Token`] precompile.

use alloy::{
    primitives::Address,
    sol_types::{SolCall, SolInterface},
};
use base_precompiles_contracts::{B20Error, IB20::IB20Calls, IRolesAuth::IRolesAuthCalls};
use revm::precompile::PrecompileResult;

use crate::{
    BaseBSpec, Precompile, SelectorSchedule,
    b20::{B20Token, IB20},
    charge_input_cost, dispatch_call, metadata, mutate, mutate_void,
    storage::ContractStorage,
    view,
};

const T2_ADDED: &[[u8; 4]] =
    &[IB20::permitCall::SELECTOR, IB20::noncesCall::SELECTOR, IB20::DOMAIN_SEPARATORCall::SELECTOR];

/// Decoded call variant — either a B20 token call or a role-management call.
enum B20Call {
    B20(IB20Calls),
    RolesAuth(IRolesAuthCalls),
}

impl B20Call {
    fn decode(calldata: &[u8]) -> Result<Self, alloy::sol_types::Error> {
        // safe to expect as `dispatch_call` pre-validates calldata len
        let selector: [u8; 4] = calldata[..4].try_into().expect("calldata len >= 4");

        if IRolesAuthCalls::valid_selector(selector) {
            IRolesAuthCalls::abi_decode(calldata).map(Self::RolesAuth)
        } else {
            IB20Calls::abi_decode(calldata).map(Self::B20)
        }
    }
}

impl Precompile for B20Token {
    fn call(&mut self, calldata: &[u8], msg_sender: Address) -> PrecompileResult {
        if let Some(err) = charge_input_cost(&mut self.storage, calldata) {
            return err;
        }

        // Ensure that the token is initialized (has bytecode)
        let initialized = match self.is_initialized() {
            Ok(v) => v,
            Err(_) if !self.storage.spec().is_enabled_in(crate::BaseBSpec::Beryl) => false,
            Err(e) => return self.storage.error_result(e),
        };
        if !initialized {
            return self.storage.error_result(B20Error::uninitialized());
        }

        dispatch_call(
            calldata,
            &[SelectorSchedule::new(BaseBSpec::Beryl).with_added(T2_ADDED)],
            B20Call::decode,
            |call| match call {
                // Metadata functions (no calldata decoding needed)
                B20Call::B20(IB20Calls::name(_)) => metadata::<IB20::nameCall>(|| self.name()),
                B20Call::B20(IB20Calls::symbol(_)) => {
                    metadata::<IB20::symbolCall>(|| self.symbol())
                }
                B20Call::B20(IB20Calls::decimals(_)) => {
                    metadata::<IB20::decimalsCall>(|| self.decimals())
                }
                B20Call::B20(IB20Calls::currency(_)) => {
                    metadata::<IB20::currencyCall>(|| self.currency())
                }
                B20Call::B20(IB20Calls::totalSupply(_)) => {
                    metadata::<IB20::totalSupplyCall>(|| self.total_supply())
                }
                B20Call::B20(IB20Calls::supplyCap(_)) => {
                    metadata::<IB20::supplyCapCall>(|| self.supply_cap())
                }
                B20Call::B20(IB20Calls::transferPolicyId(_)) => {
                    metadata::<IB20::transferPolicyIdCall>(|| self.transfer_policy_id())
                }
                B20Call::B20(IB20Calls::paused(_)) => {
                    metadata::<IB20::pausedCall>(|| self.paused())
                }

                // View functions
                B20Call::B20(IB20Calls::balanceOf(call)) => view(call, |c| self.balance_of(c)),
                B20Call::B20(IB20Calls::allowance(call)) => view(call, |c| self.allowance(c)),
                B20Call::B20(IB20Calls::PAUSE_ROLE(call)) => view(call, |_| Ok(Self::pause_role())),
                B20Call::B20(IB20Calls::UNPAUSE_ROLE(call)) => {
                    view(call, |_| Ok(Self::unpause_role()))
                }
                B20Call::B20(IB20Calls::ISSUER_ROLE(call)) => {
                    view(call, |_| Ok(Self::issuer_role()))
                }
                B20Call::B20(IB20Calls::BURN_BLOCKED_ROLE(call)) => {
                    view(call, |_| Ok(Self::burn_blocked_role()))
                }

                // State changing functions
                B20Call::B20(IB20Calls::transferFrom(call)) => {
                    mutate(call, msg_sender, |s, c| self.transfer_from(s, c))
                }
                B20Call::B20(IB20Calls::transfer(call)) => {
                    mutate(call, msg_sender, |s, c| self.transfer(s, c))
                }
                B20Call::B20(IB20Calls::approve(call)) => {
                    mutate(call, msg_sender, |s, c| self.approve(s, c))
                }
                B20Call::B20(IB20Calls::changeTransferPolicyId(call)) => {
                    mutate_void(call, msg_sender, |s, c| self.change_transfer_policy_id(s, c))
                }
                B20Call::B20(IB20Calls::setSupplyCap(call)) => {
                    mutate_void(call, msg_sender, |s, c| self.set_supply_cap(s, c))
                }
                B20Call::B20(IB20Calls::pause(call)) => {
                    mutate_void(call, msg_sender, |s, c| self.pause(s, c))
                }
                B20Call::B20(IB20Calls::unpause(call)) => {
                    mutate_void(call, msg_sender, |s, c| self.unpause(s, c))
                }
                B20Call::B20(IB20Calls::mint(call)) => {
                    mutate_void(call, msg_sender, |s, c| self.mint(s, c))
                }
                B20Call::B20(IB20Calls::mintWithMemo(call)) => {
                    mutate_void(call, msg_sender, |s, c| self.mint_with_memo(s, c))
                }
                B20Call::B20(IB20Calls::burn(call)) => {
                    mutate_void(call, msg_sender, |s, c| self.burn(s, c))
                }
                B20Call::B20(IB20Calls::burnWithMemo(call)) => {
                    mutate_void(call, msg_sender, |s, c| self.burn_with_memo(s, c))
                }
                B20Call::B20(IB20Calls::burnBlocked(call)) => {
                    mutate_void(call, msg_sender, |s, c| self.burn_blocked(s, c))
                }
                B20Call::B20(IB20Calls::transferWithMemo(call)) => {
                    mutate_void(call, msg_sender, |s, c| self.transfer_with_memo(s, c))
                }
                B20Call::B20(IB20Calls::transferFromWithMemo(call)) => {
                    mutate(call, msg_sender, |sender, c| self.transfer_from_with_memo(sender, c))
                }

                B20Call::B20(IB20Calls::permit(call)) => {
                    mutate_void(call, msg_sender, |_s, c| self.permit(c))
                }
                B20Call::B20(IB20Calls::nonces(call)) => view(call, |c| self.nonces(c)),
                B20Call::B20(IB20Calls::DOMAIN_SEPARATOR(call)) => {
                    view(call, |_| self.domain_separator())
                }

                // RolesAuth functions
                B20Call::RolesAuth(IRolesAuthCalls::hasRole(call)) => {
                    view(call, |c| self.has_role(c))
                }
                B20Call::RolesAuth(IRolesAuthCalls::getRoleAdmin(call)) => {
                    view(call, |c| self.get_role_admin(c))
                }
                B20Call::RolesAuth(IRolesAuthCalls::grantRole(call)) => {
                    mutate_void(call, msg_sender, |s, c| self.grant_role(s, c))
                }
                B20Call::RolesAuth(IRolesAuthCalls::revokeRole(call)) => {
                    mutate_void(call, msg_sender, |s, c| self.revoke_role(s, c))
                }
                B20Call::RolesAuth(IRolesAuthCalls::renounceRole(call)) => {
                    mutate_void(call, msg_sender, |s, c| self.renounce_role(s, c))
                }
                B20Call::RolesAuth(IRolesAuthCalls::setRoleAdmin(call)) => {
                    mutate_void(call, msg_sender, |s, c| self.set_role_admin(s, c))
                }
            },
        )
    }
}
