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
    pub(super) input: InputIdentity,
    pub(super) semantic: NativeSemanticProgram,
    types: Arc<TypeInterner>,
    pub(super) operations: hirsel_layout::operation::OperationRegistry,
    functions: BTreeMap<DefPath, u32>,
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
            operations: declarations.program.operation_registry(),
            functions: declarations
                .context
                .sources
                .iter()
                .filter_map(|(path, source)| {
                    matches!(&declarations.program.signatures[path].kind,
                    hirsel_types::SignatureKind::Function(f) if f.generics.is_empty())
                    .then(|| (path.clone(), source.entry.0))
                })
                .collect(),
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

    pub fn sites(&self) -> crate::native::Sites<'_> {
        crate::native::Sites::from_anchors(
            &self.sources,
            self.anchors
                .iter()
                .map(|(path, anchors)| (path, anchors.clone()))
                .collect(),
        )
    }

    pub(super) fn function(&self, key: &super::InstanceAddress) -> bool {
        self.functions.get(key.instance.definition()) == Some(&key.instance.body())
    }
    pub(super) fn launches(&self) -> BTreeMap<DefPath, crate::native::link::LaunchKind> {
        self.signatures
            .iter()
            .filter_map(|(path, signature)| {
                crate::native::link::launch_kind(&self.types, signature)
                    .map(|kind| (path.clone(), kind))
            })
            .collect()
    }
    /// Source semantic, foreign and diagnostic-position inputs consumed by link
    /// assembly. Numeric Anchor/FileId allocation is deliberately absent.
    pub(super) fn manifest_bytes(&self) -> Result<Vec<u8>, String> {
        let revisions = self
            .launches()
            .into_keys()
            .map(|path| {
                self.semantic
                    .semantic_program_revision(&path)
                    .map(|revision| {
                        (
                            path.clone(),
                            self.signatures[&path].fingerprint,
                            revision.digest(),
                        )
                    })
                    .map_err(|error| format!("semantic link revision: {error:?}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let positions = self
            .anchors
            .iter()
            .map(|(path, anchors)| {
                let rows = anchors
                    .iter()
                    .map(|anchor| {
                        let (file, offset) = self
                            .sources
                            .anchor(*anchor)
                            .ok_or("missing native source anchor")?;
                        let file = self
                            .sources
                            .file(file)
                            .ok_or("missing native source file")?;
                        Ok((
                            file.logical_path().to_path_buf(),
                            offset,
                            hirsel_syntax::ContentHash::of_bytes(file.text().as_bytes()),
                        ))
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                Ok((path, rows))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let links = self
            .foreign_links
            .iter()
            .map(|input| match input {
                LinkInput::Library(name) => (0u32, name.as_bytes().to_vec()),
                LinkInput::File(path) => (1, path.as_os_str().as_encoded_bytes().to_vec()),
            })
            .collect::<Vec<_>>();
        let dispatch = crate::native::foreign::dispatch(&self.view());
        serde_json::to_vec(&(
            1u32,
            self.input.digest,
            revisions,
            positions,
            links,
            self.foreign_unreplayable,
            dispatch.iter().collect::<Vec<_>>(),
            self.functions.iter().collect::<Vec<_>>(),
        ))
        .map_err(|e| e.to_string())
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
