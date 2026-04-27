//! Benchmarks for recent-transaction startup scan channel draining.

use std::hint::black_box;

use alloy_eips::eip1898::BlockNumHash;
use alloy_primitives::B256;
use alloy_rlp::Encodable;
use base_batcher_service::{RecentTxScanner, TouchedChannelTracker};
use base_common_genesis::{ChainGenesis, RollupConfig};
use base_protocol::{Batch, BlockInfo, Channel, ChannelId, Frame, SingleBatch};
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};

const READY_CHANNEL_COUNT: usize = 4_096;
const SPARSE_TOUCHED_CHANNEL_COUNT: usize = 64;
const DUPLICATE_FANOUT_FRAME_COUNT: usize = 4_096;
const DUPLICATE_FANOUT_UNIQUE_CHANNEL_COUNT: usize = 512;

fn test_rollup_config() -> RollupConfig {
    RollupConfig {
        genesis: ChainGenesis {
            l2: BlockNumHash { number: 1_000, hash: B256::ZERO },
            l2_time: 1_000,
            ..Default::default()
        },
        block_time: 2,
        ..Default::default()
    }
}

fn encode_single_batch(batch: &SingleBatch) -> Vec<u8> {
    let typed_batch = Batch::Single(batch.clone());
    let mut batch_bytes = Vec::new();
    typed_batch.encode(&mut batch_bytes).expect("batch must encode");

    let mut rlp_buf = Vec::new();
    batch_bytes.as_slice().encode(&mut rlp_buf);
    miniz_oxide::deflate::compress_to_vec_zlib(&rlp_buf, 6)
}

fn ready_channel(id: ChannelId, timestamp: u64) -> Channel {
    let block_info = BlockInfo::default();
    let mut channel = Channel::new(id, block_info);
    channel
        .add_frame(
            Frame {
                id,
                number: 0,
                data: encode_single_batch(&SingleBatch { timestamp, ..Default::default() }),
                is_last: true,
            },
            block_info,
        )
        .expect("frame must be accepted");
    channel
}

fn incomplete_channel(id: ChannelId) -> Channel {
    let block_info = BlockInfo::default();
    let mut channel = Channel::new(id, block_info);
    channel
        .add_frame(Frame { id, number: 0, data: b"partial".to_vec(), is_last: false }, block_info)
        .expect("frame must be accepted");
    channel
}

fn channel_id(seed: usize) -> ChannelId {
    let mut id = [0u8; 16];
    id[..8].copy_from_slice(&(seed as u64).to_be_bytes());
    id
}

fn mixed_channel_map() -> std::collections::HashMap<ChannelId, Channel> {
    let mut channels = std::collections::HashMap::with_capacity(READY_CHANNEL_COUNT * 2);
    for index in 0..READY_CHANNEL_COUNT {
        channels
            .insert(channel_id(index), ready_channel(channel_id(index), 1_010 + index as u64 * 2));
        channels.insert(
            channel_id(index + READY_CHANNEL_COUNT),
            incomplete_channel(channel_id(index + READY_CHANNEL_COUNT)),
        );
    }
    channels
}

fn incomplete_channel_map() -> std::collections::HashMap<ChannelId, Channel> {
    let mut channels = std::collections::HashMap::with_capacity(READY_CHANNEL_COUNT * 2);
    for index in 0..READY_CHANNEL_COUNT * 2 {
        channels.insert(channel_id(index), incomplete_channel(channel_id(index)));
    }
    channels
}

fn sparse_ready_channel_map() -> std::collections::HashMap<ChannelId, Channel> {
    let mut channels = std::collections::HashMap::with_capacity(READY_CHANNEL_COUNT * 2);
    for index in 0..SPARSE_TOUCHED_CHANNEL_COUNT {
        channels
            .insert(channel_id(index), ready_channel(channel_id(index), 1_010 + index as u64 * 2));
    }
    for index in SPARSE_TOUCHED_CHANNEL_COUNT..READY_CHANNEL_COUNT * 2 {
        channels.insert(channel_id(index), incomplete_channel(channel_id(index)));
    }
    channels
}

fn touched_ready_ids() -> Vec<ChannelId> {
    (0..READY_CHANNEL_COUNT).map(channel_id).collect()
}

fn touched_incomplete_ids() -> Vec<ChannelId> {
    (0..READY_CHANNEL_COUNT).map(channel_id).collect()
}

fn touched_sparse_ids() -> Vec<ChannelId> {
    (0..SPARSE_TOUCHED_CHANNEL_COUNT).map(channel_id).collect()
}

fn unique_frame_channel_ids() -> Vec<ChannelId> {
    (0..READY_CHANNEL_COUNT).map(channel_id).collect()
}

fn duplicate_fanout_frame_channel_ids() -> Vec<ChannelId> {
    (0..DUPLICATE_FANOUT_FRAME_COUNT)
        .map(|index| channel_id(index % DUPLICATE_FANOUT_UNIQUE_CHANNEL_COUNT))
        .collect()
}

fn track_touched_channel_ids_with_vec(frame_channel_ids: &[ChannelId]) -> Vec<ChannelId> {
    let mut touched_channel_ids = Vec::with_capacity(frame_channel_ids.len());
    for channel_id in frame_channel_ids {
        if !touched_channel_ids.contains(channel_id) {
            touched_channel_ids.push(*channel_id);
        }
    }
    touched_channel_ids
}

fn track_touched_channel_ids_with_tracker(frame_channel_ids: &[ChannelId]) -> Vec<ChannelId> {
    let mut tracker = TouchedChannelTracker::with_capacity(frame_channel_ids.len());
    for channel_id in frame_channel_ids {
        tracker.record(*channel_id);
    }
    tracker.touched_channel_ids().to_vec()
}

fn bench_recent_tx_drain_ready_channels(c: &mut Criterion) {
    let mut group = c.benchmark_group("batcher_service/recent_txs/drain_ready_channels");
    group.sample_size(20);

    let rollup_config = test_rollup_config();
    let ready_ids = touched_ready_ids();
    group.bench_function("baseline_scan_all_with_4096_ready_and_4096_incomplete", |b| {
        b.iter_batched(
            mixed_channel_map,
            |mut channels| {
                let mut highest = None;
                RecentTxScanner::drain_all_ready_channels(
                    black_box(&mut channels),
                    black_box(0),
                    black_box(&rollup_config),
                    black_box(&mut highest),
                );
                black_box((channels, highest));
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("4096_touched_ready_among_8192_channels", |b| {
        b.iter_batched(
            mixed_channel_map,
            |mut channels| {
                let mut highest = None;
                RecentTxScanner::drain_ready_channels(
                    black_box(&mut channels),
                    black_box(&ready_ids),
                    black_box(0),
                    black_box(&rollup_config),
                    black_box(&mut highest),
                );
                black_box((channels, highest));
            },
            BatchSize::SmallInput,
        );
    });

    let incomplete_ids = touched_incomplete_ids();
    group.bench_function("baseline_scan_all_with_8192_incomplete", |b| {
        b.iter_batched(
            incomplete_channel_map,
            |mut channels| {
                let mut highest = None;
                RecentTxScanner::drain_all_ready_channels(
                    black_box(&mut channels),
                    black_box(0),
                    black_box(&rollup_config),
                    black_box(&mut highest),
                );
                black_box((channels, highest));
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("4096_touched_incomplete_among_8192_channels", |b| {
        b.iter_batched(
            incomplete_channel_map,
            |mut channels| {
                let mut highest = None;
                RecentTxScanner::drain_ready_channels(
                    black_box(&mut channels),
                    black_box(&incomplete_ids),
                    black_box(0),
                    black_box(&rollup_config),
                    black_box(&mut highest),
                );
                black_box((channels, highest));
            },
            BatchSize::SmallInput,
        );
    });

    let sparse_ids = touched_sparse_ids();
    group.bench_function("baseline_scan_all_with_64_touched_ready_among_8192_channels", |b| {
        b.iter_batched(
            sparse_ready_channel_map,
            |mut channels| {
                let mut highest = None;
                RecentTxScanner::drain_all_ready_channels(
                    black_box(&mut channels),
                    black_box(0),
                    black_box(&rollup_config),
                    black_box(&mut highest),
                );
                black_box((channels, highest));
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("64_touched_ready_among_8192_channels", |b| {
        b.iter_batched(
            sparse_ready_channel_map,
            |mut channels| {
                let mut highest = None;
                RecentTxScanner::drain_ready_channels(
                    black_box(&mut channels),
                    black_box(&sparse_ids),
                    black_box(0),
                    black_box(&rollup_config),
                    black_box(&mut highest),
                );
                black_box((channels, highest));
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("baseline_scan_all_with_64_touched_incomplete_among_8192_channels", |b| {
        b.iter_batched(
            incomplete_channel_map,
            |mut channels| {
                let mut highest = None;
                RecentTxScanner::drain_all_ready_channels(
                    black_box(&mut channels),
                    black_box(0),
                    black_box(&rollup_config),
                    black_box(&mut highest),
                );
                black_box((channels, highest));
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("64_touched_incomplete_among_8192_channels", |b| {
        b.iter_batched(
            incomplete_channel_map,
            |mut channels| {
                let mut highest = None;
                RecentTxScanner::drain_ready_channels(
                    black_box(&mut channels),
                    black_box(&sparse_ids),
                    black_box(0),
                    black_box(&rollup_config),
                    black_box(&mut highest),
                );
                black_box((channels, highest));
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

fn bench_recent_tx_track_touched_channel_ids(c: &mut Criterion) {
    let mut group = c.benchmark_group("batcher_service/recent_txs/track_touched_channel_ids");
    group.sample_size(20);

    let unique_frame_ids = unique_frame_channel_ids();
    group.bench_function("baseline_vec_scan_4096_unique_frame_channel_ids", |b| {
        b.iter(|| black_box(track_touched_channel_ids_with_vec(black_box(&unique_frame_ids))));
    });
    group.bench_function("hashset_tracker_4096_unique_frame_channel_ids", |b| {
        b.iter(|| black_box(track_touched_channel_ids_with_tracker(black_box(&unique_frame_ids))));
    });

    let duplicate_fanout_frame_ids = duplicate_fanout_frame_channel_ids();
    group.bench_function("baseline_vec_scan_4096_frames_across_512_unique_channel_ids", |b| {
        b.iter(|| {
            black_box(track_touched_channel_ids_with_vec(black_box(&duplicate_fanout_frame_ids)))
        });
    });
    group.bench_function("hashset_tracker_4096_frames_across_512_unique_channel_ids", |b| {
        b.iter(|| {
            black_box(track_touched_channel_ids_with_tracker(black_box(
                &duplicate_fanout_frame_ids,
            )))
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_recent_tx_drain_ready_channels,
    bench_recent_tx_track_touched_channel_ids,
);
criterion_main!(benches);
