//! Whether a definition's dependency closure reaches an escape from capability
//! checking (CAP-6), and the path from it to the first such escape.
use super::*;
use hirsel_types::SignatureKind;
use hirsel_types::core_roles::{Intrinsic, intrinsic_roles};
use std::collections::VecDeque;

/// A construct outside capability checking (CAP-6).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Escape {
    /// An `extern "c"` function (PROG-4).
    Extern,
    /// A core operation that requires `Unsafe` (MEM-9).
    UnsafeOperation,
}

impl std::fmt::Display for Escape {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Extern => "the extern \"c\" function",
            Self::UnsafeOperation => "the Unsafe operation",
        })
    }
}

/// The first escape a definition reaches, on a shortest dependency path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnsafeReach {
    /// From the root definition to the escape, both included.
    pub path: Vec<DefPath>,
    pub escape: Escape,
}

impl UnsafeReach {
    /// The path as its definitions' versioned spellings, root first.
    pub fn spelled(&self) -> Vec<String> {
        self.path.iter().map(ToString::to_string).collect()
    }
}

impl QueryEngine {
    /// The escape `root` reaches, if any (CAP-6). Externs and `Unsafe` core
    /// operations are the escapes; core operations behind safe signatures are the
    /// runtime's own and reach none. Std is checked like any other package. The
    /// search is breadth-first in definition order, so the path is a shortest one
    /// and the same on every check.
    pub fn unsafe_reach(&mut self, root: &DefPath) -> Result<Option<UnsafeReach>, QueryError> {
        let program = self.semantic_program([root.clone()])?;
        let unsafe_operations: BTreeSet<_> = intrinsic_roles()
            .into_iter()
            .filter(|(_, role)| *role == Intrinsic::UnsafeOperation)
            .map(|(path, _)| path)
            .collect();
        let mut escapes = BTreeMap::new();
        for path in &program.definitions {
            if unsafe_operations.contains(path) {
                escapes.insert(path.clone(), Escape::UnsafeOperation);
            } else if matches!(&self.item_signature(path)?.value.kind, SignatureKind::Function(f) if f.foreign.is_some())
            {
                escapes.insert(path.clone(), Escape::Extern);
            }
        }
        let mut parents: BTreeMap<&DefPath, &DefPath> = BTreeMap::new();
        let mut queue = VecDeque::from([root]);
        while let Some(at) = queue.pop_front() {
            if let Some(&escape) = escapes.get(at) {
                let mut path = vec![at.clone()];
                let mut step = at;
                while let Some(&parent) = parents.get(step) {
                    path.push(parent.clone());
                    step = parent;
                }
                path.reverse();
                return Ok(Some(UnsafeReach { path, escape }));
            }
            for next in program.uses.get(at).into_iter().flatten() {
                if next != root && !parents.contains_key(next) {
                    parents.insert(next, at);
                    queue.push_back(next);
                }
            }
        }
        Ok(None)
    }
}
