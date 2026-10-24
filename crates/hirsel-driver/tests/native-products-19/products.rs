use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_syntax::ContentHash;
use native::object_queries::{
    CodegenKey, LinkInputs, LinkKey, ObjectProducts, ObjectQueries, load_object,
};
use native::queries::{
    NativeProducts,
    arena::{ArenaQueryCounts, EquationProduct},
    declarations::NativeDeclarations,
    demand::DemandInstances,
    graph::{ArenaNode, settle_arena_queries},
    products::{DemandProducts, EmissionOptions, NativeSnapshot},
};
use std::{collections::BTreeSet, sync::Arc};

#[test]
fn actual_native_products_emit_both_backends_and_observe_static_edits_without_value_rebuilds() {
    with_compiler_stack(|| run_product_case(false, None));
}

#[test]
fn aggregate_constant_slots_reach_both_emitters_and_linked_observations() {
    with_compiler_stack(|| run_product_case(true, None));
}

#[test]
fn admitted_ready_and_codegen_tasks_share_real_native_products_at_widths_one_and_four() {
    with_compiler_stack(|| {
        for jobs in [1, 4] {
            run_product_case(false, Some(jobs));
        }
    });
}

#[derive(Default)]
struct TaskEvents(std::sync::Mutex<Vec<hirsel_engine::runtime::RuntimeEvent>>);
impl hirsel_engine::runtime::RuntimeObserver for TaskEvents {
    fn record(&self, event: hirsel_engine::runtime::RuntimeEvent) {
        self.0.lock().unwrap().push(event);
    }
}

fn run_product_case(aggregate: bool, scheduled_jobs: Option<usize>) {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(store::DirStore::new(directory.path().join("products")));
    let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
    let options = native::Options::new(target.clone());
    let tools =
        native::Toolchain::in_tree_diagnostic(&directory.path().join("runtime"), &[&target])
            .unwrap();
    let policy = ContentHash::of_bytes(b"actual NativeProducts emitter integration");
    let mut previous: Option<(
        store::Reference,
        store::Reference,
        store::Reference,
        Vec<u8>,
        Vec<u8>,
        Vec<u8>,
        Vec<u8>,
    )> = None;
    let mut previous_fragments: Option<[store::Reference; 2]> = None;
    for (literal, fresh) in [(7u64, false), (7, true), (8, false), (8, true)] {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        let source = if aggregate {
            format!(
                "type Number {{n:u64}}\nconst SECOND: Number = comptime Number(n:{literal}u64)\npub fn main()->u64 {{let s=SECOND;s.n}}"
            )
        } else {
            format!("const SECOND: u64 = comptime {literal}u64\npub fn main()->u64 {{SECOND}}")
        };
        let file = engine.sources.add("products-demand.hir", source).unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        if fresh {
            engine.adapters.lowering = None;
        }
        let mut source_queries = NativeDiscoveryCache::new(engine.types().clone(), store.clone());
        let mut declarations =
            NativeDeclarations::collect(&mut engine, &mut source_queries, &[]).unwrap();
        if fresh {
            assert_eq!(source_queries.counts.body_demands, 0);
        }
        let trace = hirsel_lir::construction::ValueTrace::default();
        let _observe = trace.enter();
        let mut instances = DemandInstances::new(&engine, store.clone(), target.clone(), policy);
        let mut pending = instances.roots(&engine, &declarations).unwrap();
        let mut seen = BTreeSet::new();
        let mut completed = Vec::new();
        while let Some(key) = pending.pop() {
            if !seen.insert(key.instance.encoding().to_vec()) {
                continue;
            }
            let output = instances
                .instance(&mut engine, &mut declarations, &key, &options)
                .unwrap();
            pending.extend(output.dependencies.iter().cloned());
            completed.push((key, output));
        }
        completed.sort_by(|a, b| a.0.canonical_cmp(&b.0));
        let nodes = completed
            .iter()
            .map(|(key, output)| ArenaNode {
                equations: EquationProduct {
                    owner: key.clone(),
                    reference: output.product.local_equations,
                },
                callees: output.callees.clone(),
            })
            .collect::<Vec<_>>();
        let arena = settle_arena_queries(
            store.as_ref(),
            &nodes,
            &[],
            0,
            &mut ArenaQueryCounts::default(),
        )
        .unwrap();
        let operations = declarations.program.operation_registry();
        let manifest = declarations.manifest().unwrap();
        let emission = EmissionOptions {
            input: declarations.program.input.clone(),
            opt: hirsel_lir::emission::OptLevel::None,
            debug: true,
            callbacks: false,
            tracking: false,
            foreign: Default::default(),
            inline_instances: Default::default(),
            verify_backend: true,
            measurement: false,
            fuel: false,
        };
        let before_create = trace.totals();
        let key = completed
            .iter()
            .find(|(key, _)| key.instance.definition() == &entry_path(&default_context()))
            .unwrap()
            .0
            .clone();
        let counts = (instances.imports, instances.recipe_decodes);
        let snapshot = NativeSnapshot::new(
            store.clone(),
            instances,
            arena,
            options.clone(),
            emission,
            manifest,
            Arc::new(operations),
        )
        .unwrap();
        let mut products = DemandProducts::new(snapshot.clone());
        assert_eq!(
            trace.totals(),
            before_create,
            "provider creation must not execute dependencies"
        );
        let base = products.instance(&key).unwrap();
        let ready_before = if scheduled_jobs.is_none() {
            Some(products.ready(&key).unwrap())
        } else {
            None
        };
        if fresh {
            assert_eq!(
                (
                    counts.0,
                    counts.1,
                    products.local.queries.counts.transformations
                ),
                (0, 0, 0)
            );
            assert_eq!(
                trace.totals(),
                hirsel_lir::construction::ValueCounts::default()
            );
        }
        let codegen_key = |backend: &str| CodegenKey {
            owner: key.clone(),
            backend: backend.into(),
            backend_version: ContentHash::of_parts(
                "compiled backend",
                [backend.as_bytes(), COMPILER_IDENTITY.as_slice()],
            ),
            target_features: target.digest(),
            profile_options: ContentHash::of_bytes(b"debug, opt none"),
            runtime_abi: ContentHash::of_bytes(&hirsel_lir::RUNTIME_ABI_VERSION.to_le_bytes()),
        };
        let (llvm_fragment, cranelift_fragment, mut products) = if let Some(jobs) = scheduled_jobs {
            use hirsel_engine::runtime::*;
            use native::queries::tasks::{CodegenTasks, NativeTaskFactory, NativeTasks};
            let weight = MemoryWeight {
                scratch: 32 << 20,
                retained: 32 << 10,
                stack: 0,
            };
            let tasks = NativeTasks::new(snapshot.clone(), weight).unwrap();
            let native_factory = Arc::new(NativeTaskFactory::new(tasks.clone()));
            let before = trace.totals();
            let mut lazy = native_factory.create(key.ready_query_id()).unwrap();
            struct Pending;
            impl QueryContext for Pending {
                fn read(
                    &mut self,
                    _: QueryId,
                ) -> std::task::Poll<Result<ProductRef, store::Reference>> {
                    std::task::Poll::Pending
                }
            }
            assert!(
                matches!(lazy.poll(&mut Pending), QueryPoll::Waiting(ref keys) if !keys.is_empty())
            );
            assert_eq!(
                tasks
                    .ready_transformations
                    .load(std::sync::atomic::Ordering::Relaxed),
                0
            );
            assert_eq!(snapshot.ready_materializations(), 0);
            assert_eq!(
                trace.totals(),
                before,
                "factory creation and pending poll must not execute native dependencies"
            );
            drop(lazy);
            let keys = [codegen_key("llvm"), codegen_key("cranelift")];
            let objects = CodegenTasks::new(tasks.clone(), keys.to_vec(), weight).unwrap();
            let object_factory = Arc::new(native::object_queries::ObjectTaskFactory::new(objects));
            let errors = RuntimeErrors {
                resource: tasks.failure("resource"),
                cycle: tasks.failure("cycle"),
                cancelled: tasks.failure("cancelled"),
                protocol: tasks.failure("protocol"),
                panicked: tasks.failure("panic"),
            };
            let budget = ProcessBudget::new(768 << 20);
            let mut runtime = AdmittedRuntime::new(
                hirsel_intern::jobs::Jobs::new(jobs),
                budget.reserve(768 << 20).unwrap(),
                errors,
            )
            .unwrap();
            for kind in [
                native::queries::INSTANCE_KIND,
                native::queries::ARENA_CANDIDATES_KIND,
                native::queries::ARENA_CHOSEN_KIND,
                native::queries::READY_KIND,
            ] {
                runtime.register(kind, native_factory.clone()).unwrap();
            }
            runtime
                .register(native::object_queries::CODEGEN_KIND, object_factory)
                .unwrap();
            let events = Arc::new(TaskEvents::default());
            runtime.observe(events.clone()).unwrap();
            let request =
                runtime.request(&keys.iter().map(CodegenKey::query_id).collect::<Vec<_>>());
            let results = runtime.wait(request).unwrap_or_else(|error| {
                use hirsel_engine::store::ResultStore;
                panic!(
                    "native task failure: {:?}",
                    store
                        .get_content(error)
                        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                )
            });
            assert_eq!(
                tasks
                    .ready_transformations
                    .load(std::sync::atomic::Ordering::Relaxed),
                if fresh { 0 } else { 1 }
            );
            assert_eq!(
                snapshot.ready_materializations(),
                if fresh { 0 } else { 2 },
                "object hits do not materialize Ready inside the runtime"
            );
            let events = events.0.lock().unwrap();
            assert_eq!(events.iter().filter(|event| matches!(event, RuntimeEvent::Claim(id) if *id == key.ready_query_id())).count(), 1, "both backend tasks coalesce their actual Ready dependency");
            assert!(events.iter().any(|event| matches!(event, RuntimeEvent::Read {query, dependency} if query.kind == native::object_queries::CODEGEN_KIND && dependency.kind == native::queries::READY_KIND)));
            assert!(events.iter().any(|event| matches!(event, RuntimeEvent::Read {query, dependency} if query.kind == native::queries::READY_KIND && dependency.kind == native::queries::ARENA_CHOSEN_KIND)));
            drop(events);
            drop(runtime);
            assert_eq!(budget.reserved(), 0);
            let llvm =
                native::object_queries::fragment_product(store.as_ref(), results[0]).unwrap();
            let cranelift =
                native::object_queries::fragment_product(store.as_ref(), results[1]).unwrap();
            (Arc::new(llvm), Arc::new(cranelift), products)
        } else {
            let mut objects = ObjectQueries::new(
                store.clone(),
                products,
                |_: &LinkKey,
                 _: &LinkInputs,
                 _: &[hirsel_lir::stage::ObjectArtifact]|
                 -> Result<Vec<u8>, String> {
                    Err(
                        "this codegen control links with the generated native C entry harness"
                            .into(),
                    )
                },
            );
            let llvm_fragment = objects.codegen(&codegen_key("llvm")).unwrap();
            let cranelift_fragment = objects.codegen(&codegen_key("cranelift")).unwrap();
            assert_eq!(
                objects.native.ready_materializations,
                if fresh { 0 } else { 2 },
                "object hits must not materialize a Ready body"
            );
            (llvm_fragment, cranelift_fragment, objects.native)
        };
        let ready = ready_before.unwrap_or_else(|| products.ready(&key).unwrap());
        if fresh {
            assert_eq!(
                trace.totals(),
                hirsel_lir::construction::ValueCounts::default()
            );
        }
        let before_emission = trace.totals();
        let fragments = [
            llvm_fragment.product.record,
            cranelift_fragment.product.record,
        ];
        if let Some(old) = previous_fragments {
            if fresh || !aggregate {
                assert_eq!(
                    fragments, old,
                    "equal object recomputation backdates its fragment output"
                );
            } else {
                assert_ne!(fragments[0], old[0]);
                assert_ne!(fragments[1], old[1]);
            }
        }
        previous_fragments = Some(fragments);
        let llvm = load_object(store.as_ref(), llvm_fragment.object).unwrap();
        let cranelift = load_object(store.as_ref(), cranelift_fragment.object).unwrap();
        let (glue_llvm, glue_cranelift, call) = products
            .with_ready(&[ready.clone()], |units, inputs| {
                assert_eq!(units.len(), 1);
                assert_eq!(inputs.statics.len(), 1);
                assert_eq!(inputs.statics[0].blocks[0].bytes, literal.to_le_bytes());
                let slots = hirsel_lir::statics::constant_slots(units[0].unit(), inputs.statics);
                assert_eq!(
                    slots.iter().any(Option::is_some),
                    aggregate,
                    "the fixture must exercise the intended constant-slot selection"
                );
                let finalizers = Default::default();
                let registry = hirsel_lir::descriptor_registry::DescriptorRegistry::new(
                    inputs.layouts,
                    &finalizers,
                );
                let glue = hirsel_lir::glue::UnitGlue::of(
                    units[0].unit(),
                    inputs.layouts,
                    inputs.keys,
                    inputs.manifest,
                )?;
                let failures = hirsel_lir::glue::failure_codes(&glue.failure_rules);
                let glue_inputs = hirsel_lir::emission::GlueInputs {
                    target: inputs.target,
                    layouts: inputs.layouts,
                    keys: inputs.keys,
                    registry,
                    finalizers: &finalizers,
                    glue: &glue,
                    failures: &failures,
                    statics: inputs.statics,
                    callbacks: &[],
                    foreign: inputs.foreign,
                    aliasing_unstated: inputs.callbacks,
                };
                let glue_llvm = hirsel_llvm::emit_glue(&glue_inputs).map_err(|e| e.to_string())?;
                let glue_cranelift =
                    hirsel_cranelift::emit_glue(&glue_inputs).map_err(|e| e.to_string())?;
                let unit = units[0].unit();
                let call = native::c_call(
                    inputs.layouts,
                    &inputs.layouts.fn_abis[unit.header.abi.0 as usize],
                    &hirsel_lir::emission::symbol_name(
                        &unit.header.definition,
                        Some(&unit.header.instance),
                    ),
                    "call_main",
                );
                Ok((glue_llvm, glue_cranelift, call))
            })
            .unwrap();
        assert_eq!(
            trace.totals(),
            before_emission,
            "emission may decode Ready but may not reconstruct a Value"
        );
        assert!(!llvm.bytes.is_empty() && !cranelift.bytes.is_empty());
        for (backend, object, glue) in [
            ("llvm", &llvm, &glue_llvm),
            ("cranelift", &cranelift, &glue_cranelift),
        ] {
            let dir = directory
                .path()
                .join(format!("{literal}-{fresh}-{backend}"));
            std::fs::create_dir_all(&dir).unwrap();
            let harness = dir.join("call.c");
            std::fs::write(&harness, include_str!("call.c").replace("@CALL@", &call)).unwrap();
            let unit = dir.join("unit.o");
            let glue_path = dir.join("glue.o");
            std::fs::write(&unit, &object.bytes).unwrap();
            std::fs::write(&glue_path, &glue.bytes).unwrap();
            let executable = dir.join("program");
            let linked = std::process::Command::new(&tools.clang)
                .arg(format!("--ld-path={}", tools.lld.display()))
                .arg(&harness)
                .arg("-I")
                .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../hirsel-rt/include"))
                .arg(&unit)
                .arg(&glue_path)
                .arg(tools.archive(&target.triple).unwrap())
                .args([
                    "-lgcc_s",
                    "-lutil",
                    "-lrt",
                    "-lpthread",
                    "-lm",
                    "-ldl",
                    "-o",
                ])
                .arg(&executable)
                .output()
                .unwrap();
            assert!(
                linked.status.success(),
                "{backend} link: {}",
                String::from_utf8_lossy(&linked.stderr)
            );
            let observed = std::process::Command::new(executable).output().unwrap();
            assert!(
                observed.status.success(),
                "{backend} run: {}",
                String::from_utf8_lossy(&observed.stderr)
            );
            assert_eq!(
                observed.stdout,
                format!("{literal}\n").as_bytes(),
                "{backend} observable source result"
            );
        }
        if let Some((
            old_value,
            old_ready,
            old_context,
            old_llvm,
            old_cranelift,
            old_glue_llvm,
            old_glue_cranelift,
        )) = &previous
        {
            assert_eq!(
                base.value, *old_value,
                "constant edit keeps the caller Value equal"
            );
            assert_eq!(
                ready.ready, *old_ready,
                "constant edit keeps the Ready body equal"
            );
            if fresh {
                assert_eq!(ready.emission_context, *old_context);
                assert_eq!(llvm.bytes, *old_llvm);
                assert_eq!(cranelift.bytes, *old_cranelift);
                assert_eq!(glue_llvm.bytes, *old_glue_llvm);
                assert_eq!(glue_cranelift.bytes, *old_glue_cranelift);
            } else {
                assert_ne!(ready.emission_context, *old_context);
                if aggregate {
                    assert_ne!(
                        llvm.bytes, *old_llvm,
                        "LLVM must observe changed constant slots"
                    );
                    assert_ne!(
                        cranelift.bytes, *old_cranelift,
                        "Cranelift must observe changed constant slots"
                    );
                } else {
                    // Scalar loads relocate through the unchanged symbol;
                    // the actual linked value changes in the glue object.
                    assert_eq!(llvm.bytes, *old_llvm);
                    assert_eq!(cranelift.bytes, *old_cranelift);
                }
                assert_ne!(glue_llvm.bytes, *old_glue_llvm);
                assert_ne!(glue_cranelift.bytes, *old_glue_cranelift);
            }
        }
        previous = Some((
            base.value,
            ready.ready,
            ready.emission_context,
            llvm.bytes,
            cranelift.bytes,
            glue_llvm.bytes,
            glue_cranelift.bytes,
        ));
    }
}
