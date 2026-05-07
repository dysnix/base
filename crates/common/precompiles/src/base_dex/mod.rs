//! Base DEX precompile for native B20 constant-product swaps.

mod dispatch;

use crate::{
    b20::{B20Token, IB20},
    b20_factory::B20Factory,
    error::{BasePrecompileError, Result},
    storage::{ContractStorage, Handler, Mapping},
};
use alloy::primitives::{Address, U256, uint};
use base_precompiles_contracts::{
    BASE_DEX_ADDRESS, BASE_USD_ADDRESS, BaseDexError, BaseDexEvent, IBaseDex,
};
use base_precompiles_macros::{Storable, contract};

/// Swap fee numerator, matching Uniswap V2's 0.30% fee.
pub const FEE_NUMERATOR: U256 = uint!(997_U256);
/// Swap fee denominator, matching Uniswap V2's 0.30% fee.
pub const FEE_DENOMINATOR: U256 = uint!(1000_U256);
/// Minimum liquidity locked on first pool creation.
pub const MINIMUM_LIQUIDITY: U256 = uint!(1000_U256);

/// Reserves for a token/Base USD pool.
#[derive(Clone, Debug, Default, PartialEq, Eq, Storable)]
pub struct Pool {
    /// Reserve of the non-base B20 token.
    pub reserve_token: u128,
    /// Reserve of Base USD.
    pub reserve_base: u128,
}

impl From<Pool> for IBaseDex::Pool {
    fn from(value: Pool) -> Self {
        Self { reserveToken: value.reserve_token, reserveBase: value.reserve_base }
    }
}

/// Singleton Base DEX precompile.
#[contract(addr = BASE_DEX_ADDRESS)]
pub struct BaseDex {
    pools: Mapping<Address, Pool>,
    total_supply: Mapping<Address, U256>,
    liquidity_balances: Mapping<Address, Mapping<Address, U256>>,
}

impl BaseDex {
    /// Returns the reserved Base USD token address.
    pub fn base_token(&self) -> Address {
        BASE_USD_ADDRESS
    }

    /// Initializes the singleton DEX account when it has not been materialized yet.
    pub fn ensure_initialized(&mut self) -> Result<()> {
        if !self.is_initialized()? {
            self.__initialize()?;
        }
        Ok(())
    }

    /// Initializes the reserved Base USD token for local/devnet bootstrapping.
    ///
    /// Production hardfork code can perform the same setup directly and leave this as a no-op.
    pub fn initialize_base_token(&mut self, msg_sender: Address) -> Result<Address> {
        self.ensure_initialized()?;

        if B20Factory::new().is_b20(BASE_USD_ADDRESS)? {
            return Ok(BASE_USD_ADDRESS);
        }

        B20Factory::new().create_token_reserved_address(
            BASE_USD_ADDRESS,
            "Base USD",
            "BUSD",
            "USD",
            msg_sender,
        )
    }

    /// Returns the pool for a non-base token.
    pub fn get_pool(&self, token: Address) -> Result<Pool> {
        self.pools[token].read()
    }

    /// Returns total LP supply for a non-base token pool.
    pub fn get_total_supply(&self, token: Address) -> Result<U256> {
        self.total_supply[token].read()
    }

    /// Returns a user's LP balance for a non-base token pool.
    pub fn get_liquidity_balance(&self, token: Address, user: Address) -> Result<U256> {
        self.liquidity_balances[token][user].read()
    }

    /// Quotes an exact-input swap across one or two Base USD legs.
    pub fn quote_exact_input(
        &self,
        token_in: Address,
        token_out: Address,
        amount_in: U256,
    ) -> Result<U256> {
        self.validate_swap_path(token_in, token_out)?;

        if token_in == BASE_USD_ADDRESS {
            let pool = self.pools[token_out].read()?;
            return Self::amount_out(
                amount_in,
                U256::from(pool.reserve_base),
                U256::from(pool.reserve_token),
            );
        }

        if token_out == BASE_USD_ADDRESS {
            let pool = self.pools[token_in].read()?;
            return Self::amount_out(
                amount_in,
                U256::from(pool.reserve_token),
                U256::from(pool.reserve_base),
            );
        }

        let first_pool = self.pools[token_in].read()?;
        let base_out = Self::amount_out(
            amount_in,
            U256::from(first_pool.reserve_token),
            U256::from(first_pool.reserve_base),
        )?;
        let second_pool = self.pools[token_out].read()?;
        Self::amount_out(
            base_out,
            U256::from(second_pool.reserve_base),
            U256::from(second_pool.reserve_token),
        )
    }

    /// Adds exact liquidity to a token/Base USD pool.
    pub fn add_liquidity(
        &mut self,
        msg_sender: Address,
        token: Address,
        amount_token: U256,
        amount_base: U256,
        to: Address,
    ) -> Result<U256> {
        self.ensure_initialized()?;
        self.validate_pool_token(token)?;
        self.validate_base_token()?;
        Self::validate_amount(amount_token)?;
        Self::validate_amount(amount_base)?;

        let mut pool = self.pools[token].read()?;
        let mut total_supply = self.total_supply[token].read()?;
        let liquidity = if total_supply.is_zero() {
            let root = Self::sqrt(
                amount_token
                    .checked_mul(amount_base)
                    .ok_or(BasePrecompileError::under_overflow())?,
            );
            if root <= MINIMUM_LIQUIDITY {
                return Err(BaseDexError::insufficient_liquidity().into());
            }
            total_supply = MINIMUM_LIQUIDITY;
            root.checked_sub(MINIMUM_LIQUIDITY).ok_or(BasePrecompileError::under_overflow())?
        } else {
            let token_liquidity = amount_token
                .checked_mul(total_supply)
                .and_then(|value| value.checked_div(U256::from(pool.reserve_token)))
                .ok_or(BasePrecompileError::under_overflow())?;
            let base_liquidity = amount_base
                .checked_mul(total_supply)
                .and_then(|value| value.checked_div(U256::from(pool.reserve_base)))
                .ok_or(BasePrecompileError::under_overflow())?;
            token_liquidity.min(base_liquidity)
        };

        if liquidity.is_zero() {
            return Err(BaseDexError::insufficient_liquidity().into());
        }

        self.transfer_in(token, msg_sender, amount_token)?;
        self.transfer_in(BASE_USD_ADDRESS, msg_sender, amount_base)?;

        pool.reserve_token = Self::add_reserve(pool.reserve_token, amount_token)?;
        pool.reserve_base = Self::add_reserve(pool.reserve_base, amount_base)?;
        self.pools[token].write(pool)?;

        let new_total_supply =
            total_supply.checked_add(liquidity).ok_or(BasePrecompileError::under_overflow())?;
        self.total_supply[token].write(new_total_supply)?;

        let balance = self.liquidity_balances[token][to].read()?;
        self.liquidity_balances[token][to]
            .write(balance.checked_add(liquidity).ok_or(BasePrecompileError::under_overflow())?)?;

        self.emit_event(BaseDexEvent::Mint(IBaseDex::Mint {
            sender: msg_sender,
            token,
            amountToken: amount_token,
            amountBase: amount_base,
            liquidity,
            to,
        }))?;

        Ok(liquidity)
    }

    /// Removes liquidity from a token/Base USD pool.
    pub fn remove_liquidity(
        &mut self,
        msg_sender: Address,
        token: Address,
        liquidity: U256,
        to: Address,
    ) -> Result<(U256, U256)> {
        self.ensure_initialized()?;
        self.validate_pool_token(token)?;
        Self::validate_amount(liquidity)?;

        let balance = self.liquidity_balances[token][msg_sender].read()?;
        if balance < liquidity {
            return Err(BaseDexError::insufficient_liquidity().into());
        }

        let total_supply = self.total_supply[token].read()?;
        if total_supply.is_zero() {
            return Err(BaseDexError::insufficient_liquidity().into());
        }

        let mut pool = self.pools[token].read()?;
        let amount_token = liquidity
            .checked_mul(U256::from(pool.reserve_token))
            .and_then(|value| value.checked_div(total_supply))
            .ok_or(BasePrecompileError::under_overflow())?;
        let amount_base = liquidity
            .checked_mul(U256::from(pool.reserve_base))
            .and_then(|value| value.checked_div(total_supply))
            .ok_or(BasePrecompileError::under_overflow())?;
        Self::validate_amount(amount_token)?;
        Self::validate_amount(amount_base)?;

        pool.reserve_token = Self::sub_reserve(pool.reserve_token, amount_token)?;
        pool.reserve_base = Self::sub_reserve(pool.reserve_base, amount_base)?;
        self.pools[token].write(pool)?;
        self.total_supply[token].write(
            total_supply.checked_sub(liquidity).ok_or(BasePrecompileError::under_overflow())?,
        )?;
        self.liquidity_balances[token][msg_sender]
            .write(balance.checked_sub(liquidity).ok_or(BasePrecompileError::under_overflow())?)?;

        self.transfer_out(token, to, amount_token)?;
        self.transfer_out(BASE_USD_ADDRESS, to, amount_base)?;

        self.emit_event(BaseDexEvent::Burn(IBaseDex::Burn {
            sender: msg_sender,
            token,
            amountToken: amount_token,
            amountBase: amount_base,
            liquidity,
            to,
        }))?;

        Ok((amount_token, amount_base))
    }

    /// Swaps an exact input amount across one or two Base USD legs.
    pub fn swap_exact_tokens_for_tokens(
        &mut self,
        msg_sender: Address,
        token_in: Address,
        token_out: Address,
        amount_in: U256,
        min_amount_out: U256,
        to: Address,
    ) -> Result<U256> {
        self.ensure_initialized()?;
        self.validate_swap_path(token_in, token_out)?;
        Self::validate_amount(amount_in)?;

        let amount_out = self.quote_exact_input(token_in, token_out, amount_in)?;
        if amount_out < min_amount_out {
            return Err(BaseDexError::insufficient_output_amount().into());
        }

        self.transfer_in(token_in, msg_sender, amount_in)?;

        if token_in == BASE_USD_ADDRESS {
            self.apply_base_to_token_swap(token_out, amount_in, amount_out)?;
        } else if token_out == BASE_USD_ADDRESS {
            self.apply_token_to_base_swap(token_in, amount_in, amount_out)?;
        } else {
            let base_out = self.apply_token_to_base_swap(token_in, amount_in, U256::ZERO)?;
            self.apply_base_to_token_swap(token_out, base_out, amount_out)?;
        }

        self.transfer_out(token_out, to, amount_out)?;

        self.emit_event(BaseDexEvent::Swap(IBaseDex::Swap {
            sender: msg_sender,
            tokenIn: token_in,
            tokenOut: token_out,
            amountIn: amount_in,
            amountOut: amount_out,
            to,
        }))?;

        Ok(amount_out)
    }

    /// Computes output amount for a Uniswap V2 style exact-input swap.
    pub fn amount_out(amount_in: U256, reserve_in: U256, reserve_out: U256) -> Result<U256> {
        Self::validate_amount(amount_in)?;
        if reserve_in.is_zero() || reserve_out.is_zero() {
            return Err(BaseDexError::insufficient_liquidity().into());
        }

        let amount_in_with_fee =
            amount_in.checked_mul(FEE_NUMERATOR).ok_or(BasePrecompileError::under_overflow())?;
        let numerator = amount_in_with_fee
            .checked_mul(reserve_out)
            .ok_or(BasePrecompileError::under_overflow())?;
        let denominator = reserve_in
            .checked_mul(FEE_DENOMINATOR)
            .and_then(|value| value.checked_add(amount_in_with_fee))
            .ok_or(BasePrecompileError::under_overflow())?;
        let amount_out =
            numerator.checked_div(denominator).ok_or(BasePrecompileError::under_overflow())?;

        if amount_out.is_zero() {
            return Err(BaseDexError::insufficient_output_amount().into());
        }
        Ok(amount_out)
    }

    /// Integer square root using the Babylonian method.
    pub fn sqrt(value: U256) -> U256 {
        if value.is_zero() {
            return U256::ZERO;
        }
        let mut z = (value + U256::ONE) / uint!(2_U256);
        let mut y = value;
        while z < y {
            y = z;
            z = (value / z + z) / uint!(2_U256);
        }
        y
    }

    /// Validates a non-zero amount.
    pub fn validate_amount(amount: U256) -> Result<()> {
        if amount.is_zero() {
            return Err(BaseDexError::invalid_amount().into());
        }
        Ok(())
    }

    /// Validates that `token` is a deployed non-base B20.
    pub fn validate_pool_token(&self, token: Address) -> Result<()> {
        if token == BASE_USD_ADDRESS {
            return Err(BaseDexError::identical_tokens().into());
        }
        self.validate_b20(token)
    }

    /// Validates that Base USD has been deployed.
    pub fn validate_base_token(&self) -> Result<()> {
        self.validate_b20(BASE_USD_ADDRESS)
    }

    /// Validates that `token` is a deployed B20 token.
    pub fn validate_b20(&self, token: Address) -> Result<()> {
        if !B20Factory::new().is_b20(token)? {
            return Err(BaseDexError::invalid_token().into());
        }
        Ok(())
    }

    /// Validates a supported swap path.
    pub fn validate_swap_path(&self, token_in: Address, token_out: Address) -> Result<()> {
        if token_in == token_out {
            return Err(BaseDexError::identical_tokens().into());
        }
        if token_in == BASE_USD_ADDRESS && token_out == BASE_USD_ADDRESS {
            return Err(BaseDexError::invalid_swap_path().into());
        }

        if token_in != BASE_USD_ADDRESS {
            self.validate_pool_token(token_in)?;
        } else {
            self.validate_base_token()?;
        }

        if token_out != BASE_USD_ADDRESS {
            self.validate_pool_token(token_out)?;
        } else {
            self.validate_base_token()?;
        }

        Ok(())
    }

    /// Pulls tokens from a caller into the DEX.
    pub fn transfer_in(&mut self, token: Address, from: Address, amount: U256) -> Result<()> {
        B20Token::from_address(token)?.system_transfer_from(from, self.address, amount)?;
        Ok(())
    }

    /// Sends tokens from the DEX to a recipient.
    pub fn transfer_out(&mut self, token: Address, to: Address, amount: U256) -> Result<()> {
        B20Token::from_address(token)?.transfer(self.address, IB20::transferCall { to, amount })?;
        Ok(())
    }

    /// Applies token to Base USD reserve updates and returns the Base USD output.
    pub fn apply_token_to_base_swap(
        &mut self,
        token: Address,
        amount_in: U256,
        expected_amount_out: U256,
    ) -> Result<U256> {
        let mut pool = self.pools[token].read()?;
        let amount_out = if expected_amount_out.is_zero() {
            Self::amount_out(
                amount_in,
                U256::from(pool.reserve_token),
                U256::from(pool.reserve_base),
            )?
        } else {
            expected_amount_out
        };

        pool.reserve_token = Self::add_reserve(pool.reserve_token, amount_in)?;
        pool.reserve_base = Self::sub_reserve(pool.reserve_base, amount_out)?;
        self.pools[token].write(pool)?;
        Ok(amount_out)
    }

    /// Applies Base USD to token reserve updates.
    pub fn apply_base_to_token_swap(
        &mut self,
        token: Address,
        amount_in: U256,
        amount_out: U256,
    ) -> Result<()> {
        let mut pool = self.pools[token].read()?;
        pool.reserve_base = Self::add_reserve(pool.reserve_base, amount_in)?;
        pool.reserve_token = Self::sub_reserve(pool.reserve_token, amount_out)?;
        self.pools[token].write(pool)
    }

    /// Adds a U256 amount to a u128 reserve.
    pub fn add_reserve(reserve: u128, amount: U256) -> Result<u128> {
        let amount: u128 = amount.try_into().map_err(|_| BasePrecompileError::under_overflow())?;
        reserve.checked_add(amount).ok_or(BasePrecompileError::under_overflow())
    }

    /// Subtracts a U256 amount from a u128 reserve.
    pub fn sub_reserve(reserve: u128, amount: U256) -> Result<u128> {
        let amount: u128 = amount.try_into().map_err(|_| BasePrecompileError::under_overflow())?;
        reserve.checked_sub(amount).ok_or_else(|| BaseDexError::insufficient_liquidity().into())
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        BASE_USD_ADDRESS, BaseBSpec,
        b20::{B20Token, IRolesAuth},
        b20_factory::B20Factory,
        base_dex::{BaseDex, FEE_DENOMINATOR, FEE_NUMERATOR, MINIMUM_LIQUIDITY},
        storage::{ContractStorage, StorageCtx, hashmap::HashMapStorageProvider},
    };
    use alloy::primitives::{Address, U256, address, uint};

    const TOKEN_A: Address = address!("0x84530000000000000000000000000000000000a1");
    const TOKEN_B: Address = address!("0x84530000000000000000000000000000000000a2");

    fn setup_token(address: Address, name: &str, symbol: &str, admin: Address) -> B20Token {
        B20Factory::new()
            .create_token_reserved_address(address, name, symbol, "USD", admin)
            .unwrap();
        let mut token = B20Token::from_address(address).unwrap();
        token
            .grant_role(
                admin,
                IRolesAuth::grantRoleCall { role: B20Token::issuer_role(), account: admin },
            )
            .unwrap();
        token
    }

    fn mint(token: &mut B20Token, admin: Address, to: Address, amount: U256) {
        token.mint(admin, crate::b20::IB20::mintCall { to, amount }).unwrap();
    }

    fn balance(token: Address, account: Address) -> U256 {
        B20Token::from_address(token)
            .unwrap()
            .balance_of(crate::b20::IB20::balanceOfCall { account })
            .unwrap()
    }

    #[test]
    fn add_liquidity_locks_minimum_liquidity() {
        let mut storage = HashMapStorageProvider::new_with_spec(1, BaseBSpec::Beryl);
        let admin = Address::with_last_byte(1);
        StorageCtx::enter(&mut storage, || {
            let mut busd = setup_token(BASE_USD_ADDRESS, "Base USD", "BUSD", admin);
            let mut token = setup_token(TOKEN_A, "Token A", "TOKA", admin);
            let amount = uint!(100000_U256);
            mint(&mut busd, admin, admin, amount);
            mint(&mut token, admin, admin, amount);

            let mut dex = BaseDex::new();
            let liquidity = dex.add_liquidity(admin, TOKEN_A, amount, amount, admin).unwrap();

            assert_eq!(liquidity, amount - MINIMUM_LIQUIDITY);
            assert_eq!(dex.get_total_supply(TOKEN_A).unwrap(), amount);
            assert_eq!(dex.get_liquidity_balance(TOKEN_A, admin).unwrap(), liquidity);
            assert_eq!(balance(TOKEN_A, dex.address()), amount);
            assert_eq!(balance(BASE_USD_ADDRESS, dex.address()), amount);
        });
    }

    #[test]
    fn swaps_token_for_base_with_constant_product_fee() {
        let mut storage = HashMapStorageProvider::new_with_spec(1, BaseBSpec::Beryl);
        let admin = Address::with_last_byte(1);
        let trader = Address::with_last_byte(2);
        StorageCtx::enter(&mut storage, || {
            let mut busd = setup_token(BASE_USD_ADDRESS, "Base USD", "BUSD", admin);
            let mut token = setup_token(TOKEN_A, "Token A", "TOKA", admin);
            let liquidity_amount = uint!(100000_U256);
            let swap_amount = uint!(1000_U256);
            mint(&mut busd, admin, admin, liquidity_amount);
            mint(&mut token, admin, admin, liquidity_amount);
            mint(&mut token, admin, trader, swap_amount);

            let mut dex = BaseDex::new();
            dex.add_liquidity(admin, TOKEN_A, liquidity_amount, liquidity_amount, admin).unwrap();

            let expected = swap_amount
                .checked_mul(FEE_NUMERATOR)
                .and_then(|with_fee| {
                    with_fee.checked_mul(liquidity_amount).and_then(|num| {
                        liquidity_amount
                            .checked_mul(FEE_DENOMINATOR)
                            .and_then(|den| den.checked_add(with_fee))
                            .and_then(|den| num.checked_div(den))
                    })
                })
                .unwrap();

            let actual = dex
                .swap_exact_tokens_for_tokens(
                    trader,
                    TOKEN_A,
                    BASE_USD_ADDRESS,
                    swap_amount,
                    U256::ZERO,
                    trader,
                )
                .unwrap();

            assert_eq!(actual, expected);
            assert_eq!(balance(TOKEN_A, trader), U256::ZERO);
            assert_eq!(balance(BASE_USD_ADDRESS, trader), expected);
            let pool = dex.get_pool(TOKEN_A).unwrap();
            assert_eq!(pool.reserve_token, 101000);
            assert_eq!(U256::from(pool.reserve_base), liquidity_amount - expected);
        });
    }

    #[test]
    fn swaps_between_non_base_tokens_via_base_usd() {
        let mut storage = HashMapStorageProvider::new_with_spec(1, BaseBSpec::Beryl);
        let admin = Address::with_last_byte(1);
        let trader = Address::with_last_byte(2);
        StorageCtx::enter(&mut storage, || {
            let mut busd = setup_token(BASE_USD_ADDRESS, "Base USD", "BUSD", admin);
            let mut token_a = setup_token(TOKEN_A, "Token A", "TOKA", admin);
            let mut token_b = setup_token(TOKEN_B, "Token B", "TOKB", admin);
            let liquidity_amount = uint!(100000_U256);
            let swap_amount = uint!(1000_U256);
            mint(&mut busd, admin, admin, liquidity_amount * uint!(2_U256));
            mint(&mut token_a, admin, admin, liquidity_amount);
            mint(&mut token_b, admin, admin, liquidity_amount);
            mint(&mut token_a, admin, trader, swap_amount);

            let mut dex = BaseDex::new();
            dex.add_liquidity(admin, TOKEN_A, liquidity_amount, liquidity_amount, admin).unwrap();
            dex.add_liquidity(admin, TOKEN_B, liquidity_amount, liquidity_amount, admin).unwrap();

            let expected_base =
                BaseDex::amount_out(swap_amount, liquidity_amount, liquidity_amount).unwrap();
            let expected_token_b =
                BaseDex::amount_out(expected_base, liquidity_amount, liquidity_amount).unwrap();
            let quoted = dex.quote_exact_input(TOKEN_A, TOKEN_B, swap_amount).unwrap();
            assert_eq!(quoted, expected_token_b);

            let actual = dex
                .swap_exact_tokens_for_tokens(
                    trader,
                    TOKEN_A,
                    TOKEN_B,
                    swap_amount,
                    U256::ZERO,
                    trader,
                )
                .unwrap();

            assert_eq!(actual, expected_token_b);
            assert_eq!(balance(TOKEN_A, trader), U256::ZERO);
            assert_eq!(balance(TOKEN_B, trader), expected_token_b);
            assert_eq!(balance(BASE_USD_ADDRESS, trader), U256::ZERO);

            let pool_a = dex.get_pool(TOKEN_A).unwrap();
            let pool_b = dex.get_pool(TOKEN_B).unwrap();
            assert_eq!(U256::from(pool_a.reserve_base), liquidity_amount - expected_base);
            assert_eq!(U256::from(pool_b.reserve_base), liquidity_amount + expected_base);
            assert_eq!(U256::from(pool_b.reserve_token), liquidity_amount - expected_token_b);
        });
    }

    #[test]
    fn remove_liquidity_returns_pro_rata_reserves() {
        let mut storage = HashMapStorageProvider::new_with_spec(1, BaseBSpec::Beryl);
        let admin = Address::with_last_byte(1);
        StorageCtx::enter(&mut storage, || {
            let mut busd = setup_token(BASE_USD_ADDRESS, "Base USD", "BUSD", admin);
            let mut token = setup_token(TOKEN_A, "Token A", "TOKA", admin);
            let amount = uint!(100000_U256);
            mint(&mut busd, admin, admin, amount);
            mint(&mut token, admin, admin, amount);

            let mut dex = BaseDex::new();
            let liquidity = dex.add_liquidity(admin, TOKEN_A, amount, amount, admin).unwrap();
            let (amount_token, amount_base) =
                dex.remove_liquidity(admin, TOKEN_A, liquidity / uint!(2_U256), admin).unwrap();

            assert_eq!(amount_token, uint!(49500_U256));
            assert_eq!(amount_base, uint!(49500_U256));
            assert_eq!(balance(TOKEN_A, admin), amount_token);
            assert_eq!(balance(BASE_USD_ADDRESS, admin), amount_base);
        });
    }
}
