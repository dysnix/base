use std::fmt::Display;

use alloy::primitives::{Address, LogData, U256, address};
use revm::{
    Database, DatabaseCommit,
    context::journaled_state::JournalCheckpoint,
    primitives::HashMap,
    state::{Account, AccountInfo, Bytecode, EvmStorageSlot},
};

use crate::{
    BaseBSpec,
    b20_factory::B20Factory,
    error::{BasePrecompileError, Result},
    storage::{PrecompileStorageProvider, StorageCtx},
};

/// Base USD B20 token address under the reserved `0x8453` B20 prefix range.
pub const BUSD_ADDRESS: Address = address!("0x8453000000000000000000000000000000000000");

/// Bootstrap operations for protocol-owned B20 tokens.
#[derive(Debug, Default, Clone, Copy)]
pub struct B20Bootstrap;

impl B20Bootstrap {
    /// Ensures the canonical BUSD token exists at [`BUSD_ADDRESS`].
    ///
    /// The operation is idempotent so it can be safely called by genesis or hardfork bootstrap
    /// code.
    pub fn ensure_busd(admin: Address) -> Result<Address> {
        let mut factory = B20Factory::new();
        if factory.is_b20(BUSD_ADDRESS)? {
            return Ok(BUSD_ADDRESS);
        }

        factory.create_token_reserved_address(BUSD_ADDRESS, "Base USD", "BUSD", "USD", admin)
    }

    /// Ensures the canonical BUSD token exists in an EVM database.
    pub fn ensure_busd_in_database<DB>(
        db: &mut DB,
        chain_id: u64,
        timestamp: U256,
        beneficiary: Address,
        block_number: u64,
        admin: Address,
    ) -> Result<Address>
    where
        DB: Database + DatabaseCommit,
        DB::Error: Display,
    {
        let mut storage =
            DbBootstrapStorageProvider::new(db, chain_id, timestamp, beneficiary, block_number);
        let address = StorageCtx::enter(&mut storage, || Self::ensure_busd(admin))?;
        storage.commit();
        Ok(address)
    }
}

struct DbBootstrapStorageProvider<'a, DB> {
    db: &'a mut DB,
    accounts: HashMap<Address, Account>,
    transient: HashMap<(Address, U256), U256>,
    snapshots: Vec<Snapshot>,
    chain_id: u64,
    timestamp: U256,
    beneficiary: Address,
    block_number: u64,
}

struct Snapshot {
    accounts: HashMap<Address, Account>,
    transient: HashMap<(Address, U256), U256>,
}

impl<'a, DB> DbBootstrapStorageProvider<'a, DB>
where
    DB: Database,
    DB::Error: Display,
{
    fn new(
        db: &'a mut DB,
        chain_id: u64,
        timestamp: U256,
        beneficiary: Address,
        block_number: u64,
    ) -> Self {
        Self {
            db,
            accounts: HashMap::default(),
            transient: HashMap::default(),
            snapshots: Vec::new(),
            chain_id,
            timestamp,
            beneficiary,
            block_number,
        }
    }

    fn load_account(&mut self, address: Address) -> Result<&mut Account> {
        if !self.accounts.contains_key(&address) {
            let info = self
                .db
                .basic(address)
                .map_err(|error| BasePrecompileError::Fatal(error.to_string()))?
                .unwrap_or_default();
            self.accounts.insert(address, Account::from(info));
        }

        Ok(self.accounts.get_mut(&address).expect("account inserted above"))
    }

    fn load_storage(&mut self, address: Address, key: U256) -> Result<U256> {
        if let Some(value) = self
            .accounts
            .get(&address)
            .and_then(|account| account.storage.get(&key).map(|slot| slot.present_value))
        {
            return Ok(value);
        }

        let value = self
            .db
            .storage(address, key)
            .map_err(|error| BasePrecompileError::Fatal(error.to_string()))?;
        let account = self.load_account(address)?;
        account.storage.insert(key, EvmStorageSlot::new(value, 0));
        Ok(value)
    }
}

impl<DB> DbBootstrapStorageProvider<'_, DB>
where
    DB: Database + DatabaseCommit,
{
    fn commit(self) {
        self.db.commit(self.accounts);
    }
}

impl<DB> PrecompileStorageProvider for DbBootstrapStorageProvider<'_, DB>
where
    DB: Database,
    DB::Error: Display,
{
    fn chain_id(&self) -> u64 {
        self.chain_id
    }

    fn timestamp(&self) -> U256 {
        self.timestamp
    }

    fn beneficiary(&self) -> Address {
        self.beneficiary
    }

    fn block_number(&self) -> u64 {
        self.block_number
    }

    fn set_code(&mut self, address: Address, code: Bytecode) -> Result<()> {
        let account = self.load_account(address)?;
        account.info.code_hash = code.hash_slow();
        account.info.code = Some(code);
        account.mark_touch();
        Ok(())
    }

    fn with_account_info(
        &mut self,
        address: Address,
        f: &mut dyn FnMut(&AccountInfo),
    ) -> Result<()> {
        let account = self.load_account(address)?;
        f(&account.info);
        Ok(())
    }

    fn sload(&mut self, address: Address, key: U256) -> Result<U256> {
        self.load_storage(address, key)
    }

    fn tload(&mut self, address: Address, key: U256) -> Result<U256> {
        Ok(self.transient.get(&(address, key)).copied().unwrap_or_default())
    }

    fn sstore(&mut self, address: Address, key: U256, value: U256) -> Result<()> {
        let original = self.load_storage(address, key)?;
        let account = self.load_account(address)?;
        account.storage.insert(key, EvmStorageSlot::new_changed(original, value, 0));
        account.mark_touch();
        Ok(())
    }

    fn tstore(&mut self, address: Address, key: U256, value: U256) -> Result<()> {
        self.transient.insert((address, key), value);
        Ok(())
    }

    fn emit_event(&mut self, _address: Address, _event: LogData) -> Result<()> {
        Ok(())
    }

    fn deduct_gas(&mut self, _gas: u64) -> Result<()> {
        Ok(())
    }

    fn refund_gas(&mut self, _gas: i64) {}

    fn gas_limit(&self) -> u64 {
        0
    }

    fn gas_used(&self) -> u64 {
        0
    }

    fn state_gas_used(&self) -> u64 {
        0
    }

    fn gas_refunded(&self) -> i64 {
        0
    }

    fn reservoir(&self) -> u64 {
        0
    }

    fn spec(&self) -> BaseBSpec {
        BaseBSpec::Beryl
    }

    fn amsterdam_eip8037_enabled(&self) -> bool {
        false
    }

    fn is_static(&self) -> bool {
        false
    }

    fn checkpoint(&mut self) -> JournalCheckpoint {
        let idx = self.snapshots.len();
        self.snapshots
            .push(Snapshot { accounts: self.accounts.clone(), transient: self.transient.clone() });
        JournalCheckpoint { log_i: 0, journal_i: idx }
    }

    fn checkpoint_commit(&mut self, checkpoint: JournalCheckpoint) {
        assert_eq!(checkpoint.journal_i, self.snapshots.len() - 1);
        self.snapshots.pop();
    }

    fn checkpoint_revert(&mut self, checkpoint: JournalCheckpoint) {
        assert_eq!(checkpoint.journal_i, self.snapshots.len() - 1);
        let snapshot = self.snapshots.pop().expect("snapshot exists");
        self.accounts = snapshot.accounts;
        self.transient = snapshot.transient;
    }
}

#[cfg(test)]
mod tests {
    use alloy::primitives::{Address, U256};
    use base_precompiles_contracts::IRolesAuth;
    use revm::{
        Database as _,
        database::{InMemoryDB, State},
        state::AccountInfo,
    };

    use crate::{
        BaseBSpec,
        b20::{B20Token, roles::DEFAULT_ADMIN_ROLE},
        bootstrap::{B20Bootstrap, BUSD_ADDRESS},
        storage::{ContractStorage, StorageCtx, hashmap::HashMapStorageProvider},
    };

    #[test]
    fn ensure_busd_deploys_canonical_token_once() {
        let admin = Address::with_last_byte(1);
        let mut storage = HashMapStorageProvider::new_with_spec(1, BaseBSpec::Beryl);

        StorageCtx::enter(&mut storage, || {
            let address = B20Bootstrap::ensure_busd(admin).unwrap();
            assert_eq!(address, BUSD_ADDRESS);

            let token = B20Token::from_address(BUSD_ADDRESS).unwrap();
            assert!(token.is_initialized().unwrap());
            assert_eq!(token.name().unwrap(), "Base USD");
            assert_eq!(token.symbol().unwrap(), "BUSD");
            assert_eq!(token.currency().unwrap(), "USD");
            assert_eq!(token.decimals().unwrap(), 6);
            assert!(
                token
                    .has_role(IRolesAuth::hasRoleCall { account: admin, role: DEFAULT_ADMIN_ROLE })
                    .unwrap()
            );

            let address = B20Bootstrap::ensure_busd(admin).unwrap();
            assert_eq!(address, BUSD_ADDRESS);
        });
    }

    #[test]
    fn ensure_busd_in_database_commits_canonical_token() {
        let admin = Address::with_last_byte(1);
        let mut db = InMemoryDB::default();
        db.insert_account_info(Address::ZERO, AccountInfo::default());

        let address =
            B20Bootstrap::ensure_busd_in_database(&mut db, 1, U256::ZERO, Address::ZERO, 0, admin)
                .unwrap();

        assert_eq!(address, BUSD_ADDRESS);
        assert!(!db.basic(BUSD_ADDRESS).unwrap().unwrap().is_empty_code_hash());

        let address =
            B20Bootstrap::ensure_busd_in_database(&mut db, 1, U256::ZERO, Address::ZERO, 0, admin)
                .unwrap();
        assert_eq!(address, BUSD_ADDRESS);
    }

    #[test]
    fn ensure_busd_in_state_database_commits_canonical_token() {
        let admin = Address::with_last_byte(1);
        let mut db = State::builder().with_database(InMemoryDB::default()).build();

        let address =
            B20Bootstrap::ensure_busd_in_database(&mut db, 1, U256::ZERO, Address::ZERO, 0, admin)
                .unwrap();

        assert_eq!(address, BUSD_ADDRESS);
        assert!(!db.basic(BUSD_ADDRESS).unwrap().unwrap().is_empty_code_hash());
    }

    #[test]
    fn busd_address_uses_reserved_protocol_range() {
        let mut lower_bytes = [0u8; 8];
        lower_bytes.copy_from_slice(&BUSD_ADDRESS.as_slice()[12..]);

        assert_eq!(u64::from_be_bytes(lower_bytes), 0);
    }
}
