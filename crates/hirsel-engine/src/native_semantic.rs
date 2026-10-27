//! Bodyless semantic launch binding, independent of native topology and IR.
use crate::*;
use std::collections::BTreeMap;
use std::sync::Arc;

/// The existing source semantic closure and capability escapes. Construction
/// runs the ordinary checked source queries; no artifact or native descriptor
/// can construct this input. This conveys no native Value/Ready authority.
pub struct NativeSemanticProgram {
    program: crate::queries::SemanticProgram,
    escapes: BTreeMap<DefPath, Escape>,
    foreign_threads: BTreeMap<hirsel_syntax::PackageId, hirsel_types::ForeignThreads>,
    signatures: Arc<SignatureSnapshot>,
    sources: BTreeMap<DefPath, (ContentHash, ContentHash)>,
    foreign_links: Vec<LinkInput>,
    foreign_unreplayable: bool,
}
impl NativeSemanticProgram {
    pub(crate) fn metadata_weight(&self, weight: &mut crate::NativeMetadataWeight) {
        weight.add(std::mem::size_of::<Self>() + 2 * std::mem::size_of::<usize>());
        weight.vec(&self.program.nodes, |weight, node| {
            weight.add(node.identity.capacity());
            weight.vec(&node.content, |weight, bytes| weight.flat_vec(bytes));
            weight.vec(&node.dependencies, |weight, name| {
                weight.add(name.capacity())
            });
        });
        weight.tree::<DefPath, ()>(self.program.definitions.len());
        for path in &self.program.definitions {
            weight.path(path);
        }
        weight.map(&self.program.uses, |weight, path, uses| {
            weight.path(path);
            weight.tree::<DefPath, ()>(uses.len());
            for path in uses {
                weight.path(path);
            }
        });
        weight.map(&self.escapes, |weight, path, _| weight.path(path));
        weight.map(&self.sources, |weight, path, _| weight.path(path));
        weight.map(&self.foreign_threads, |weight, package, _| {
            weight.add(package.name.capacity() + package.version.capacity());
        });
        weight.vec(&self.foreign_links, |weight, link| match link {
            crate::LinkInput::Library(name) => weight.add(name.capacity()),
            crate::LinkInput::File(path) => weight.add(path.capacity()),
        });
    }
    pub(crate) fn definitions(&self) -> &std::collections::BTreeSet<DefPath> {
        &self.program.definitions
    }

    pub(crate) fn new(
        program: crate::queries::SemanticProgram,
        escapes: BTreeMap<DefPath, Escape>,
        foreign_threads: BTreeMap<hirsel_syntax::PackageId, hirsel_types::ForeignThreads>,
        signatures: Arc<SignatureSnapshot>,
        sources: BTreeMap<DefPath, (ContentHash, ContentHash)>,
        foreign_links: Vec<LinkInput>,
        foreign_unreplayable: bool,
    ) -> Self {
        Self {
            program,
            escapes,
            foreign_threads,
            signatures,
            sources,
            foreign_links,
            foreign_unreplayable,
        }
    }
    pub fn semantic_program_revision(
        &self,
        entry: &DefPath,
    ) -> Result<hirsel_run_schema::ProgramRevision, QueryError> {
        self.program.revision(entry)
    }
    pub fn semantic_contains(&self, definition: &DefPath) -> bool {
        self.program.definitions.contains(definition)
    }
    /// The immutable signature/private-view snapshot used by this checked
    /// semantic closure. This is a local source binding, not native authority.
    pub fn signatures(&self) -> &Arc<SignatureSnapshot> {
        &self.signatures
    }
    pub fn foreign_links(&self) -> &[LinkInput] {
        &self.foreign_links
    }
    pub fn foreign_unreplayable(&self) -> bool {
        self.foreign_unreplayable
    }
    pub fn foreign_threads(
        &self,
    ) -> &BTreeMap<hirsel_syntax::PackageId, hirsel_types::ForeignThreads> {
        &self.foreign_threads
    }
    /// Current source implementation and signature observations of this
    /// closure. A native launch constructor can check settled Lower bindings
    /// without reopening a source body or querying the semantic controller.
    pub fn source_bindings(&self) -> impl Iterator<Item = (&DefPath, ContentHash, ContentHash)> {
        self.sources
            .iter()
            .map(|(path, (implementation, signature))| (path, *implementation, *signature))
    }
    pub fn unsafe_reach(&self, entry: &DefPath) -> Option<UnsafeReach> {
        self.program.reach(entry, &self.escapes)
    }
    pub fn home_reach(&self, root: &DefPath) -> Option<UnsafeReach> {
        let home = self
            .escapes
            .iter()
            .filter(|(path, escape)| {
                **escape == Escape::Extern
                    && self
                        .foreign_threads
                        .get(&path.0.package)
                        .copied()
                        .unwrap_or_default()
                        == hirsel_types::ForeignThreads::Home
            })
            .map(|(path, escape)| (path.clone(), *escape))
            .collect();
        self.program.reach(root, &home)
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
