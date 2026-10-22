# Frozen compiler measurements — 2026-10-22

The [combined service/compiler scoreboard](../../bench/apps/tasks/RESULTS-app-perf-refresh-2.md)
records the frozen `ab44bbb731be91d79289eb3df88aeb139e3dfb75` compiler,
matching runtime/std, locked Rust ports, raw samples, comparison limits and
ranked residual triggers. All 32 cold/unchanged check/debug cells completed
ten rounds (320 measured commands), including accepted standard syn10k.

Fresh CLI and resident checks are separate. Rust unchanged includes Cargo's
no-op fast path; no wall-time ratio or overall performance gate is claimed.
The historical `RESULTS.md` is preserved with its original inputs.
