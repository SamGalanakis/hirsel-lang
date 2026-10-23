//! Simulation-only file readiness, following the scripted stream's Stall/Expect gates.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Gate every `file.read` of an opened regular file at this resolved snapshot path.
/// This changes readiness only: bytes, metadata, faults and cancellation stay ordinary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum FileReadScript {
    /// No readiness source; only cancellation can complete the read.
    Pending,
    /// Release after a successful `file.write_bytes` to this resolved path in the same
    /// tree. A write before the read also releases it; failed writes never release it.
    AfterWrite { path: String },
}

/// Per-world script state. Test directories are independent trees, even at equal paths.
#[derive(Clone, Debug, Default)]
pub struct FileReadiness {
    scripts: BTreeMap<String, FileReadScript>,
    written: BTreeSet<(u64, String)>,
}
impl FileReadiness {
    pub fn new(scripts: BTreeMap<String, FileReadScript>) -> Self {
        Self {
            scripts,
            written: BTreeSet::new(),
        }
    }
    pub fn blocked(&self, tree: u64, path: &str) -> bool {
        match self.scripts.get(path) {
            None => false,
            Some(FileReadScript::Pending) => true,
            Some(FileReadScript::AfterWrite { path }) => {
                !self.written.contains(&(tree, path.clone()))
            }
        }
    }
    pub fn wrote(&mut self, tree: u64, path: &str) -> bool {
        if self.scripts.values().any(|script| matches!(script, FileReadScript::AfterWrite { path: trigger } if trigger == path)) {
            return self.written.insert((tree, path.into()));
        }
        false
    }
}
