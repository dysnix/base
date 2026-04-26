# Performance Autopilot Journal

This file records each autonomous performance run.

Entry format

## YYYY-MM-DD HH:MM UTC
Focus:
Hypothesis:
Commands:
Results:
Next:

## 2026-04-26 14:01 UTC
Focus: bootstrap performance autopilot
Hypothesis: the repo already contains enough service hotspots, metrics, load tests, and benchmark hooks to start an autonomous optimization loop without guessing blindly.
Commands:
- inspected consensus, batcher, zk service, and shared benchmarking infrastructure
- created dedicated worktree `/home/refcell/dev/base-perf-autopilot`
Results:
- consensus hotspots identified around sequencer build/seal, engine request processing, derivation, provider RPC/cache behavior, and gossip paths
- batcher hotspots identified around driver scheduling, encoder/compression, blob packing, recent-tx startup scan, and source polling/catchup
- zk hotspots identified around witness generation, status polling, repeated GetProof sync calls, session round-trips, and proxy rate limiting
- reusable benchmarking infrastructure exists in `crates/infra/load-tests` plus several criterion benches elsewhere in the repo
Next:
- start with the narrowest measurable improvement in one hotspot area, likely batcher encoder/submission or zk service polling behavior

## 2026-04-26 18:32 UTC
Focus: `base-batcher-encoder` DA backlog accounting in `BatchEncoder::da_backlog_bytes()`.
Hypothesis: the backlog getter is on the batcher throttle path and should not rescan every unencoded block/transaction; caching the pending DA bytes should turn reads from O(n) into O(1) while preserving exact behavior.
Commands:
- `cargo test -p base-batcher-encoder`
- `cargo bench -p base-batcher-encoder --bench da_backlog -- --warm-up-time 0.5 --measurement-time 1.5 --sample-size 20`
- `cargo clippy -p base-batcher-encoder --tests --benches -- -D warnings`
- `cargo fmt --all`
Results:
- added a cached `da_backlog_bytes: u64` field to `BatchEncoder`, updating it on `add_block`, successful single/span encoding, and `reset()` so `da_backlog_bytes()` is now O(1)
- added `test_da_backlog_cache_tracks_encoding_lifecycle` to verify cache correctness through add/encode/reset transitions; full crate tests pass (`60 passed`)
- added a new Criterion bench at `crates/batcher/encoder/benches/da_backlog.rs` covering `4096_blocks_pending` and `4096_blocks_half_encoded`
- fixed the bench harness so clippy passes and benchmark calls are not trivially constant-folded via `black_box`
- post-change benchmark results: `4096_blocks_pending` = `202.18 ps .. 203.75 ps`, `4096_blocks_half_encoded` = `201.22 ps .. 201.49 ps`, confirming constant-time reads regardless of backlog depth/encoded cursor position
Next:
- watch for any follow-up review feedback on whether additional encoder counters or a comparative regression benchmark against the old linear scan would be useful

## 2026-04-26 20:42 UTC
Focus: `base-consensus-node` derivation finalizer drain path in `L2Finalizer::try_finalize_next()`.
Hypothesis: when finalizing after a deep derivation backlog, draining finalized epochs with `BTreeMap::retain` does unnecessary full-map scanning; replacing it with `BTreeMap::split_off` should keep only future epochs in O(log n) tree work plus moved survivors, reducing the hot-path cost while preserving semantics.
Commands:
- `cargo bench -p base-consensus-node --bench finalizer -- --warm-up-time 0.5 --measurement-time 1.5 --sample-size 20`
- `cargo test -p base-consensus-node actors::derivation::finalizer:: -- --nocapture --test-threads=1`
- `cargo clippy -p base-consensus-node --tests --benches --no-deps -- -D warnings`
- `cargo fmt --all`
Results:
- added a new Criterion bench at `crates/consensus/service/benches/finalizer.rs` covering `enqueue_for_finalization`, `try_finalize_next` on a `4096`-entry queue, and an empty-queue miss case
- replaced the finalization drain in `L2Finalizer::try_finalize_next()` with a helper backed by `BTreeMap::split_off`, and handled `u64::MAX` explicitly to avoid overflow when computing the first surviving epoch
- added `max_l1_finalization_drains_without_overflow` to lock in the overflow edge case
- initial baseline bench before the code change measured `4096_entries_finalize_half` at `40.104 µs .. 42.834 µs`, `4096_unique_l1_epochs` at `107.75 µs .. 109.15 µs`, and `empty_queue_miss` at `4.5253 ns .. 4.7974 ns`
- post-change re-run measured `4096_entries_finalize_half` at `10.928 µs .. 14.483 µs`, roughly a 3-4x improvement on the drain path; `4096_unique_l1_epochs` stayed effectively flat at `103.65 µs .. 105.15 µs`, and `empty_queue_miss` stayed flat at `6.5262 ns .. 6.6165 ns` on the confirming run
- focused finalizer tests passed (`9 passed`)
- full `cargo clippy -p base-consensus-node --tests --benches -- -D warnings` is currently blocked by pre-existing lint failures in dependency crate `base-consensus-disc`, so validation used `--no-deps` and passed for the touched crate
Next:
- watch PR feedback on whether the finalizer bench should grow a larger survivor-heavy case (for example, finalizing 1 block out of a much larger queue) to characterize `split_off` behavior under different retained-tail sizes
