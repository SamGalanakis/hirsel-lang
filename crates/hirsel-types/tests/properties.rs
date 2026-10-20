//! Differential properties share the library types used by the analysis crate.
hirsel_guard::install!();

use hirsel_syntax::DefPath;
use hirsel_types::*;

#[path = "properties/parametric.rs"]
mod parametric_tests;
#[path = "properties/properties.rs"]
mod property_tests;
