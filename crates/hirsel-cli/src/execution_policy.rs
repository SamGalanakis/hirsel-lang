/// The CLI tooling budget for simulated runs and replay.
pub(crate) const DEFAULT_RUN_MAX_STEPS: u64 = 1_000_000_000;

/// `hirsel test --bench`'s target time per bench, as Go's default -benchtime.
pub(crate) const DEFAULT_BENCH_MILLISECONDS: u64 = 1000;
