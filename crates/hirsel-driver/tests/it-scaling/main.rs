//! The scaling probes of this crate: work counters stay linear in program size.
//! They link as their own binary so a compiler edit relinks three smaller
//! harnesses in parallel.

hirsel_guard::install!();

include!("modules.rs");
