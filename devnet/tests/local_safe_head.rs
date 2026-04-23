//! Tests demonstrating that the base-consensus sequencer's `optimism_syncStatus`
//! reports a stale `safe_l2` — stuck at genesis — even after the sequencer has
//! produced blocks and the batcher has posted batches to L1.
//!
//! The op-batcher reads `safe_l2` from `optimism_syncStatus` to determine:
//!
//! 1. The starting point for block fetching (`safe_l2 + 1`)
//! 2. The pruning cursor for confirmed blocks
//! 3. The catchup position after reorgs
//!
//! Without the `local_safe_head` fix (PR #2362), `safe_l2` stays at genesis
//! because `EngineSyncState::safe_head()` is never advanced for sequencer-built
//! blocks — the `InsertTask` only sets `safe_head` when `is_payload_safe` is
//! true, which is false for sequencer-built (non-derived) blocks.
//!
//! This causes the batcher to:
//! - Never prune confirmed blocks (unbounded memory growth)
//! - Catch up from genesis after reorgs (resubmitting already-posted batches)
//! - Waste L1 gas on duplicate batch submissions

use std::time::Duration;

use alloy_provider::Provider;
use base_consensus_rpc::SyncStatusApiClient;
use devnet::DevnetBuilder;
use eyre::Result;
use jsonrpsee::http_client::HttpClientBuilder;
use tokio::time::{sleep, timeout};

const L1_CHAIN_ID: u64 = 1337;
const L2_CHAIN_ID: u64 = 84538453;

/// Demonstrates that the sequencer's `optimism_syncStatus` reports `safe_l2`
/// stuck at genesis (block 0) even after the sequencer produces blocks and the
/// batcher submits batches to L1.
///
/// This is the root cause of the op-batcher failure: the batcher reads
/// `safe_l2.number == 0` and cannot properly manage its submission lifecycle.
///
/// ## Setup
///
/// Spins up a full devnet stack (L1 + builder + consensus + batcher + client)
/// and waits for the sequencer to produce multiple L2 blocks. Then queries the
/// builder-consensus node's `optimism_syncStatus` RPC to check `safe_l2`.
///
/// ## Expected (bug) behavior
///
/// `safe_l2.number` stays at 0 while `unsafe_l2.number` advances well past 0.
/// The batcher, which connects to this same `optimism_syncStatus` endpoint,
/// sees the stale safe head and cannot properly prune or recover from reorgs.
#[tokio::test]
async fn sequencer_sync_status_safe_l2_stalled_at_genesis() -> Result<()> {
    base_node_runner::test_utils::init_silenced_tracing();

    let devnet = DevnetBuilder::new()
        .with_l1_chain_id(L1_CHAIN_ID)
        .with_l2_chain_id(L2_CHAIN_ID)
        .build()
        .await?;

    let l2_builder_provider = devnet.l2_builder_provider()?;
    let builder_consensus_url = devnet.l2_stack().builder_consensus_rpc_url();

    // Wait for the sequencer to produce at least 5 L2 blocks.
    let target_unsafe = 5u64;
    timeout(Duration::from_secs(30), async {
        loop {
            let block = l2_builder_provider.get_block_number().await?;
            if block >= target_unsafe {
                return Ok::<_, eyre::Error>(block);
            }
            sleep(Duration::from_millis(500)).await;
        }
    })
    .await
    .map_err(|_| eyre::eyre!("timed out waiting for L2 block production"))??;

    // Give the batcher time to submit batches to L1 and for L1 to mine them.
    sleep(Duration::from_secs(10)).await;

    // Query optimism_syncStatus on the builder-consensus (sequencer) node.
    let op_client = HttpClientBuilder::default().build(builder_consensus_url.as_str())?;
    let sync_status = op_client.sync_status().await?;

    let unsafe_l2_number = sync_status.unsafe_l2.block_info.number;
    let safe_l2_number = sync_status.safe_l2.block_info.number;

    // The unsafe head should have advanced well past genesis.
    assert!(
        unsafe_l2_number >= target_unsafe,
        "unsafe_l2 should be at least {target_unsafe}, got {unsafe_l2_number}"
    );

    // BUG: safe_l2 stays at 0 because the sequencer's EngineSyncState::safe_head()
    // is never updated for sequencer-built blocks (is_payload_safe = false).
    //
    // The op-batcher reads this value and uses it as its submission cursor.
    // With safe_l2 stuck at genesis:
    //   - The batcher's safe head poller always sees 0
    //   - prune_safe(0) never frees any blocks from the encoder
    //   - On reorg recovery, catchup_from = 0 + 1 = 1 (re-batches from genesis)
    //
    // When PR #2362 lands (adding local_safe_head), this assertion should be
    // updated: safe_l2 (or local_safe_l2) should advance alongside derivation.
    assert_eq!(
        safe_l2_number, 0,
        "safe_l2 should be stuck at 0 (stale) — this is the bug. \
         unsafe_l2={unsafe_l2_number}, safe_l2={safe_l2_number}. \
         The op-batcher sees safe_l2=0 and cannot properly manage batches."
    );

    Ok(())
}
