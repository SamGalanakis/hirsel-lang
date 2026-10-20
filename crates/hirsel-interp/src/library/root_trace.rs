//! `--trace-roots` (S-14), the reference engine's half: at every scheduling point (the yield
//! after an operation, or a scope exit's park), the count of every live `Shared` root, keyed
//! by the running task's spawn path and that task's scheduling-point ordinal. A root is named
//! by its freeze ordinal in the run, and the lines are spelled as the native runtime's
//! (`hirsel_rt` `root_trace`), so the traces of the interpreter, LLVM and Cranelift compare
//! line by line: every engine has the same holders of every root at every operation.
use crate::value::SharedRoot;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default)]
pub struct RootTrace {
    /// Roots in freeze order, with their freeze ordinal; dropped once they have no root.
    roots: Vec<(u64, SharedRoot)>,
    frozen: u64,
    points: BTreeMap<Vec<u32>, u64>,
    pub lines: Vec<String>,
}

impl RootTrace {
    /// `shared.freeze` made `root`.
    pub fn frozen(&mut self, root: &SharedRoot) {
        self.frozen += 1;
        self.roots.push((self.frozen, root.clone()));
    }

    /// A scheduling point of the task at `path`.
    pub fn point(&mut self, path: Vec<u32>) {
        self.roots.retain(|(_, root)| root.count() > 0);
        let ordinal = self.points.entry(path.clone()).or_insert(0);
        *ordinal += 1;
        let mut line = if path.is_empty() {
            "root".to_string()
        } else {
            path.iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(".")
        };
        line.push_str(&format!(" {ordinal}:"));
        for (id, root) in &self.roots {
            line.push_str(&format!(" {id}={}", root.count()));
        }
        self.lines.push(line);
    }
}
