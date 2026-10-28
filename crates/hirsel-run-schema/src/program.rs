//! One semantic dependency-closure encoder for every launch engine.
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// An adapter-supplied semantic component. Identities are stable, namespaced
/// names, never arena indices, source positions or native instance identities.
/// Content fields are ordered; dependencies are a set. Adapters must supply
/// private implementations, constant initializers and core/runtime dependencies,
/// not merely exported declaration fingerprints.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProgramNode {
    pub identity: String,
    pub content: Vec<Vec<u8>>,
    pub dependencies: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RevisionError {
    Missing { identity: String },
    Conflict { identity: String },
}
impl fmt::Display for RevisionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing { identity } => write!(f, "missing semantic dependency {identity}"),
            Self::Conflict { identity } => write!(f, "conflicting semantic dependency {identity}"),
        }
    }
}
impl std::error::Error for RevisionError {}

/// Canonical semantic selection, separate from compiler/artifact provenance.
/// Only the closure encoder constructs it. Cycles are finite graph edges, not
/// recursive hashes. Unreachable nodes do not participate in the revision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProgramRevision {
    entry: String,
    digest: [u8; 32],
}
/// An immutable normalized semantic graph reusable across rooted revisions.
/// Construction checks every duplicate, including unreachable declarations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProgramGraph {
    nodes: BTreeMap<String, ProgramNode>,
}
impl ProgramGraph {
    pub fn new(nodes: impl IntoIterator<Item = ProgramNode>) -> Result<Self, RevisionError> {
        let mut graph = BTreeMap::new();
        for mut node in nodes {
            node.dependencies.sort();
            node.dependencies.dedup();
            if let Some(previous) = graph.get(&node.identity) {
                if previous != &node {
                    return Err(RevisionError::Conflict {
                        identity: node.identity,
                    });
                }
            } else {
                graph.insert(node.identity.clone(), node);
            }
        }
        Ok(Self { nodes: graph })
    }
    pub fn nodes(&self) -> impl ExactSizeIterator<Item = &ProgramNode> {
        self.nodes.values()
    }
    pub fn revision(&self, entry: &str) -> Result<ProgramRevision, RevisionError> {
        let mut closure = BTreeSet::new();
        let mut pending = vec![entry];
        while let Some(identity) = pending.pop() {
            if !closure.insert(identity) {
                continue;
            }
            let node = self
                .nodes
                .get(identity)
                .ok_or_else(|| RevisionError::Missing {
                    identity: identity.to_owned(),
                })?;
            pending.extend(node.dependencies.iter().map(String::as_str));
        }
        let mut hash = Sha256::new();
        // Every atom is framed by its u64 little-endian byte length. Counts
        // frame ordered content and edge lists; names and closure rows sort by
        // UTF-8 bytes. No Rust Debug, platform hash or session-local ID enters.
        fn atom(hash: &mut Sha256, bytes: &[u8]) {
            hash.update((bytes.len() as u64).to_le_bytes());
            hash.update(bytes);
        }
        atom(&mut hash, b"hirsel.semantic-program.v1");
        atom(&mut hash, hirsel_core_contract::depth::SEMANTICS.as_bytes());
        atom(&mut hash, entry.as_bytes());
        hash.update((closure.len() as u64).to_le_bytes());
        for identity in closure {
            let node = &self.nodes[identity];
            atom(&mut hash, identity.as_bytes());
            hash.update((node.content.len() as u64).to_le_bytes());
            for field in &node.content {
                atom(&mut hash, field);
            }
            hash.update((node.dependencies.len() as u64).to_le_bytes());
            for dependency in &node.dependencies {
                atom(&mut hash, dependency.as_bytes());
            }
        }
        Ok(ProgramRevision {
            entry: entry.into(),
            digest: hash.finalize().into(),
        })
    }
}
impl ProgramRevision {
    pub fn encode(
        entry: &str,
        nodes: impl IntoIterator<Item = ProgramNode>,
    ) -> Result<Self, RevisionError> {
        ProgramGraph::new(nodes)?.revision(entry)
    }
    pub fn entry(&self) -> &str {
        &self.entry
    }
    /// Owned backing storage, excluding this inline value. A copied revision
    /// must retain its own charge even when the original source remains live.
    pub fn retained_metadata_bytes(&self) -> u64 {
        self.entry.capacity() as u64
    }
    pub fn digest(&self) -> [u8; 32] {
        self.digest
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn node(identity: &str, content: &[u8], dependencies: &[&str]) -> ProgramNode {
        ProgramNode {
            identity: identity.into(),
            content: vec![content.to_vec()],
            dependencies: dependencies.iter().map(|s| (*s).into()).collect(),
        }
    }
    #[test]
    fn reusable_graph_selects_roots_and_cycles_without_mutating_shared_nodes() {
        let graph = ProgramGraph::new([
            node("a", b"first", &["b", "b"]),
            node("b", b"second", &["a"]),
            node("unused", b"third", &[]),
        ])
        .unwrap();
        let before = graph.clone();
        let first = graph.revision("a").unwrap();
        let second = graph.revision("b").unwrap();
        assert_ne!(first, second);
        assert_eq!(first, graph.revision("a").unwrap());
        assert_eq!(graph, before);
        let unrelated = ProgramGraph::new([
            node("unused", b"changed", &[]),
            node("b", b"second", &["a"]),
            node("a", b"first", &["b"]),
        ])
        .unwrap();
        assert_eq!(first, unrelated.revision("a").unwrap());
        let changed =
            ProgramGraph::new([node("a", b"first", &["b"]), node("b", b"changed", &["a"])])
                .unwrap();
        assert_ne!(first, changed.revision("a").unwrap());
        assert!(matches!(
            graph.revision("absent"),
            Err(RevisionError::Missing { .. })
        ));
        assert!(matches!(
            ProgramGraph::new([
                node("unused", b"third", &[]),
                node("unused", b"changed", &[]),
            ]),
            Err(RevisionError::Conflict { .. })
        ));
    }
}
