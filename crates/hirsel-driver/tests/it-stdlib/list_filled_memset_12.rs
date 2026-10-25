//! Byte repetition must retain the reserve-and-fill loop's bulk lowering.
use hirsel_driver::{load_program_source, native};
use hirsel_engine::with_compiler_stack;
use std::path::Path;
