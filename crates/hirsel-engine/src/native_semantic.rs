//! Bodyless semantic launch binding, independent of native topology and IR.
use crate::*;
use std::collections::BTreeMap;
use std::sync::Arc;

/// The existing source semantic closure and capability escapes. Construction
/// runs the ordinary checked source queries; no artifact or native descriptor
/// can construct this input. This conveys no native Value/Ready authority.
pub struct NativeSemanticProgram {
    shared: Arc<Shared>,
    selected: Option<std::collections::BTreeSet<DefPath>>,
    foreign_unreplayable: bool,
}
struct Shared {
    program: crate::queries::SemanticProgram,
    escapes: BTreeMap<DefPath, Escape>,
    foreign_threads: BTreeMap<hirsel_syntax::PackageId, hirsel_types::ForeignThreads>,
    signatures: Arc<SignatureSnapshot>,
    sources: BTreeMap<DefPath, (ContentHash, ContentHash)>,
    foreign_links: Vec<LinkInput>,
}
impl NativeSemanticProgram {
    pub(crate) fn shared_node(
        &self,
        definition: Option<DefId>,
    ) -> Option<Arc<hirsel_run_schema::ProgramNode>> {
        if definition.is_some_and(|id| !self.semantic_contains(id.path())) {
            return None;
        }
        let identity = definition.map_or_else(
            || "$hirsel/runtime@1".to_owned(),
            |definition| definition.path().to_string(),
        );
        self.shared
            .program
            .nodes
            .binary_search_by(|node| node.identity.cmp(&identity))
            .ok()
            .map(|index| self.shared.program.nodes[index].clone())
    }
    pub(crate) fn metadata_weight(
        &self,
        weight: &mut crate::NativeMetadataWeight,
        nodes: &mut std::collections::BTreeSet<usize>,
    ) {
        weight.add(std::mem::size_of::<Self>() + 2 * std::mem::size_of::<usize>());
        if let Some(selected) = &self.selected {
            weight.tree::<DefPath, ()>(selected.len());
            for path in selected {
                weight.path(path);
            }
        }
        if !nodes.insert(Arc::as_ptr(&self.shared) as usize) {
            return;
        }
        weight.arc(&self.shared);
        weight.vec(&self.shared.program.nodes, |weight, node| {
            if !nodes.insert(Arc::as_ptr(node) as usize) {
                return;
            }
            weight.arc(node);
            weight.add(node.identity.capacity());
            weight.vec(&node.content, |weight, bytes| weight.flat_vec(bytes));
            weight.vec(&node.dependencies, |weight, name| {
                weight.add(name.capacity())
            });
        });
        weight.tree::<DefPath, ()>(self.shared.program.definitions.len());
        for path in &self.shared.program.definitions {
            weight.path(path);
        }
        weight.map(&self.shared.program.uses, |weight, path, uses| {
            weight.path(path);
            weight.tree::<DefPath, ()>(uses.len());
            for path in uses {
                weight.path(path);
            }
        });
        weight.map(&self.shared.escapes, |weight, path, _| weight.path(path));
        weight.map(&self.shared.sources, |weight, path, _| weight.path(path));
        weight.map(&self.shared.foreign_threads, |weight, package, _| {
            weight.add(package.name.capacity() + package.version.capacity());
        });
        weight.vec(&self.shared.foreign_links, |weight, link| match link {
            crate::LinkInput::Library(name) => weight.add(name.capacity()),
            crate::LinkInput::File(path) => weight.add(path.capacity()),
        });
    }
    pub(crate) fn definitions(&self) -> &std::collections::BTreeSet<DefPath> {
        self.selected
            .as_ref()
            .unwrap_or(&self.shared.program.definitions)
    }
    pub(crate) fn select_definitions(
        &self,
        definitions: std::collections::BTreeSet<DefPath>,
        foreign_unreplayable: bool,
    ) -> Self {
        debug_assert!(definitions.is_subset(&self.shared.program.definitions));
        Self {
            shared: self.shared.clone(),
            selected: Some(definitions),
            foreign_unreplayable,
        }
    }

    pub(crate) fn new(
        mut program: crate::queries::SemanticProgram,
        escapes: BTreeMap<DefPath, Escape>,
        foreign_threads: BTreeMap<hirsel_syntax::PackageId, hirsel_types::ForeignThreads>,
        signatures: Arc<SignatureSnapshot>,
        sources: BTreeMap<DefPath, (ContentHash, ContentHash)>,
        foreign_links: Vec<LinkInput>,
        foreign_unreplayable: bool,
    ) -> Self {
        program
            .nodes
            .sort_by(|left, right| left.identity.cmp(&right.identity));
        Self {
            shared: Arc::new(Shared {
                program,
                escapes,
                foreign_threads,
                signatures,
                sources,
                foreign_links,
            }),
            selected: None,
            foreign_unreplayable,
        }
    }
    pub fn semantic_program_revision(
        &self,
        entry: &DefPath,
    ) -> Result<hirsel_run_schema::ProgramRevision, QueryError> {
        if !self.semantic_contains(entry) {
            return Err(QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "semantic program revision".into(),
                message: hirsel_run_schema::RevisionError::Missing {
                    identity: entry.to_string(),
                }
                .to_string(),
            }));
        }
        self.shared.program.revision(entry)
    }
    pub fn semantic_contains(&self, definition: &DefPath) -> bool {
        self.definitions().contains(definition)
    }
    /// The immutable signature/private-view snapshot used by this checked
    /// semantic closure. This is a local source binding, not native authority.
    pub fn signatures(&self) -> &Arc<SignatureSnapshot> {
        &self.shared.signatures
    }
    pub fn foreign_links(&self) -> &[LinkInput] {
        &self.shared.foreign_links
    }
    pub fn foreign_unreplayable(&self) -> bool {
        self.foreign_unreplayable
    }
    pub fn foreign_threads(
        &self,
    ) -> &BTreeMap<hirsel_syntax::PackageId, hirsel_types::ForeignThreads> {
        &self.shared.foreign_threads
    }
    /// Current source implementation and signature observations of this
    /// closure. A native launch constructor can check settled Lower bindings
    /// without reopening a source body or querying the semantic controller.
    pub fn source_bindings(&self) -> impl Iterator<Item = (&DefPath, ContentHash, ContentHash)> {
        self.shared
            .sources
            .iter()
            .filter(|(path, _)| self.semantic_contains(path))
            .map(|(path, (implementation, signature))| (path, *implementation, *signature))
    }
    pub fn unsafe_reach(&self, entry: &DefPath) -> Option<UnsafeReach> {
        self.semantic_contains(entry)
            .then_some(())
            .and_then(|()| self.shared.program.reach(entry, &self.shared.escapes))
    }
    pub fn home_reach(&self, root: &DefPath) -> Option<UnsafeReach> {
        let home = self
            .shared
            .escapes
            .iter()
            .filter(|(path, escape)| {
                **escape == Escape::Extern
                    && self
                        .shared
                        .foreign_threads
                        .get(&path.0.package)
                        .copied()
                        .unwrap_or_default()
                        == hirsel_types::ForeignThreads::Home
            })
            .map(|(path, escape)| (path.clone(), *escape))
            .collect();
        self.semantic_contains(root)
            .then_some(())
            .and_then(|()| self.shared.program.reach(root, &home))
    }
}
impl QueryEngine {
    /// Capture the same semantic closure as snapshot import, retaining only
    /// checked dependency nodes and escape metadata, never source Definitions.
    pub fn native_semantic_program(
        &mut self,
        roots: impl IntoIterator<Item = DefPath>,
    ) -> Result<NativeSemanticProgram, QueryError> {
        let program = self.semantic_program(roots)?;
        let escapes = self.escapes(&program)?;
        let mut sources = BTreeMap::new();
        for path in self.source_paths() {
            if !program.definitions.contains(&path) {
                continue;
            }
            let syntax = self.parse_definition(&path)?.value;
            let signature = self.item_signature(&path)?.value;
            sources.insert(path, (syntax.text_hash, signature.fingerprint));
        }
        let signatures = self.signature_snapshot();
        Ok(NativeSemanticProgram::new(
            program,
            escapes,
            self.foreign_threads.clone(),
            signatures,
            sources,
            self.foreign_links.clone(),
            self.foreign_unreplayable,
        ))
    }
}
