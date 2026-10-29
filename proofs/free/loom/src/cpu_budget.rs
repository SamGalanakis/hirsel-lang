//! Exact production Jobs/Settled/Stretch, paired with the production slot.
#[path = "../../../../crates/hirsel-rt/src/budget/stretch.rs"]
pub mod stretch;
#[path = "../../../../crates/hirsel-rt/src/budget/stretch_trace/charge.rs"]
pub(crate) mod stretch_trace;

pub fn retire(
    jobs: &mut stretch::Jobs,
    ordinal: usize,
    state: &mut stretch::Settled,
) -> (stretch::Stretch, Result<(), stretch::JobStop>) {
    jobs.retire_published_at(ordinal, state)
}
pub fn finish(
    jobs: &mut stretch::Jobs,
    ordinal: usize,
    prefix: stretch::Stretch,
    state: &mut stretch::Settled,
) -> Result<(), hirsel_run_schema::BudgetKind> {
    jobs.retire_prefix_at(ordinal, prefix, true, state)
        .map_err(|stop| stop.kind)
}
