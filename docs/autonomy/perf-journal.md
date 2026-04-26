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
