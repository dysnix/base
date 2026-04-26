//! Benchmarks for [`BatchEncoder::da_backlog_bytes`].

use std::{hint::black_box, sync::Arc};

use alloy_consensus::{BlockBody, Header, SignableTransaction, TxLegacy};
use alloy_primitives::{B256, Bytes, Sealed, Signature};
use base_batcher_encoder::{BatchEncoder, BatchPipeline, EncoderConfig};
use base_common_consensus::{BaseBlock, BaseTxEnvelope, TxDeposit};
use base_common_genesis::RollupConfig;
use base_protocol::{L1BlockInfoBedrock, L1BlockInfoTx};
use criterion::{Criterion, criterion_group, criterion_main};

const BLOCK_COUNT: usize = 4_096;
const USER_TXS_PER_BLOCK: usize = 8;
const USER_TX_INPUT_BYTES: usize = 512;

fn make_deposit_tx() -> BaseTxEnvelope {
    let calldata = L1BlockInfoTx::Bedrock(L1BlockInfoBedrock::default()).encode_calldata();
    BaseTxEnvelope::Deposit(Sealed::new(TxDeposit { input: calldata, ..Default::default() }))
}

fn make_user_tx(seed: u64) -> BaseTxEnvelope {
    let tx = TxLegacy {
        nonce: seed,
        input: Bytes::from(vec![seed as u8; USER_TX_INPUT_BYTES]),
        ..Default::default()
    };
    BaseTxEnvelope::Legacy(tx.into_signed(Signature::test_signature()))
}

fn make_block(parent_hash: B256, number: u64) -> BaseBlock {
    let mut transactions = Vec::with_capacity(1 + USER_TXS_PER_BLOCK);
    transactions.push(make_deposit_tx());
    transactions
        .extend((0..USER_TXS_PER_BLOCK).map(|offset| make_user_tx(number * 100 + offset as u64)));

    BaseBlock {
        header: Header { parent_hash, number, ..Default::default() },
        body: BlockBody { transactions, ..Default::default() },
    }
}

fn encoder_with_backlog(encoded_blocks: usize) -> BatchEncoder {
    let config = EncoderConfig { target_frame_size: usize::MAX / 4, ..EncoderConfig::default() };
    let mut encoder = BatchEncoder::new(Arc::new(RollupConfig::default()), config);

    let mut parent_hash = B256::ZERO;
    for number in 0..BLOCK_COUNT as u64 {
        let block = make_block(parent_hash, number);
        parent_hash = block.header.hash_slow();
        encoder.add_block(block).unwrap();
    }

    for _ in 0..encoded_blocks {
        encoder.step().unwrap();
    }

    encoder
}

fn bench_da_backlog_bytes(c: &mut Criterion) {
    let mut group = c.benchmark_group("batcher_encoder/da_backlog_bytes");
    group.sample_size(20);

    let full_backlog = encoder_with_backlog(0);
    group.bench_function("4096_blocks_pending", |b| {
        b.iter(|| {
            let encoder = black_box(&full_backlog);
            black_box(encoder.da_backlog_bytes());
        });
    });

    let partially_encoded = encoder_with_backlog(BLOCK_COUNT / 2);
    group.bench_function("4096_blocks_half_encoded", |b| {
        b.iter(|| {
            let encoder = black_box(&partially_encoded);
            black_box(encoder.da_backlog_bytes());
        });
    });

    group.finish();
}

criterion_group!(benches, bench_da_backlog_bytes);
criterion_main!(benches);
