use hirsel_driver::*;
use hirsel_engine::with_compiler_stack;
use hirsel_lir::ir::UnitOrigin;
use std::{collections::BTreeSet, path::PathBuf};

// Executed corpus vectors establish behavior; native headers establish that the
// wrapper uses ordinary source functions rather than privileged operations.
