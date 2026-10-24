use super::*;
use hirsel_engine::store::{Bytes, EntryKey, MemStore, ResultStore};
use hirsel_engine::{DefinitionQueries, QueryEngine, Verify, default_context, default_environment};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
struct CountedStore {
    inner: MemStore,
    reads: AtomicU64,
}
impl ResultStore for CountedStore {
    fn get(&self, key: &EntryKey) -> Option<Bytes> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.inner.get(key)
    }
    fn put(&self, key: &EntryKey, bytes: &[u8]) {
        self.inner.put(key, bytes);
    }
    fn verifier(&self) -> Option<Arc<hirsel_validate::VerifierCache>> {
        self.inner.verifier()
    }
    fn name(&self) -> &'static str {
        "root-source-test"
    }
}
struct Fixture {
    store: Arc<CountedStore>,
    program: hirsel_rc::instance::Program,
    operations: Arc<hirsel_layout::operation::OperationRegistry>,
    roots: NativeSourceRoots,
    declarations: BTreeMap<DefPath, NativeDefinitionProduct>,
    entries: BTreeMap<DefPath, u32>,
    anchors: BTreeMap<DefPath, Vec<Anchor>>,
    signatures: Arc<SignatureSnapshot>,
    semantic: Arc<NativeSemanticProgram>,
    selected: Vec<DefPath>,
    lower: NativeOriginalReader,
    empty: NativeOriginalReader,
    validated: ValidatedNativeInput,
}
impl Fixture {
    fn new(verify: Verify) -> Self {
        let store = Arc::new(CountedStore::default());
        let mut engine = QueryEngine::new(default_environment(), crate::compiler_adapters())
            .with_result_store(store.clone());
        engine.verify = verify;
        let file = engine
            .sources
            .add(
                "immutable-launch.hir",
                "fn helper()->u64{7u64}\npub fn main()->Result[(),u64]{Error(helper())}".into(),
            )
            .unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let selected = vec![hirsel_engine::entry_path(&default_context())];
        let roots = engine.native_source_roots(&selected).unwrap();
        let preliminary = engine
            .native_semantic_program(roots.definitions.clone())
            .unwrap();
        let mut paths: BTreeSet<_> = roots.definitions.iter().cloned().collect();
        paths.extend(
            preliminary
                .source_bindings()
                .map(|(path, _, _)| path.clone()),
        );
        let paths: Vec<_> = paths.into_iter().collect();
        let lower = engine
            .native_original_reader(&paths, store.clone())
            .unwrap();
        let raw = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(raw).unwrap();
        let mut program = super::super::adapt::program(&validated);
        let operations = Arc::new(program.operation_registry());
        let entries = program.definitions.iter().filter(|(path,_)| {
            matches!(&program.signatures[*path].kind,SignatureKind::Function(f) if f.generics.is_empty())
        }).map(|(path,definition)|(path.clone(),definition.entry.0)).collect();
        let declarations: BTreeMap<_, _> = paths
            .iter()
            .map(|path| {
                (
                    path.clone(),
                    engine.native_definition_product(path).unwrap(),
                )
            })
            .collect();
        let anchors = declarations
            .iter()
            .map(|(path, product)| {
                (
                    path.clone(),
                    validated.snapshot().definitions.get(path).map_or_else(
                        || vec![product.source.syntax.anchor],
                        |body| hirsel_tir::source_anchors(body),
                    ),
                )
            })
            .collect();
        let semantic = Arc::new(
            engine
                .native_semantic_program(roots.definitions.clone())
                .unwrap(),
        );
        let signatures = engine.signature_snapshot();
        let empty = engine.native_original_reader(&[], store.clone()).unwrap();
        program.definitions.clear();
        // The constructor cannot fall back to source/controller queries.
        drop(engine);
        store.reads.store(0, Ordering::Relaxed);
        Self {
            store,
            program,
            operations,
            roots,
            declarations,
            entries,
            anchors,
            signatures,
            semantic,
            selected,
            lower,
            empty,
            validated,
        }
    }
    fn facts(&self) -> NativeLinkFacts<'_> {
        NativeLinkFacts {
            program: &self.program,
            operations: self.operations.clone(),
            roots: &self.roots,
            declarations: &self.declarations,
            entries: &self.entries,
            anchors: &self.anchors,
            signatures: self.signatures.clone(),
            semantic: self.semantic.clone(),
            selected: &self.selected,
            callees: None,
        }
    }
}

#[test]
fn settled_source_assembles_without_engine_or_body_reads_and_binds_actual_ready() {
    hirsel_engine::with_compiler_stack(|| {
        for verify in [Verify::Off, Verify::On] {
            let fixture = Fixture::new(verify);
            let source = NativeLinkSource::from_settled(fixture.facts(), &fixture.lower).unwrap();
            assert_eq!(fixture.store.reads.load(Ordering::Relaxed), 0);
            assert_eq!(source.functions(), &fixture.entries);
            let options =
                super::super::Options::new(super::super::TargetSpec::x86_64_unknown_linux_gnu());
            let mut ready = super::super::build(&fixture.validated, &options).unwrap();
            let entry = &fixture.selected[0];
            let revision = ready.revision(entry).unwrap().clone();
            source.bind(&mut ready).unwrap();
            assert_eq!(ready.revision(entry), Some(&revision));
            assert!(
                super::super::run_heap(&ready, entry, &source.sites())
                    .unwrap()
                    .observation
                    .failures
                    .is_empty()
            );
        }
    });
}

#[test]
fn immutable_constructor_refuses_missing_lower_and_cross_wired_source_facts() {
    hirsel_engine::with_compiler_stack(|| {
        let mut fixture = Fixture::new(Verify::On);
        assert!(NativeLinkSource::from_settled(fixture.facts(), &fixture.empty).is_err());
        let mut foreign = QueryEngine::new(default_environment(), crate::compiler_adapters());
        let mut facts = fixture.facts();
        facts.signatures = foreign.signature_snapshot();
        assert!(NativeLinkSource::from_settled(facts, &fixture.lower).is_err());
        let path = fixture.selected[0].clone();
        let original = fixture.entries[&path];
        fixture.entries.insert(path.clone(), u32::MAX);
        assert!(NativeLinkSource::from_settled(fixture.facts(), &fixture.lower).is_err());
        fixture.entries.insert(path.clone(), original);
        fixture.roots.foreign_unreplayable = !fixture.roots.foreign_unreplayable;
        assert!(NativeLinkSource::from_settled(fixture.facts(), &fixture.lower).is_err());
        fixture.roots.foreign_unreplayable = !fixture.roots.foreign_unreplayable;
        fixture
            .roots
            .foreign_links
            .push(LinkInput::Library("unbound".into()));
        assert!(NativeLinkSource::from_settled(fixture.facts(), &fixture.lower).is_err());
        fixture.roots.foreign_links.pop();
        let old = fixture.declarations[&path].source.implementation_revision;
        fixture
            .declarations
            .get_mut(&path)
            .unwrap()
            .source
            .implementation_revision = hirsel_syntax::ContentHash::of_bytes(b"stale source");
        assert!(NativeLinkSource::from_settled(fixture.facts(), &fixture.lower).is_err());
        fixture
            .declarations
            .get_mut(&path)
            .unwrap()
            .source
            .implementation_revision = old;
        let anchors = fixture.anchors.get_mut(&path).unwrap();
        anchors.push(anchors[0]);
        assert!(NativeLinkSource::from_settled(fixture.facts(), &fixture.lower).is_err());
    });
}

#[test]
fn immutable_launch_manifest_binds_run_seed_and_all_profile_fields() {
    hirsel_engine::with_compiler_stack(|| {
        let fixture = Fixture::new(Verify::On);
        let mut source = NativeLinkSource::from_settled(fixture.facts(), &fixture.lower).unwrap();
        let entry = &fixture.selected[0];
        let revision = source.semantic().semantic_program_revision(entry).unwrap();
        let mut run = hirsel_run_config::RunConfig::for_program(
            &revision,
            hirsel_run_schema::LaunchKind::Main,
        );
        let unbound = source.manifest_bytes().unwrap();
        source
            .bind_run(entry, Arc::new(run.validated().unwrap()))
            .unwrap();
        let bound = source.manifest_bytes().unwrap();
        assert_ne!(unbound, bound);
        run.inputs.hash_seed = 42;
        source
            .bind_run(entry, Arc::new(run.validated().unwrap()))
            .unwrap();
        assert_ne!(source.manifest_bytes().unwrap(), bound);
        let wrong = hirsel_run_config::RunConfig::for_program(
            &revision,
            hirsel_run_schema::LaunchKind::Test,
        )
        .validated()
        .unwrap();
        assert!(source.bind_run(entry, Arc::new(wrong)).is_err());
        let original = source.profiles.as_ref().clone();
        let baseline = source.manifest_bytes().unwrap();
        for change in 0..4 {
            let mut profiles = original.clone();
            let profile = profiles.get_mut(entry).unwrap();
            match change {
                0 => profile.base += 1,
                1 => profile.generated = Some(1),
                2 => profile.anchors[0].1 += 1,
                3 => {
                    let mut map = SourceMap::default();
                    let file = map
                        .add("changed-logical.hir", profile.file.text().into())
                        .unwrap();
                    profile.file = map.file(file).unwrap().clone();
                }
                _ => unreachable!(),
            }
            source.profiles = Arc::new(profiles);
            assert_ne!(
                source.manifest_bytes().unwrap(),
                baseline,
                "profile field {change}"
            );
        }
        let mut profiles = original;
        let profile = profiles.get_mut(entry).unwrap();
        let mut moved = SourceMap::default();
        let file = moved
            .add_logical(
                "/moved/immutable-launch.hir",
                profile.file.logical_path(),
                profile.file.text().into(),
            )
            .unwrap();
        profile.file = moved.file(file).unwrap().clone();
        profile.path = profile.file.path.clone();
        source.profiles = Arc::new(profiles);
        assert_eq!(source.manifest_bytes().unwrap(), baseline);
    });
}
