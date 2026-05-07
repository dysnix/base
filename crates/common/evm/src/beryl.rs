use alloy_evm::{Database, Evm, block::BlockExecutionError};
use alloy_primitives::{Address, address};
use base_common_chains::Upgrades;
use base_common_consensus::Predeploys;
use base_precompiles::B20Bootstrap;
use revm::{DatabaseCommit, context::Block};

const DEVNET_CHAIN_ID: u64 = 84538453;
const DEVNET_BUSD_ADMIN: Address = address!("0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc");

fn busd_admin_for_chain(chain_id: u64) -> Address {
    if chain_id == DEVNET_CHAIN_ID { DEVNET_BUSD_ADMIN } else { Predeploys::PROXY_ADMIN }
}

/// Ensures native ERC-20 protocol state exists during the Beryl transition.
pub fn ensure_native_erc20s<E>(
    chain_spec: impl Upgrades,
    timestamp: u64,
    evm: &mut E,
) -> Result<(), BlockExecutionError>
where
    E: Evm<DB: Database + DatabaseCommit>,
{
    if !chain_spec.is_beryl_active_at_timestamp(timestamp)
        || (timestamp != 0 && chain_spec.is_beryl_active_at_timestamp(timestamp.saturating_sub(2)))
    {
        return Ok(());
    }

    let chain_id = evm.chain_id();
    let timestamp = evm.block().timestamp();
    let beneficiary = evm.block().beneficiary();
    let block_number = evm.block().number().saturating_to();
    let admin = busd_admin_for_chain(chain_id);

    B20Bootstrap::ensure_busd_in_database(
        evm.db_mut(),
        chain_id,
        timestamp,
        beneficiary,
        block_number,
        admin,
    )
    .map_err(|error| BlockExecutionError::msg(format!("BUSD deployment failed: {error}")))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use alloy_evm::{Evm, EvmEnv, EvmFactory, precompiles::PrecompilesMap};
    use alloy_hardforks::ForkCondition;
    use alloy_primitives::{Address, U256};
    use alloy_sol_types::SolCall;
    use base_common_chains::{BaseUpgrade, ChainUpgrades};
    use base_precompiles::{BUSD_ADDRESS, b20::IB20};
    use revm::{
        Database as _,
        context::{BlockEnv, CfgEnv},
        database::{InMemoryDB, State},
        inspector::NoOpInspector,
    };

    use super::*;
    use crate::{BaseEvm, BaseEvmFactory, BaseSpecId};

    const AZUL_TIMESTAMP: u64 = 0;
    const BERYL_TIMESTAMP: u64 = 10;

    fn beryl_upgrades() -> ChainUpgrades {
        let mut forks = BaseUpgrade::mainnet();
        forks[BaseUpgrade::Azul.idx()].1 = ForkCondition::Timestamp(AZUL_TIMESTAMP);
        forks[BaseUpgrade::Beryl.idx()].1 = ForkCondition::Timestamp(BERYL_TIMESTAMP);
        ChainUpgrades::new(forks)
    }

    fn beryl_evm(timestamp: u64) -> BaseEvm<State<InMemoryDB>, NoOpInspector, PrecompilesMap> {
        BaseEvmFactory::default().create_evm(
            State::builder().with_database(InMemoryDB::default()).build(),
            EvmEnv::new(
                CfgEnv::new_with_spec(BaseSpecId::BERYL),
                BlockEnv { timestamp: U256::from(timestamp), ..Default::default() },
            ),
        )
    }

    fn read_busd_symbol<E>(evm: &mut E) -> String
    where
        E: Evm,
    {
        let result_and_state = evm
            .transact_system_call(
                Address::ZERO,
                BUSD_ADDRESS,
                IB20::symbolCall {}.abi_encode().into(),
            )
            .unwrap();
        assert!(result_and_state.result.is_success(), "{:?}", result_and_state.result);

        let output = result_and_state.result.output().unwrap();
        IB20::symbolCall::abi_decode_returns(output).unwrap()
    }

    #[test]
    fn ensure_native_erc20s_deploys_busd_on_beryl_transition() {
        let mut evm = beryl_evm(BERYL_TIMESTAMP);

        ensure_native_erc20s(beryl_upgrades(), BERYL_TIMESTAMP, &mut evm).unwrap();

        let info = evm.db_mut().basic(BUSD_ADDRESS).unwrap().unwrap_or_default();
        assert!(!info.is_empty_code_hash(), "{info:?}");
        assert_eq!(read_busd_symbol(&mut evm), "BUSD");
    }

    #[test]
    fn ensure_native_erc20s_skips_after_beryl_transition() {
        let mut evm = beryl_evm(BERYL_TIMESTAMP + 2);

        ensure_native_erc20s(beryl_upgrades(), BERYL_TIMESTAMP + 2, &mut evm).unwrap();

        let result_and_state = evm
            .transact_system_call(
                Address::ZERO,
                BUSD_ADDRESS,
                IB20::symbolCall {}.abi_encode().into(),
            )
            .unwrap();
        assert!(!result_and_state.result.is_success());
    }
}
