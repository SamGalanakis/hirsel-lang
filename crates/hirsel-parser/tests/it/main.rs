//! The integration tests of this crate, linked as one binary.

hirsel_guard::install!();

mod analyses;
mod closures;
mod depth;
mod diagnostics;
mod grammar_edges;
mod grammar_examples;
mod identity;
mod newlines;
mod path_division;
mod recovery;
#[path = "../support/mod.rs"]
mod support;
