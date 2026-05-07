//! ABI dispatch for the [`BaseDex`] precompile.

use crate::{
    Precompile, charge_input_cost, dispatch_call, metadata, mutate, storage::Handler, view,
};
use alloy::{primitives::Address, sol_types::SolInterface};
use base_precompiles_contracts::{IBaseDex, IBaseDex::IBaseDexCalls};
use revm::precompile::PrecompileResult;

use super::{BaseDex, FEE_DENOMINATOR, FEE_NUMERATOR, MINIMUM_LIQUIDITY};

impl Precompile for BaseDex {
    fn call(&mut self, calldata: &[u8], msg_sender: Address) -> PrecompileResult {
        if let Some(err) = charge_input_cost(&mut self.storage, calldata) {
            return err;
        }

        dispatch_call(calldata, &[], IBaseDexCalls::abi_decode, |call| match call {
            IBaseDexCalls::BASE_TOKEN(_) => {
                metadata::<IBaseDex::BASE_TOKENCall>(|| Ok(self.base_token()))
            }
            IBaseDexCalls::FEE_NUMERATOR(_) => {
                metadata::<IBaseDex::FEE_NUMERATORCall>(|| Ok(FEE_NUMERATOR))
            }
            IBaseDexCalls::FEE_DENOMINATOR(_) => {
                metadata::<IBaseDex::FEE_DENOMINATORCall>(|| Ok(FEE_DENOMINATOR))
            }
            IBaseDexCalls::MINIMUM_LIQUIDITY(_) => {
                metadata::<IBaseDex::MINIMUM_LIQUIDITYCall>(|| Ok(MINIMUM_LIQUIDITY))
            }
            IBaseDexCalls::initializeBaseToken(call) => {
                mutate(call, msg_sender, |sender, _| self.initialize_base_token(sender))
            }
            IBaseDexCalls::getPool(call) => view(call, |c| Ok(self.get_pool(c.token)?.into())),
            IBaseDexCalls::pools(call) => view(call, |c| Ok(self.pools[c.token].read()?.into())),
            IBaseDexCalls::totalSupply(call) => view(call, |c| self.total_supply[c.token].read()),
            IBaseDexCalls::liquidityBalances(call) => {
                view(call, |c| self.liquidity_balances[c.token][c.user].read())
            }
            IBaseDexCalls::quoteExactInput(call) => {
                view(call, |c| self.quote_exact_input(c.tokenIn, c.tokenOut, c.amountIn))
            }
            IBaseDexCalls::addLiquidity(call) => mutate(call, msg_sender, |sender, c| {
                self.add_liquidity(sender, c.token, c.amountToken, c.amountBase, c.to)
            }),
            IBaseDexCalls::removeLiquidity(call) => mutate(call, msg_sender, |sender, c| {
                let (amount_token, amount_base) =
                    self.remove_liquidity(sender, c.token, c.liquidity, c.to)?;
                Ok(IBaseDex::removeLiquidityReturn {
                    amountToken: amount_token,
                    amountBase: amount_base,
                })
            }),
            IBaseDexCalls::swapExactTokensForTokens(call) => {
                mutate(call, msg_sender, |sender, c| {
                    self.swap_exact_tokens_for_tokens(
                        sender,
                        c.tokenIn,
                        c.tokenOut,
                        c.amountIn,
                        c.minAmountOut,
                        c.to,
                    )
                })
            }
        })
    }
}
