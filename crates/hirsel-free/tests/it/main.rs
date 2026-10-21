//! The integration tests of this crate, linked as one binary.

hirsel_guard::install!();

mod codec;
mod executor;
mod gate;
mod traces;

mod schedule;

mod concurrent;
