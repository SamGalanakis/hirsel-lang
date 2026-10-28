use hirsel_driver::*;
use hirsel_engine::{runtime::*, store::MemStore, *};
use std::sync::Arc;

#[test]
fn admitted_native_roots_preserve_selection_constants_and_detached_metadata_ownership() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.hir");
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(&path,"pub type Secret{level:u64}\nconst A:u64=comptime B;\nconst B:u64=comptime 3u64;\nfn view(x:Secret)->u64{x.level}\nfn orphan(x:u64)->u64{x}\nfn ct_body()->Result[Body[fn(u64)->u64 uses {}],String]{Ok(codegen.body(\"fn(x:u64)->u64 uses {} {9u64/x}\"))}\npub fn main()->u64{A+view(Secret(level:4u64))}\n").unwrap();
        std::fs::write(&manifest,r#"{"schema":1,"package":{"name":"roots","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        for jobs in [1, 2] {
            for explicit in [false, true] {
                let store = Arc::new(MemStore::default());
                let mut program = COMPILER
                    .with_config(CompileConfig {
                        store: store.clone(),
                        hold_bytes: 0,
                        ..Default::default()
                    })
                    .with_owned_engines()
                    .load_program(&manifest)
                    .unwrap();
                let paths = program.engine.front_source_roots();
                let seeds = if explicit {
                    vec![program.entry.clone()]
                } else {
                    Vec::new()
                };
                let entry = program.entry.clone();
                let budget = ProcessBudget::new(1 << 30);
                let errors = crate::scheduled_front::errors();
                let weight = MemoryWeight {
                    retained: 1 << 20,
                    scratch: 8 << 20,
                    stack: 0,
                };
                let mut runtime = AdmittedRuntime::new(
                    hirsel_intern::jobs::Jobs::new(jobs),
                    budget.reserve(512 << 20).unwrap(),
                    errors,
                )
                .unwrap();
                let front = program
                    .engine
                    .front_body_queries(&paths, weight, budget.reserve(16 << 20).unwrap(), errors)
                    .unwrap();
                let lower = program
                    .engine
                    .front_lower_queries(
                        &paths,
                        front.clone(),
                        weight,
                        3 * paths.len(),
                        budget.reserve(16 << 20).unwrap(),
                        errors,
                    )
                    .unwrap();
                let growth = program
                    .engine
                    .front_growth_queries(
                        &entry,
                        front.clone(),
                        weight,
                        budget.reserve(8 << 20).unwrap(),
                        errors,
                    )
                    .unwrap();
                let check = program
                    .engine
                    .front_check_queries(
                        FrontCheckPlan {
                            files: program
                                .files
                                .iter()
                                .map(|(file, context)| {
                                    (*file, context.clone(), LoweringPolicy::Comptime)
                                })
                                .collect(),
                            front: front.clone(),
                            lower: lower.clone(),
                            growth: growth.clone(),
                            unsafe_ffi: program.unsafe_ffi.clone(),
                            lint_package: None,
                        },
                        weight,
                        budget.reserve(16 << 20).unwrap(),
                        errors,
                    )
                    .unwrap();
                let run = program
                    .engine
                    .front_run_queries(
                        check.clone(),
                        front.clone(),
                        lower.clone(),
                        weight,
                        budget.reserve(8 << 20).unwrap(),
                        errors,
                    )
                    .unwrap();
                let definitions = program
                    .engine
                    .front_native_definition_queries(
                        &paths,
                        front.clone(),
                        lower.clone(),
                        weight,
                        budget.reserve(16 << 20).unwrap(),
                        errors,
                    )
                    .unwrap();
                let native = program
                    .engine
                    .front_native_queries(
                        FrontNativePlan {
                            entry: entry.clone(),
                            roots: seeds.clone(),
                            run,
                            definitions,
                        },
                        weight,
                        budget.reserve(8 << 20).unwrap(),
                        errors,
                    )
                    .unwrap();
                assert!(native.run_query(&entry).is_some());
                for path in &paths {
                    if path != &entry {
                        assert!(
                            native.run_query(path).is_none(),
                            "unselected whole-program Run: {path}"
                        );
                    }
                    assert!(
                        native.definition_query(path).is_some(),
                        "source catalogue was pruned: {path}"
                    );
                }
                assert!(
                    native.run_query_inventory().count() > paths.len(),
                    "core semantic producers remain in the inventory"
                );
                runtime.register(FRONT_BODY_KIND, front.factory()).unwrap();
                runtime
                    .register_local(FRONT_LOWER_KIND, lower.factory())
                    .unwrap();
                runtime
                    .register_local(FRONT_EVALUATE_KIND, lower.factory())
                    .unwrap();
                runtime
                    .register(FRONT_GROWTH_KIND, growth.factory())
                    .unwrap();
                runtime.register(FRONT_CHECK_KIND, check.factory()).unwrap();
                runtime
                    .register(FRONT_RUN_KIND, native.run_factory())
                    .unwrap();
                runtime
                    .register(FRONT_NATIVE_DEFINITION_KIND, native.definition_factory())
                    .unwrap();
                runtime
                    .register(FRONT_NATIVE_ROOT_KIND, native.factory())
                    .unwrap();
                let request = runtime.request(&[native.query()]);
                let products = runtime
                    .wait(request)
                    .unwrap_or_else(|r| panic!("native root {r:?}: {:?}", native.failure(r)));
                let output = native.outcome(products[0]).unwrap();
                let actual = output.products.clone();
                let anchors = NativeRecordContext::capture(&program.engine);
                let codec = NativeRecords::new(program.engine.types().clone(), store.clone());
                assert_eq!(
                    actual
                        .roots
                        .definitions
                        .iter()
                        .any(|p| p.0.name == "orphan"),
                    !explicit,
                    "whole-package default must retain unexecuted declarations"
                );
                let ct = paths.iter().find(|p| p.0.name == "ct_body").unwrap();
                assert!(
                    !actual.roots.definitions.contains(ct),
                    "CT-only declarations stay outside runtime registration"
                );
                let checked_ct = actual.catalogue.iter().find(|d| &d.source.definition == ct);
                assert_eq!(
                    checked_ct.is_some(),
                    !explicit,
                    "bootstrap keeps the selected semantic declaration union"
                );
                if let Some(ct) = checked_ct {
                    assert!(ct.source.lower.is_none());
                    assert!(ct.source.audit.is_none());
                    assert!(ct.contracts.is_none());
                }
                assert!(!actual.originals.iter().any(|o| &o.definition == ct));
                assert!(Arc::ptr_eq(
                    &actual.signatures,
                    actual.semantic.signatures()
                ));
                assert_eq!(
                    actual.run.revision,
                    actual.semantic.semantic_program_revision(&entry).unwrap()
                );
                assert_eq!(front.live_body_count(), 0);
                runtime.shutdown();
                drop(runtime);
                let held_bytes = budget.reserved();
                drop(output);
                drop(program);
                drop(native);
                drop(check);
                drop(growth);
                drop(front);
                drop(lower);
                assert_eq!(
                    budget.reserved(),
                    held_bytes,
                    "copied root product handle keeps every admitted producer owner"
                );
                let reader = actual
                    .original_context
                    .reader(
                        actual.originals.iter().map(|p| (**p).clone()),
                        store.clone(),
                    )
                    .unwrap();
                assert!(reader.read(&entry).is_some());
                drop(reader);
                let expected_store = Arc::new(MemStore::default());
                let mut oracle = session::Session::open_configured(
                    &manifest,
                    CompileConfig {
                        store: expected_store.clone(),
                        ..Default::default()
                    },
                );
                let expected_output = oracle
                    .request(hirsel_driver::requests::CompileOperation::NativeSources {
                        roots: seeds.clone(),
                    })
                    .unwrap();
                let hirsel_driver::requests::CompileResult::NativeSources(expected) =
                    &expected_output.result
                else {
                    panic!("native source oracle returned another operation");
                };
                assert_eq!(actual.roots.definitions, expected.roots.definitions);
                assert_eq!(actual.statics.definitions, expected.statics.definitions);
                let constant_order = &actual.statics.definitions;
                assert!(
                    constant_order.iter().position(|p| p.0.name == "B").unwrap()
                        < constant_order.iter().position(|p| p.0.name == "A").unwrap()
                );
                let expected_codec = NativeRecords::new(
                    expected.signatures.types().clone(),
                    Arc::new(MemStore::default()),
                );
                let expected_reader = expected
                    .original_context
                    .reader(
                        expected.originals.iter().map(|product| (**product).clone()),
                        expected_store.clone(),
                    )
                    .unwrap();
                assert_eq!(
                    codec.encode(&anchors, &actual.statics.constants),
                    expected_codec.encode(
                        expected_reader.record_context().as_ref(),
                        &expected.statics.constants
                    )
                );
                assert_eq!(
                    codec.encode(&anchors, &actual.extra),
                    expected_codec
                        .encode(expected_reader.record_context().as_ref(), &expected.extra)
                );
                assert_eq!(actual.roots.environment, expected.roots.environment);
                let sources = actual.roots.sources.clone();
                let file = sources.file(hirsel_span::FileId(0)).unwrap().clone();
                drop(actual);
                assert!(
                    budget.reserved() > 0,
                    "extracted map retains source admission"
                );
                assert!(file.text().contains("pub fn main"));
                drop(sources);
                assert!(
                    budget.reserved() > 0,
                    "extracted file retains source admission"
                );
                drop(file);
                assert_eq!(budget.reserved(), 0);
            }
        }
    });
}
