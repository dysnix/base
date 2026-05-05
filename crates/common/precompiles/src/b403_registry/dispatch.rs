//! ABI dispatch for the [`B403Registry`] precompile.

use crate::BaseBSpec;
use crate::{
    Precompile, SelectorSchedule,
    b403_registry::{AuthRole, B403Registry},
    charge_input_cost, dispatch_call, mutate, mutate_void, view,
};
use alloy::{
    primitives::Address,
    sol_types::{SolCall, SolInterface},
};
use base_precompiles_contracts::IB403Registry::{self, IB403RegistryCalls};
use revm::precompile::PrecompileResult;

const T2_ADDED: &[[u8; 4]] = &[
    IB403Registry::isAuthorizedSenderCall::SELECTOR,
    IB403Registry::isAuthorizedRecipientCall::SELECTOR,
    IB403Registry::isAuthorizedMintRecipientCall::SELECTOR,
    IB403Registry::compoundPolicyDataCall::SELECTOR,
    IB403Registry::createCompoundPolicyCall::SELECTOR,
];

impl Precompile for B403Registry {
    fn call(&mut self, calldata: &[u8], msg_sender: Address) -> PrecompileResult {
        if let Some(err) = charge_input_cost(&mut self.storage, calldata) {
            return err;
        }

        dispatch_call(
            calldata,
            &[SelectorSchedule::new(BaseBSpec::Azul).with_added(T2_ADDED)],
            IB403RegistryCalls::abi_decode,
            |call| match call {
                IB403RegistryCalls::policyIdCounter(call) => {
                    view(call, |_| self.policy_id_counter())
                }
                IB403RegistryCalls::policyExists(call) => view(call, |c| self.policy_exists(c)),
                IB403RegistryCalls::policyData(call) => view(call, |c| self.policy_data(c)),
                IB403RegistryCalls::isAuthorized(call) => {
                    view(call, |c| self.is_authorized_as(c.policyId, c.user, AuthRole::Transfer))
                }
                // B1015: T2+ only (gated via T2_ADDED_SELECTORS)
                IB403RegistryCalls::isAuthorizedSender(call) => {
                    view(call, |c| self.is_authorized_as(c.policyId, c.user, AuthRole::Sender))
                }
                IB403RegistryCalls::isAuthorizedRecipient(call) => {
                    view(call, |c| self.is_authorized_as(c.policyId, c.user, AuthRole::Recipient))
                }
                IB403RegistryCalls::isAuthorizedMintRecipient(call) => view(call, |c| {
                    self.is_authorized_as(c.policyId, c.user, AuthRole::MintRecipient)
                }),
                IB403RegistryCalls::compoundPolicyData(call) => {
                    view(call, |c| self.compound_policy_data(c))
                }
                IB403RegistryCalls::createPolicy(call) => {
                    mutate(call, msg_sender, |s, c| self.create_policy(s, c))
                }
                IB403RegistryCalls::createPolicyWithAccounts(call) => {
                    mutate(call, msg_sender, |s, c| self.create_policy_with_accounts(s, c))
                }
                IB403RegistryCalls::setPolicyAdmin(call) => {
                    mutate_void(call, msg_sender, |s, c| self.set_policy_admin(s, c))
                }
                IB403RegistryCalls::modifyPolicyWhitelist(call) => {
                    mutate_void(call, msg_sender, |s, c| self.modify_policy_whitelist(s, c))
                }
                IB403RegistryCalls::modifyPolicyBlacklist(call) => {
                    mutate_void(call, msg_sender, |s, c| self.modify_policy_blacklist(s, c))
                }
                // B1015: T2+ only (gated via T2_ADDED_SELECTORS)
                IB403RegistryCalls::createCompoundPolicy(call) => {
                    mutate(call, msg_sender, |s, c| self.create_compound_policy(s, c))
                }
            },
        )
    }
}
