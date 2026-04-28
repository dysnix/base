//! Benchmarks for [`BatchReader`] constructor and decode paths.

use std::hint::black_box;

use alloy_primitives::{Bytes, hex};
use base_common_genesis::RollupConfig;
use base_protocol::BatchReader;
use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use miniz_oxide::{
    deflate::{CompressionLevel, compress_to_vec_zlib},
    inflate::decompress_to_vec_zlib,
};

const BATCH_COUNTS: [usize; 2] = [1, 64];

fn compressed_batch_fixture(batch_count: usize) -> (Bytes, usize) {
    let file_contents = String::from_utf8_lossy(include_bytes!("../testdata/batch.hex"));
    let file_contents = &file_contents[..file_contents.len() - 1];
    let raw = hex::decode(file_contents).expect("batch fixture must decode");
    let single_batch = decompress_to_vec_zlib(&raw).expect("batch fixture must decompress");

    let mut multi_batch = Vec::with_capacity(single_batch.len() * batch_count);
    for _ in 0..batch_count {
        multi_batch.extend_from_slice(&single_batch);
    }
    let max_rlp_bytes_per_channel = multi_batch.len();
    let compressed = compress_to_vec_zlib(&multi_batch, CompressionLevel::BestSpeed.into()).into();

    (compressed, max_rlp_bytes_per_channel)
}

fn decode_all_batches(mut reader: BatchReader, cfg: &RollupConfig) -> usize {
    let mut batch_count = 0;
    while reader.next_batch(cfg).is_some() {
        batch_count += 1;
    }
    batch_count
}

fn bench_batch_reader_constructor(c: &mut Criterion) {
    let mut group = c.benchmark_group("protocol/batch_reader/constructor");
    group.sample_size(20);

    for batch_count in BATCH_COUNTS {
        let (compressed, max_rlp_bytes_per_channel) = compressed_batch_fixture(batch_count);

        group.bench_with_input(
            BenchmarkId::new("baseline_vec_clone", batch_count),
            &compressed,
            |b, compressed| {
                b.iter_batched(
                    || compressed.clone(),
                    |data| {
                        black_box(BatchReader::new(
                            black_box(data).to_vec(),
                            black_box(max_rlp_bytes_per_channel),
                        ));
                    },
                    BatchSize::SmallInput,
                );
            },
        );

        group.bench_with_input(
            BenchmarkId::new("owned_bytes", batch_count),
            &compressed,
            |b, compressed| {
                b.iter_batched(
                    || compressed.clone(),
                    |data| {
                        black_box(BatchReader::new(
                            black_box(data),
                            black_box(max_rlp_bytes_per_channel),
                        ));
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

fn bench_batch_reader_decode_all_batches(c: &mut Criterion) {
    let mut group = c.benchmark_group("protocol/batch_reader/decode_all_batches");
    group.sample_size(20);

    let cfg = RollupConfig::default();
    for batch_count in BATCH_COUNTS {
        let (compressed, max_rlp_bytes_per_channel) = compressed_batch_fixture(batch_count);

        group.bench_with_input(
            BenchmarkId::new("baseline_vec_clone", batch_count),
            &compressed,
            |b, compressed| {
                b.iter_batched(
                    || compressed.clone(),
                    |data| {
                        black_box(decode_all_batches(
                            BatchReader::new(
                                black_box(data).to_vec(),
                                black_box(max_rlp_bytes_per_channel),
                            ),
                            black_box(&cfg),
                        ));
                    },
                    BatchSize::SmallInput,
                );
            },
        );

        group.bench_with_input(
            BenchmarkId::new("owned_bytes", batch_count),
            &compressed,
            |b, compressed| {
                b.iter_batched(
                    || compressed.clone(),
                    |data| {
                        black_box(decode_all_batches(
                            BatchReader::new(black_box(data), black_box(max_rlp_bytes_per_channel)),
                            black_box(&cfg),
                        ));
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_batch_reader_constructor, bench_batch_reader_decode_all_batches,);
criterion_main!(benches);
