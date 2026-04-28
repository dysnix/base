//! Benchmarks for [`BatchReader`] constructor, decompression, and decode paths.

use std::hint::black_box;

use alloy_primitives::{Bytes, hex};
use alloy_rlp::Decodable;
use base_common_genesis::{HardForkConfig, RollupConfig};
use base_protocol::{Batch, BatchReader, Brotli};
use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use miniz_oxide::{
    deflate::{CompressionLevel, compress_to_vec_zlib},
    inflate::{decompress_to_vec_zlib, decompress_to_vec_zlib_with_limit},
};

const BATCH_COUNTS: [usize; 2] = [1, 64];

#[derive(Clone)]
struct CompressionFixture {
    label: &'static str,
    compressed: Bytes,
    max_rlp_bytes_per_channel: usize,
}

fn decompressed_batch_fixture(batch_count: usize) -> Vec<u8> {
    let file_contents = String::from_utf8_lossy(include_bytes!("../testdata/batch.hex"));
    let file_contents = &file_contents[..file_contents.len() - 1];
    let raw = hex::decode(file_contents).expect("batch fixture must decode");
    let single_batch = decompress_to_vec_zlib(&raw).expect("batch fixture must decompress");

    let mut multi_batch = Vec::with_capacity(single_batch.len() * batch_count);
    for _ in 0..batch_count {
        multi_batch.extend_from_slice(&single_batch);
    }
    multi_batch
}

fn compressed_batch_fixture(batch_count: usize) -> (Bytes, usize) {
    let multi_batch = decompressed_batch_fixture(batch_count);
    let max_rlp_bytes_per_channel = multi_batch.len();
    let compressed = compress_to_vec_zlib(&multi_batch, CompressionLevel::BestSpeed.into()).into();

    (compressed, max_rlp_bytes_per_channel)
}

fn brotli_compressed_batch_fixture(batch_count: usize) -> (Bytes, usize) {
    let multi_batch = decompressed_batch_fixture(batch_count);
    let max_rlp_bytes_per_channel = multi_batch.len();

    let mut compressed = vec![BatchReader::CHANNEL_VERSION_BROTLI];
    let mut input = multi_batch.as_slice();
    let params = brotli::enc::BrotliEncoderParams::default();
    brotli::BrotliCompress(&mut input, &mut compressed, &params)
        .expect("batch fixture must brotli compress");

    (compressed.into(), max_rlp_bytes_per_channel)
}

fn compression_fixtures(batch_count: usize) -> [CompressionFixture; 2] {
    let (zlib_compressed, zlib_max_rlp_bytes_per_channel) = compressed_batch_fixture(batch_count);
    let (brotli_compressed, brotli_max_rlp_bytes_per_channel) =
        brotli_compressed_batch_fixture(batch_count);

    [
        CompressionFixture {
            label: "zlib",
            compressed: zlib_compressed,
            max_rlp_bytes_per_channel: zlib_max_rlp_bytes_per_channel,
        },
        CompressionFixture {
            label: "brotli",
            compressed: brotli_compressed,
            max_rlp_bytes_per_channel: brotli_max_rlp_bytes_per_channel,
        },
    ]
}

fn decode_all_batches(mut reader: BatchReader, cfg: &RollupConfig) -> usize {
    let mut batch_count = 0;
    while reader.next_batch(cfg).is_some() {
        batch_count += 1;
    }
    batch_count
}

fn decode_all_batches_from_decompressed(mut data: &[u8], cfg: &RollupConfig) -> usize {
    let mut batch_count = 0;

    while !data.is_empty() {
        let Ok(bytes) = Bytes::decode(&mut data) else {
            break;
        };
        let Ok(_) = Batch::decode(&mut bytes.as_ref(), cfg) else {
            break;
        };
        batch_count += 1;
    }

    batch_count
}

fn batch_payloads_from_decompressed(mut data: &[u8]) -> Vec<Bytes> {
    let mut batch_payloads = Vec::new();

    while !data.is_empty() {
        let bytes = Bytes::decode(&mut data).expect("decompressed fixture must decode bytes");
        batch_payloads.push(bytes);
    }

    batch_payloads
}

fn count_rlp_wrapped_batches(mut data: &[u8]) -> usize {
    let mut batch_count = 0;

    while !data.is_empty() {
        let Ok(_) = Bytes::decode(&mut data) else {
            break;
        };
        batch_count += 1;
    }

    batch_count
}

fn decode_all_batch_payloads(batch_payloads: &[Bytes], cfg: &RollupConfig) -> usize {
    let mut batch_count = 0;

    for payload in batch_payloads {
        let Ok(_) = Batch::decode(&mut payload.as_ref(), cfg) else {
            break;
        };
        batch_count += 1;
    }

    batch_count
}

fn bench_rollup_config(label: &'static str) -> RollupConfig {
    match label {
        "brotli" => RollupConfig {
            hardforks: HardForkConfig { fjord_time: Some(0), ..Default::default() },
            ..Default::default()
        },
        "zlib" => RollupConfig::default(),
        unsupported => panic!("unsupported compression label: {unsupported}"),
    }
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

fn bench_batch_reader_decompression_only(c: &mut Criterion) {
    let mut group = c.benchmark_group("protocol/batch_reader/decompression_only");
    group.sample_size(20);

    for batch_count in BATCH_COUNTS {
        let [zlib_fixture, brotli_fixture] = compression_fixtures(batch_count);

        group.bench_with_input(
            BenchmarkId::new("zlib", batch_count),
            &zlib_fixture,
            |b, fixture| {
                b.iter_batched(
                    || fixture.compressed.clone(),
                    |data| {
                        black_box(
                            decompress_to_vec_zlib_with_limit(
                                black_box(data).as_ref(),
                                black_box(fixture.max_rlp_bytes_per_channel),
                            )
                            .expect("zlib fixture must decompress"),
                        );
                    },
                    BatchSize::SmallInput,
                );
            },
        );

        group.bench_with_input(
            BenchmarkId::new("brotli", batch_count),
            &brotli_fixture,
            |b, fixture| {
                b.iter_batched(
                    || fixture.compressed.clone(),
                    |data| {
                        black_box(
                            Brotli
                                .decompress(
                                    black_box(&data[1..]),
                                    black_box(fixture.max_rlp_bytes_per_channel),
                                )
                                .expect("brotli fixture must decompress"),
                        );
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

    for batch_count in BATCH_COUNTS {
        for fixture in compression_fixtures(batch_count) {
            let cfg = bench_rollup_config(fixture.label);
            group.bench_with_input(
                BenchmarkId::new(format!("baseline_vec_clone_{}", fixture.label), batch_count),
                &fixture,
                |b, fixture| {
                    b.iter_batched(
                        || fixture.compressed.clone(),
                        |data| {
                            black_box(decode_all_batches(
                                BatchReader::new(
                                    black_box(data).to_vec(),
                                    black_box(fixture.max_rlp_bytes_per_channel),
                                ),
                                black_box(&cfg),
                            ));
                        },
                        BatchSize::SmallInput,
                    );
                },
            );

            group.bench_with_input(
                BenchmarkId::new(format!("owned_bytes_{}", fixture.label), batch_count),
                &fixture,
                |b, fixture| {
                    b.iter_batched(
                        || fixture.compressed.clone(),
                        |data| {
                            black_box(decode_all_batches(
                                BatchReader::new(
                                    black_box(data),
                                    black_box(fixture.max_rlp_bytes_per_channel),
                                ),
                                black_box(&cfg),
                            ));
                        },
                        BatchSize::SmallInput,
                    );
                },
            );
        }
    }

    group.finish();
}

fn bench_batch_reader_post_decompression_decode_only(c: &mut Criterion) {
    let mut group = c.benchmark_group("protocol/batch_reader/post_decompression_decode_only");
    group.sample_size(20);

    for batch_count in BATCH_COUNTS {
        for fixture in compression_fixtures(batch_count) {
            let cfg = bench_rollup_config(fixture.label);
            let decompressed = decompressed_batch_fixture(batch_count);

            group.bench_with_input(
                BenchmarkId::new(fixture.label, batch_count),
                &decompressed,
                |b, decompressed| {
                    b.iter_batched(
                        || decompressed.clone(),
                        |data| {
                            black_box(decode_all_batches_from_decompressed(
                                black_box(data).as_slice(),
                                black_box(&cfg),
                            ));
                        },
                        BatchSize::SmallInput,
                    );
                },
            );
        }
    }

    group.finish();
}

fn bench_batch_reader_post_decompression_components(c: &mut Criterion) {
    let mut group = c.benchmark_group("protocol/batch_reader/post_decompression_components");
    group.sample_size(20);

    for batch_count in BATCH_COUNTS {
        for fixture in compression_fixtures(batch_count) {
            let cfg = bench_rollup_config(fixture.label);
            let decompressed = decompressed_batch_fixture(batch_count);
            let batch_payloads = batch_payloads_from_decompressed(decompressed.as_slice());

            group.bench_with_input(
                BenchmarkId::new(format!("rlp_only_{}", fixture.label), batch_count),
                &decompressed,
                |b, decompressed| {
                    b.iter_batched(
                        || decompressed.clone(),
                        |data| {
                            black_box(count_rlp_wrapped_batches(black_box(data).as_slice()));
                        },
                        BatchSize::SmallInput,
                    );
                },
            );

            group.bench_with_input(
                BenchmarkId::new(format!("batch_decode_only_{}", fixture.label), batch_count),
                &batch_payloads,
                |b, batch_payloads| {
                    b.iter(|| {
                        black_box(decode_all_batch_payloads(
                            black_box(batch_payloads.as_slice()),
                            black_box(&cfg),
                        ));
                    });
                },
            );
        }
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_batch_reader_constructor,
    bench_batch_reader_decompression_only,
    bench_batch_reader_decode_all_batches,
    bench_batch_reader_post_decompression_decode_only,
    bench_batch_reader_post_decompression_components,
);
criterion_main!(benches);
