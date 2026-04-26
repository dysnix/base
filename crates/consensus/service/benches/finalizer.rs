//! Benchmarks for [`L2Finalizer`] queue operations.

use std::hint::black_box;

use alloy_eips::BlockNumHash;
use base_common_rpc_types_engine::BasePayloadAttributes;
use base_consensus_node::L2Finalizer;
use base_protocol::{AttributesWithParent, BlockInfo, L2BlockInfo};
use criterion::{Criterion, criterion_group, criterion_main};

const ENTRY_COUNT: u64 = 4_096;

fn attrs(l2_parent_number: u64, l1_origin_number: u64) -> AttributesWithParent {
    let parent = L2BlockInfo {
        block_info: BlockInfo { number: l2_parent_number, ..Default::default() },
        l1_origin: BlockNumHash::default(),
        seq_num: 0,
    };
    let derived_from = BlockInfo { number: l1_origin_number, ..Default::default() };
    AttributesWithParent::new(BasePayloadAttributes::default(), parent, Some(derived_from), false)
}

fn finalized_l1(number: u64) -> BlockInfo {
    BlockInfo { number, ..Default::default() }
}

fn finalizer_with_entries() -> L2Finalizer {
    let mut finalizer = L2Finalizer::default();
    for number in 1..=ENTRY_COUNT {
        finalizer.enqueue_for_finalization(&attrs(number, number));
    }
    finalizer
}

fn bench_enqueue_for_finalization(c: &mut Criterion) {
    let mut group = c.benchmark_group("consensus_finalizer/enqueue_for_finalization");

    group.bench_function("4096_unique_l1_epochs", |b| {
        b.iter(|| {
            let mut finalizer = L2Finalizer::default();
            for number in 1..=ENTRY_COUNT {
                finalizer.enqueue_for_finalization(black_box(&attrs(number, number)));
            }
            black_box(finalizer);
        });
    });

    group.finish();
}

fn bench_try_finalize_next(c: &mut Criterion) {
    let mut group = c.benchmark_group("consensus_finalizer/try_finalize_next");

    let finalized_tip = finalized_l1(ENTRY_COUNT / 2);
    group.bench_function("4096_entries_finalize_half", |b| {
        b.iter_batched(
            finalizer_with_entries,
            |mut finalizer| black_box(finalizer.try_finalize_next(black_box(finalized_tip))),
            criterion::BatchSize::SmallInput,
        );
    });

    let old_tip = finalized_l1(0);
    group.bench_function("empty_queue_miss", |b| {
        b.iter_batched(
            L2Finalizer::default,
            |mut finalizer| black_box(finalizer.try_finalize_next(black_box(old_tip))),
            criterion::BatchSize::SmallInput,
        );
    });

    group.finish();
}

criterion_group!(benches, bench_enqueue_for_finalization, bench_try_finalize_next);
criterion_main!(benches);
