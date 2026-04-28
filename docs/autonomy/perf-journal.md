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

## 2026-04-26 22:49 UTC
Focus: `base-consensus-node` finalizer benchmarking coverage for survivor-heavy drain cases in `L2Finalizer::try_finalize_next()`.
Hypothesis: the prior finalizer bench only measured the half-drain case, leaving a gap for the likely worst retained-tail shape after the `split_off` optimization; adding a `finalize_first` benchmark should quantify the cost when almost the entire queue survives.
Commands:
- `cargo bench -p base-consensus-node --bench finalizer -- --warm-up-time 0.5 --measurement-time 1.5 --sample-size 20`
- edited `crates/consensus/service/benches/finalizer.rs` to add `4096_entries_finalize_first`
- `cargo bench -p base-consensus-node --bench finalizer -- --warm-up-time 0.5 --measurement-time 1.5 --sample-size 20`
- `cargo test -p base-consensus-node actors::derivation::finalizer:: -- --nocapture --test-threads=1`
- `cargo clippy -p base-consensus-node --tests --benches --no-deps -- -D warnings`
- `cargo fmt --all`
Results:
- baseline before the bench edit confirmed the existing post-optimization behavior: `4096_entries_finalize_half` = `11.155 µs .. 14.292 µs`, `4096_unique_l1_epochs` = `105.13 µs .. 107.81 µs`, `empty_queue_miss` = `6.5632 ns .. 6.8584 ns`
- added a new survivor-heavy Criterion case, `4096_entries_finalize_first`, without changing production logic
- the new benchmark measured `4096_entries_finalize_first` at `10.206 µs .. 11.149 µs`, showing the `split_off` drain remains in the same low-`10 µs` band even when `4095/4096` entries survive
- confirming run kept `4096_entries_finalize_half` in the same range at `11.880 µs .. 16.408 µs`; `empty_queue_miss` stayed flat at `6.5506 ns .. 6.8324 ns`
- focused finalizer tests still passed (`9 passed`)
- `cargo clippy -p base-consensus-node --tests --benches --no-deps -- -D warnings` passed again; full clippy without `--no-deps` remains blocked by pre-existing dependency lints outside the touched crate
Next:
- if the finalizer is revisited, add a larger matrix of retained-tail sizes (for example finalize-at-1, finalize-at-1/4, finalize-at-1/2, finalize-at-3/4) to characterize how `split_off` scales with survivor count and to catch future regressions in queue-shape sensitivity

## 2026-04-27 01:10 UTC
Focus: `base-batcher-service` recent transaction startup scan in `RecentTxScanner::highest_submitted_l2_block()`.
Hypothesis: the startup scan should not rescan every buffered channel after each L1 block when only channels touched by frames in the current block can transition to ready; restricting the drain to touched channel IDs should reduce unnecessary map scans, especially when the buffered channel set is large and the current block only advances a small subset.
Commands:
- `cargo test -p base-batcher-service recent_txs -- --nocapture`
- `cargo bench -p base-batcher-service --bench recent_txs -- --warm-up-time 0.5 --measurement-time 1.5 --sample-size 20`
- added a focused Criterion bench at `crates/batcher/service/benches/recent_txs.rs`
- changed `RecentTxScanner` to track per-block `touched_channel_ids` and drain only those ready channels
- added `drain_ready_channels_only_checks_touched_ids`
- `cargo bench -p base-batcher-service --bench recent_txs -- --warm-up-time 0.5 --measurement-time 1.0 --sample-size 15`
- `cargo clippy -p base-batcher-service --tests --benches --no-deps -- -D warnings`
Results:
- extracted two public benchmark hooks: `drain_ready_channels` for the touched-only path and `drain_all_ready_channels` for the old full-scan baseline, without changing `decode_channel` behavior
- replaced the per-block `channels.iter().filter(|(_, ch)| ch.is_ready())` full scan with touched-ID draining in `highest_submitted_l2_block()`
- added a regression test proving untouched ready channels remain buffered, touched incomplete channels stay buffered, and touched ready channels are decoded and removed when their ID is supplied
- added `criterion` bench coverage for three shapes: fully touched mixed channels, fully touched incomplete channels, and sparse touched channels against a larger buffered set
- validation passed: focused tests `6 passed`; `cargo clippy -p base-batcher-service --tests --benches --no-deps -- -D warnings` passed
- benchmark results after the change:
  - `baseline_scan_all_with_4096_ready_and_4096_incomplete`: `7.8886 ms .. 8.4072 ms`
  - `4096_touched_ready_among_8192_channels`: `7.8312 ms .. 8.2868 ms` (effectively flat because ready-channel decode dominates)
  - `baseline_scan_all_with_8192_incomplete`: `391.27 µs .. 672.34 µs`
  - `4096_touched_incomplete_among_8192_channels`: `427.93 µs .. 511.84 µs` (same rough band)
  - `baseline_scan_all_with_64_touched_ready_among_8192_channels`: `513.58 µs .. 720.29 µs`
  - `64_touched_ready_among_8192_channels`: `507.07 µs .. 833.40 µs` (noisy, effectively flat)
  - `baseline_scan_all_with_64_touched_incomplete_among_8192_channels`: `396.74 µs .. 424.37 µs`
  - `64_touched_incomplete_among_8192_channels`: `366.17 µs .. 382.39 µs` (~8-10% lower in the sparse no-decode case, matching the intended avoided-scan scenario)
Next:
- if this path is revisited, grow the benchmark matrix around frame fan-out and touched-ID cardinality so the crossover point between touched-only draining and full-map scanning is explicit, especially for startup scans with many buffered but mostly untouched channels

## 2026-04-27 03:21 UTC
Focus: `base-batcher-service` touched-channel deduplication inside `RecentTxScanner::highest_submitted_l2_block()`.
Hypothesis: after moving the startup scan to touched-only draining, per-block deduplication via `Vec::contains` is still O(k²) in the number of parsed frames; replacing it with a small reusable tracker backed by `HashSet` + ordered `Vec` should materially reduce the bookkeeping cost for high fan-out blocks without changing drain semantics.
Commands:
- `cargo bench -p base-batcher-service --bench recent_txs -- --warm-up-time 0.5 --measurement-time 1.0 --sample-size 15`
- edited `crates/batcher/service/src/recent_txs.rs` to add `TouchedChannelTracker` and use it from `highest_submitted_l2_block()`
- extended `crates/batcher/service/benches/recent_txs.rs` with comparative touched-ID tracking microbenches for the old `Vec::contains` path vs. the new tracker
- `cargo fmt --all`
- `cargo test -p base-batcher-service recent_txs -- --nocapture`
- `cargo bench -p base-batcher-service --bench recent_txs -- --warm-up-time 0.5 --measurement-time 1.0 --sample-size 15`
- `cargo clippy -p base-batcher-service --tests --benches --no-deps -- -D warnings`
Results:
- added a public `TouchedChannelTracker` type, re-exported from `lib.rs`, that preserves first-seen channel order while deduplicating in O(1)-average membership checks via `HashSet`
- switched `highest_submitted_l2_block()` from ad hoc `Vec::contains` deduplication to `TouchedChannelTracker::record`, keeping the touched-only drain API unchanged
- added `touched_channel_tracker_deduplicates_and_preserves_first_seen_order` to lock in ordering and dedup behavior alongside the existing drain regression test
- added focused Criterion coverage that compares the old vector scan against the new tracker for `4096` unique touched IDs and `4096` frames spread across `512` unique channel IDs
- validation passed: focused tests `7 passed`; `cargo clippy -p base-batcher-service --tests --benches --no-deps -- -D warnings` passed
- touched-ID tracking benchmark results after the change:
  - `baseline_vec_scan_4096_unique_frame_channel_ids`: `1.9804 ms .. 2.0371 ms`
  - `hashset_tracker_4096_unique_frame_channel_ids`: `46.993 µs .. 47.127 µs` (~42x lower)
  - `baseline_vec_scan_4096_frames_across_512_unique_channel_ids`: `272.70 µs .. 278.45 µs`
  - `hashset_tracker_4096_frames_across_512_unique_channel_ids`: `43.241 µs .. 43.367 µs` (~6.3x lower)
- existing drain-path benches stayed in the same rough bands as the prior run, so this iteration primarily improved per-block touched-ID bookkeeping rather than decode-heavy channel draining
Next:
- if startup scan latency is revisited again, add an end-to-end block-parsing bench that combines frame parsing, touched-ID tracking, and touched-only draining so the next iteration can quantify where bookkeeping stops mattering relative to channel decode cost

## 2026-04-27 05:28 UTC
Focus: `base-batcher-service` per-block tracker reuse inside `RecentTxScanner::highest_submitted_l2_block()`.
Hypothesis: after replacing touched-channel deduplication with `TouchedChannelTracker`, the startup scan still allocates a fresh tracker per L1 block; reusing one tracker across blocks should shave a little more bookkeeping overhead off the scan without changing touched-ID order or drain semantics.
Commands:
- `cargo bench -p base-batcher-service --bench recent_txs -- --warm-up-time 0.5 --measurement-time 1.0 --sample-size 15`
- edited `crates/batcher/service/src/recent_txs.rs` to let `TouchedChannelTracker` clear/reset its storage and reused a single tracker across the scan loop
- extended `crates/batcher/service/benches/recent_txs.rs` with comparative microbenches for fresh-allocating vs. reused tracker bookkeeping
- `cargo fmt --all`
- `cargo test -p base-batcher-service recent_txs -- --nocapture`
- `cargo bench -p base-batcher-service --bench recent_txs -- --warm-up-time 0.5 --measurement-time 1.0 --sample-size 15`
- `cargo clippy -p base-batcher-service --tests --benches --no-deps -- -D warnings`
Results:
- added `TouchedChannelTracker::clear` and `reset_with_capacity`, then moved `highest_submitted_l2_block()` to reuse one tracker across the whole block scan instead of allocating a fresh `HashSet` + `Vec` pair for every fetched L1 block
- added `touched_channel_tracker_reset_allows_reuse_after_clear` to lock in reuse semantics and prove dedup/order behavior survives resets; focused tests now pass (`8 passed`)
- added two new Criterion cases to compare the old fresh-allocation tracker path against the reused tracker path directly
- end-to-end drain-path benches stayed noisy and effectively flat, which matches prior runs that showed decode work dominates there
- comparative bookkeeping microbench results on the confirming run:
  - `hashset_tracker_4096_unique_frame_channel_ids`: `48.533 µs .. 48.898 µs`
  - `reused_hashset_tracker_4096_unique_frame_channel_ids`: `48.225 µs .. 48.270 µs` (~0.9% lower median)
  - `hashset_tracker_4096_frames_across_512_unique_channel_ids`: `44.627 µs .. 44.936 µs`
  - `reused_hashset_tracker_4096_frames_across_512_unique_channel_ids`: `44.311 µs .. 44.498 µs` (~0.8% lower median)
- `cargo clippy -p base-batcher-service --tests --benches --no-deps -- -D warnings` passed
Next:
- if the startup scan is revisited again, add a block-level benchmark that includes frame parsing plus touched-ID tracking so allocator reuse can be measured in a shape closer to real RPC-fetched blocks, not just the isolated tracker microbench

## 2026-04-27 07:36 UTC
Focus: `base-batcher-service` ready-channel lookup cost inside `RecentTxScanner::drain_ready_channels()`.
Hypothesis: after touched-only draining and touched-ID tracker improvements, the remaining per-channel bookkeeping might still benefit from avoiding the separate `HashMap::get` + `HashMap::remove` sequence; a focused lookup microbench should show whether an `entry`-based path is worth pursuing before touching production code.
Commands:
- `cargo bench -p base-batcher-service --bench recent_txs -- --warm-up-time 0.5 --measurement-time 1.0 --sample-size 15`
- temporarily changed `RecentTxScanner::drain_ready_channels()` to use `HashMap::entry` and re-ran focused validation/benchmarks
- reverted the production change after measurement showed no win
- extended `crates/batcher/service/benches/recent_txs.rs` with a new `ready_channel_lookup` Criterion group that compares `get`-based readiness checks against `entry`-based lookups on the touched-only path
- `cargo fmt --all`
- `cargo test -p base-batcher-service recent_txs -- --nocapture`
- `cargo clippy -p base-batcher-service --tests --benches --no-deps -- -D warnings`
- `cargo bench -p base-batcher-service --bench recent_txs -- --warm-up-time 0.5 --measurement-time 1.0 --sample-size 15`
Results:
- added benchmark-only coverage for the ready-check bookkeeping gap without changing production logic; the bench file is now the only code delta for this run
- the focused lookup microbench showed `entry` is not a clear win here and is often worse on the important no-ready and sparse-ready shapes, so the production `get` + `remove` implementation was intentionally left unchanged
- confirming lookup results with the new benchmark group:
  - `baseline_get_4096_touched_incomplete_among_8192_channels`: `411.61 µs .. 447.80 µs`
  - `entry_api_4096_touched_incomplete_among_8192_channels`: `441.73 µs .. 593.54 µs` (slower / noisier)
  - `baseline_get_64_touched_ready_among_8192_channels`: `395.70 µs .. 751.97 µs`
  - `entry_api_64_touched_ready_among_8192_channels`: `378.14 µs .. 418.30 µs` (not enough evidence of a durable win given the baseline noise)
- existing drain-path benches remained in the same rough bands, with decode-heavy cases still dominating end-to-end startup scan cost
- focused tests still passed (`8 passed`) and `cargo clippy -p base-batcher-service --tests --benches --no-deps -- -D warnings` passed
Next:
- if this startup scan path is revisited again, benchmark frame parsing plus touched-only draining together so the next candidate optimization is chosen from a more end-to-end block-level profile instead of another tiny lookup tweak

## 2026-04-27 09:48 UTC
Focus: `base-batcher-service` recent-tx startup scan block-level benchmarking coverage.
Hypothesis: the existing microbenches proved touched-ID bookkeeping wins in isolation, but the next useful measurement layer is a block-level harness that includes version-0 frame parsing, channel mutation, and draining together; this should show whether the touched-only startup-scan changes still matter once more realistic per-block work is included.
Commands:
- `cargo bench -p base-batcher-service --bench recent_txs -- --warm-up-time 0.5 --measurement-time 1.0 --sample-size 15`
- extended `crates/batcher/service/benches/recent_txs.rs` with a new `process_block` Criterion group that replays encoded version-0 frame payloads into the same `Frame::parse_frames` + channel update + drain flow used by `RecentTxScanner`
- `cargo fmt --all`
- `cargo bench -p base-batcher-service --bench recent_txs -- --warm-up-time 0.5 --measurement-time 1.0 --sample-size 15`
- `cargo test -p base-batcher-service recent_txs -- --nocapture`
- `cargo clippy -p base-batcher-service --tests --benches --no-deps -- -D warnings`
Results:
- added benchmark-only helpers that synthesize encoded transaction payloads and compare the old per-block `Vec::contains` + full-map drain strategy against the current tracker-backed touched-only drain path without requiring RPC I/O
- the new `process_block` group closes the gap between tiny bookkeeping microbenches and the noisier end-to-end drain benchmarks by measuring parsing, channel insertion, touched-ID tracking, and draining together on a single fixture block
- new block-level benchmark results:
  - `baseline_vec_scan_all_4096_ready_unique_channels_from_empty`: `9.5927 ms .. 9.6277 ms`
  - `tracker_touched_only_4096_ready_unique_channels_from_empty`: `7.6659 ms .. 7.8739 ms` (~19% lower median)
  - `baseline_vec_scan_all_64_incomplete_touches_among_8192_buffered_channels`: `24.384 µs .. 25.826 µs`
  - `tracker_touched_only_64_incomplete_touches_among_8192_buffered_channels`: `7.2882 µs .. 10.650 µs` (roughly 2.5-3x lower despite some noise)
- this confirms the prior touched-only + tracker work still produces a measurable win once frame parsing and channel mutation are included, even though the decode-heavy drain benchmarks remain noisy on some shapes
- focused validation passed again: `cargo test -p base-batcher-service recent_txs -- --nocapture` (`8 passed`) and `cargo clippy -p base-batcher-service --tests --benches --no-deps -- -D warnings`
Next:
- if this path is revisited again, add a multi-block benchmark that reuses the same tracker across successive synthetic L1 blocks so allocator reuse and survivor-heavy buffered-channel sets can be measured in a shape even closer to the real startup scan loop

## 2026-04-27 11:57 UTC
Focus: `base-batcher-service` recent-tx startup scan multi-block benchmarking coverage for tracker reuse.
Hypothesis: the prior single-block harness still hid most of the small tracker-reuse benefit; a multi-block benchmark that keeps the buffered channel map alive across several synthetic L1 blocks should make the fresh-vs-reused tracker choice measurable in a shape closer to the real startup scan loop.
Commands:
- `cargo bench -p base-batcher-service --bench recent_txs -- --warm-up-time 0.5 --measurement-time 1.0 --sample-size 15`
- extended `crates/batcher/service/benches/recent_txs.rs` with new multi-block payload builders plus a `process_blocks` Criterion group that compares fresh tracker allocation per block against `TouchedChannelTracker::reset_with_capacity` reuse across eight successive synthetic blocks
- `cargo test -p base-batcher-service recent_txs -- --nocapture`
- `cargo clippy -p base-batcher-service --tests --benches --no-deps -- -D warnings`
- `cargo bench -p base-batcher-service --bench recent_txs -- --warm-up-time 0.5 --measurement-time 1.0 --sample-size 15`
Results:
- kept production logic unchanged and added benchmark-only coverage for the exact next gap identified in the previous run: persistent buffered channels across multiple synthetic blocks with touched-only draining after each block
- the new `process_blocks` group isolates tracker lifecycle cost better than the single-block harness by letting channels persist while replaying `8 × 4096` incomplete touches, which is closer to a survivor-heavy startup scan
- new multi-block benchmark results:
  - `fresh_tracker_8_blocks_4096_incomplete_touches_each_among_persistent_channels`: `3.8344 ms .. 4.2619 ms`
  - `reused_tracker_8_blocks_4096_incomplete_touches_each_among_persistent_channels`: `3.7320 ms .. 3.8966 ms` (~6.4% lower median)
- confirming run also kept the earlier block-level signal intact: `baseline_vec_scan_all_4096_ready_unique_channels_from_empty` = `9.5838 ms .. 9.7675 ms`, `tracker_touched_only_4096_ready_unique_channels_from_empty` = `7.6493 ms .. 7.8187 ms`
- focused validation passed again: `cargo test -p base-batcher-service recent_txs -- --nocapture` (`8 passed`) and `cargo clippy -p base-batcher-service --tests --benches --no-deps -- -D warnings`
Next:
- if this startup-scan path is revisited again, extend the multi-block harness with channels that become ready only on later blocks so the benchmark covers both tracker reuse and the eventual decode/drain transition inside a persistent survivor-heavy channel set

## 2026-04-27 14:07 UTC
Focus: `base-batcher-service` recent-tx startup scan delayed-ready multi-block benchmark coverage.
Hypothesis: the prior multi-block harness only measured incomplete survivor-heavy channels, so it still underrepresented the moment when touched channels finally become ready and trigger decode/drain work; a delayed-ready multi-block fixture should better capture the durable benefit of touched-only draining once channels finish on a later block.
Commands:
- `cargo bench -p base-batcher-service --bench recent_txs process_blocks -- --warm-up-time 0.5 --measurement-time 1.0 --sample-size 15`
- extended `crates/batcher/service/benches/recent_txs.rs` with `split_frame_data_across_blocks`, `multi_block_ready_transition_tx_payloads`, `process_blocks_with_vec_tracking_and_full_scan`, and a new `process_blocks_ready_transition` Criterion group
- `cargo fmt --all`
- `cargo bench -p base-batcher-service --bench recent_txs process_blocks_ready_transition -- --warm-up-time 0.5 --measurement-time 1.0 --sample-size 15`
- `cargo test -p base-batcher-service recent_txs -- --nocapture`
- `cargo clippy -p base-batcher-service --tests --benches --no-deps -- -D warnings`
Results:
- the pre-change check confirmed the earlier incomplete-only `process_blocks` harness still does not show a durable tracker-reuse win on this machine (`fresh_tracker_8_blocks_4096_incomplete_touches_each_among_persistent_channels`: `3.4449 ms .. 3.7434 ms`; `reused_tracker_8_blocks_4096_incomplete_touches_each_among_persistent_channels`: `3.8477 ms .. 4.3095 ms`)
- added benchmark-only delayed-ready coverage where `1024` channels receive four split frames across four synthetic blocks and only become ready on the final block, keeping production logic unchanged
- the new delayed-ready harness showed the touched-only path still materially beats the old full-map drain once readiness and decode happen later in a persistent channel set:
  - `baseline_vec_scan_all_4_blocks_1024_channels_ready_on_final_block`: `2.6940 ms .. 2.7362 ms`
  - `fresh_tracker_4_blocks_1024_channels_ready_on_final_block`: `2.3213 ms .. 2.3388 ms` (~14.1% lower median)
  - `reused_tracker_4_blocks_1024_channels_ready_on_final_block`: `2.3446 ms .. 2.3634 ms` (~12.8% lower median, slightly slower than fresh allocation in this shape)
- focused validation passed again: `cargo test -p base-batcher-service recent_txs -- --nocapture` (`8 passed`) and `cargo clippy -p base-batcher-service --tests --benches --no-deps -- -D warnings`
Next:
- if this path is revisited again, vary the delayed-ready matrix (for example channel count, block count, and percentage of channels completing on each block) so the crossover between decode cost, touched-only draining, and tracker reuse is explicit before attempting another production optimization

## 2026-04-27 16:13 UTC
Focus: `base-batcher-service` recent-tx startup scan staggered-ready multi-block benchmark coverage.
Hypothesis: the prior delayed-ready harness forced every channel to complete on the same final block, which still compressed all decode/drain work into one step; a staggered-ready fixture where channels complete across successive blocks should better expose whether touched-only draining keeps its advantage once readiness is distributed over time and whether tracker reuse matters more in that incremental-completion shape.
Commands:
- `cargo bench -p base-batcher-service --bench recent_txs process_blocks_ready_transition -- --warm-up-time 0.5 --measurement-time 1.0 --sample-size 15`
- extended `crates/batcher/service/benches/recent_txs.rs` with `multi_block_staggered_ready_tx_payloads` and a new `process_blocks_staggered_ready` Criterion group
- `cargo fmt --all`
- `cargo bench -p base-batcher-service --bench recent_txs process_blocks_staggered_ready -- --warm-up-time 0.5 --measurement-time 1.0 --sample-size 15`
- `cargo test -p base-batcher-service recent_txs -- --nocapture`
- `cargo clippy -p base-batcher-service --tests --benches --no-deps -- -D warnings`
Results:
- re-ran the existing delayed-ready harness before editing the bench file to refresh the baseline shape on this machine: `baseline_vec_scan_all_4_blocks_1024_channels_ready_on_final_block` = `2.6685 ms .. 2.6910 ms`, `fresh_tracker_4_blocks_1024_channels_ready_on_final_block` = `2.3085 ms .. 2.3116 ms`, and `reused_tracker_4_blocks_1024_channels_ready_on_final_block` = `2.3507 ms .. 2.3932 ms`
- added benchmark-only staggered-ready coverage where `1024` channels are evenly distributed across four completion blocks (`25%` become ready on each block), keeping production logic unchanged
- the new staggered-ready harness showed the touched-only path still beats the old full-map scan when readiness is spread over time:
  - `baseline_vec_scan_all_4_blocks_1024_channels_ready_in_quarters`: `2.2951 ms .. 2.3111 ms`
  - `fresh_tracker_4_blocks_1024_channels_ready_in_quarters`: `2.1147 ms .. 2.1479 ms` (~7.4% lower median)
  - `reused_tracker_4_blocks_1024_channels_ready_in_quarters`: `2.1165 ms .. 2.1321 ms` (~7.7% lower median, effectively tied with fresh allocation and only ~0.3% lower median)
- focused validation passed again: `cargo test -p base-batcher-service recent_txs -- --nocapture` (`8 passed`) and `cargo clippy -p base-batcher-service --tests --benches --no-deps -- -D warnings`
Next:
- if this startup-scan path is revisited again, extend the readiness matrix again (for example front-loaded, back-loaded, and mixed completion ratios) to map the crossover between incremental decode cost and touched-only drain savings before attempting any further production optimization

## 2026-04-27 18:19 UTC
Focus: `base-batcher-service` recent-tx startup scan weighted readiness benchmark coverage.
Hypothesis: the prior delayed/staggered multi-block fixtures showed touched-only draining still wins when readiness is distributed across blocks, but they did not distinguish whether the win is stronger when channels complete early versus late; adding front-loaded and back-loaded readiness matrices should make that crossover explicit before any further production optimization.
Commands:
- `cargo bench -p base-batcher-service --bench recent_txs process_blocks_staggered_ready -- --warm-up-time 0.5 --measurement-time 1.0 --sample-size 15`
- extended `crates/batcher/service/benches/recent_txs.rs` with `multi_block_weighted_ready_tx_payloads`, front-loaded/back-loaded readiness distributions, and a new `process_blocks_weighted_ready` Criterion group
- `cargo fmt --all`
- `cargo bench -p base-batcher-service --bench recent_txs process_blocks_weighted_ready -- --warm-up-time 0.5 --measurement-time 1.0 --sample-size 15`
- `cargo test -p base-batcher-service recent_txs -- --nocapture`
- `cargo clippy -p base-batcher-service --tests --benches --no-deps -- -D warnings`
Results:
- refreshed the even staggered-ready baseline before editing the bench file: `baseline_vec_scan_all_4_blocks_1024_channels_ready_in_quarters` = `2.2775 ms .. 2.3121 ms`, `fresh_tracker_4_blocks_1024_channels_ready_in_quarters` = `2.1231 ms .. 2.1526 ms`, and `reused_tracker_4_blocks_1024_channels_ready_in_quarters` = `2.1196 ms .. 2.1639 ms`, confirming the touched-only path still holds an about `6.8%` win when completions are evenly distributed
- added benchmark-only weighted readiness coverage where `1024` channels complete across four blocks in front-loaded (`512/256/128/128`) and back-loaded (`128/128/256/512`) distributions, keeping production logic unchanged
- the new weighted harness shows touched-only draining helps in both shapes, with a larger gain when completion is back-loaded:
  - front-loaded: `baseline_vec_scan_all_front_loaded_4_blocks_1024_channels` = `2.1548 ms .. 2.1787 ms`, `fresh_tracker_front_loaded_4_blocks_1024_channels` = `2.0488 ms .. 2.0856 ms` (~`4.7%` lower median), `reused_tracker_front_loaded_4_blocks_1024_channels` = `2.0433 ms .. 2.0741 ms` (~`4.8%` lower median)
  - back-loaded: `baseline_vec_scan_all_back_loaded_4_blocks_1024_channels` = `2.4837 ms .. 2.5261 ms`, `fresh_tracker_back_loaded_4_blocks_1024_channels` = `2.2126 ms .. 2.2628 ms` (~`10.6%` lower median), `reused_tracker_back_loaded_4_blocks_1024_channels` = `2.2077 ms .. 2.2229 ms` (~`11.5%` lower median)
- this makes the crossover clearer: touched-only draining buys more once a larger survivor-heavy channel set persists into later blocks, while tracker reuse remains effectively tied with fresh allocation and at most a small secondary factor
- focused validation passed again: `cargo test -p base-batcher-service recent_txs -- --nocapture` (`8 passed`) and `cargo clippy -p base-batcher-service --tests --benches --no-deps -- -D warnings`
Next:
- if this startup-scan path is revisited again, add a mixed readiness matrix with uneven per-block touch counts (not just completion counts) so the next experiment can separate savings from survivor-heavy draining versus savings from fewer per-block frame parses

## 2026-04-27 20:25 UTC
Focus: `base-batcher-service` recent-tx startup scan mixed readiness benchmark coverage with uneven touch-start timing.
Hypothesis: the weighted readiness matrix still starts every channel on block 0, so it conflates survivor-heavy drain savings with the cost of touching every channel on every earlier block; a cohort-based mixed fixture where some channels first appear on later blocks should separate those effects before any more production changes.
Commands:
- `cargo bench -p base-batcher-service --bench recent_txs process_blocks_weighted_ready -- --warm-up-time 0.5 --measurement-time 1.0 --sample-size 15`
- extended `crates/batcher/service/benches/recent_txs.rs` with `ReadyTransitionCohort`, `multi_block_cohort_ready_tx_payloads`, and a new `process_blocks_mixed_ready` Criterion group
- `cargo fmt --all`
- `cargo bench -p base-batcher-service --bench recent_txs process_blocks_mixed_ready -- --warm-up-time 0.5 --measurement-time 1.0 --sample-size 15`
- `cargo test -p base-batcher-service recent_txs -- --nocapture`
- `cargo clippy -p base-batcher-service --tests --benches --no-deps -- -D warnings`
Results:
- refreshed the weighted-ready baseline before editing the bench file: front-loaded median `2.2425 ms` baseline vs `2.0634 ms` fresh tracker (~`8.0%` lower) vs `2.0278 ms` reused tracker (~`9.6%` lower); back-loaded median `2.5104 ms` baseline vs `2.2357 ms` fresh tracker (~`10.9%` lower) vs `2.1885 ms` reused tracker (~`12.8%` lower)
- added benchmark-only mixed readiness coverage where `1024` channels are split into five cohorts with staggered start blocks and back-loaded completion (`128` ready on block 0, `128` on block 1, `256` starting at block 1 and ready on block 2, `256` spanning all four blocks, and `256` starting at block 2 and ready on block 3)
- the new mixed fixture showed the touched-only path still wins when later cohorts do not exist in early blocks, but the gain is smaller than the fully back-loaded weighted case: `baseline_vec_scan_all_mixed_back_loaded_4_blocks_1024_channels` = `2.2023 ms`, `fresh_tracker_mixed_back_loaded_4_blocks_1024_channels` = `2.0648 ms` (~`6.2%` lower), `reused_tracker_mixed_back_loaded_4_blocks_1024_channels` = `2.0934 ms` (~`4.9%` lower)
- compared with the weighted back-loaded result, the reduced win in the mixed fixture suggests a meaningful part of the touched-only benefit comes from avoiding scans over long-lived survivor-heavy channels, not just from distributing completion later in time
- focused validation passed again: `cargo test -p base-batcher-service recent_txs -- --nocapture` (`8 passed`) and `cargo clippy -p base-batcher-service --tests --benches --no-deps -- -D warnings`
Next:
- if this startup-scan path is revisited again, add another mixed fixture that holds completion timing constant while varying only touch-start sparsity, so the next experiment can quantify how much of the win comes from survivor-heavy buffered maps versus per-block frame parsing volume

## 2026-04-27 22:31 UTC
Focus: `base-batcher-service` recent-tx startup scan touch-start sparsity benchmark coverage.
Hypothesis: the new mixed readiness fixture suggested part of the touched-only drain win comes from survivor-heavy buffered channels, but it still did not compare that against a dense-start shape with similar back-loaded completion; adding a paired dense-start vs sparse-start benchmark should isolate how much of the win comes from later touch-start sparsity versus long-lived survivor-heavy channels.
Commands:
- `cargo bench -p base-batcher-service --bench recent_txs process_blocks_weighted_ready -- --warm-up-time 0.5 --measurement-time 1.0 --sample-size 15`
- `cargo bench -p base-batcher-service --bench recent_txs process_blocks_mixed_ready -- --warm-up-time 0.5 --measurement-time 1.0 --sample-size 15`
- extended `crates/batcher/service/benches/recent_txs.rs` with a new `process_blocks_touch_start_sparsity` Criterion group that runs paired dense-start and sparse-start back-loaded fixtures
- `cargo fmt --all`
- `cargo bench -p base-batcher-service --bench recent_txs process_blocks_touch_start_sparsity -- --warm-up-time 0.5 --measurement-time 1.0 --sample-size 15`
- `cargo test -p base-batcher-service recent_txs -- --nocapture`
- `cargo clippy -p base-batcher-service --tests --benches --no-deps -- -D warnings`
Results:
- refreshed the existing baselines before editing the bench file: weighted back-loaded median `2.4868 ms` baseline vs `2.2218 ms` fresh tracker (~`10.7%` lower) vs `2.1999 ms` reused tracker (~`11.5%` lower); mixed sparse-start median `2.2125 ms` baseline vs `2.0913 ms` fresh tracker (~`5.5%` lower) vs `2.0833 ms` reused tracker (~`5.8%` lower)
- added benchmark-only paired coverage that keeps the same approximate back-loaded completion shape (`128/128/256/512`) but compares a dense-start fixture where every channel begins on block 0 against the existing sparse-start cohort fixture where later channels only appear on later blocks
- the new pairwise comparison makes the survivor-heavy contribution explicit: dense-start baseline `2.4536 ms` vs sparse-start baseline `2.2503 ms` (~`8.3%` lower just from later touch-start sparsity), while the touched-only path still improves both shapes but wins more on dense-start survivor-heavy blocks: dense-start fresh tracker `2.2463 ms` (~`8.4%` lower) and reused tracker `2.2340 ms` (~`9.0%` lower) versus sparse-start fresh tracker `2.1074 ms` (~`6.4%` lower) and reused tracker `2.1106 ms` (~`6.2%` lower)
- this narrows the interpretation: a meaningful part of the startup-scan gain comes from avoiding scans over channels that have existed since early blocks, while later touch-start sparsity alone already removes a large slice of the old full-scan cost
Next:
- if this startup-scan path is revisited again, add a third pair where dense-start and sparse-start fixtures keep both completion counts and per-block transaction counts even closer so the remaining gap can be attributed almost entirely to survivor-heavy channel-map size rather than total parse volume

## 2026-04-28 00:36 UTC
Focus: `base-batcher-service` recent-tx startup scan matched-volume touch-start benchmark coverage.
Hypothesis: the prior dense-start vs sparse-start pair still changed per-block transaction volume, so some of the baseline gap could still come from less frame parsing; adding a matched-volume sparse-start fixture should isolate survivor-heavy buffered-map cost more cleanly by keeping block-by-block payload counts aligned with the dense-start back-loaded shape.
Commands:
- `cargo bench -p base-batcher-service --bench recent_txs process_blocks_touch_start_sparsity -- --warm-up-time 0.5 --measurement-time 1.0 --sample-size 15`
- extended `crates/batcher/service/benches/recent_txs.rs` with `MATCHED_VOLUME_BACK_LOADED_READY_COHORTS` and a new `process_blocks_touch_start_matched_volume` Criterion group
- `cargo fmt --all`
- `cargo bench -p base-batcher-service --bench recent_txs process_blocks_touch_start_matched_volume -- --warm-up-time 0.5 --measurement-time 1.0 --sample-size 15`
- `cargo test -p base-batcher-service recent_txs -- --nocapture`
- `cargo clippy -p base-batcher-service --tests --benches --no-deps -- -D warnings`
Results:
- refreshed the existing touch-start sparsity baseline before editing the bench file: dense-start baseline `2.4297 ms`, fresh tracker `2.1906 ms`, reused tracker `2.2198 ms`; sparse-start baseline `2.2413 ms`, fresh tracker `2.1163 ms`, reused tracker `2.1221 ms`
- added benchmark-only matched-volume coverage where the sparse-start shape keeps the same total `1024` channels and the same per-block completion distribution as the dense-start back-loaded fixture, but delays only a `128`-channel cohort from block `0` to block `1`
- the matched-volume harness reduced the old dense-vs-sparse baseline gap to about `0.8%` (`2.4297 ms` dense vs `2.4097 ms` matched-volume sparse), showing most of the earlier `8%+` gap came from reduced parse volume rather than survivor-heavy map size alone
- even with matched block-by-block payload volume, the touched-only drain still beat the full scan in both shapes: dense-start fresh tracker `2.1906 ms` (~`9.8%` lower) and reused tracker `2.2198 ms` (~`8.6%` lower); matched-volume sparse-start fresh tracker `2.2298 ms` (~`7.5%` lower) and reused tracker `2.1861 ms` (~`9.3%` lower)
- this narrows the interpretation further: touched-only draining still has a durable win even after controlling for parse volume, but the extra dense-vs-sparse gap is much smaller once per-block transaction counts are matched, so future production changes should target the drain/decode path rather than assuming large additional gains from touch-start sparsity alone
- focused validation passed again: `cargo test -p base-batcher-service recent_txs -- --nocapture` (`8 passed`) and `cargo clippy -p base-batcher-service --tests --benches --no-deps -- -D warnings`
Next:
- if this startup-scan path is revisited again, add a benchmark that holds both block payload counts and touched-ID cardinality constant while varying ready-channel decode density, so the next experiment can decide whether any remaining optimization opportunity sits in touched-only draining, channel decode, or `Frame::parse_frames`

## 2026-04-28 02:42 UTC
Focus: `base-batcher-service` recent-tx startup scan decode-density benchmark coverage with constant touched-ID cardinality.
Hypothesis: the matched-volume touch-start harness narrowed the survivor-heavy map effect, but it still left ambiguity about how much remaining variance comes from ready-channel decode versus drain bookkeeping; a prebuffered single-block fixture with fixed touched cardinality and payload count should isolate that crossover.
Commands:
- `cargo fmt --all`
- `cargo bench -p base-batcher-service --bench recent_txs process_block_decode_density -- --warm-up-time 0.5 --measurement-time 1.0 --sample-size 15`
- `cargo test -p base-batcher-service recent_txs -- --nocapture`
- `cargo clippy -p base-batcher-service --tests --benches --no-deps -- -D warnings`
Results:
- added benchmark-only coverage in `crates/batcher/service/benches/recent_txs.rs` for `process_block_decode_density`, using a new `prebuffered_decode_density_fixture` that starts `1024` touched channels with two frames already buffered and then replays one current-block frame per channel while varying how many touched channels become ready on that block (`0`, `256`, `512`, `1024`)
- this fixture keeps touched-ID cardinality and current-block payload count constant, separating ready-channel decode cost from touched-only drain bookkeeping without changing production logic
- validation passed: focused tests still pass (`8 passed`) and `cargo clippy -p base-batcher-service --tests --benches --no-deps -- -D warnings` passed
- median benchmark results from Criterion `estimates.json`:
  - `0` ready channels: baseline full scan `284.11 µs` vs tracker+touched-only `107.05 µs` (~`62.3%` lower)
  - `256` ready channels: baseline `711.99 µs` vs tracker+touched-only `588.78 µs` (~`17.3%` lower)
  - `512` ready channels: baseline `1.2746 ms` vs tracker+touched-only `1.0348 ms` (~`18.8%` lower)
  - `1024` ready channels: baseline `2.2045 ms` vs tracker+touched-only `1.9483 ms` (~`11.6%` lower)
- the new harness makes the crossover clearer: touched-only draining remains beneficial across the whole range, but the relative win shrinks as more touched channels finish and decode cost dominates more of the block budget
Next:
- if this startup-scan path is revisited again, use the new decode-density harness to test any drain/decode candidate directly; the next likely production opportunity is in channel decode or `Frame::parse_frames`, not in another touched-ID bookkeeping tweak

## 2026-04-28 04:48 UTC
Focus: `base-batcher-service` recent-tx startup scan decode path in `RecentTxScanner::decode_channel()`.
Hypothesis: the decode path still clones concatenated channel frame data with `data.to_vec()` before constructing `BatchReader`; passing the owned `Bytes` directly should avoid one full payload copy per ready channel while preserving decoding semantics.
Commands:
- `cargo bench -p base-batcher-service --bench recent_txs process_block_decode_density -- --warm-up-time 0.5 --measurement-time 1.0 --sample-size 15`
- edited `crates/batcher/service/src/recent_txs.rs` to pass `channel.frame_data()` directly into `BatchReader::new`
- `cargo fmt --all`
- `cargo test -p base-batcher-service recent_txs -- --nocapture`
- `cargo clippy -p base-batcher-service --tests --benches --no-deps -- -D warnings`
- `cargo bench -p base-batcher-service --bench recent_txs process_block_decode_density -- --warm-up-time 0.5 --measurement-time 1.0 --sample-size 15`
- extracted median `point_estimate` values from `target/criterion/.../estimates.json` for before/after comparison
Results:
- changed `RecentTxScanner::decode_channel()` from `BatchReader::new(data.to_vec(), max_rlp)` to `BatchReader::new(data, max_rlp)`, removing an unnecessary `Bytes`→`Vec<u8>` clone before decompression while leaving all tests green
- validation passed: `cargo test -p base-batcher-service recent_txs -- --nocapture` (`8 passed`) and `cargo clippy -p base-batcher-service --tests --benches --no-deps -- -D warnings`
- decode-density median results after the change (before → after):
  - `0` ready channels: baseline full scan `284.11 µs` → `197.00 µs`; tracker+touched-only `107.05 µs` → `105.80 µs`
  - `256` ready channels: baseline `711.99 µs` → `686.94 µs`; tracker+touched-only `588.78 µs` → `572.95 µs`
  - `512` ready channels: baseline `1.2746 ms` → `1.1505 ms`; tracker+touched-only `1.0348 ms` → `1.0337 ms`
  - `1024` ready channels: baseline `2.2045 ms` → `2.1306 ms`; tracker+touched-only `1.9483 ms` → `1.9690 ms`
- the signal is modest and somewhat noisy, but the touched-only path stayed essentially flat or slightly better on three of four decode-density cases while the code simplification safely removes one per-channel buffer materialization from the hot decode path
Next:
- if this startup-scan path is revisited again, add a decode-only microbenchmark around `channel.frame_data()` + `BatchReader::next_batch()` so future work can isolate frame concatenation and decompression costs from touched-ID tracking and channel-map drain behavior.

## 2026-04-28 06:57 UTC
Focus: `base-batcher-service` recent-tx startup scan decode-component benchmark coverage.
Hypothesis: the decode-density harness still mixed touched-ID draining, frame parsing, `channel.frame_data()`, and `BatchReader` work; a decode-only component bench should isolate whether the next likely hotspot is frame concatenation or decompression/decoding before touching production code again.
Commands:
- `cargo bench -p base-batcher-service --bench recent_txs process_block_decode_density -- --warm-up-time 0.5 --measurement-time 1.0 --sample-size 15`
- edited `crates/batcher/service/benches/recent_txs.rs` to add `decode_channel_components` coverage with ready multi-batch channels split across `1`, `4`, and `16` frames
- `cargo fmt --all`
- `cargo test -p base-batcher-service recent_txs -- --nocapture`
- `cargo clippy -p base-batcher-service --tests --benches --no-deps -- -D warnings`
- `cargo bench -p base-batcher-service --bench recent_txs decode_channel_components -- --warm-up-time 0.5 --measurement-time 1.0 --sample-size 15`
- extracted median `point_estimate` values from `target/criterion/.../estimates.json`
Results:
- kept production logic unchanged and added benchmark-only helpers that separately measure `channel.frame_data()` alone, `BatchReader` on preaggregated channel bytes alone, and the combined decode path for a `16`-batch ready channel
- validation passed: `cargo test -p base-batcher-service recent_txs -- --nocapture` (`8 passed`) and `cargo clippy -p base-batcher-service --tests --benches --no-deps -- -D warnings`
- refreshed decode-density medians on the current tree to preserve continuity before the harness edit:
  - `0` ready channels: baseline full scan `205.30 µs`; tracker+touched-only `104.18 µs` (~`49.3%` lower)
  - `256` ready channels: baseline `681.31 µs`; tracker+touched-only `589.41 µs` (~`13.5%` lower)
  - `512` ready channels: baseline `1.1581 ms`; tracker+touched-only `1.0310 ms` (~`11.0%` lower)
  - `1024` ready channels: baseline `2.1614 ms`; tracker+touched-only `1.9537 ms` (~`9.6%` lower)
- new decode-component medians show `BatchReader` dominates while `channel.frame_data()` stays a small but growing additive cost as frame count rises:
  - `1` frame: `frame_data_only` `19.52 ns`; `batch_reader_only` `3.4034 µs`; combined `3.4197 µs`
  - `4` frames: `frame_data_only` `25.62 ns`; `batch_reader_only` `3.4136 µs`; combined `3.3986 µs`
  - `16` frames: `frame_data_only` `82.71 ns`; `batch_reader_only` `3.4251 µs`; combined `3.4930 µs`
- interpretation: even when a ready channel is split across many frames, frame concatenation remains tiny relative to decompression and batch decoding, so the next production opportunity is more likely inside `BatchReader`/decode work than in additional `channel.frame_data()` tweaks
Next:
- if this path is revisited again, inspect `BatchReader::decompress` and per-batch decode overhead for reusable scratch buffers or avoidable allocations, using the new component harness to verify whether any candidate actually moves the `~3.4 µs` decode floor.

