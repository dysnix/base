//! Benchmarks for recent-transaction startup scan channel draining.

use std::{
    collections::{HashMap, hash_map::Entry},
    hint::black_box,
};

use alloy_eips::eip1898::BlockNumHash;
use alloy_primitives::{B256, Bytes};
use alloy_rlp::Encodable;
use base_batcher_service::{RecentTxScanner, TouchedChannelTracker};
use base_common_genesis::{ChainGenesis, RollupConfig};
use base_protocol::{
    Batch, BatchReader, BlockInfo, Channel, ChannelId, DERIVATION_VERSION_0, Frame, SingleBatch,
};
use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};

const READY_CHANNEL_COUNT: usize = 4_096;
const SPARSE_TOUCHED_CHANNEL_COUNT: usize = 64;
const DUPLICATE_FANOUT_FRAME_COUNT: usize = 4_096;
const DUPLICATE_FANOUT_UNIQUE_CHANNEL_COUNT: usize = 512;
const MULTI_BLOCK_COUNT: usize = 8;
const MULTI_BLOCK_TOUCHED_CHANNEL_COUNT: usize = 4_096;
const READY_TRANSITION_BLOCK_COUNT: usize = 4;
const READY_TRANSITION_CHANNEL_COUNT: usize = 1_024;
const STAGGERED_READY_BLOCK_COUNT: usize = 4;
const STAGGERED_READY_CHANNEL_COUNT: usize = 1_024;
const MIXED_READY_BLOCK_COUNT: usize = 4;
const DECODE_DENSITY_CHANNEL_COUNT: usize = 1_024;
const DECODE_COMPONENT_BATCH_COUNT: usize = 16;
const DECODE_COMPONENT_FRAME_COUNTS: [usize; 3] = [1, 4, 16];
const DECODE_DENSITY_READY_CHANNEL_COUNTS: [usize; 4] = [0, 256, 512, DECODE_DENSITY_CHANNEL_COUNT];
const FRONT_LOADED_READY_CHANNEL_COUNTS: [usize; STAGGERED_READY_BLOCK_COUNT] =
    [512, 256, 128, 128];
const BACK_LOADED_READY_CHANNEL_COUNTS: [usize; STAGGERED_READY_BLOCK_COUNT] = [128, 128, 256, 512];
const MIXED_BACK_LOADED_READY_COHORTS: [ReadyTransitionCohort; 5] = [
    ReadyTransitionCohort { start_block: 0, ready_block: 0, channel_count: 128 },
    ReadyTransitionCohort { start_block: 0, ready_block: 1, channel_count: 128 },
    ReadyTransitionCohort { start_block: 1, ready_block: 2, channel_count: 256 },
    ReadyTransitionCohort { start_block: 0, ready_block: 3, channel_count: 256 },
    ReadyTransitionCohort { start_block: 2, ready_block: 3, channel_count: 256 },
];
const MATCHED_VOLUME_BACK_LOADED_READY_COHORTS: [ReadyTransitionCohort; 5] = [
    ReadyTransitionCohort { start_block: 0, ready_block: 0, channel_count: 128 },
    ReadyTransitionCohort { start_block: 0, ready_block: 1, channel_count: 128 },
    ReadyTransitionCohort { start_block: 0, ready_block: 2, channel_count: 256 },
    ReadyTransitionCohort { start_block: 0, ready_block: 3, channel_count: 384 },
    ReadyTransitionCohort { start_block: 1, ready_block: 3, channel_count: 128 },
];

#[derive(Clone, Copy, Debug)]
struct ReadyTransitionCohort {
    start_block: usize,
    ready_block: usize,
    channel_count: usize,
}

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

fn encode_single_batch_stream(batch_count: usize) -> Vec<u8> {
    let mut rlp_buf = Vec::new();
    for batch_index in 0..batch_count {
        let typed_batch = Batch::Single(SingleBatch {
            timestamp: 1_010 + batch_index as u64 * 2,
            ..Default::default()
        });
        let mut batch_bytes = Vec::new();
        typed_batch.encode(&mut batch_bytes).expect("batch must encode");
        batch_bytes.as_slice().encode(&mut rlp_buf);
    }
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

fn ready_multi_batch_channel(id: ChannelId, frame_count: usize, batch_count: usize) -> Channel {
    let block_info = BlockInfo::default();
    let mut channel = Channel::new(id, block_info);
    let compressed_batches = encode_single_batch_stream(batch_count);

    for (frame_number, frame_data) in
        split_frame_data_across_blocks(&compressed_batches, frame_count).into_iter().enumerate()
    {
        channel
            .add_frame(
                Frame {
                    id,
                    number: frame_number as u16,
                    data: frame_data,
                    is_last: frame_number + 1 == frame_count,
                },
                block_info,
            )
            .expect("frame must be accepted");
    }

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

fn encode_tx_frames_payload(frames: &[Frame]) -> Vec<u8> {
    let mut encoded =
        Vec::with_capacity(1 + frames.iter().map(|frame| frame.encode().len()).sum::<usize>());
    encoded.push(DERIVATION_VERSION_0);
    for frame in frames {
        encoded.extend_from_slice(&frame.encode());
    }
    encoded
}

fn ready_tx_payload(id: ChannelId, timestamp: u64) -> Vec<u8> {
    encode_tx_frames_payload(&[Frame {
        id,
        number: 0,
        data: encode_single_batch(&SingleBatch { timestamp, ..Default::default() }),
        is_last: true,
    }])
}

fn incomplete_tx_payload(id: ChannelId, number: u16) -> Vec<u8> {
    encode_tx_frames_payload(&[Frame { id, number, data: b"partial".to_vec(), is_last: false }])
}

fn ready_block_tx_payloads() -> Vec<Vec<u8>> {
    (0..READY_CHANNEL_COUNT)
        .map(|index| ready_tx_payload(channel_id(index), 1_010 + index as u64 * 2))
        .collect()
}

fn incomplete_followup_tx_payloads(frame_number: u16, channel_count: usize) -> Vec<Vec<u8>> {
    (0..channel_count).map(|index| incomplete_tx_payload(channel_id(index), frame_number)).collect()
}

fn sparse_incomplete_followup_tx_payloads() -> Vec<Vec<u8>> {
    incomplete_followup_tx_payloads(1, SPARSE_TOUCHED_CHANNEL_COUNT)
}

fn multi_block_incomplete_tx_payloads(
    block_count: usize,
    touched_channel_count: usize,
) -> Vec<Vec<Vec<u8>>> {
    (0..block_count)
        .map(|block_index| {
            incomplete_followup_tx_payloads((block_index + 1) as u16, touched_channel_count)
        })
        .collect()
}

fn split_frame_data_across_blocks(data: &[u8], block_count: usize) -> Vec<Vec<u8>> {
    assert!(block_count > 0, "ready-transition fixtures require at least one block");
    assert!(
        data.len() >= block_count,
        "ready-transition fixtures require at least one payload byte per block"
    );

    let base_chunk_len = data.len() / block_count;
    let remainder = data.len() % block_count;
    let mut chunks = Vec::with_capacity(block_count);
    let mut offset = 0;

    for block_index in 0..block_count {
        let chunk_len = base_chunk_len + usize::from(block_index < remainder);
        let next_offset = offset + chunk_len;
        chunks.push(data[offset..next_offset].to_vec());
        offset = next_offset;
    }

    chunks
}

fn multi_block_ready_transition_tx_payloads(
    block_count: usize,
    channel_count: usize,
) -> Vec<Vec<Vec<u8>>> {
    let mut block_tx_payloads: Vec<Vec<Vec<u8>>> =
        (0..block_count).map(|_| Vec::with_capacity(channel_count)).collect();

    for index in 0..channel_count {
        let channel_id = channel_id(index);
        let encoded_batch = encode_single_batch(&SingleBatch {
            timestamp: 1_010 + index as u64 * 2,
            ..Default::default()
        });

        for (block_index, frame_data) in
            split_frame_data_across_blocks(&encoded_batch, block_count).into_iter().enumerate()
        {
            block_tx_payloads[block_index].push(encode_tx_frames_payload(&[Frame {
                id: channel_id,
                number: block_index as u16,
                data: frame_data,
                is_last: block_index + 1 == block_count,
            }]));
        }
    }

    block_tx_payloads
}

fn multi_block_staggered_ready_tx_payloads(
    block_count: usize,
    channel_count: usize,
) -> Vec<Vec<Vec<u8>>> {
    let mut block_tx_payloads: Vec<Vec<Vec<u8>>> =
        (0..block_count).map(|_| Vec::with_capacity(channel_count)).collect();

    for index in 0..channel_count {
        let channel_id = channel_id(index);
        let encoded_batch = encode_single_batch(&SingleBatch {
            timestamp: 1_010 + index as u64 * 2,
            ..Default::default()
        });
        let ready_block = index % block_count;

        for (block_index, frame_data) in
            split_frame_data_across_blocks(&encoded_batch, ready_block + 1).into_iter().enumerate()
        {
            block_tx_payloads[block_index].push(encode_tx_frames_payload(&[Frame {
                id: channel_id,
                number: block_index as u16,
                data: frame_data,
                is_last: block_index == ready_block,
            }]));
        }
    }

    block_tx_payloads
}

fn multi_block_weighted_ready_tx_payloads(ready_channel_counts: &[usize]) -> Vec<Vec<Vec<u8>>> {
    let block_count = ready_channel_counts.len();
    let mut block_tx_payloads: Vec<Vec<Vec<u8>>> = ready_channel_counts
        .iter()
        .map(|channel_count| Vec::with_capacity(*channel_count))
        .collect();

    let mut channel_index = 0usize;
    for (ready_block, ready_channel_count) in ready_channel_counts.iter().copied().enumerate() {
        for _ in 0..ready_channel_count {
            let channel_id = channel_id(channel_index);
            let encoded_batch = encode_single_batch(&SingleBatch {
                timestamp: 1_010 + channel_index as u64 * 2,
                ..Default::default()
            });
            for (block_index, frame_data) in
                split_frame_data_across_blocks(&encoded_batch, ready_block + 1)
                    .into_iter()
                    .enumerate()
            {
                block_tx_payloads[block_index].push(encode_tx_frames_payload(&[Frame {
                    id: channel_id,
                    number: block_index as u16,
                    data: frame_data,
                    is_last: block_index == ready_block,
                }]));
            }
            channel_index += 1;
        }
    }

    debug_assert_eq!(channel_index, ready_channel_counts.iter().sum::<usize>());
    debug_assert_eq!(block_tx_payloads.len(), block_count);

    block_tx_payloads
}

fn multi_block_cohort_ready_tx_payloads(
    block_count: usize,
    ready_cohorts: &[ReadyTransitionCohort],
) -> Vec<Vec<Vec<u8>>> {
    let mut block_tx_payloads: Vec<Vec<Vec<u8>>> = (0..block_count).map(|_| Vec::new()).collect();
    let mut channel_index = 0usize;

    for ready_cohort in ready_cohorts {
        assert!(
            ready_cohort.start_block <= ready_cohort.ready_block,
            "ready cohort start block must not exceed its ready block"
        );
        assert!(
            ready_cohort.ready_block < block_count,
            "ready cohort ready block must stay within the requested block count"
        );

        for _ in 0..ready_cohort.channel_count {
            let channel_id = channel_id(channel_index);
            let encoded_batch = encode_single_batch(&SingleBatch {
                timestamp: 1_010 + channel_index as u64 * 2,
                ..Default::default()
            });
            let active_block_count = ready_cohort.ready_block - ready_cohort.start_block + 1;

            for (frame_offset, frame_data) in
                split_frame_data_across_blocks(&encoded_batch, active_block_count)
                    .into_iter()
                    .enumerate()
            {
                let block_index = ready_cohort.start_block + frame_offset;
                block_tx_payloads[block_index].push(encode_tx_frames_payload(&[Frame {
                    id: channel_id,
                    number: frame_offset as u16,
                    data: frame_data,
                    is_last: block_index == ready_cohort.ready_block,
                }]));
            }

            channel_index += 1;
        }
    }

    debug_assert_eq!(
        channel_index,
        ready_cohorts.iter().map(|ready_cohort| ready_cohort.channel_count).sum::<usize>()
    );

    block_tx_payloads
}

fn prebuffered_decode_density_fixture(
    channel_count: usize,
    ready_channel_count: usize,
) -> (HashMap<ChannelId, Channel>, Vec<Vec<u8>>) {
    let block_info = BlockInfo::default();
    let mut channels = HashMap::with_capacity(channel_count);
    let mut tx_payloads = Vec::with_capacity(channel_count);

    for index in 0..channel_count {
        let channel_id = channel_id(index);
        let encoded_batch = encode_single_batch(&SingleBatch {
            timestamp: 1_010 + index as u64 * 2,
            ..Default::default()
        });
        let total_frame_count = if index < ready_channel_count { 3 } else { 4 };
        let frame_data_chunks = split_frame_data_across_blocks(&encoded_batch, total_frame_count);
        let mut channel = Channel::new(channel_id, block_info);

        for (frame_number, frame_data) in frame_data_chunks.iter().take(2).enumerate() {
            channel
                .add_frame(
                    Frame {
                        id: channel_id,
                        number: frame_number as u16,
                        data: frame_data.clone(),
                        is_last: false,
                    },
                    block_info,
                )
                .expect("fixture frame must be accepted");
        }

        tx_payloads.push(encode_tx_frames_payload(&[Frame {
            id: channel_id,
            number: 2,
            data: frame_data_chunks[2].clone(),
            is_last: index < ready_channel_count,
        }]));
        channels.insert(channel_id, channel);
    }

    (channels, tx_payloads)
}

fn decode_channel_data(
    data: Bytes,
    inclusion_timestamp: u64,
    rollup_config: &RollupConfig,
) -> Option<u64> {
    let max_rlp = rollup_config.max_rlp_bytes_per_channel(inclusion_timestamp) as usize;
    let mut reader = BatchReader::new(data, max_rlp);
    let mut highest_l2 = None;

    while let Some(batch) = reader.next_batch(rollup_config) {
        let last_timestamp = match &batch {
            Batch::Single(single_batch) => single_batch.timestamp,
            Batch::Span(span_batch) => span_batch.final_timestamp(),
        };
        let relative = rollup_config.block_number_from_timestamp(last_timestamp);
        let l2_block = rollup_config.genesis.l2.number + relative;
        highest_l2 = Some(highest_l2.map_or(l2_block, |highest: u64| highest.max(l2_block)));
    }

    highest_l2
}

fn decode_ready_channel(
    channel: &Channel,
    inclusion_timestamp: u64,
    rollup_config: &RollupConfig,
) -> Option<u64> {
    let data = channel.frame_data().expect("fixture channel must be ready");
    decode_channel_data(data, inclusion_timestamp, rollup_config)
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

fn track_touched_channel_ids_with_reused_tracker(
    tracker: &mut TouchedChannelTracker,
    frame_channel_ids: &[ChannelId],
) -> Vec<ChannelId> {
    tracker.reset_with_capacity(frame_channel_ids.len());
    for channel_id in frame_channel_ids {
        tracker.record(*channel_id);
    }
    tracker.touched_channel_ids().to_vec()
}

fn count_ready_touched_channels_with_entry_api(
    channels: &mut HashMap<ChannelId, Channel>,
    touched_channel_ids: &[ChannelId],
) -> usize {
    let mut ready_channels = 0;
    for channel_id in touched_channel_ids {
        let Entry::Occupied(channel_entry) = channels.entry(*channel_id) else {
            continue;
        };
        if channel_entry.get().is_ready() {
            ready_channels += 1;
        }
    }
    ready_channels
}

fn count_ready_touched_channels_with_get(
    channels: &HashMap<ChannelId, Channel>,
    touched_channel_ids: &[ChannelId],
) -> usize {
    let mut ready_channels = 0;
    for channel_id in touched_channel_ids {
        if channels.get(channel_id).is_some_and(Channel::is_ready) {
            ready_channels += 1;
        }
    }
    ready_channels
}

fn process_block_with_vec_tracking_and_full_scan(
    channels: &mut HashMap<ChannelId, Channel>,
    tx_payloads: &[Vec<u8>],
    rollup_config: &RollupConfig,
) -> Option<u64> {
    let block_info = BlockInfo::default();
    let mut touched_channel_ids = Vec::with_capacity(tx_payloads.len());
    for tx_payload in tx_payloads {
        let frames =
            Frame::parse_frames(tx_payload).expect("fixture tx payload must parse into frames");
        for frame in frames {
            if !touched_channel_ids.contains(&frame.id) {
                touched_channel_ids.push(frame.id);
            }
            let channel =
                channels.entry(frame.id).or_insert_with(|| Channel::new(frame.id, block_info));
            channel.add_frame(frame, block_info).expect("fixture frame must be accepted");
        }
    }

    let mut highest = None;
    RecentTxScanner::drain_all_ready_channels(channels, 0, rollup_config, &mut highest);
    highest
}

fn process_block_with_tracker_and_touched_only_drain(
    channels: &mut HashMap<ChannelId, Channel>,
    tx_payloads: &[Vec<u8>],
    rollup_config: &RollupConfig,
) -> Option<u64> {
    let block_info = BlockInfo::default();
    let mut touched_channel_ids = TouchedChannelTracker::with_capacity(tx_payloads.len());
    for tx_payload in tx_payloads {
        let frames =
            Frame::parse_frames(tx_payload).expect("fixture tx payload must parse into frames");
        for frame in frames {
            touched_channel_ids.record(frame.id);
            let channel =
                channels.entry(frame.id).or_insert_with(|| Channel::new(frame.id, block_info));
            channel.add_frame(frame, block_info).expect("fixture frame must be accepted");
        }
    }

    let mut highest = None;
    RecentTxScanner::drain_ready_channels(
        channels,
        touched_channel_ids.touched_channel_ids(),
        0,
        rollup_config,
        &mut highest,
    );
    highest
}

fn process_blocks_with_vec_tracking_and_full_scan(
    channels: &mut HashMap<ChannelId, Channel>,
    block_tx_payloads: &[Vec<Vec<u8>>],
    rollup_config: &RollupConfig,
) -> Option<u64> {
    let block_info = BlockInfo::default();
    let mut highest = None;

    for tx_payloads in block_tx_payloads {
        let mut touched_channel_ids = Vec::with_capacity(tx_payloads.len());
        for tx_payload in tx_payloads {
            let frames =
                Frame::parse_frames(tx_payload).expect("fixture tx payload must parse into frames");
            for frame in frames {
                if !touched_channel_ids.contains(&frame.id) {
                    touched_channel_ids.push(frame.id);
                }
                let channel =
                    channels.entry(frame.id).or_insert_with(|| Channel::new(frame.id, block_info));
                channel.add_frame(frame, block_info).expect("fixture frame must be accepted");
            }
        }

        RecentTxScanner::drain_all_ready_channels(channels, 0, rollup_config, &mut highest);
    }

    highest
}

fn bench_recent_tx_ready_channel_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("batcher_service/recent_txs/ready_channel_lookup");
    group.sample_size(20);

    let incomplete_ids = touched_incomplete_ids();
    group.bench_function("baseline_get_4096_touched_incomplete_among_8192_channels", |b| {
        b.iter_batched(
            incomplete_channel_map,
            |channels| {
                black_box(count_ready_touched_channels_with_get(
                    black_box(&channels),
                    black_box(&incomplete_ids),
                ))
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("entry_api_4096_touched_incomplete_among_8192_channels", |b| {
        b.iter_batched(
            incomplete_channel_map,
            |mut channels| {
                black_box(count_ready_touched_channels_with_entry_api(
                    black_box(&mut channels),
                    black_box(&incomplete_ids),
                ))
            },
            BatchSize::SmallInput,
        );
    });

    let sparse_ids = touched_sparse_ids();
    group.bench_function("baseline_get_64_touched_ready_among_8192_channels", |b| {
        b.iter_batched(
            sparse_ready_channel_map,
            |channels| {
                black_box(count_ready_touched_channels_with_get(
                    black_box(&channels),
                    black_box(&sparse_ids),
                ))
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("entry_api_64_touched_ready_among_8192_channels", |b| {
        b.iter_batched(
            sparse_ready_channel_map,
            |mut channels| {
                black_box(count_ready_touched_channels_with_entry_api(
                    black_box(&mut channels),
                    black_box(&sparse_ids),
                ))
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
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
    let mut reused_unique_tracker = TouchedChannelTracker::with_capacity(unique_frame_ids.len());
    group.bench_function("reused_hashset_tracker_4096_unique_frame_channel_ids", |b| {
        b.iter(|| {
            black_box(track_touched_channel_ids_with_reused_tracker(
                black_box(&mut reused_unique_tracker),
                black_box(&unique_frame_ids),
            ))
        });
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
    let mut reused_duplicate_tracker =
        TouchedChannelTracker::with_capacity(duplicate_fanout_frame_ids.len());
    group.bench_function("reused_hashset_tracker_4096_frames_across_512_unique_channel_ids", |b| {
        b.iter(|| {
            black_box(track_touched_channel_ids_with_reused_tracker(
                black_box(&mut reused_duplicate_tracker),
                black_box(&duplicate_fanout_frame_ids),
            ))
        });
    });

    group.finish();
}

fn bench_recent_tx_process_block(c: &mut Criterion) {
    let mut group = c.benchmark_group("batcher_service/recent_txs/process_block");
    group.sample_size(20);

    let rollup_config = test_rollup_config();
    let ready_tx_payloads = ready_block_tx_payloads();
    group.bench_function("baseline_vec_scan_all_4096_ready_unique_channels_from_empty", |b| {
        b.iter_batched(
            HashMap::new,
            |mut channels| {
                black_box(process_block_with_vec_tracking_and_full_scan(
                    black_box(&mut channels),
                    black_box(&ready_tx_payloads),
                    black_box(&rollup_config),
                ));
                black_box(channels)
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("tracker_touched_only_4096_ready_unique_channels_from_empty", |b| {
        b.iter_batched(
            HashMap::new,
            |mut channels| {
                black_box(process_block_with_tracker_and_touched_only_drain(
                    black_box(&mut channels),
                    black_box(&ready_tx_payloads),
                    black_box(&rollup_config),
                ));
                black_box(channels)
            },
            BatchSize::SmallInput,
        );
    });

    let sparse_incomplete_tx_payloads = sparse_incomplete_followup_tx_payloads();
    group.bench_function(
        "baseline_vec_scan_all_64_incomplete_touches_among_8192_buffered_channels",
        |b| {
            b.iter_batched(
                incomplete_channel_map,
                |mut channels| {
                    black_box(process_block_with_vec_tracking_and_full_scan(
                        black_box(&mut channels),
                        black_box(&sparse_incomplete_tx_payloads),
                        black_box(&rollup_config),
                    ));
                    black_box(channels)
                },
                BatchSize::SmallInput,
            );
        },
    );
    group.bench_function(
        "tracker_touched_only_64_incomplete_touches_among_8192_buffered_channels",
        |b| {
            b.iter_batched(
                incomplete_channel_map,
                |mut channels| {
                    black_box(process_block_with_tracker_and_touched_only_drain(
                        black_box(&mut channels),
                        black_box(&sparse_incomplete_tx_payloads),
                        black_box(&rollup_config),
                    ));
                    black_box(channels)
                },
                BatchSize::SmallInput,
            );
        },
    );

    group.finish();
}

fn process_blocks_with_tracker_and_touched_only_drain(
    channels: &mut HashMap<ChannelId, Channel>,
    block_tx_payloads: &[Vec<Vec<u8>>],
    rollup_config: &RollupConfig,
) -> Option<u64> {
    let block_info = BlockInfo::default();
    let mut touched_channel_ids = TouchedChannelTracker::default();
    let mut highest = None;

    for tx_payloads in block_tx_payloads {
        touched_channel_ids.reset_with_capacity(tx_payloads.len());
        for tx_payload in tx_payloads {
            let frames =
                Frame::parse_frames(tx_payload).expect("fixture tx payload must parse into frames");
            for frame in frames {
                touched_channel_ids.record(frame.id);
                let channel =
                    channels.entry(frame.id).or_insert_with(|| Channel::new(frame.id, block_info));
                channel.add_frame(frame, block_info).expect("fixture frame must be accepted");
            }
        }

        RecentTxScanner::drain_ready_channels(
            channels,
            touched_channel_ids.touched_channel_ids(),
            0,
            rollup_config,
            &mut highest,
        );
    }

    highest
}

fn process_blocks_with_fresh_tracker_and_touched_only_drain(
    channels: &mut HashMap<ChannelId, Channel>,
    block_tx_payloads: &[Vec<Vec<u8>>],
    rollup_config: &RollupConfig,
) -> Option<u64> {
    let block_info = BlockInfo::default();
    let mut highest = None;

    for tx_payloads in block_tx_payloads {
        let mut touched_channel_ids = TouchedChannelTracker::with_capacity(tx_payloads.len());
        for tx_payload in tx_payloads {
            let frames =
                Frame::parse_frames(tx_payload).expect("fixture tx payload must parse into frames");
            for frame in frames {
                touched_channel_ids.record(frame.id);
                let channel =
                    channels.entry(frame.id).or_insert_with(|| Channel::new(frame.id, block_info));
                channel.add_frame(frame, block_info).expect("fixture frame must be accepted");
            }
        }

        RecentTxScanner::drain_ready_channels(
            channels,
            touched_channel_ids.touched_channel_ids(),
            0,
            rollup_config,
            &mut highest,
        );
    }

    highest
}

fn bench_recent_tx_process_blocks(c: &mut Criterion) {
    let mut group = c.benchmark_group("batcher_service/recent_txs/process_blocks");
    group.sample_size(20);

    let rollup_config = test_rollup_config();
    let block_tx_payloads =
        multi_block_incomplete_tx_payloads(MULTI_BLOCK_COUNT, MULTI_BLOCK_TOUCHED_CHANNEL_COUNT);

    group.bench_function(
        "fresh_tracker_8_blocks_4096_incomplete_touches_each_among_persistent_channels",
        |b| {
            b.iter_batched(
                HashMap::new,
                |mut channels| {
                    black_box(process_blocks_with_fresh_tracker_and_touched_only_drain(
                        black_box(&mut channels),
                        black_box(&block_tx_payloads),
                        black_box(&rollup_config),
                    ));
                    black_box(channels)
                },
                BatchSize::SmallInput,
            );
        },
    );
    group.bench_function(
        "reused_tracker_8_blocks_4096_incomplete_touches_each_among_persistent_channels",
        |b| {
            b.iter_batched(
                HashMap::new,
                |mut channels| {
                    black_box(process_blocks_with_tracker_and_touched_only_drain(
                        black_box(&mut channels),
                        black_box(&block_tx_payloads),
                        black_box(&rollup_config),
                    ));
                    black_box(channels)
                },
                BatchSize::SmallInput,
            );
        },
    );

    group.finish();
}

fn bench_recent_tx_process_blocks_ready_transition(c: &mut Criterion) {
    let mut group = c.benchmark_group("batcher_service/recent_txs/process_blocks_ready_transition");
    group.sample_size(15);

    let rollup_config = test_rollup_config();
    let block_tx_payloads = multi_block_ready_transition_tx_payloads(
        READY_TRANSITION_BLOCK_COUNT,
        READY_TRANSITION_CHANNEL_COUNT,
    );

    group.bench_function(
        "baseline_vec_scan_all_4_blocks_1024_channels_ready_on_final_block",
        |b| {
            b.iter_batched(
                HashMap::new,
                |mut channels| {
                    black_box(process_blocks_with_vec_tracking_and_full_scan(
                        black_box(&mut channels),
                        black_box(&block_tx_payloads),
                        black_box(&rollup_config),
                    ));
                    black_box(channels)
                },
                BatchSize::SmallInput,
            );
        },
    );
    group.bench_function("fresh_tracker_4_blocks_1024_channels_ready_on_final_block", |b| {
        b.iter_batched(
            HashMap::new,
            |mut channels| {
                black_box(process_blocks_with_fresh_tracker_and_touched_only_drain(
                    black_box(&mut channels),
                    black_box(&block_tx_payloads),
                    black_box(&rollup_config),
                ));
                black_box(channels)
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("reused_tracker_4_blocks_1024_channels_ready_on_final_block", |b| {
        b.iter_batched(
            HashMap::new,
            |mut channels| {
                black_box(process_blocks_with_tracker_and_touched_only_drain(
                    black_box(&mut channels),
                    black_box(&block_tx_payloads),
                    black_box(&rollup_config),
                ));
                black_box(channels)
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

fn bench_recent_tx_process_blocks_staggered_ready(c: &mut Criterion) {
    let mut group = c.benchmark_group("batcher_service/recent_txs/process_blocks_staggered_ready");
    group.sample_size(15);

    let rollup_config = test_rollup_config();
    let block_tx_payloads = multi_block_staggered_ready_tx_payloads(
        STAGGERED_READY_BLOCK_COUNT,
        STAGGERED_READY_CHANNEL_COUNT,
    );

    group.bench_function("baseline_vec_scan_all_4_blocks_1024_channels_ready_in_quarters", |b| {
        b.iter_batched(
            HashMap::new,
            |mut channels| {
                black_box(process_blocks_with_vec_tracking_and_full_scan(
                    black_box(&mut channels),
                    black_box(&block_tx_payloads),
                    black_box(&rollup_config),
                ));
                black_box(channels)
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("fresh_tracker_4_blocks_1024_channels_ready_in_quarters", |b| {
        b.iter_batched(
            HashMap::new,
            |mut channels| {
                black_box(process_blocks_with_fresh_tracker_and_touched_only_drain(
                    black_box(&mut channels),
                    black_box(&block_tx_payloads),
                    black_box(&rollup_config),
                ));
                black_box(channels)
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("reused_tracker_4_blocks_1024_channels_ready_in_quarters", |b| {
        b.iter_batched(
            HashMap::new,
            |mut channels| {
                black_box(process_blocks_with_tracker_and_touched_only_drain(
                    black_box(&mut channels),
                    black_box(&block_tx_payloads),
                    black_box(&rollup_config),
                ));
                black_box(channels)
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

fn bench_recent_tx_process_blocks_weighted_ready(c: &mut Criterion) {
    let mut group = c.benchmark_group("batcher_service/recent_txs/process_blocks_weighted_ready");
    group.sample_size(15);

    let rollup_config = test_rollup_config();
    let front_loaded_block_tx_payloads =
        multi_block_weighted_ready_tx_payloads(&FRONT_LOADED_READY_CHANNEL_COUNTS);
    let back_loaded_block_tx_payloads =
        multi_block_weighted_ready_tx_payloads(&BACK_LOADED_READY_CHANNEL_COUNTS);

    group.bench_function("baseline_vec_scan_all_front_loaded_4_blocks_1024_channels", |b| {
        b.iter_batched(
            HashMap::new,
            |mut channels| {
                black_box(process_blocks_with_vec_tracking_and_full_scan(
                    black_box(&mut channels),
                    black_box(&front_loaded_block_tx_payloads),
                    black_box(&rollup_config),
                ));
                black_box(channels)
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("fresh_tracker_front_loaded_4_blocks_1024_channels", |b| {
        b.iter_batched(
            HashMap::new,
            |mut channels| {
                black_box(process_blocks_with_fresh_tracker_and_touched_only_drain(
                    black_box(&mut channels),
                    black_box(&front_loaded_block_tx_payloads),
                    black_box(&rollup_config),
                ));
                black_box(channels)
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("reused_tracker_front_loaded_4_blocks_1024_channels", |b| {
        b.iter_batched(
            HashMap::new,
            |mut channels| {
                black_box(process_blocks_with_tracker_and_touched_only_drain(
                    black_box(&mut channels),
                    black_box(&front_loaded_block_tx_payloads),
                    black_box(&rollup_config),
                ));
                black_box(channels)
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("baseline_vec_scan_all_back_loaded_4_blocks_1024_channels", |b| {
        b.iter_batched(
            HashMap::new,
            |mut channels| {
                black_box(process_blocks_with_vec_tracking_and_full_scan(
                    black_box(&mut channels),
                    black_box(&back_loaded_block_tx_payloads),
                    black_box(&rollup_config),
                ));
                black_box(channels)
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("fresh_tracker_back_loaded_4_blocks_1024_channels", |b| {
        b.iter_batched(
            HashMap::new,
            |mut channels| {
                black_box(process_blocks_with_fresh_tracker_and_touched_only_drain(
                    black_box(&mut channels),
                    black_box(&back_loaded_block_tx_payloads),
                    black_box(&rollup_config),
                ));
                black_box(channels)
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("reused_tracker_back_loaded_4_blocks_1024_channels", |b| {
        b.iter_batched(
            HashMap::new,
            |mut channels| {
                black_box(process_blocks_with_tracker_and_touched_only_drain(
                    black_box(&mut channels),
                    black_box(&back_loaded_block_tx_payloads),
                    black_box(&rollup_config),
                ));
                black_box(channels)
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

fn bench_recent_tx_process_blocks_mixed_ready(c: &mut Criterion) {
    let mut group = c.benchmark_group("batcher_service/recent_txs/process_blocks_mixed_ready");
    group.sample_size(15);

    let rollup_config = test_rollup_config();
    let block_tx_payloads = multi_block_cohort_ready_tx_payloads(
        MIXED_READY_BLOCK_COUNT,
        &MIXED_BACK_LOADED_READY_COHORTS,
    );

    group.bench_function("baseline_vec_scan_all_mixed_back_loaded_4_blocks_1024_channels", |b| {
        b.iter_batched(
            HashMap::new,
            |mut channels| {
                black_box(process_blocks_with_vec_tracking_and_full_scan(
                    black_box(&mut channels),
                    black_box(&block_tx_payloads),
                    black_box(&rollup_config),
                ));
                black_box(channels)
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("fresh_tracker_mixed_back_loaded_4_blocks_1024_channels", |b| {
        b.iter_batched(
            HashMap::new,
            |mut channels| {
                black_box(process_blocks_with_fresh_tracker_and_touched_only_drain(
                    black_box(&mut channels),
                    black_box(&block_tx_payloads),
                    black_box(&rollup_config),
                ));
                black_box(channels)
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("reused_tracker_mixed_back_loaded_4_blocks_1024_channels", |b| {
        b.iter_batched(
            HashMap::new,
            |mut channels| {
                black_box(process_blocks_with_tracker_and_touched_only_drain(
                    black_box(&mut channels),
                    black_box(&block_tx_payloads),
                    black_box(&rollup_config),
                ));
                black_box(channels)
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

fn bench_recent_tx_process_blocks_touch_start_sparsity(c: &mut Criterion) {
    let mut group =
        c.benchmark_group("batcher_service/recent_txs/process_blocks_touch_start_sparsity");
    group.sample_size(15);

    let rollup_config = test_rollup_config();
    let dense_start_block_tx_payloads =
        multi_block_weighted_ready_tx_payloads(&BACK_LOADED_READY_CHANNEL_COUNTS);
    let sparse_start_block_tx_payloads = multi_block_cohort_ready_tx_payloads(
        MIXED_READY_BLOCK_COUNT,
        &MIXED_BACK_LOADED_READY_COHORTS,
    );

    group.bench_function(
        "baseline_vec_scan_all_dense_start_back_loaded_4_blocks_1024_channels",
        |b| {
            b.iter_batched(
                HashMap::new,
                |mut channels| {
                    black_box(process_blocks_with_vec_tracking_and_full_scan(
                        black_box(&mut channels),
                        black_box(&dense_start_block_tx_payloads),
                        black_box(&rollup_config),
                    ));
                    black_box(channels)
                },
                BatchSize::SmallInput,
            );
        },
    );
    group.bench_function("fresh_tracker_dense_start_back_loaded_4_blocks_1024_channels", |b| {
        b.iter_batched(
            HashMap::new,
            |mut channels| {
                black_box(process_blocks_with_fresh_tracker_and_touched_only_drain(
                    black_box(&mut channels),
                    black_box(&dense_start_block_tx_payloads),
                    black_box(&rollup_config),
                ));
                black_box(channels)
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("reused_tracker_dense_start_back_loaded_4_blocks_1024_channels", |b| {
        b.iter_batched(
            HashMap::new,
            |mut channels| {
                black_box(process_blocks_with_tracker_and_touched_only_drain(
                    black_box(&mut channels),
                    black_box(&dense_start_block_tx_payloads),
                    black_box(&rollup_config),
                ));
                black_box(channels)
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function(
        "baseline_vec_scan_all_sparse_start_back_loaded_4_blocks_1024_channels",
        |b| {
            b.iter_batched(
                HashMap::new,
                |mut channels| {
                    black_box(process_blocks_with_vec_tracking_and_full_scan(
                        black_box(&mut channels),
                        black_box(&sparse_start_block_tx_payloads),
                        black_box(&rollup_config),
                    ));
                    black_box(channels)
                },
                BatchSize::SmallInput,
            );
        },
    );
    group.bench_function("fresh_tracker_sparse_start_back_loaded_4_blocks_1024_channels", |b| {
        b.iter_batched(
            HashMap::new,
            |mut channels| {
                black_box(process_blocks_with_fresh_tracker_and_touched_only_drain(
                    black_box(&mut channels),
                    black_box(&sparse_start_block_tx_payloads),
                    black_box(&rollup_config),
                ));
                black_box(channels)
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("reused_tracker_sparse_start_back_loaded_4_blocks_1024_channels", |b| {
        b.iter_batched(
            HashMap::new,
            |mut channels| {
                black_box(process_blocks_with_tracker_and_touched_only_drain(
                    black_box(&mut channels),
                    black_box(&sparse_start_block_tx_payloads),
                    black_box(&rollup_config),
                ));
                black_box(channels)
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

fn bench_recent_tx_process_blocks_touch_start_matched_volume(c: &mut Criterion) {
    let mut group =
        c.benchmark_group("batcher_service/recent_txs/process_blocks_touch_start_matched_volume");
    group.sample_size(15);

    let rollup_config = test_rollup_config();
    let dense_start_block_tx_payloads =
        multi_block_weighted_ready_tx_payloads(&BACK_LOADED_READY_CHANNEL_COUNTS);
    let matched_volume_sparse_start_block_tx_payloads = multi_block_cohort_ready_tx_payloads(
        MIXED_READY_BLOCK_COUNT,
        &MATCHED_VOLUME_BACK_LOADED_READY_COHORTS,
    );

    group.bench_function(
        "baseline_vec_scan_all_dense_start_back_loaded_4_blocks_1024_channels_matched_volume",
        |b| {
            b.iter_batched(
                HashMap::new,
                |mut channels| {
                    black_box(process_blocks_with_vec_tracking_and_full_scan(
                        black_box(&mut channels),
                        black_box(&dense_start_block_tx_payloads),
                        black_box(&rollup_config),
                    ));
                    black_box(channels)
                },
                BatchSize::SmallInput,
            );
        },
    );
    group.bench_function(
        "fresh_tracker_dense_start_back_loaded_4_blocks_1024_channels_matched_volume",
        |b| {
            b.iter_batched(
                HashMap::new,
                |mut channels| {
                    black_box(process_blocks_with_fresh_tracker_and_touched_only_drain(
                        black_box(&mut channels),
                        black_box(&dense_start_block_tx_payloads),
                        black_box(&rollup_config),
                    ));
                    black_box(channels)
                },
                BatchSize::SmallInput,
            );
        },
    );
    group.bench_function(
        "reused_tracker_dense_start_back_loaded_4_blocks_1024_channels_matched_volume",
        |b| {
            b.iter_batched(
                HashMap::new,
                |mut channels| {
                    black_box(process_blocks_with_tracker_and_touched_only_drain(
                        black_box(&mut channels),
                        black_box(&dense_start_block_tx_payloads),
                        black_box(&rollup_config),
                    ));
                    black_box(channels)
                },
                BatchSize::SmallInput,
            );
        },
    );

    group.bench_function(
        "baseline_vec_scan_all_matched_volume_sparse_start_back_loaded_4_blocks_1024_channels",
        |b| {
            b.iter_batched(
                HashMap::new,
                |mut channels| {
                    black_box(process_blocks_with_vec_tracking_and_full_scan(
                        black_box(&mut channels),
                        black_box(&matched_volume_sparse_start_block_tx_payloads),
                        black_box(&rollup_config),
                    ));
                    black_box(channels)
                },
                BatchSize::SmallInput,
            );
        },
    );
    group.bench_function(
        "fresh_tracker_matched_volume_sparse_start_back_loaded_4_blocks_1024_channels",
        |b| {
            b.iter_batched(
                HashMap::new,
                |mut channels| {
                    black_box(process_blocks_with_fresh_tracker_and_touched_only_drain(
                        black_box(&mut channels),
                        black_box(&matched_volume_sparse_start_block_tx_payloads),
                        black_box(&rollup_config),
                    ));
                    black_box(channels)
                },
                BatchSize::SmallInput,
            );
        },
    );
    group.bench_function(
        "reused_tracker_matched_volume_sparse_start_back_loaded_4_blocks_1024_channels",
        |b| {
            b.iter_batched(
                HashMap::new,
                |mut channels| {
                    black_box(process_blocks_with_tracker_and_touched_only_drain(
                        black_box(&mut channels),
                        black_box(&matched_volume_sparse_start_block_tx_payloads),
                        black_box(&rollup_config),
                    ));
                    black_box(channels)
                },
                BatchSize::SmallInput,
            );
        },
    );

    group.finish();
}

fn bench_recent_tx_process_block_decode_density(c: &mut Criterion) {
    let mut group = c.benchmark_group("batcher_service/recent_txs/process_block_decode_density");
    group.sample_size(15);

    let rollup_config = test_rollup_config();

    for ready_channel_count in DECODE_DENSITY_READY_CHANNEL_COUNTS {
        group.bench_with_input(
            BenchmarkId::new(
                "baseline_vec_scan_all_prebuffered_touched_channels_ready_on_current_block",
                ready_channel_count,
            ),
            &ready_channel_count,
            |b, &ready_channel_count| {
                b.iter_batched(
                    || {
                        prebuffered_decode_density_fixture(
                            DECODE_DENSITY_CHANNEL_COUNT,
                            ready_channel_count,
                        )
                    },
                    |(mut channels, tx_payloads)| {
                        black_box(process_block_with_vec_tracking_and_full_scan(
                            black_box(&mut channels),
                            black_box(&tx_payloads),
                            black_box(&rollup_config),
                        ));
                        black_box(channels)
                    },
                    BatchSize::SmallInput,
                );
            },
        );
        group.bench_with_input(
            BenchmarkId::new(
                "tracker_touched_only_prebuffered_touched_channels_ready_on_current_block",
                ready_channel_count,
            ),
            &ready_channel_count,
            |b, &ready_channel_count| {
                b.iter_batched(
                    || {
                        prebuffered_decode_density_fixture(
                            DECODE_DENSITY_CHANNEL_COUNT,
                            ready_channel_count,
                        )
                    },
                    |(mut channels, tx_payloads)| {
                        black_box(process_block_with_tracker_and_touched_only_drain(
                            black_box(&mut channels),
                            black_box(&tx_payloads),
                            black_box(&rollup_config),
                        ));
                        black_box(channels)
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

fn bench_recent_tx_decode_channel_components(c: &mut Criterion) {
    let mut group = c.benchmark_group("batcher_service/recent_txs/decode_channel_components");
    group.sample_size(15);

    let rollup_config = test_rollup_config();

    for frame_count in DECODE_COMPONENT_FRAME_COUNTS {
        let channel = ready_multi_batch_channel(
            channel_id(frame_count),
            frame_count,
            DECODE_COMPONENT_BATCH_COUNT,
        );
        let frame_data = channel.frame_data().expect("fixture channel must be ready");

        group.bench_with_input(
            BenchmarkId::new("frame_data_only_16_batches_split_across_frames", frame_count),
            &frame_count,
            |b, _| {
                b.iter(|| {
                    black_box(
                        black_box(&channel).frame_data().expect("fixture channel must be ready"),
                    )
                });
            },
        );
        group.bench_with_input(
            BenchmarkId::new(
                "batch_reader_only_preaggregated_16_batches_split_across_frames",
                frame_count,
            ),
            &frame_count,
            |b, _| {
                b.iter(|| {
                    black_box(decode_channel_data(
                        black_box(frame_data.clone()),
                        black_box(0),
                        black_box(&rollup_config),
                    ))
                });
            },
        );
        group.bench_with_input(
            BenchmarkId::new(
                "frame_data_plus_batch_reader_16_batches_split_across_frames",
                frame_count,
            ),
            &frame_count,
            |b, _| {
                b.iter(|| {
                    black_box(decode_ready_channel(
                        black_box(&channel),
                        black_box(0),
                        black_box(&rollup_config),
                    ))
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_recent_tx_ready_channel_lookup,
    bench_recent_tx_drain_ready_channels,
    bench_recent_tx_track_touched_channel_ids,
    bench_recent_tx_process_block,
    bench_recent_tx_process_blocks,
    bench_recent_tx_process_blocks_ready_transition,
    bench_recent_tx_process_blocks_staggered_ready,
    bench_recent_tx_process_blocks_weighted_ready,
    bench_recent_tx_process_blocks_mixed_ready,
    bench_recent_tx_process_blocks_touch_start_sparsity,
    bench_recent_tx_process_blocks_touch_start_matched_volume,
    bench_recent_tx_process_block_decode_density,
    bench_recent_tx_decode_channel_components,
);
criterion_main!(benches);
