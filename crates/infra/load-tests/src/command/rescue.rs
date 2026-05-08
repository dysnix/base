//! Rescue command for recovering funds from load-test accounts.

use std::time::Duration;

use alloy_network::{EthereumWallet, ReceiptResponse, TransactionBuilder};
use alloy_primitives::{Address, TxHash, U256, utils::format_ether};
use alloy_provider::Provider;
use alloy_rpc_types::TransactionRequest;
use alloy_signer_local::PrivateKeySigner;
use eyre::{Result, bail};
use futures::stream::{self, StreamExt};
use indicatif::{ProgressBar, ProgressStyle};
use tracing::{debug, info, warn};
use url::Url;

use crate::{
    AccountPool, BaselineError, BatchRpcClient, FundedAccount, Result as LoadResult, RpcClient,
    create_wallet_provider,
};

/// Options for the rescue command.
#[derive(Debug)]
pub struct RescueOptions {
    /// RPC endpoint used to scan and drain accounts.
    pub rpc_url: Url,
    /// Seed used for account generation.
    pub seed: Option<u64>,
    /// Number of accounts to scan.
    pub scan_count: usize,
    /// Starting account offset.
    pub offset: usize,
    /// Private key of the funder account.
    pub funder_key: PrivateKeySigner,
    /// Mnemonic used for account generation.
    pub mnemonic: Option<String>,
}

/// Accounts to derive and check per batch during rescue.
const RESCUE_BATCH_SIZE: usize = 100;

/// Maximum concurrent RPC requests during rescue.
const RESCUE_CONCURRENCY: usize = 32;

/// Default number of accounts to scan during rescue.
const DEFAULT_RESCUE_SCAN_COUNT: usize = 1000;

/// Default maximum gas price (1000 gwei).
const DEFAULT_MAX_GAS_PRICE: u128 = 1_000_000_000_000;

/// Runs the rescue command.
#[derive(Debug)]
pub struct Rescue;

impl Rescue {
    /// Runs the rescue command.
    pub async fn run(options: RescueOptions) -> Result<()> {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "info".into()),
            )
            .init();

        options.validate()?;

        let client = RpcClient::new(options.rpc_url.clone());
        let chain_id = client.chain_id().await?;
        let funder_address = options.funder_key.address();

        println!("=== Load Test Rescue ===");
        println!("RPC: {} | Chain: {} | Funder: {}", options.rpc_url, chain_id, funder_address);
        println!(
            "Scanning {} accounts (seed={}, offset={})\n",
            options.scan_count,
            options.seed.unwrap_or(0),
            options.offset
        );

        let gas_price = client.get_gas_price().await?;
        let max_priority_fee = (gas_price / 10).max(1);
        let max_fee = gas_price.saturating_mul(2).max(max_priority_fee).min(DEFAULT_MAX_GAS_PRICE);
        let drain_gas_limit = 21_000u128;
        let l1_fee_buffer = 1_000_000_000_000_000u128;
        let drain_gas_cost =
            U256::from(drain_gas_limit.saturating_mul(max_fee).saturating_add(l1_fee_buffer));

        let params = DrainParams {
            funder_address,
            chain_id,
            max_fee,
            max_priority_fee,
            drain_gas_cost,
            drain_gas_limit,
            rpc_url: options.rpc_url.clone(),
        };

        let mut total_rescued = U256::ZERO;
        let mut total_accounts_drained = 0usize;
        let mut batch_offset = options.offset;
        let mut remaining = options.scan_count;

        let pb = Self::rescue_progress_bar(options.scan_count as u64, "Scanning accounts");

        while remaining > 0 {
            let batch_count = remaining.min(RESCUE_BATCH_SIZE);

            let accounts = if let Some(ref mnemonic) = options.mnemonic {
                AccountPool::from_mnemonic(mnemonic, batch_count, batch_offset)?
            } else {
                AccountPool::with_offset(options.seed.unwrap_or(0), batch_count, batch_offset)?
            };

            let batch_rpc = BatchRpcClient::new(options.rpc_url.clone());
            let (rescued, drained) =
                Self::rescue_batch(&client, &batch_rpc, &accounts, &params, &pb).await?;

            total_rescued = total_rescued.saturating_add(rescued);
            total_accounts_drained += drained;

            batch_offset += batch_count;
            remaining -= batch_count;
        }

        pb.finish_and_clear();

        println!("\n=== Rescue Complete ===");
        println!(
            "Drained {} accounts | Total rescued: {} ETH",
            total_accounts_drained,
            format_ether(total_rescued)
        );

        Ok(())
    }

    async fn rescue_batch(
        client: &RpcClient,
        batch_rpc: &BatchRpcClient,
        accounts: &AccountPool,
        params: &DrainParams,
        pb: &ProgressBar,
    ) -> LoadResult<(U256, usize)> {
        let balance_futs: Vec<_> = accounts
            .accounts()
            .iter()
            .map(|a| {
                let client = client.clone();
                let address = a.address;
                async move {
                    let balance = client.get_pending_balance(address).await?;
                    Ok::<_, BaselineError>((address, balance))
                }
            })
            .collect();

        let balance_results: Vec<_> =
            stream::iter(balance_futs).buffered(RESCUE_CONCURRENCY).collect().await;

        let mut to_drain: Vec<(&FundedAccount, U256)> = Vec::new();
        for (result, account) in balance_results.into_iter().zip(accounts.accounts().iter()) {
            pb.inc(1);
            let (_, balance) = result?;
            if balance > params.drain_gas_cost {
                to_drain.push((account, balance));
            }
        }

        if to_drain.is_empty() {
            return Ok((U256::ZERO, 0));
        }

        let recoverable: U256 = to_drain
            .iter()
            .map(|(_, balance)| balance.saturating_sub(params.drain_gas_cost))
            .fold(U256::ZERO, |a, b| a.saturating_add(b));
        info!(
            accounts = to_drain.len(),
            recoverable_eth = %format_ether(recoverable),
            "found accounts with recoverable balance"
        );

        let drain_futs: Vec<_> = to_drain
            .iter()
            .map(|&(account, balance)| {
                let rpc_url = params.rpc_url.clone();
                let funder_address = params.funder_address;
                let chain_id = params.chain_id;
                let max_fee = params.max_fee;
                let max_priority_fee = params.max_priority_fee;
                let drain_gas_cost = params.drain_gas_cost;
                let drain_gas_limit = params.drain_gas_limit;
                let signer = account.signer.clone();
                let address = account.address;
                async move {
                    let send_amount = balance.saturating_sub(drain_gas_cost);
                    let wallet = EthereumWallet::from(signer);
                    let provider = create_wallet_provider(rpc_url, wallet);
                    let nonce = provider
                        .get_transaction_count(address)
                        .pending()
                        .await
                        .map_err(|e| BaselineError::Rpc(e.to_string()))?;

                    let tx = TransactionRequest::default()
                        .with_to(funder_address)
                        .with_value(send_amount)
                        .with_nonce(nonce)
                        .with_chain_id(chain_id)
                        .with_gas_limit(drain_gas_limit as u64)
                        .with_max_fee_per_gas(max_fee)
                        .with_max_priority_fee_per_gas(max_priority_fee);

                    match provider.send_transaction(tx).await {
                        Ok(pending) => {
                            let tx_hash = *pending.tx_hash();
                            debug!(
                                from = %address,
                                amount = %format_ether(send_amount),
                                tx_hash = %tx_hash,
                                "rescue drain tx sent"
                            );
                            Ok(Some((tx_hash, address, send_amount)))
                        }
                        Err(e) => {
                            warn!(from = %address, error = %e, "rescue drain tx failed, skipping");
                            Ok(None)
                        }
                    }
                }
            })
            .collect();

        let drain_results: Vec<_> =
            stream::iter(drain_futs).buffer_unordered(RESCUE_CONCURRENCY).collect().await;

        let mut pending_txs: Vec<(TxHash, Address)> = Vec::new();
        let mut total_drained = U256::ZERO;
        let mut drain_count = 0usize;
        for result in drain_results {
            let result: LoadResult<Option<(TxHash, Address, U256)>> = result;
            if let Some((tx_hash, address, amount)) = result? {
                pending_txs.push((tx_hash, address));
                total_drained = total_drained.saturating_add(amount);
                drain_count += 1;
            }
        }

        if !pending_txs.is_empty() {
            Self::await_confirmations(batch_rpc, &mut pending_txs).await?;
        }

        Ok((total_drained, drain_count))
    }

    async fn await_confirmations(
        batch_rpc: &BatchRpcClient,
        pending_txs: &mut Vec<(TxHash, Address)>,
    ) -> LoadResult<()> {
        let timeout = Duration::from_secs(60);
        let poll_interval = Duration::from_millis(500);
        let start = std::time::Instant::now();

        while !pending_txs.is_empty() && start.elapsed() < timeout {
            tokio::time::sleep(poll_interval).await;

            let hashes: Vec<TxHash> = pending_txs.iter().map(|(h, _)| *h).collect();
            let results = match batch_rpc.batch_get_transaction_receipts(&hashes).await {
                Ok(r) => r,
                Err(e) => {
                    warn!(error = %e, "batch receipt fetch failed during rescue confirmation");
                    continue;
                }
            };

            let mut still_pending = Vec::new();
            for ((tx_hash, address), receipt_opt) in pending_txs.drain(..).zip(results.into_iter())
            {
                match receipt_opt {
                    Some(receipt) => {
                        if receipt.status() {
                            debug!(tx_hash = %tx_hash, address = %address, "rescue tx confirmed");
                        } else {
                            warn!(tx_hash = %tx_hash, address = %address, "rescue tx reverted");
                        }
                    }
                    None => {
                        still_pending.push((tx_hash, address));
                    }
                }
            }
            *pending_txs = still_pending;
        }

        if !pending_txs.is_empty() {
            let unconfirmed: Vec<_> = pending_txs.iter().map(|(_, addr)| addr).collect();
            warn!(accounts = ?unconfirmed, "some rescue txs did not confirm within timeout");
        }

        Ok(())
    }

    fn rescue_progress_bar(total: u64, prefix: &str) -> ProgressBar {
        let pb = ProgressBar::new(total);
        pb.set_style(
            ProgressStyle::with_template("{prefix} [{bar:40.cyan/blue}] {pos}/{len} ({eta})")
                .expect("valid template")
                .progress_chars("█▓░"),
        );
        pb.set_prefix(prefix.to_string());
        pb
    }
}

impl RescueOptions {
    /// Default number of accounts scanned by the rescue command.
    pub const DEFAULT_SCAN_COUNT: usize = DEFAULT_RESCUE_SCAN_COUNT;

    /// Validates rescue options.
    pub fn validate(&self) -> Result<()> {
        if self.seed.is_none() && self.mnemonic.is_none() {
            bail!("either --seed or --mnemonic is required");
        }

        Ok(())
    }
}

#[derive(Debug)]
struct DrainParams {
    funder_address: Address,
    chain_id: u64,
    max_fee: u128,
    max_priority_fee: u128,
    drain_gas_cost: U256,
    drain_gas_limit: u128,
    rpc_url: Url,
}
