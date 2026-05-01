//! `SQLite` storage layer. All SQL lives here so the rest of the crate can
//! pretend the DB is just a typed API.

use std::{path::Path, str::FromStr};

use alloy_primitives::{Address, B256, U256};
use eyre::{Result, WrapErr, eyre};
use sqlx::{
    ConnectOptions, Pool, Sqlite,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};

type BlockDbRow = (i64, Vec<u8>, i64, Vec<u8>, i64, i64, i64, Option<Vec<u8>>);
type TxDbRow = (Vec<u8>, i64, i64, Vec<u8>, Option<Vec<u8>>, Vec<u8>, i64, Option<Vec<u8>>);
type ActivityDbRow = (i64, i64, i64, Vec<u8>, i64, Option<Vec<u8>>);

/// Role column values. Kept in sync with `migrations/0001_init.sql`.
#[repr(i64)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ActivityRole {
    /// Transaction sender.
    Sender = 0,
    /// Transaction recipient / called address.
    Recipient = 1,
    /// Contract created by a transaction.
    Creator = 2,
    /// Token transfer sender from a log.
    LogFrom = 3,
    /// Token transfer recipient from a log.
    LogTo = 4,
}

/// Compact representation of a block row for listings.
#[derive(Debug, Clone)]
pub struct BlockRow {
    pub(crate) number: u64,
    pub(crate) hash: B256,
    pub(crate) timestamp: u64,
    pub(crate) miner: Address,
    pub(crate) tx_count: u64,
    pub(crate) gas_used: u64,
    pub(crate) gas_limit: u64,
    pub(crate) base_fee: Option<U256>,
}

/// Compact representation of a tx row for listings.
#[derive(Debug, Clone)]
pub struct TxRow {
    pub(crate) hash: B256,
    pub(crate) block_num: u64,
    pub(crate) tx_index: u64,
    pub(crate) from_addr: Address,
    pub(crate) to_addr: Option<Address>,
    pub(crate) value: U256,
    pub(crate) status: u8,
    pub(crate) created: Option<Address>,
}

/// One row in the activity feed for an address.
#[derive(Debug, Clone)]
pub struct ActivityRow {
    pub(crate) block_num: u64,
    pub(crate) tx_index: u64,
    pub(crate) log_index: i64,
    pub(crate) tx_hash: B256,
    pub(crate) role: ActivityRole,
    pub(crate) token: Option<Address>,
}

/// Buffered writes for a single block. The indexer assembles one of these
/// per block and hands it to [`Storage::insert_block`] in a single
/// transaction, so a block either lands atomically or not at all.
#[derive(Debug, Default, Clone)]
pub struct BlockWrite {
    pub(crate) block: Option<BlockRow>,
    pub(crate) txs: Vec<TxRow>,
    pub(crate) activity: Vec<ActivityWrite>,
}

/// One derived address activity row to insert for a block.
#[derive(Debug, Clone)]
pub struct ActivityWrite {
    pub(crate) address: Address,
    pub(crate) block_num: u64,
    pub(crate) tx_index: u64,
    pub(crate) log_index: i64,
    pub(crate) tx_hash: B256,
    pub(crate) role: ActivityRole,
    pub(crate) token: Option<Address>,
}

/// SQLite-backed address activity index used by the explorer.
#[derive(Clone, Debug)]
pub struct Storage {
    pool: Pool<Sqlite>,
}

impl Storage {
    /// Open / create the database file and run migrations.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(dir) = path.parent()
            && !dir.as_os_str().is_empty()
        {
            tokio::fs::create_dir_all(dir)
                .await
                .wrap_err_with(|| format!("creating db parent dir {}", dir.display()))?;
        }

        let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
            .wrap_err_with(|| format!("parsing sqlite url for {}", path.display()))?
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .disable_statement_logging();

        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(opts)
            .await
            .wrap_err("opening sqlite pool")?;

        sqlx::migrate!("./migrations").run(&pool).await.wrap_err("running migrations")?;
        Ok(Self { pool })
    }

    /// Read the cursor row. Returns `None` if the DB is empty.
    pub(crate) async fn cursor(&self) -> Result<Option<(u64, B256)>> {
        let row: Option<(i64, Vec<u8>)> =
            sqlx::query_as("SELECT last_indexed_block, last_indexed_hash FROM cursor WHERE id = 0")
                .fetch_optional(&self.pool)
                .await?;
        row.map(|(n, h)| Ok((n as u64, b256_from_db(&h, "cursor.last_indexed_hash")?))).transpose()
    }

    /// Fetch the stored hash for a specific block number, if we have it.
    /// Used by the indexer at startup to detect a chain-reset / volume-
    /// resurrected situation where our cursor points at blocks that no
    /// longer exist upstream.
    pub(crate) async fn block_hash(&self, number: u64) -> Result<Option<B256>> {
        let row: Option<(Vec<u8>,)> = sqlx::query_as("SELECT hash FROM blocks WHERE number = ?")
            .bind(number as i64)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|(h,)| b256_from_db(&h, "blocks.hash")).transpose()
    }

    /// Drop every indexed row and reset the cursor. Called when the DB
    /// disagrees with the upstream chain on the genesis block hash.
    pub(crate) async fn wipe(&self) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM address_activity").execute(&mut *tx).await?;
        sqlx::query("DELETE FROM txs").execute(&mut *tx).await?;
        sqlx::query("DELETE FROM blocks").execute(&mut *tx).await?;
        sqlx::query("DELETE FROM cursor").execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Persist a single indexed block + all derived rows atomically.
    pub(crate) async fn insert_block(&self, write: BlockWrite) -> Result<()> {
        let Some(block) = write.block else {
            return Ok(());
        };
        let mut tx = self.pool.begin().await?;

        let base_fee = block.base_fee.map(|v| v.to_be_bytes::<32>().to_vec());
        sqlx::query(
            "INSERT OR REPLACE INTO blocks \
             (number, hash, timestamp, miner, tx_count, gas_used, gas_limit, base_fee) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(block.number as i64)
        .bind(block.hash.as_slice())
        .bind(block.timestamp as i64)
        .bind(block.miner.as_slice())
        .bind(block.tx_count as i64)
        .bind(block.gas_used as i64)
        .bind(block.gas_limit as i64)
        .bind(base_fee)
        .execute(&mut *tx)
        .await?;

        for t in write.txs {
            sqlx::query(
                "INSERT OR REPLACE INTO txs \
                 (hash, block_num, tx_index, from_addr, to_addr, value, status, created) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(t.hash.as_slice())
            .bind(t.block_num as i64)
            .bind(t.tx_index as i64)
            .bind(t.from_addr.as_slice())
            .bind(t.to_addr.as_ref().map(|a| a.as_slice().to_vec()))
            .bind(t.value.to_be_bytes::<32>().to_vec())
            .bind(t.status as i64)
            .bind(t.created.as_ref().map(|a| a.as_slice().to_vec()))
            .execute(&mut *tx)
            .await?;
        }

        for a in write.activity {
            sqlx::query(
                "INSERT OR IGNORE INTO address_activity \
                 (address, block_num, tx_index, log_index, tx_hash, role, token) \
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(a.address.as_slice())
            .bind(a.block_num as i64)
            .bind(a.tx_index as i64)
            .bind(a.log_index)
            .bind(a.tx_hash.as_slice())
            .bind(a.role as i64)
            .bind(a.token.as_ref().map(|t| t.as_slice().to_vec()))
            .execute(&mut *tx)
            .await?;
        }

        sqlx::query(
            "INSERT INTO cursor (id, last_indexed_block, last_indexed_hash, updated_at) \
             VALUES (0, ?, ?, strftime('%s', 'now')) \
             ON CONFLICT(id) DO UPDATE SET \
               last_indexed_block = excluded.last_indexed_block, \
               last_indexed_hash  = excluded.last_indexed_hash, \
               updated_at         = excluded.updated_at",
        )
        .bind(block.number as i64)
        .bind(block.hash.as_slice())
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    /// Most recent `limit` blocks, ordered newest first.
    pub(crate) async fn latest_blocks(&self, limit: i64) -> Result<Vec<BlockRow>> {
        let rows: Vec<BlockDbRow> = sqlx::query_as(
            "SELECT number, hash, timestamp, miner, tx_count, gas_used, gas_limit, base_fee \
             FROM blocks ORDER BY number DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_block).collect()
    }

    /// Most recent `limit` txs across all blocks.
    pub(crate) async fn latest_txs(&self, limit: i64) -> Result<Vec<TxRow>> {
        let rows: Vec<TxDbRow> = sqlx::query_as(
            "SELECT hash, block_num, tx_index, from_addr, to_addr, value, status, created \
             FROM txs ORDER BY block_num DESC, tx_index DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_tx).collect()
    }

    /// Paginated activity feed for an address. Rows are ordered newest
    /// first; pass the `(block_num, tx_index, log_index)` of the last row
    /// in the previous page to continue.
    pub(crate) async fn activity_for(
        &self,
        address: Address,
        before: Option<(u64, u64, i64)>,
        limit: i64,
    ) -> Result<Vec<ActivityRow>> {
        let (bn, ti, li) = before.unwrap_or((i64::MAX as u64, i64::MAX as u64, i64::MAX));
        let rows: Vec<ActivityDbRow> = sqlx::query_as(
            "SELECT block_num, tx_index, log_index, tx_hash, role, token \
             FROM address_activity \
             WHERE address = ? \
               AND (block_num, tx_index, log_index) < (?, ?, ?) \
             ORDER BY block_num DESC, tx_index DESC, log_index DESC \
             LIMIT ?",
        )
        .bind(address.as_slice())
        .bind(bn as i64)
        .bind(ti as i64)
        .bind(li)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|(bn, ti, li, h, r, tok)| {
                Ok(ActivityRow {
                    block_num: bn as u64,
                    tx_index: ti as u64,
                    log_index: li,
                    tx_hash: b256_from_db(&h, "address_activity.tx_hash")?,
                    role: role_from_i64(r),
                    token: tok
                        .as_deref()
                        .map(|bytes| address_from_db(bytes, "address_activity.token"))
                        .transpose()?,
                })
            })
            .collect()
    }

    /// Simple health/stats for the home page.
    pub(crate) async fn stats(&self) -> Result<Stats> {
        let blocks: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM blocks").fetch_one(&self.pool).await?;
        let txs: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM txs").fetch_one(&self.pool).await?;
        let addresses: (i64,) =
            sqlx::query_as("SELECT COUNT(DISTINCT address) FROM address_activity")
                .fetch_one(&self.pool)
                .await?;
        Ok(Stats { blocks: blocks.0 as u64, txs: txs.0 as u64, addresses: addresses.0 as u64 })
    }
}

#[derive(Debug, Clone, Copy)]
/// Aggregate counts for the explorer home page.
pub struct Stats {
    pub(crate) blocks: u64,
    pub(crate) txs: u64,
    pub(crate) addresses: u64,
}

fn row_to_block(
    (number, hash, timestamp, miner, tx_count, gas_used, gas_limit, base_fee): BlockDbRow,
) -> Result<BlockRow> {
    Ok(BlockRow {
        number: number as u64,
        hash: b256_from_db(&hash, "blocks.hash")?,
        timestamp: timestamp as u64,
        miner: address_from_db(&miner, "blocks.miner")?,
        tx_count: tx_count as u64,
        gas_used: gas_used as u64,
        gas_limit: gas_limit as u64,
        base_fee: base_fee.map(|b| U256::from_be_slice(&b)),
    })
}

fn row_to_tx(
    (hash, block_num, tx_index, from_addr, to_addr, value, status, created): TxDbRow,
) -> Result<TxRow> {
    Ok(TxRow {
        hash: b256_from_db(&hash, "txs.hash")?,
        block_num: block_num as u64,
        tx_index: tx_index as u64,
        from_addr: address_from_db(&from_addr, "txs.from_addr")?,
        to_addr: to_addr
            .as_deref()
            .map(|bytes| address_from_db(bytes, "txs.to_addr"))
            .transpose()?,
        value: U256::from_be_slice(&value),
        status: status as u8,
        created: created
            .as_deref()
            .map(|bytes| address_from_db(bytes, "txs.created"))
            .transpose()?,
    })
}

const fn role_from_i64(v: i64) -> ActivityRole {
    match v {
        1 => ActivityRole::Recipient,
        2 => ActivityRole::Creator,
        3 => ActivityRole::LogFrom,
        4 => ActivityRole::LogTo,
        // Corrupt row: default to sender so we never panic serving a page.
        _ => ActivityRole::Sender,
    }
}

fn b256_from_db(bytes: &[u8], column: &str) -> Result<B256> {
    if bytes.len() != 32 {
        return Err(eyre!("{column} has {} bytes, expected 32", bytes.len()));
    }
    Ok(B256::from_slice(bytes))
}

fn address_from_db(bytes: &[u8], column: &str) -> Result<Address> {
    if bytes.len() != 20 {
        return Err(eyre!("{column} has {} bytes, expected 20", bytes.len()));
    }
    Ok(Address::from_slice(bytes))
}
