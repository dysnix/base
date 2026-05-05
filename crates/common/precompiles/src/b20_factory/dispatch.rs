//! ABI dispatch for the [`B20Factory`] precompile.

use crate::{Precompile, b20_factory::B20Factory, charge_input_cost, dispatch_call, mutate, view};
use alloy::{primitives::Address, sol_types::SolInterface};
use base_precompiles_contracts::IB20Factory::IB20FactoryCalls;
use revm::precompile::PrecompileResult;

impl Precompile for B20Factory {
    fn call(&mut self, calldata: &[u8], msg_sender: Address) -> PrecompileResult {
        if let Some(err) = charge_input_cost(&mut self.storage, calldata) {
            return err;
        }

        dispatch_call(calldata, &[], IB20FactoryCalls::abi_decode, |call| match call {
            IB20FactoryCalls::createToken(call) => {
                mutate(call, msg_sender, |s, c| self.create_token(s, c))
            }
            IB20FactoryCalls::isB20(call) => view(call, |c| self.is_b20(c.token)),
            IB20FactoryCalls::getTokenAddress(call) => view(call, |c| self.get_token_address(c)),
        })
    }
}
