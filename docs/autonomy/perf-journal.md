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
