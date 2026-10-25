use hirsel_driver::*;
use hirsel_engine::with_compiler_stack;
use hirsel_lir::ir::UnitOrigin;
use std::{collections::BTreeSet, path::PathBuf};

// Corpus output checks cannot distinguish source bodies from privileged fallbacks.
