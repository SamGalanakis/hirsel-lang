//! The integration tests of this crate, linked as one binary.

hirsel_guard::install!(crate::runtime50::Counting: crate::runtime50::Counting);

include!("modules.rs");
