use super::*;
use hirsel_engine::store::DirStore;
fn output_path(store: &DirStore, reference: Reference) -> std::path::PathBuf {
    hirsel_artifact_store::Store::new(store.root().into()).output_path(reference)
}
use hirsel_lir::layout_types::TargetSpec;
use std::cell::Cell;

const INPUT: Domain = Domain {
    kind: 0x4f00ff00,
    schema: 1,
};
fn hash(s: &str) -> ContentHash {
    ContentHash::of_bytes(s.as_bytes())
}
fn input(store: &dyn ResultStore, s: &str) -> Reference {
    store.put_content(INPUT, s.as_bytes()).unwrap()
}
fn product(store: &dyn ResultStore, s: &str) -> ProductRef {
    ProductRef {
        address: Address(hash(s)),
        action: Action(hash(s)),
        record: input(store, s),
    }
}
fn key(name: &str) -> CodegenKey {
    let types = hirsel_types::TypeInterner::new(None);
    CodegenKey {
        owner: InstanceAddress {
            instance: hirsel_layout::instance::InstanceKey::new(
                &hirsel_syntax::DefPath::core(name),
                0,
                &[],
                &types,
                None,
                None,
            )
            .unwrap(),
            target_abi: TargetSpec::x86_64_unknown_linux_gnu().digest(),
            policy: hash("VerifyOn"),
        },
        backend: "llvm".into(),
        backend_version: hash("llvm23.1.4"),
        target_features: hash("features"),
        profile_options: hash("debug"),
        runtime_abi: hash("abi"),
    }
}
fn ready(store: &dyn ResultStore) -> ReadyProduct {
    ReadyProduct {
        product: product(store, "Ready product"),
        debug: true,
        ready: input(store, "Ready"),
        layout_closure: input(store, "layouts"),
        emission_context: input(store, "emit"),
        unit_summary: input(store, "summary"),
        optimizer_reads: vec![],
    }
}
fn object() -> ObjectArtifact {
    ObjectArtifact {
        ahead: None,
        target: TargetSpec::x86_64_unknown_linux_gnu().digest(),
        format: hirsel_lir::layout_types::ObjectFormat::Elf,
        bytes: b"fixture object".to_vec(),
        exports: vec![],
        imports: vec![],
        relocations: vec![],
        abi: hash("abi"),
        reproducibility: hirsel_lir::stage::Reproducibility {
            toolchain: "fixture".into(),
            options: hash("options"),
            lir_version: hirsel_lir::LIR_VERSION,
            runtime_abi_version: hirsel_lir::RUNTIME_ABI_VERSION,
        },
        debug: vec![],
    }
}
fn inputs(store: &dyn ResultStore, fragments: Vec<FragmentProduct>) -> LinkInputs {
    LinkInputs {
        execution: vec![],
        fragments,
        static_manifest: input(store, "statics"),
        runtime_archives: vec![input(store, "runtime")],
        foreign_inputs: vec![],
        launch_manifest: input(store, "launch"),
    }
}
fn link_key() -> LinkKey {
    LinkKey {
        roots: vec![key("main").owner].into(),
        target: object().target,
        linker_options: hash("linker"),
    }
}

#[test]
fn addresses_cover_every_option_and_canonical_roots() {
    let original = key("main");
    for axis in 0..9 {
        let mut changed = original.clone();
        match axis {
            0 => changed.owner = key("other").owner,
            1 => changed.owner.target_abi = hash("target2"),
            2 => changed.owner.policy = hash("VerifyOff"),
            3 => changed.backend = "cranelift".into(),
            4 => changed.backend_version = hash("backend2"),
            5 => changed.target_features = hash("features2"),
            6 => changed.profile_options = hash("release"),
            7 => changed.runtime_abi = hash("abi2"),
            8 => changed.owner.instance = key("other").owner.instance,
            _ => unreachable!(),
        }
        assert_ne!(original.address(), changed.address(), "axis {axis}");
    }
    let mut a = link_key();
    let mut roots = a.roots.to_vec();
    roots.push(key("other").owner);
    a.roots = roots.into();
    let mut b = a.clone();
    Arc::make_mut(&mut b.roots).reverse();
    assert_eq!(a.address(), b.address());
    b.target = hash("different target");
    assert_ne!(a.address(), b.address());
    b = a.clone();
    b.linker_options = hash("different linker");
    assert_ne!(a.address(), b.address());
}

#[test]
fn hit_never_materializes_and_equal_recomputation_backdates() {
    let dir = tempfile::tempdir().unwrap();
    let store = DirStore::new(dir.path().into());
    let key = key("main");
    let mut ready = ready(&store);
    let cold = codegen(&store, &key, &ready, || Ok(object())).unwrap();
    let warm = codegen(&store, &key, &ready, || panic!("hit materialized a body")).unwrap();
    assert_eq!(cold.product, warm.product);
    ready.product.action = Action(hash("recomputed with equal output"));
    assert_eq!(
        codegen(&store, &key, &ready, || panic!(
            "equal action output missed"
        ))
        .unwrap()
        .product,
        cold.product
    );
    ready.ready = input(&store, "changed Ready, equal object");
    let changed = codegen(&store, &key, &ready, || Ok(object())).unwrap();
    assert_ne!(changed.product.action, cold.product.action);
    assert_eq!(
        changed.product.record, cold.product.record,
        "equal output failed to backdate"
    );
    let fresh = DirStore::new(dir.path().into());
    assert_eq!(
        codegen(&fresh, &key, &ready, || panic!("fresh hit decoded body"))
            .unwrap()
            .product,
        changed.product
    );
}

#[test]
fn optimizer_body_reads_invalidate_release_but_not_ordinary_debug_calls() {
    let dir = tempfile::tempdir().unwrap();
    let store = DirStore::new(dir.path().into());
    let debug = key("caller");
    let ordinary = ready(&store);
    let d = codegen(&store, &debug, &ordinary, || Ok(object())).unwrap();
    let mut release = debug.clone();
    release.profile_options = hash("release");
    let mut copied = ordinary.clone();
    copied.optimizer_reads = vec![product(&store, "callee before")];
    let r = codegen(&store, &release, &copied, || Ok(object())).unwrap();
    copied.optimizer_reads[0] = product(&store, "same-summary callee body after");
    let calls = Cell::new(0);
    let after = codegen(&store, &release, &copied, || {
        calls.set(calls.get() + 1);
        Ok(object())
    })
    .unwrap();
    assert_eq!(calls.get(), 1);
    assert_ne!(r.product.action, after.product.action);
    assert_eq!(
        d.product,
        codegen(&store, &debug, &ordinary, || panic!(
            "ordinary caller read callee body"
        ))
        .unwrap()
        .product
    );
}

#[test]
fn complete_ready_context_controls_hits() {
    let dir = tempfile::tempdir().unwrap();
    let store = DirStore::new(dir.path().into());
    let key = key("main");
    let original = ready(&store);
    for axis in 0..5 {
        let _ = codegen(&store, &key, &original, || Ok(object())).unwrap();
        let mut changed = original.clone();
        let next = input(&store, &format!("changed-{axis}"));
        match axis {
            0 => changed.product.record = next,
            1 => changed.ready = next,
            2 => changed.layout_closure = next,
            3 => changed.emission_context = next,
            4 => changed.unit_summary = next,
            _ => unreachable!(),
        }
        let called = Cell::new(false);
        codegen(&store, &key, &changed, || {
            called.set(true);
            Ok(object())
        })
        .unwrap();
        assert!(called.get(), "axis {axis}");
    }
}

#[test]
fn corrupt_or_missing_object_recomputes_instead_of_hitting() {
    let dir = tempfile::tempdir().unwrap();
    let store = DirStore::new(dir.path().into());
    let key = key("main");
    let ready = ready(&store);
    for bad in [b"truncated".as_slice(), b"another corrupt payload"] {
        let cold = codegen(&store, &key, &ready, || Ok(object())).unwrap();
        let record: ObjectRecord = get_record(&store, OBJECT, cold.object).unwrap();
        let blob: BlobRecord = get_record(&store, BLOB, record.bytes.into()).unwrap();
        let path = output_path(&store, blob.chunks[0].clone().into());
        std::fs::write(&path, bad).unwrap();
        let called = Cell::new(false);
        codegen(&store, &key, &ready, || {
            called.set(true);
            Ok(object())
        })
        .unwrap();
        assert!(called.get());
    }
}

#[test]
fn link_order_is_canonical_and_archive_order_is_significant() {
    let dir = tempfile::tempdir().unwrap();
    let store = DirStore::new(dir.path().into());
    let ready = ready(&store);
    let a = codegen(&store, &key("a"), &ready, || Ok(object())).unwrap();
    let b = codegen(&store, &key("b"), &ready, || Ok(object())).unwrap();
    let mut manifest = inputs(&store, vec![b, a]);
    let calls = Cell::new(0);
    let mut compute = |_: &LinkKey, _: &LinkInputs, _: &[ObjectArtifact]| {
        calls.set(calls.get() + 1);
        Ok(b"executable".to_vec())
    };
    let first = link(&store, &link_key(), &manifest, &mut compute).unwrap();
    manifest.fragments.reverse();
    let second = link(&store, &link_key(), &manifest, &mut compute).unwrap();
    assert_eq!(first.product, second.product);
    assert_eq!(calls.get(), 1);
    manifest
        .runtime_archives
        .push(input(&store, "second runtime"));
    let third = link(&store, &link_key(), &manifest, &mut compute).unwrap();
    manifest.runtime_archives.reverse();
    let fourth = link(&store, &link_key(), &manifest, &mut compute).unwrap();
    assert_ne!(third.product.action, fourth.product.action);
    assert_eq!(third.product.record, fourth.product.record);
    assert_eq!(calls.get(), 3);
    manifest.fragments.push(manifest.fragments[0].clone());
    assert!(link(&store, &link_key(), &manifest, &mut compute).is_err());
}

#[test]
fn linked_hit_refuses_stale_fragments_and_relinks_missing_executable() {
    let dir = tempfile::tempdir().unwrap();
    let store = DirStore::new(dir.path().into());
    let fragment = codegen(&store, &key("main"), &ready(&store), || Ok(object())).unwrap();
    let manifest = inputs(&store, vec![fragment.clone()]);
    let mut compute = |_: &LinkKey, _: &LinkInputs, _: &[ObjectArtifact]| Ok(b"exe".to_vec());
    let first = link(&store, &link_key(), &manifest, &mut compute).unwrap();
    std::fs::remove_file(output_path(&store, first.executable)).unwrap();
    let called = Cell::new(false);
    link(&store, &link_key(), &manifest, &mut |_, _, _| {
        called.set(true);
        Ok(b"exe".to_vec())
    })
    .unwrap();
    assert!(called.get());
    std::fs::remove_file(output_path(&store, fragment.object)).unwrap();
    assert!(
        link(&store, &link_key(), &manifest, &mut |_, _, _| panic!(
            "stale fragment reached linker"
        ))
        .is_err()
    );
}

#[test]
fn static_foreign_launch_runtime_and_linker_changes_invalidate() {
    let dir = tempfile::tempdir().unwrap();
    let store = DirStore::new(dir.path().into());
    let original = inputs(&store, vec![]);
    let key = link_key();
    let mut compute = |_: &LinkKey, _: &LinkInputs, _: &[ObjectArtifact]| Ok(b"exe".to_vec());
    let first = link(&store, &key, &original, &mut compute).unwrap();
    for axis in 0..5 {
        let mut changed = original.clone();
        let mut changed_key = key.clone();
        let next = input(&store, &format!("manifest-{axis}"));
        match axis {
            0 => changed.static_manifest = next,
            1 => changed.foreign_inputs.push(next),
            2 => changed.launch_manifest = next,
            3 => changed.runtime_archives[0] = next,
            4 => changed_key.linker_options = hash("new linker"),
            _ => unreachable!(),
        }
        let after = link(&store, &changed_key, &changed, &mut compute).unwrap();
        assert_ne!(first.product.action, after.product.action, "axis {axis}");
    }
}

#[test]
fn executable_materialization_reuses_inode_and_repairs_wrong_mode_or_bytes() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let dir = tempfile::tempdir().unwrap();
    let store = DirStore::new(dir.path().join("store"));
    let manifest = inputs(&store, vec![]);
    let linked = link(&store, &link_key(), &manifest, &mut |_, _, _| {
        Ok(b"#!/bin/sh\nexit 0\n".to_vec())
    })
    .unwrap();
    let path = dir.path().join("out/program");
    materialize_executable(&store, &linked, &path).unwrap();
    let ino = path.metadata().unwrap().ino();
    materialize_executable(&store, &linked, &path).unwrap();
    assert_eq!(path.metadata().unwrap().ino(), ino);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    materialize_executable(&store, &linked, &path).unwrap();
    assert_ne!(path.metadata().unwrap().ino(), ino);
    assert!(
        std::process::Command::new(&path)
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(&path, b"wrong").unwrap();
    materialize_executable(&store, &linked, &path).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"#!/bin/sh\nexit 0\n");
    std::fs::remove_file(&path).unwrap();
    let fresh = DirStore::new(dir.path().join("store"));
    materialize_executable(&fresh, &linked, &path).unwrap();
    assert!(path.exists());
}

#[test]
fn chunk_boundaries_domains_and_complete_lengths_are_checked() {
    let dir = tempfile::tempdir().unwrap();
    let store = DirStore::new(dir.path().into());
    let bytes = vec![0x5a; MAX_RECORD + 1];
    let reference = put_blob(&store, &bytes).unwrap();
    assert_eq!(read_blob(&store, reference).unwrap(), bytes);
    let mut record: BlobRecord = get_record(&store, BLOB, reference).unwrap();
    record.len += 1;
    let bad = put_record(&store, BLOB, &record).unwrap();
    assert!(validate_link_input(&store, bad).is_err());
    record.len -= 1;
    record.chunks[0].kind = INPUT.kind;
    let bad = put_record(&store, BLOB, &record).unwrap();
    assert!(validate_link_input(&store, bad).is_err());
    assert!(
        validate_link_input(
            &store,
            Reference {
                domain: INPUT,
                ..reference
            }
        )
        .is_err()
    );
}

// Test-only NativeProducts implementation; never a production fallback.
struct NativeFixture {
    program: Arc<crate::native::ReadyProgram>,
    address: InstanceAddress,
    descriptor: Arc<ReadyProduct>,
    owner: usize,
    opt: hirsel_lir::emission::OptLevel,
    materializations: Arc<Cell<usize>>,
}
impl NativeProducts for NativeFixture {
    fn instance(
        &mut self,
        _: &InstanceAddress,
    ) -> Result<Arc<super::super::queries::InstanceProduct>, String> {
        Err("fixture does not produce Value".into())
    }
    fn arena_summary(
        &mut self,
        _: &super::super::queries::ComponentKey,
    ) -> Result<Arc<super::super::queries::ArenaSummary>, String> {
        Err("fixture does not produce arena summaries".into())
    }
    fn ready(&mut self, key: &InstanceAddress) -> Result<Arc<ReadyProduct>, String> {
        if *key != self.address {
            return Err("unknown fixture owner".into());
        }
        Ok(self.descriptor.clone())
    }
    fn with_ready<R>(
        &mut self,
        products: &[Arc<ReadyProduct>],
        use_units: impl FnOnce(
            &[&hirsel_lir::stage::ValidatedUnit<hirsel_lir::stage::Ready>],
            &hirsel_lir::emission::EmitInputs<'_>,
        ) -> Result<R, String>,
    ) -> Result<R, String> {
        if products.len() != 1 || products[0].product != self.descriptor.product {
            return Err("foreign fixture descriptor".into());
        }
        self.materializations.set(self.materializations.get() + 1);
        let program = &self.program;
        let units = [&program.units[self.owner]];
        let inputs = hirsel_lir::emission::EmitInputs {
            ahead: None,
            continuation: None,
            alternate: false,
            index: None,
            input: &program.input,
            target: &program.target,
            layouts: &program.table,
            keys: &program.keys,
            manifest: &program.manifest,
            statics: &program.statics,
            inline_instances: &program.inline_instances,
            available_externally: &Default::default(),
            callbacks: !program.callbacks.is_empty(),
            tracking: program.tracking(),
            foreign: &program.foreign_dispatch,
            opt: self.opt,
            verify_backend: true,
            measurement: false,
            fuel: program.fuel,
        };
        use_units(&units, &inputs)
    }
}

#[test]
fn actual_ready_fixture_emits_links_restores_and_exits_on_both_backends() {
    hirsel_engine::with_compiler_stack(|| {
        use crate::native;
        use hirsel_engine::{QueryEngine, default_context, default_environment, entry_path};
        let mut engine = QueryEngine::new(default_environment(), crate::compiler_adapters());
        let file = engine
            .sources
            .add(
                "object-fixture.hir",
                "pub fn main()->Result[(),String]{Ok(())}".into(),
            )
            .unwrap();
        let checked = hirsel_engine::check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let entry = entry_path(&default_context());
        let reference = hirsel_engine::run_observed_snapshot(
            &mut engine,
            &hirsel_engine::RunConfiguration {
                entry: entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            hirsel_engine::SimulationConfig::default(),
        )
        .unwrap();
        let reference = native::Observation::of_reference(reference.observation.as_ref().unwrap());
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let mut options = native::Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        options.validation = native::Validation::EveryStage;
        let program = Arc::new(native::build(&validated, &options).unwrap());
        let owner = program
            .units
            .iter()
            .position(|u| u.unit().header.definition == entry)
            .unwrap();
        let unit = program.units[owner].unit();
        let instance =
            hirsel_layout::instance::InstanceKey::new(&entry, 0, &[], engine.types(), None, None)
                .unwrap();
        assert_eq!(instance.digest(), unit.header.instance);
        let address = InstanceAddress {
            instance,
            target_abi: program.target.digest(),
            policy: hash("V/import/C/Ready every stage"),
        };
        let root = tempfile::tempdir().unwrap();
        let tools =
            native::Toolchain::in_tree(&root.path().join("toolchain"), &[&program.target]).unwrap();
        let sites = native::Sites::new(validated.snapshot());
        let heap = native::run_heap(&program, &entry, &sites).unwrap();
        assert_eq!(heap.observation, reference, "checked H");
        for backend in [native::Backend::Cranelift, native::Backend::Llvm] {
            for opt in [
                hirsel_lir::emission::OptLevel::None,
                hirsel_lir::emission::OptLevel::Default,
            ] {
                let dir = root.path().join(format!("{}-{opt:?}", backend.name()));
                std::fs::create_dir_all(&dir).unwrap();
                let store: Arc<dyn hirsel_engine::store::ResultStore> =
                    Arc::new(hirsel_engine::store::MemStore::default());
                let mut descriptor = ready(store.as_ref());
                descriptor.ready = store
                    .put_content(INPUT, hirsel_codec::to_text(unit).as_bytes())
                    .unwrap();
                descriptor.debug = true;
                let count = Arc::new(Cell::new(0));
                let fixture = NativeFixture {
                    program: program.clone(),
                    address: address.clone(),
                    descriptor: Arc::new(descriptor),
                    owner,
                    opt,
                    materializations: count.clone(),
                };
                let baseline = native::emit_backend_objects(
                    &program,
                    backend,
                    opt,
                    native::Partition::Singleton,
                    false,
                    None,
                )
                .unwrap();
                let owner_symbol =
                    hirsel_lir::emission::symbol_name(&entry, Some(&unit.header.instance));
                let remaining: Vec<_> = baseline
                    .into_iter()
                    .filter(|object| {
                        !object
                            .exports
                            .iter()
                            .any(|export| export.name == owner_symbol)
                    })
                    .collect();
                let supplementary =
                    store_link_input(store.as_ref(), hirsel_codec::to_text(&remaining).as_bytes())
                        .unwrap();
                let links = Cell::new(0);
                let link_store = store.clone();
                let mut queries = ObjectQueries::new(
                    store.clone(),
                    fixture,
                    |_: &LinkKey, _: &LinkInputs, objects: &[ObjectArtifact]| {
                        links.set(links.get() + 1);
                        let mut objects = objects.to_vec();
                        objects.extend(remaining.iter().cloned());
                        let path = native::link_objects(&program, &entry, &objects, &tools, &dir)
                            .map_err(error)?;
                        std::fs::read(path).map_err(error)
                    },
                );
                let key = CodegenKey {
                    owner: address.clone(),
                    backend: backend.name().into(),
                    backend_version: hash(&native::llvm_version()),
                    target_features: hash("default target features"),
                    profile_options: hash(&format!("{opt:?},debug=true")),
                    runtime_abi: hash("current ABI"),
                };
                let cold = queries.codegen(&key).unwrap();
                let warm = queries.codegen(&key).unwrap();
                assert_eq!(cold.product, warm.product);
                assert_eq!(count.get(), 1, "hit materialized Ready");
                let mut manifest = inputs(link_store.as_ref(), vec![(*cold).clone()]);
                manifest.static_manifest = supplementary;
                let archive = tools
                    .runtime
                    .get(&(
                        program.target.triple.clone(),
                        native::RuntimeBuild::default(),
                    ))
                    .unwrap();
                manifest.runtime_archives =
                    vec![put_blob(link_store.as_ref(), &std::fs::read(archive).unwrap()).unwrap()];
                let key = LinkKey {
                    roots: vec![address.clone()].into(),
                    target: program.target.digest(),
                    linker_options: hash("actual pinned lld+kit fixture"),
                };
                let cold = queries.link(&key, &manifest).unwrap();
                let warm = queries.link(&key, &manifest).unwrap();
                assert_eq!(cold.product, warm.product);
                assert_eq!(links.get(), 1);
                let path = dir.join("restored-program");
                materialize_executable(store.as_ref(), &cold, &path).unwrap();
                let run = native::launch(&path, &program.target, &tools, &dir, false).unwrap();
                assert_eq!(run.exit, Some(0), "{run:?}");
                let observation = native::observe(&run, &program, &sites).unwrap();
                assert_eq!(observation, reference, "{backend:?} {opt:?}");
            }
        }
    });
}

#[test]
fn explicit_recompute_checks_exact_cached_bytes() {
    let store = hirsel_engine::store::MemStore::default();
    let key = key("main");
    let descriptor = ready(&store);
    let cold = codegen(&store, &key, &descriptor, || Ok(object())).unwrap();
    let calls = Cell::new(0);
    let checked = codegen_checked(
        &store,
        &key,
        &descriptor,
        &[],
        VerifyStore::Recompute,
        || {
            calls.set(calls.get() + 1);
            Ok(object())
        },
    )
    .unwrap();
    assert_eq!(calls.get(), 1);
    assert_eq!(cold.product, checked.product);
    let error = codegen_checked(
        &store,
        &key,
        &descriptor,
        &[],
        VerifyStore::Recompute,
        || {
            let mut changed = object();
            changed.bytes.push(0);
            Ok(changed)
        },
    )
    .unwrap_err();
    assert!(error.contains("differs from cached output"), "{error}");
}

#[test]
fn object_hit_never_reads_ready_body_payload() {
    struct NoBody {
        inner: hirsel_engine::store::MemStore,
        body: Reference,
    }
    impl ResultStore for NoBody {
        fn get(
            &self,
            key: &hirsel_engine::store::EntryKey,
        ) -> Option<hirsel_artifact_store::Bytes> {
            self.inner.get(key)
        }
        fn put(&self, key: &hirsel_engine::store::EntryKey, value: &[u8]) {
            self.inner.put(key, value)
        }
        fn get_content(&self, r: Reference) -> Option<hirsel_artifact_store::Bytes> {
            assert_ne!(r, self.body, "object hit read unchanged Ready body");
            self.inner.get_content(r)
        }
        fn put_content(&self, d: Domain, b: &[u8]) -> Option<Reference> {
            self.inner.put_content(d, b)
        }
        fn name(&self) -> &'static str {
            "fixture-no-body"
        }
    }
    let inner = hirsel_engine::store::MemStore::default();
    let descriptor = ready(&inner);
    let store = NoBody {
        inner,
        body: descriptor.ready,
    };
    let key = key("main");
    let cold = codegen(&store, &key, &descriptor, || Ok(object())).unwrap();
    let warm = codegen(&store, &key, &descriptor, || {
        panic!("hit materialized Ready")
    })
    .unwrap();
    assert_eq!(cold.product, warm.product);
}

use super::tasks::{LinkDependencies, ObjectTaskFactory, ObjectTaskRequest, ObjectTaskSnapshot};
use hirsel_engine::runtime::{MemoryWeight, QueryContext, QueryFactory, QueryId, QueryPoll};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Poll;

struct TaskSnapshot {
    request: ObjectTaskRequest,
    execution: Option<QueryId>,
    ready: ReadyProduct,
    link: LinkDependencies,
    result: ProductRef,
    store: Arc<hirsel_engine::store::MemStore>,
    makes: Arc<AtomicUsize>,
}
struct TaskProducts {
    result: ProductRef,
    reference: Reference,
}
impl ObjectProducts for TaskProducts {
    fn codegen_whole(
        &mut self,
        _: &super::whole::WholeCodegenKey,
    ) -> Result<Arc<FragmentProduct>, String> {
        Ok(Arc::new(FragmentProduct {
            product: self.result,
            object: self.reference,
            summary: self.reference,
        }))
    }
    fn codegen(&mut self, _: &CodegenKey) -> Result<Arc<FragmentProduct>, String> {
        Ok(Arc::new(FragmentProduct {
            product: self.result,
            object: self.reference,
            summary: self.reference,
        }))
    }
    fn link(&mut self, _: &LinkKey, _: &LinkInputs) -> Result<Arc<LinkedProduct>, String> {
        Ok(Arc::new(LinkedProduct {
            product: self.result,
            executable: self.reference,
        }))
    }
}
impl ObjectTaskSnapshot for TaskSnapshot {
    fn execution_query(&self) -> Option<QueryId> {
        self.execution
    }
    fn store(&self) -> &dyn ResultStore {
        self.store.as_ref()
    }
    fn request(&self, _: QueryId) -> Result<ObjectTaskRequest, Reference> {
        Ok(self.request.clone())
    }
    fn ready_descriptor(&self, p: ProductRef) -> Result<ReadyProduct, Reference> {
        assert_eq!(p, self.ready.product);
        Ok(self.ready.clone())
    }
    fn optimizer_query(&self, p: ProductRef) -> Result<QueryId, Reference> {
        Ok(QueryId {
            kind: 0x4e000004,
            address: p.address,
        })
    }
    fn link_dependencies(&self, _: ProductRef) -> Result<LinkDependencies, Reference> {
        Ok(self.link.clone())
    }
    fn link_input(&self, p: ProductRef) -> Result<Reference, Reference> {
        Ok(p.record)
    }
    fn products(&self, settled: &[ProductRef]) -> Result<Box<dyn ObjectProducts>, Reference> {
        assert!(!settled.is_empty());
        self.makes.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(TaskProducts {
            result: self.result,
            reference: self.result.record,
        }))
    }
    fn failure(&self, message: &str) -> Reference {
        input(self.store.as_ref(), message)
    }
}
#[derive(Default)]
struct TaskContext {
    settled: Vec<(QueryId, Result<ProductRef, Reference>)>,
    reads: Vec<QueryId>,
}
impl QueryContext for TaskContext {
    fn read(&mut self, id: QueryId) -> Poll<Result<ProductRef, Reference>> {
        self.reads.push(id);
        self.settled
            .iter()
            .find(|(q, _)| *q == id)
            .map_or(Poll::Pending, |(_, p)| Poll::Ready(*p))
    }
}
fn query(product: ProductRef) -> QueryId {
    QueryId {
        kind: 0x4e000004,
        address: product.address,
    }
}
fn task_snapshot(
    request: ObjectTaskRequest,
    ready: ReadyProduct,
    store: Arc<hirsel_engine::store::MemStore>,
    result: ProductRef,
) -> Arc<TaskSnapshot> {
    Arc::new(TaskSnapshot {
        request,
        execution: None,
        ready,
        link: LinkDependencies {
            execution: vec![],
            fragments: vec![],
            native_reads: vec![],
            static_manifest: query(result),
            runtime_archives: vec![],
            foreign_inputs: vec![],
            launch_manifest: query(result),
        },
        result,
        store,
        makes: Arc::new(AtomicUsize::new(0)),
    })
}

#[test]
fn codegen_stages_local_ready_while_execution_waits_and_preserves_failure_order() {
    for whole in [false, true] {
        for (execution_ok, ready_ok) in [(true, true), (false, false), (true, false)] {
            let store = Arc::new(hirsel_engine::store::MemStore::default());
            let mut ready = ready(store.as_ref());
            ready.optimizer_reads.clear();
            let key = key("caller");
            let owner = key.owner.ready_query_id();
            ready.product.address = owner.address;
            let weight = MemoryWeight {
                retained: 4096,
                ..MemoryWeight::default()
            };
            let request = if whole {
                ObjectTaskRequest::WholeCodegen {
                    key: super::whole::WholeCodegenKey::new(vec![key]).unwrap(),
                    weight,
                }
            } else {
                ObjectTaskRequest::Codegen {
                    key,
                    ready: owner,
                    weight,
                }
            };
            let id = match &request {
                ObjectTaskRequest::Codegen { key, .. } => key.query_id(),
                ObjectTaskRequest::WholeCodegen { key, .. } => key.query_id(),
                _ => unreachable!(),
            };
            let result = ProductRef {
                address: id.address,
                ..product(store.as_ref(), "emitted")
            };
            let execution_product = product(store.as_ref(), "execution");
            let execution = query(execution_product);
            let mut snapshot = task_snapshot(request, ready.clone(), store.clone(), result);
            Arc::get_mut(&mut snapshot).unwrap().execution = Some(execution);
            let mut task = ObjectTaskFactory::new(snapshot.clone()).create(id).unwrap();
            let mut cx = TaskContext::default();
            assert!(
                matches!(task.poll(&mut cx), QueryPoll::Waiting(p) if p == vec![execution, owner])
            );
            assert_eq!(cx.reads, vec![execution, owner]);
            let ready_error = input(store.as_ref(), "Ready refused");
            cx.settled.push((
                owner,
                if ready_ok {
                    Ok(ready.product)
                } else {
                    Err(ready_error)
                },
            ));
            assert!(matches!(task.poll(&mut cx), QueryPoll::Waiting(p) if p == vec![execution]));
            assert_eq!(snapshot.makes.load(Ordering::SeqCst), 0);
            let execution_error = input(store.as_ref(), "execution refused");
            cx.settled.push((
                execution,
                if execution_ok {
                    Ok(execution_product)
                } else {
                    Err(execution_error)
                },
            ));
            match task.poll(&mut cx) {
                QueryPoll::Complete(p) => {
                    assert!(execution_ok && ready_ok);
                    assert_eq!(p, result);
                }
                QueryPoll::Failed(error) => assert_eq!(
                    error,
                    if execution_ok {
                        ready_error
                    } else {
                        execution_error
                    }
                ),
                QueryPoll::Waiting(_) => panic!("all required inputs settled"),
            }
            assert_eq!(
                snapshot.makes.load(Ordering::SeqCst),
                usize::from(execution_ok && ready_ok)
            );
        }
    }
}

#[test]
fn codegen_task_create_is_lazy_and_waits_for_complete_optimizer_inventory() {
    let store = Arc::new(hirsel_engine::store::MemStore::default());
    let mut ready = ready(store.as_ref());
    ready.optimizer_reads = vec![
        product(store.as_ref(), "copy1"),
        product(store.as_ref(), "copy2"),
    ];
    let key = key("caller");
    let id = key.query_id();
    let owner = query(ready.product);
    let result = ProductRef {
        address: id.address,
        ..product(store.as_ref(), "emitted")
    };
    let denied = task_snapshot(
        ObjectTaskRequest::Codegen {
            key: key.clone(),
            ready: owner,
            weight: MemoryWeight::default(),
        },
        ready.clone(),
        store.clone(),
        result,
    );
    assert!(matches!(
        ObjectTaskFactory::new(denied.clone()).create(id),
        Err(error) if error == input(store.as_ref(), "object task frame exceeds retained allowance")
    ));
    assert_eq!(denied.makes.load(Ordering::SeqCst), 0);
    let snapshot = task_snapshot(
        ObjectTaskRequest::Codegen {
            key,
            ready: owner,
            // The factory owns a Codegen continuation and its key. This
            // synthetic request must admit that frame before dependency reads.
            weight: MemoryWeight {
                retained: 4096,
                ..MemoryWeight::default()
            },
        },
        ready.clone(),
        store,
        result,
    );
    let mut task = ObjectTaskFactory::new(snapshot.clone()).create(id).unwrap();
    let mut cx = TaskContext::default();
    assert_eq!(snapshot.makes.load(Ordering::SeqCst), 0);
    assert!(matches!(task.poll(&mut cx),QueryPoll::Waiting(ref p) if p==&vec![owner]));
    cx.settled.push((owner, Ok(ready.product)));
    let copies: Vec<_> = ready.optimizer_reads.iter().copied().map(query).collect();
    assert!(matches!(task.poll(&mut cx),QueryPoll::Waiting(ref p) if p==&copies));
    assert_eq!(snapshot.makes.load(Ordering::SeqCst), 0);
    for (q, p) in copies.iter().zip(&ready.optimizer_reads) {
        cx.settled.push((*q, Ok(*p)));
    }
    assert!(matches!(task.poll(&mut cx),QueryPoll::Complete(p) if p==result));
    assert!(matches!(task.poll(&mut cx),QueryPoll::Complete(p) if p==result));
    assert_eq!(snapshot.makes.load(Ordering::SeqCst), 1);
}

#[test]
fn codegen_task_rejects_stale_optimizer_and_dependency_failure_before_emission() {
    for stale in [false, true] {
        let store = Arc::new(hirsel_engine::store::MemStore::default());
        let mut ready = ready(store.as_ref());
        let copy = product(store.as_ref(), "old copy");
        ready.optimizer_reads = vec![copy];
        let key = key("caller");
        let id = key.query_id();
        let owner = query(ready.product);
        let result = ProductRef {
            address: id.address,
            ..product(store.as_ref(), "emitted")
        };
        let snapshot = task_snapshot(
            ObjectTaskRequest::Codegen {
                key,
                ready: owner,
                // The factory owns a Codegen continuation and its key. This
                // synthetic request must admit that frame before dependency reads.
                weight: MemoryWeight {
                    retained: 4096,
                    ..MemoryWeight::default()
                },
            },
            ready.clone(),
            store.clone(),
            result,
        );
        let mut task = ObjectTaskFactory::new(snapshot.clone()).create(id).unwrap();
        let mut cx = TaskContext::default();
        cx.settled.push((owner, Ok(ready.product)));
        let next = if stale {
            Ok(ProductRef {
                record: input(store.as_ref(), "new body"),
                ..copy
            })
        } else {
            Err(input(store.as_ref(), "failed validation"))
        };
        cx.settled.push((query(copy), next));
        assert!(matches!(task.poll(&mut cx), QueryPoll::Failed(_)));
        assert_eq!(snapshot.makes.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn link_task_waits_for_manifest_then_all_fragments_and_runtime_inputs() {
    let store = Arc::new(hirsel_engine::store::MemStore::default());
    let ready = ready(store.as_ref());
    let fragment = codegen(store.as_ref(), &key("main"), &ready, || Ok(object())).unwrap();
    let key = link_key();
    let id = key.query_id();
    let manifest = product(store.as_ref(), "manifest");
    let manifest_id = query(manifest);
    let result = ProductRef {
        address: id.address,
        ..product(store.as_ref(), "linked")
    };
    let static_p = product(store.as_ref(), "statics");
    let runtime = product(store.as_ref(), "archive");
    let launch = product(store.as_ref(), "launch");
    let native = product(store.as_ref(), "native link metadata");
    let mut snapshot = task_snapshot(
        ObjectTaskRequest::Link {
            key,
            manifest: manifest_id,
            weight: MemoryWeight::default(),
        },
        ready,
        store,
        result,
    );
    Arc::get_mut(&mut snapshot).unwrap().link = LinkDependencies {
        execution: vec![],
        fragments: vec![QueryId {
            kind: super::tasks::CODEGEN_KIND,
            address: fragment.product.address,
        }],
        native_reads: vec![query(native)],
        static_manifest: query(static_p),
        runtime_archives: vec![query(runtime)],
        foreign_inputs: vec![],
        launch_manifest: query(launch),
    };
    let mut task = ObjectTaskFactory::new(snapshot.clone()).create(id).unwrap();
    let mut cx = TaskContext::default();
    assert!(matches!(task.poll(&mut cx),QueryPoll::Waiting(ref p) if p==&vec![manifest_id]));
    cx.settled.push((manifest_id, Ok(manifest)));
    assert!(matches!(task.poll(&mut cx),QueryPoll::Waiting(ref p) if p.len()==5));
    assert_eq!(snapshot.makes.load(Ordering::SeqCst), 0);
    let deps = &snapshot.link;
    cx.settled.extend([
        (deps.fragments[0], Ok(fragment.product)),
        (deps.static_manifest, Ok(static_p)),
        (deps.runtime_archives[0], Ok(runtime)),
        (deps.launch_manifest, Ok(launch)),
    ]);
    assert!(
        matches!(task.poll(&mut cx),QueryPoll::Waiting(ref p) if p==&vec![deps.native_reads[0]])
    );
    assert_eq!(snapshot.makes.load(Ordering::SeqCst), 0);
    cx.settled.push((deps.native_reads[0], Ok(native)));
    assert!(matches!(task.poll(&mut cx),QueryPoll::Complete(p) if p==result));
    assert_eq!(snapshot.makes.load(Ordering::SeqCst), 1);
}

#[test]
fn legacy_blob_schema_one_remains_readable_with_streamed_schema_two() {
    let store = hirsel_engine::store::MemStore::default();
    let mut bytes = vec![0x5a; MAX_RECORD];
    bytes.extend_from_slice(b"old tail");
    let old = put_record(
        &store,
        LEGACY_BLOB,
        &BlobRecord {
            len: bytes.len() as u64,
            chunks: bytes
                .chunks(MAX_RECORD)
                .map(|chunk| store.put_content(CHUNK, chunk).unwrap().into())
                .collect(),
        },
    )
    .unwrap();
    let current = store_link_input(&store, &bytes).unwrap();
    assert_ne!(old.domain, current.domain);
    assert_eq!(link_input_bytes(&store, old).unwrap(), bytes);
    assert_eq!(link_input_bytes(&store, current).unwrap(), bytes);
}
