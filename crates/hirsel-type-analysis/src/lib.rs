//! Stage-selected analyses over semantic types and declaration snapshots.
//! Stable definitions and basic graph/property queries live in hirsel-types.

#[cfg(test)]
hirsel_guard::install!();

pub mod cleanup;
pub mod logical;
pub mod parametric;
pub mod representation;
pub mod spelling;
