//! Stable package/module lookup and per-definition lexical resolution.
//!
//! Shared Resolution retains contextual names, generic scopes and failed reads.
//! ResolveOutput adds recoverable diagnostics and an operational-failure bridge.

#[cfg(test)]
hirsel_guard::install!();

mod index;
mod resolver;
pub use index::*;
pub use resolver::*;

#[cfg(test)]
mod tests;
