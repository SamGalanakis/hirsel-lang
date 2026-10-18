//! Pure leaf algorithms; adapters own representation and effects.

#[cfg(test)]
hirsel_guard::install!();

pub mod disk;
pub mod integer;
pub mod report;
pub mod siphash;
pub mod unicode;
