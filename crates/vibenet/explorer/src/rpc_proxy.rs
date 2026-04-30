//! Thin client around the upstream node for the on-demand read paths.
//!
//! The indexer uses an owned alloy [`Provider`] directly; this module is for
//! the HTTP server, which needs to answer "get me this block/tx/balance"
//! without any caching and without going through proxyd (which would make
//! the explorer eat its own rate limits).

use std::sync::Arc;

use alloy_primitives::{Address, B256, Bytes, U256};
use alloy_provider::{Provider, ProviderBuilder, RootProvider};
use alloy_rpc_types_eth::BlockId;
use base_common_network::Base;
use eyre::{Result, WrapErr};

/// RPC response alias for full blocks on the Base network. OP deposit txs
/// (type 0x7e) will deserialize correctly through this because [`Base`]
/// points [`alloy_network::Network::TransactionResponse`] at
/// `base_common_rpc_types::Transaction`, which knows about
/// [`base_common_consensus::BaseTxEnvelope`].
pub(crate) type BaseBlock = <Base as alloy_network::Network>::BlockResponse;
pub(crate) type BaseTransaction = <Base as alloy_network::Network>::TransactionResponse;
pub(crate) type BaseReceipt = <Base as alloy_network::Network>::ReceiptResponse;

/// HTTP JSON-RPC client for explorer read-through queries.
#[derive(Clone)]
pub struct RpcClient {
    inner: Arc<RootProvider<Base>>,
}

impl std::fmt::Debug for RpcClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RpcClient").finish_non_exhaustive()
    }
}

impl RpcClient {
    /// Connect to the upstream HTTP RPC endpoint.
    pub async fn connect(http_url: &str) -> Result<Self> {
        let url = http_url.parse().wrap_err_with(|| format!("parsing rpc url {http_url}"))?;
        let provider = ProviderBuilder::new()
            .disable_recommended_fillers()
            .network::<Base>()
            .connect_http(url);
        Ok(Self { inner: Arc::new(provider.root().clone()) })
    }

    pub(crate) async fn chain_id(&self) -> Result<u64> {
        Ok(self.inner.get_chain_id().await?)
    }

    pub(crate) async fn block_by_number(&self, n: u64) -> Result<Option<BaseBlock>> {
        Ok(self.inner.get_block_by_number(n.into()).full().await?)
    }

    pub(crate) async fn block_by_hash(&self, h: B256) -> Result<Option<BaseBlock>> {
        Ok(self.inner.get_block_by_hash(h).full().await?)
    }

    pub(crate) async fn tx_by_hash(&self, h: B256) -> Result<Option<BaseTransaction>> {
        Ok(self.inner.get_transaction_by_hash(h).await?)
    }

    pub(crate) async fn receipt(&self, h: B256) -> Result<Option<BaseReceipt>> {
        Ok(self.inner.get_transaction_receipt(h).await?)
    }

    pub(crate) async fn block_receipts(&self, id: BlockId) -> Result<Option<Vec<BaseReceipt>>> {
        Ok(self.inner.get_block_receipts(id).await?)
    }

    pub(crate) async fn balance(&self, addr: Address) -> Result<U256> {
        Ok(self.inner.get_balance(addr).await?)
    }

    pub(crate) async fn nonce(&self, addr: Address) -> Result<u64> {
        Ok(self.inner.get_transaction_count(addr).await?)
    }

    pub(crate) async fn code(&self, addr: Address) -> Result<Bytes> {
        Ok(self.inner.get_code_at(addr).await?)
    }

    pub(crate) async fn block_number(&self) -> Result<u64> {
        Ok(self.inner.get_block_number().await?)
    }

    /// Run `debug_traceTransaction` with the built-in `callTracer`. Returns
    /// the raw JSON result so the caller can pretty-print it without us
    /// committing to a schema for every tracer variant.
    pub(crate) async fn trace_transaction(&self, h: B256) -> Result<serde_json::Value> {
        let opts = serde_json::json!({ "tracer": "callTracer" });
        let value: serde_json::Value = self
            .inner
            .client()
            .request("debug_traceTransaction", (h, opts))
            .await
            .wrap_err("debug_traceTransaction")?;
        Ok(value)
    }
}
