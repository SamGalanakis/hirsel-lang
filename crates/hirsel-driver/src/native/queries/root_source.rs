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
    profiles: Arc<crate::native::ProfileSources>,
    foreign_links: Vec<LinkInput>,
    foreign_unreplayable: bool,
    foreign_threads: BTreeMap<hirsel_syntax::PackageId, ForeignThreads>,
    callees: Option<hirsel_rt::program::CalleeMetadata>,
    requested_run: Option<(DefPath, Arc<hirsel_run_config::ValidatedRun>)>,
    test_entries: Vec<(DefPath, u64)>,
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
            profiles: Arc::new(crate::native::declaration_profile_sources_in(
                declarations,
                &engine.sources,
            )),
            foreign_links: declarations.roots.foreign_links.clone(),
            foreign_unreplayable: declarations.roots.foreign_unreplayable,
            foreign_threads: declarations.roots.foreign_threads.clone(),
            callees: None,
            requested_run: None,
            test_entries: Vec::new(),
        })
    }

    /// Capture the existing checked callee/runner request schemas for a selected
    /// launch. This uses the sole production source algorithm, before native
    /// dispatch; no runtime authority is inferred from source descriptions.
    pub fn capture_for_entry(
        engine: &mut QueryEngine,
        declarations: &NativeDeclarations,
        entry: &DefPath,
    ) -> Result<Self, NativeError> {
        let mut source = Self::capture(engine, declarations).map_err(|error| {
            NativeError::new(
                crate::native::StageName::Manifest,
                format!("native source launch inputs: {error:?}"),
            )
        })?;
        source.callees = Some(crate::native::callee_metadata(engine, entry)?);
        Ok(source)
    }

    /// Capture the host's already bound run as an input of this exact root.
    /// This checks its source revision; runtime grant admission remains at the
    /// executable boundary. Link executors never accept a separate run record.
    pub fn capture_for_run(
        engine: &mut QueryEngine,
        declarations: &NativeDeclarations,
        entry: &DefPath,
        run: Arc<hirsel_run_config::ValidatedRun>,
    ) -> Result<Self, NativeError> {
        let mut source = Self::capture_for_entry(engine, declarations, entry)?;
        source.bind_run(entry, run)?;
        Ok(source)
    }

    fn bind_run(
        &mut self,
        entry: &DefPath,
        run: Arc<hirsel_run_config::ValidatedRun>,
    ) -> Result<(), NativeError> {
        let error = |message: String| NativeError::new(crate::native::StageName::Manifest, message);
        let revision = self
            .semantic
            .semantic_program_revision(entry)
            .map_err(|e| error(format!("native run source revision: {e:?}")))?;
        let kind = self
            .launches()
            .remove(entry)
            .ok_or_else(|| error("native run has no launchable source entry".into()))?;
        let kind = match kind {
            crate::native::link::LaunchKind::Test => hirsel_run_schema::LaunchKind::Test,
            crate::native::link::LaunchKind::Zero | crate::native::link::LaunchKind::Main(_) => {
                hirsel_run_schema::LaunchKind::Main
            }
        };
        run.check_revision(&revision, kind)
            .map_err(|e| error(format!("native run source binding: {e}")))?;
        self.requested_run = Some((entry.clone(), run));
        Ok(())
    }

    pub(super) fn requested_run(&self) -> Option<(DefPath, Arc<hirsel_run_config::ValidatedRun>)> {
        self.requested_run.clone()
    }

    /// Ordered package-test entries and explicit seeds are root inputs. Test
    /// paths and expected snapshots are not consumed by the linker.
    pub fn capture_for_tests(
        engine: &mut QueryEngine,
        declarations: &NativeDeclarations,
        entries: &[(DefPath, u64)],
    ) -> Result<Self, NativeError> {
        let mut source = Self::capture(engine, declarations).map_err(|e| {
            NativeError::new(
                crate::native::StageName::Manifest,
                format!("native test source: {e:?}"),
            )
        })?;
        let launches = source.launches();
        if entries.is_empty()
            || entries.iter().any(|(entry, _)| {
                launches.get(entry) != Some(&crate::native::link::LaunchKind::Test)
            })
        {
            return Err(NativeError::new(
                crate::native::StageName::Manifest,
                "native test selection contains no entries or a non-Test entry",
            ));
        }
        source.test_entries = entries.to_vec();
        Ok(source)
    }

    pub(super) fn test_entries(&self) -> &[(DefPath, u64)] {
        &self.test_entries
    }

    /// Bind the independently checked source closure to selected native unit
    /// summaries. This computes no Value/Ready body and performs no native query.
    pub fn bind(&self, program: &mut ReadyProgram) -> Result<(), NativeError> {
        program.bind_source(&self.view())?;
        if let Some(callees) = &self.callees {
            program.bind_callees(callees.clone())?;
        }
        Ok(())
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

    pub(super) fn profiles(&self) -> Arc<crate::native::ProfileSources> {
        self.profiles.clone()
    }

    /// Exact fields the source projection exposes to debug consumers. Primary
    /// file/base/generated and ordered selectors are all bound; physical paths
    /// are deliberately absent because artifacts consume logical paths only.
    fn profile_binding(&self) -> Result<Vec<u8>, String> {
        let file = |file: &hirsel_span::SourceFile| {
            (
                file.logical_path().to_path_buf(),
                hirsel_syntax::ContentHash::of_bytes(file.text().as_bytes()),
            )
        };
        let rows = self
            .profiles
            .iter()
            .map(|(path, source)| {
                (
                    path,
                    file(&source.file),
                    source.base,
                    source.generated,
                    source
                        .anchors
                        .iter()
                        .map(|(owner, base)| (file(owner), base))
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>();
        serde_json::to_vec(&rows).map_err(|e| e.to_string())
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
            5u32,
            self.input.digest,
            revisions,
            positions,
            self.profile_binding()?,
            links,
            self.foreign_unreplayable,
            dispatch.iter().collect::<Vec<_>>(),
            self.functions.iter().collect::<Vec<_>>(),
            self.callees
                .as_ref()
                .map(hirsel_rt::program::CalleeMetadata::json),
            self.requested_run
                .as_ref()
                .map(|(entry, run)| (entry, run.launch_bytes())),
            &self.test_entries,
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

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_engine::{
        DefinitionQueries, NativeDiscoveryCache, default_context, default_environment,
    };

    #[test]
    fn launch_manifest_binds_the_selected_run_and_refuses_cross_wired_entries() {
        hirsel_engine::with_compiler_stack(|| {
            let store = Arc::new(hirsel_engine::store::MemStore::default());
            let mut engine = QueryEngine::new(default_environment(), crate::compiler_adapters())
                .with_result_store(store.clone());
            let file = engine
                .sources
                .add(
                    "root-run.hir",
                    "pub fn main()->Result[(),()] {Ok(())}".into(),
                )
                .unwrap();
            engine.parse_file(file, &default_context()).unwrap();
            let mut cache = NativeDiscoveryCache::new(engine.types().clone(), store.clone());
            let declarations = NativeDeclarations::collect(&mut engine, &mut cache, &[]).unwrap();
            let inputs = super::super::source_inputs::NativeInstanceInputs::complete(
                &mut engine,
                declarations,
                store,
            )
            .unwrap();
            let entry = hirsel_engine::entry_path(&default_context());
            let mut source =
                NativeLinkSource::capture_for_entry(&mut engine, &inputs.declarations, &entry)
                    .unwrap();
            let unbound = source.manifest_bytes().unwrap();
            let revision = source.semantic.semantic_program_revision(&entry).unwrap();
            let mut record = hirsel_run_config::RunConfig::for_program(
                &revision,
                hirsel_run_schema::LaunchKind::Main,
            );
            let first = Arc::new(record.validated().unwrap());
            source.bind_run(&entry, first.clone()).unwrap();
            let bound = source.manifest_bytes().unwrap();
            assert_ne!(unbound, bound);
            record.inputs.hash_seed = 42;
            let second = Arc::new(record.validated().unwrap());
            source.bind_run(&entry, second.clone()).unwrap();
            let changed = source.manifest_bytes().unwrap();
            assert_ne!(changed, bound);
            assert_eq!(
                source.requested_run(),
                Some((entry.clone(), second.clone()))
            );
            let wrong_kind = hirsel_run_config::RunConfig::for_program(
                &revision,
                hirsel_run_schema::LaunchKind::Test,
            )
            .validated()
            .unwrap();
            assert!(source.bind_run(&entry, Arc::new(wrong_kind)).is_err());
            assert!(source.bind_run(&DefPath::core("other"), first).is_err());
            assert_eq!(source.manifest_bytes().unwrap(), changed);
            assert_eq!(source.requested_run(), Some((entry, second)));
        });
    }

    #[test]
    fn launch_manifest_binds_every_profile_field_and_preserves_physical_relocation() {
        hirsel_engine::with_compiler_stack(|| {
            let store = Arc::new(hirsel_engine::store::MemStore::default());
            let mut engine = QueryEngine::new(default_environment(), crate::compiler_adapters())
                .with_result_store(store.clone());
            let file = engine
                .sources
                .add(
                    "root-profile.hir",
                    "fn helper()->u64 {7u64}\npub fn main()->Result[(),u64] {Error(helper())}"
                        .into(),
                )
                .unwrap();
            engine.parse_file(file, &default_context()).unwrap();
            let mut cache = NativeDiscoveryCache::new(engine.types().clone(), store.clone());
            let declarations = NativeDeclarations::collect(&mut engine, &mut cache, &[]).unwrap();
            let inputs = super::super::source_inputs::NativeInstanceInputs::complete(
                &mut engine,
                declarations,
                store,
            )
            .unwrap();
            let mut source = NativeLinkSource::capture(&mut engine, &inputs.declarations).unwrap();
            let original = source.profiles.as_ref().clone();
            let baseline = source.manifest_bytes().unwrap();
            let path = original
                .iter()
                .find(|(_, profile)| !profile.anchors.is_empty())
                .unwrap()
                .0
                .clone();
            for change in 0..5 {
                let mut profiles = original.clone();
                let profile = profiles.get_mut(&path).unwrap();
                match change {
                    0 => profile.base += 1,
                    1 => profile.generated = Some(1),
                    2 => {
                        let mut map = SourceMap::default();
                        let file = map.add("another.hir", profile.file.text().into()).unwrap();
                        profile.file = map.file(file).unwrap().clone();
                    }
                    3 => profile.anchors[0].1 += 1,
                    4 => {
                        let mut second = profile.anchors[0].clone();
                        second.1 += 1;
                        profile.anchors.push(second);
                    }
                    _ => unreachable!(),
                }
                source.profiles = Arc::new(profiles);
                assert_ne!(
                    source.manifest_bytes().unwrap(),
                    baseline,
                    "profile field {change} must enter root action inputs"
                );
            }
            let mut profiles = original.clone();
            let profile = profiles.get_mut(&path).unwrap();
            let mut second = profile.anchors[0].clone();
            second.1 += 1;
            profile.anchors.push(second);
            source.profiles = Arc::new(profiles.clone());
            let ordered = source.manifest_bytes().unwrap();
            profiles.get_mut(&path).unwrap().anchors.reverse();
            source.profiles = Arc::new(profiles);
            assert_ne!(source.manifest_bytes().unwrap(), ordered);
            let mut profiles = original;
            let profile = profiles.get_mut(&path).unwrap();
            let mut moved = SourceMap::default();
            let file = moved
                .add_logical(
                    "/relocated/root-profile.hir",
                    profile.file.logical_path(),
                    profile.file.text().into(),
                )
                .unwrap();
            profile.file = moved.file(file).unwrap().clone();
            profile.path = profile.file.path.clone();
            source.profiles = Arc::new(profiles);
            assert_eq!(
                source.manifest_bytes().unwrap(),
                baseline,
                "physical relocation is not an emitted debug input"
            );
        });
    }
}
