//! Source metadata required by native launch assembly, without source bodies.
use super::declarations::NativeDeclarations;
use crate::native::{NativeError, ReadyProgram, adapt};
use hirsel_engine::{
    LinkInput, NativeSemanticProgram, QueryEngine, QueryError, RepresentationRecord,
    ValidatedNativeInput,
};
use hirsel_lir::stage::InputIdentity;
use hirsel_span::{Anchor, SourceMap};
use hirsel_syntax::DefPath;
use hirsel_types::{ForeignThreads, SignatureRef, TypeInterner};
use std::{collections::BTreeMap, sync::Arc};

/// Owned source facets for one immutable native request. The semantic closure
/// comes from the source engine, independently of native discovered instances.
pub struct NativeLinkSource {
    input: InputIdentity,
    semantic: NativeSemanticProgram,
    types: Arc<TypeInterner>,
    signatures: BTreeMap<DefPath, SignatureRef>,
    representations: BTreeMap<DefPath, Option<RepresentationRecord>>,
    anchors: BTreeMap<DefPath, Vec<Anchor>>,
    sources: SourceMap,
    foreign_links: Vec<LinkInput>,
    foreign_unreplayable: bool,
    foreign_threads: BTreeMap<hirsel_syntax::PackageId, ForeignThreads>,
}
impl NativeLinkSource {
    pub fn capture(
        engine: &mut QueryEngine,
        declarations: &NativeDeclarations,
    ) -> Result<Self, QueryError> {
        let semantic = engine.native_semantic_program(declarations.roots.definitions.clone())?;
        Ok(Self {
            input: declarations.program.input.clone(),
            semantic,
            types: declarations.program.types.clone(),
            signatures: declarations.program.signatures.clone(),
            representations: declarations
                .sources
                .iter()
                .map(|(path, source)| (path.clone(), source.representation.clone()))
                .collect(),
            anchors: declarations.source_anchors.clone(),
            sources: engine.sources.clone(),
            foreign_links: declarations.roots.foreign_links.clone(),
            foreign_unreplayable: declarations.roots.foreign_unreplayable,
            foreign_threads: declarations.roots.foreign_threads.clone(),
        })
    }

    /// Bind the independently checked source closure to selected native unit
    /// summaries. This computes no Value/Ready body and performs no native query.
    pub fn bind(&self, program: &mut ReadyProgram) -> Result<(), NativeError> {
        program.bind_source(&self.view())
    }

    pub(crate) fn view(&self) -> LinkSource<'_> {
        LinkSource {
            input: self.input.clone(),
            semantic: &self.semantic,
            types: &self.types,
            signatures: &self.signatures,
            representations: &self.representations,
            sources: &self.sources,
            anchors: self.anchors.iter().map(|(p, a)| (p, a.clone())).collect(),
            foreign_links: &self.foreign_links,
            foreign_unreplayable: self.foreign_unreplayable,
            foreign_threads: &self.foreign_threads,
        }
    }
}

/// The exact source facets consumed by the existing launch algorithms. Both
/// legacy inspection inputs and query assembly use these same algorithms.
pub(crate) struct LinkSource<'a> {
    pub input: InputIdentity,
    pub semantic: &'a NativeSemanticProgram,
    pub types: &'a TypeInterner,
    pub signatures: &'a BTreeMap<DefPath, SignatureRef>,
    pub representations: &'a BTreeMap<DefPath, Option<RepresentationRecord>>,
    pub sources: &'a SourceMap,
    pub anchors: BTreeMap<&'a DefPath, Vec<Anchor>>,
    pub foreign_links: &'a Vec<LinkInput>,
    pub foreign_unreplayable: bool,
    pub foreign_threads: &'a BTreeMap<hirsel_syntax::PackageId, ForeignThreads>,
}
impl<'a> LinkSource<'a> {
    pub fn snapshot(input: &'a ValidatedNativeInput) -> Self {
        let snapshot = input.snapshot();
        Self {
            input: adapt::input_identity(snapshot),
            semantic: input.semantic_program(),
            types: &snapshot.types,
            signatures: &snapshot.signatures,
            representations: &snapshot.representations,
            sources: &snapshot.sources,
            anchors: snapshot
                .source_owners
                .iter()
                .map(|(path, owner)| {
                    (
                        path,
                        snapshot.definitions.get(path).map_or_else(
                            || vec![owner.syntax.anchor],
                            |d| hirsel_tir::source_anchors(d),
                        ),
                    )
                })
                .collect(),
            foreign_links: &snapshot.foreign_links,
            foreign_unreplayable: snapshot.foreign_unreplayable,
            foreign_threads: &snapshot.foreign_threads,
        }
    }
}
