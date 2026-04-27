//! Startup scan of recent L1 blocks for submitted batcher frames.

use std::collections::{HashMap, HashSet};

use alloy_primitives::Address;
use alloy_provider::{Provider, RootProvider};
use alloy_rpc_types_eth::{BlockNumberOrTag, TransactionTrait};
use base_common_genesis::RollupConfig;
use base_protocol::{Batch, BatchReader, BlockInfo, Channel, ChannelId, Frame};
use futures::StreamExt;
use tracing::{debug, info};

/// Maximum depth allowed for the recent-transaction startup scan.
///
/// Matches the limit used by op-batcher's `--check-recent-txs-depth` flag.
pub const MAX_CHECK_RECENT_TXS_DEPTH: u64 = 128;

/// Maximum number of L1 block fetches in flight during the startup scan.
///
/// Bounds peak memory to roughly this many full L1 blocks while still
/// achieving significant speedup over sequential fetching.
pub const SCAN_FETCH_CONCURRENCY: usize = 16;

/// Tracks the unique channel IDs touched while scanning a single L1 block.
#[derive(Debug, Default)]
pub struct TouchedChannelTracker {
    touched_channel_ids: Vec<ChannelId>,
    seen_channel_ids: HashSet<ChannelId>,
}

impl TouchedChannelTracker {
    /// Creates a tracker sized for roughly `capacity` recorded frame touches.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            touched_channel_ids: Vec::with_capacity(capacity),
            seen_channel_ids: HashSet::with_capacity(capacity),
        }
    }

    /// Records a touched channel ID the first time it appears in the current block.
    pub fn record(&mut self, channel_id: ChannelId) {
        if self.seen_channel_ids.insert(channel_id) {
            self.touched_channel_ids.push(channel_id);
        }
    }

    /// Returns the touched channel IDs in first-seen order.
    pub fn touched_channel_ids(&self) -> &[ChannelId] {
        &self.touched_channel_ids
    }
}

/// Scans recent L1 blocks on startup to find the highest submitted L2 block.
///
/// When the batcher restarts after an unclean shutdown, in-memory channel state
/// is lost. `RecentTxScanner` compensates by reading the last N L1 blocks and
/// decoding any calldata batcher frames sent from the batcher address to the
/// batch inbox. Complete channels are decoded to determine the highest L2 block
/// number already submitted but not yet reflected in the safe head, allowing
/// the block cursor to be advanced accordingly and preventing re-submissions.
#[derive(Debug)]
pub struct RecentTxScanner;

impl RecentTxScanner {
    /// Scans the last `depth` L1 blocks for batcher transactions and returns
    /// the highest L2 block number covered, or `None` if no complete batcher
    /// channels were found.
    ///
    /// Only calldata transactions are decoded (those beginning with
    /// `DERIVATION_VERSION_0`). Blob transactions are identified by their
    /// empty calldata and skipped — their frame data resides in KZG sidecars
    /// that would require a separate fetch not supported by this scanner.
    ///
    /// **Limitation:** channels whose opening frame falls before the scan window
    /// are never completed and will be silently missed. The caller should treat
    /// the result as a best-effort lower bound, not a guarantee.
    pub async fn highest_submitted_l2_block(
        l1_provider: &RootProvider,
        batcher_address: Address,
        batch_inbox: Address,
        depth: u64,
        rollup_config: &RollupConfig,
    ) -> eyre::Result<Option<u64>> {
        let current_l1 = l1_provider
            .get_block_number()
            .await
            .map_err(|e| eyre::eyre!("failed to fetch L1 head for recent tx scan: {e}"))?;
        let scan_start = current_l1.saturating_sub(depth.saturating_sub(1));

        info!(
            depth = %depth,
            scan_start = %scan_start,
            scan_end = %current_l1,
            batcher = %batcher_address,
            inbox = %batch_inbox,
            "scanning recent L1 blocks for submitted batcher frames"
        );

        let mut channels: HashMap<ChannelId, Channel> = HashMap::new();
        let mut highest_l2: Option<u64> = None;

        // Fetch blocks in parallel with bounded concurrency, preserving L1 order.
        // Blocks are processed as the stream yields them so peak memory is
        // bounded by the concurrency limit (~16 blocks) rather than the full
        // scan depth (~128 blocks).
        let block_stream = futures::stream::iter(scan_start..=current_l1)
            .map(|block_num| {
                let provider = l1_provider.clone();
                async move {
                    let block = provider
                        .get_block_by_number(BlockNumberOrTag::Number(block_num))
                        .full()
                        .await
                        .map_err(|e| eyre::eyre!("failed to fetch L1 block {block_num}: {e}"))?;
                    eyre::Ok((block_num, block))
                }
            })
            .buffered(SCAN_FETCH_CONCURRENCY);
        futures::pin_mut!(block_stream);

        while let Some(result) = block_stream.next().await {
            let (block_num, block) = result?;
            let block = match block {
                Some(b) => b,
                None => {
                    debug!(block = %block_num, "L1 block not found during recent tx scan");
                    continue;
                }
            };

            let block_info = BlockInfo {
                hash: block.header.hash,
                number: block_num,
                parent_hash: block.header.inner.parent_hash,
                timestamp: block.header.inner.timestamp,
            };

            let mut touched_channel_ids =
                TouchedChannelTracker::with_capacity(block.transactions.len());
            for tx in block.transactions.txns() {
                if tx.inner.signer() != batcher_address {
                    continue;
                }
                if tx.inner.to() != Some(batch_inbox) {
                    continue;
                }

                // Only parse calldata (version-0) frames. Blob transactions have
                // empty or absent calldata and will fail parse_frames gracefully.
                let frames = match Frame::parse_frames(tx.inner.input()) {
                    Ok(f) => f,
                    Err(_) => continue,
                };

                for frame in frames {
                    touched_channel_ids.record(frame.id);
                    let channel = channels
                        .entry(frame.id)
                        .or_insert_with(|| Channel::new(frame.id, block_info));
                    if let Err(e) = channel.add_frame(frame, block_info) {
                        debug!(error = %e, "ignoring rejected batcher frame during recent tx scan");
                    }
                }
            }

            Self::drain_ready_channels(
                &mut channels,
                touched_channel_ids.touched_channel_ids(),
                block_info.timestamp,
                rollup_config,
                &mut highest_l2,
            );
        }

        if let Some(block) = highest_l2 {
            info!(highest_l2 = %block, "recent tx scan found highest submitted L2 block");
        } else {
            info!("recent tx scan found no submitted batcher frames");
        }

        Ok(highest_l2)
    }

    /// Decodes all ready channels by scanning the full buffered map.
    ///
    /// This preserves the original O(n) scan behavior and is kept public so the
    /// focused Criterion bench can compare the touched-only drain against the
    /// full-map scan it replaced.
    pub fn drain_all_ready_channels(
        channels: &mut HashMap<ChannelId, Channel>,
        inclusion_timestamp: u64,
        rollup_config: &RollupConfig,
        highest_l2: &mut Option<u64>,
    ) {
        let ready_channel_ids: Vec<ChannelId> =
            channels.iter().filter(|(_, channel)| channel.is_ready()).map(|(id, _)| *id).collect();
        for channel_id in ready_channel_ids {
            if let Some(channel) = channels.remove(&channel_id) {
                Self::decode_channel(&channel, inclusion_timestamp, rollup_config, highest_l2);
            }
        }
    }

    /// Decodes all channels that became ready within the current block.
    ///
    /// This helper is public so the startup-scan drain path can be benchmarked
    /// directly without requiring a full RPC-backed recent-transaction scan.
    pub fn drain_ready_channels(
        channels: &mut HashMap<ChannelId, Channel>,
        touched_channel_ids: &[ChannelId],
        inclusion_timestamp: u64,
        rollup_config: &RollupConfig,
        highest_l2: &mut Option<u64>,
    ) {
        for channel_id in touched_channel_ids {
            let is_ready = channels.get(channel_id).is_some_and(Channel::is_ready);
            if !is_ready {
                continue;
            }
            if let Some(channel) = channels.remove(channel_id) {
                Self::decode_channel(&channel, inclusion_timestamp, rollup_config, highest_l2);
            }
        }
    }

    /// Decodes all batches from a complete channel and updates `highest_l2` with
    /// the maximum L2 block number found.
    fn decode_channel(
        channel: &Channel,
        inclusion_timestamp: u64,
        rollup_config: &RollupConfig,
        highest_l2: &mut Option<u64>,
    ) {
        let Some(data) = channel.frame_data() else { return };
        let max_rlp = rollup_config.max_rlp_bytes_per_channel(inclusion_timestamp) as usize;
        let mut reader = BatchReader::new(data.to_vec(), max_rlp);
        while let Some(batch) = reader.next_batch(rollup_config) {
            let last_timestamp = match &batch {
                Batch::Single(sb) => sb.timestamp,
                Batch::Span(sb) => sb.final_timestamp(),
            };
            let relative = rollup_config.block_number_from_timestamp(last_timestamp);
            let l2_block = rollup_config.genesis.l2.number + relative;
            *highest_l2 = Some(highest_l2.map_or(l2_block, |h| h.max(l2_block)));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use alloy_eips::eip1898::BlockNumHash;
    use alloy_primitives::B256;
    use alloy_rlp::Encodable;
    use base_common_genesis::{ChainGenesis, RollupConfig};
    use base_protocol::{Batch, BlockInfo, Channel, ChannelId, Frame, SingleBatch};

    use super::{RecentTxScanner, TouchedChannelTracker};

    /// Build a [`RollupConfig`] with controllable genesis parameters for tests.
    fn test_rollup_config(
        genesis_l2_number: u64,
        genesis_l2_time: u64,
        block_time: u64,
    ) -> RollupConfig {
        RollupConfig {
            genesis: ChainGenesis {
                l2: BlockNumHash { number: genesis_l2_number, hash: B256::ZERO },
                l2_time: genesis_l2_time,
                ..Default::default()
            },
            block_time,
            ..Default::default()
        }
    }

    /// Encode a `SingleBatch` into the zlib-compressed channel frame data format
    /// that `BatchReader` expects:
    ///   `zlib_compress`( `rlp_bytes`( `batch_type_byte` ++ `rlp_encode(SingleBatch)` ) )
    fn encode_single_batch(batch: &SingleBatch) -> Vec<u8> {
        // Batch-level encoding: type byte + RLP body.
        let typed_batch = Batch::Single(batch.clone());
        let mut batch_bytes = Vec::new();
        typed_batch.encode(&mut batch_bytes).expect("batch must encode");

        // Wrap as RLP byte string (how ChannelOut wraps it before compressing).
        let mut rlp_buf = Vec::new();
        batch_bytes.as_slice().encode(&mut rlp_buf);

        // Compress with zlib (produces a stream whose first byte has lower nibble 0x8,
        // matching the ZLIB_DEFLATE_COMPRESSION_METHOD check in BatchReader::decompress).
        miniz_oxide::deflate::compress_to_vec_zlib(&rlp_buf, 6)
    }

    /// Create a single-frame channel whose frame data is `payload`.
    fn single_frame_channel(id: ChannelId, payload: Vec<u8>) -> Channel {
        let block_info = BlockInfo::default();
        let mut channel = Channel::new(id, block_info);
        let frame = Frame { id, number: 0, data: payload, is_last: true };
        channel.add_frame(frame, block_info).expect("frame must be accepted");
        channel
    }

    // ── decode_channel tests ─────────────────────────────────────────────────

    /// A channel with no frame data (empty, non-ready channel) must produce no
    /// output and not panic.
    #[test]
    fn decode_channel_no_frame_data_is_noop() {
        let cfg = test_rollup_config(1000, 1000, 2);
        // A channel with no frames has frame_data() == None.
        let channel = Channel::new([0u8; 16], BlockInfo::default());
        let mut highest = None;
        RecentTxScanner::decode_channel(&channel, 0, &cfg, &mut highest);
        assert_eq!(highest, None);
    }

    /// A channel containing one `SingleBatch` with a known timestamp must yield
    /// the correct L2 block number.
    #[test]
    fn decode_channel_single_batch_computes_correct_l2_block() {
        // genesis at L2 block 1000, timestamp 1000, 2-second blocks.
        // batch timestamp 1010 → relative block 5 → L2 block 1005.
        let cfg = test_rollup_config(1000, 1000, 2);
        let batch = SingleBatch { timestamp: 1010, ..Default::default() };

        let id: ChannelId = [1u8; 16];
        let channel = single_frame_channel(id, encode_single_batch(&batch));

        let mut highest = None;
        RecentTxScanner::decode_channel(&channel, 0, &cfg, &mut highest);
        assert_eq!(highest, Some(1005));
    }

    /// When the channel contains multiple batches, `decode_channel` must track
    /// the maximum L2 block across all of them.
    #[test]
    fn decode_channel_multiple_batches_returns_highest() {
        let cfg = test_rollup_config(1000, 1000, 2);

        // Encode two batches into the same compressed payload.
        // batch A: timestamp 1010 → block 1005
        // batch B: timestamp 1020 → block 1010
        let batch_a = SingleBatch { timestamp: 1010, ..Default::default() };
        let batch_b = SingleBatch { timestamp: 1020, ..Default::default() };

        // Encode both into a single byte stream the way ChannelOut would:
        // rlp_bytes(batchA) ++ rlp_bytes(batchB), then zlib-compress.
        let mut combined = Vec::new();
        let mut a_encoded = Vec::new();
        Batch::Single(batch_a).encode(&mut a_encoded).unwrap();
        a_encoded.as_slice().encode(&mut combined);

        let mut b_encoded = Vec::new();
        Batch::Single(batch_b).encode(&mut b_encoded).unwrap();
        b_encoded.as_slice().encode(&mut combined);

        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&combined, 6);

        let id: ChannelId = [2u8; 16];
        let channel = single_frame_channel(id, compressed);

        let mut highest = None;
        RecentTxScanner::decode_channel(&channel, 0, &cfg, &mut highest);
        assert_eq!(highest, Some(1010));
    }

    /// `decode_channel` must not update `highest_l2` when the channel data is
    /// corrupted and `BatchReader` fails to produce any batches.
    #[test]
    fn decode_channel_corrupted_data_is_silently_skipped() {
        let cfg = test_rollup_config(1000, 1000, 2);

        // Craft a payload whose first byte looks like zlib (0x78) but whose body
        // is garbage, so decompression fails and next_batch returns None.
        let junk = vec![0x78u8, 0x9c, 0xde, 0xad, 0xbe, 0xef];

        let id: ChannelId = [3u8; 16];
        let channel = single_frame_channel(id, junk);

        let mut highest = Some(42);
        RecentTxScanner::decode_channel(&channel, 0, &cfg, &mut highest);
        // The existing value must be preserved — no panics, no reset.
        assert_eq!(highest, Some(42));
    }

    /// `decode_channel` must not lower an existing `highest_l2` value: when a
    /// channel yields a block number below the current maximum, the maximum wins.
    #[test]
    fn decode_channel_does_not_lower_existing_highest() {
        let cfg = test_rollup_config(1000, 1000, 2);
        let batch = SingleBatch { timestamp: 1010, ..Default::default() };

        let id: ChannelId = [4u8; 16];
        let channel = single_frame_channel(id, encode_single_batch(&batch));

        // Pre-seed with a higher block number (2000 > 1005).
        let mut highest = Some(2000u64);
        RecentTxScanner::decode_channel(&channel, 0, &cfg, &mut highest);
        assert_eq!(highest, Some(2000));
    }

    #[test]
    fn drain_ready_channels_only_checks_touched_ids() {
        let cfg = test_rollup_config(1000, 1000, 2);
        let ready_channel = single_frame_channel(
            [9u8; 16],
            encode_single_batch(&SingleBatch { timestamp: 1010, ..Default::default() }),
        );
        let untouched_ready_id = ready_channel.id();
        let touched_incomplete_id: ChannelId = [8u8; 16];
        let mut touched_incomplete = Channel::new(touched_incomplete_id, BlockInfo::default());
        touched_incomplete
            .add_frame(
                Frame {
                    id: touched_incomplete_id,
                    number: 0,
                    data: b"partial".to_vec(),
                    is_last: false,
                },
                BlockInfo::default(),
            )
            .expect("frame must be accepted");

        let mut channels = HashMap::from([
            (untouched_ready_id, ready_channel),
            (touched_incomplete_id, touched_incomplete),
        ]);
        let mut highest = None;

        RecentTxScanner::drain_ready_channels(
            &mut channels,
            &[touched_incomplete_id],
            0,
            &cfg,
            &mut highest,
        );

        assert_eq!(highest, None, "untouched ready channels should not be decoded this block");
        assert!(
            channels.contains_key(&untouched_ready_id),
            "untouched ready channels must remain buffered"
        );
        assert!(
            channels.contains_key(&touched_incomplete_id),
            "touched but incomplete channels must remain buffered"
        );

        RecentTxScanner::drain_ready_channels(
            &mut channels,
            &[untouched_ready_id],
            0,
            &cfg,
            &mut highest,
        );

        assert_eq!(highest, Some(1005));
        assert!(
            !channels.contains_key(&untouched_ready_id),
            "touched ready channels must be drained once decoded"
        );
    }

    #[test]
    fn touched_channel_tracker_deduplicates_and_preserves_first_seen_order() {
        let channel_id_a: ChannelId = [1u8; 16];
        let channel_id_b: ChannelId = [2u8; 16];
        let channel_id_c: ChannelId = [3u8; 16];
        let mut tracker = TouchedChannelTracker::with_capacity(6);

        tracker.record(channel_id_b);
        tracker.record(channel_id_a);
        tracker.record(channel_id_b);
        tracker.record(channel_id_c);
        tracker.record(channel_id_a);

        assert_eq!(tracker.touched_channel_ids(), &[channel_id_b, channel_id_a, channel_id_c]);
    }
}
