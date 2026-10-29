use hirsel_driver::*;
use hirsel_engine::{runtime::*, store::MemStore, *};
use std::sync::Arc;

fn export_case(source: &str, attach_native: bool, admitted: bool, build: bool) {
    export_case_with_ownership(source, attach_native, admitted, build, false);
}
fn export_case_with_ownership(
    source: &str,
    attach_native: bool,
    admitted: bool,
    build: bool,
    consuming: bool,
) {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(directory.path().join("main.hir"), source).unwrap();
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"scope","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        for jobs in [1, 2] {
            use hirsel_engine::store::{Bytes, EntryKey, ResultStore};
            use std::sync::atomic::{AtomicUsize, Ordering};
            struct CountReads {
                inner: MemStore,
                reads: AtomicUsize,
            }
            impl ResultStore for CountReads {
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
                    "export-admission-control"
                }
            }
            let counted = Arc::new(CountReads {
                inner: MemStore::default(),
                reads: AtomicUsize::new(0),
            });
            let store: Arc<dyn ResultStore> = if admitted {
                counted.clone()
            } else {
                Arc::new(MemStore::default())
            };
            let mut program = COMPILER
                .with_config(CompileConfig {
                    store,
                    hold_bytes: 0,
                    ..Default::default()
                })
                .with_owned_engines()
                .load_program(&manifest)
                .unwrap();
            let definitions = program.engine.front_source_roots();
            let budget = ProcessBudget::new(1 << 30);
            let errors = crate::scheduled_front::errors();
            let weight = MemoryWeight {
                retained: 2 << 20,
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
                .front_body_queries(
                    &definitions,
                    weight,
                    budget.reserve(8 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let lower = program
                .engine
                .front_lower_queries(
                    &definitions,
                    front.clone(),
                    weight,
                    32,
                    budget.reserve(8 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let growth = program
                .engine
                .front_growth_queries(
                    &program.entry,
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
                    budget.reserve(8 << 20).unwrap(),
                    errors,
                )
                .unwrap();
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
            let headers = program
                .engine
                .front_checked_signatures(front.clone(), budget.reserve(4 << 20).unwrap())
                .unwrap();
            let run = program
                .engine
                .front_run_queries(
                    check.clone(),
                    front.clone(),
                    lower.clone(),
                    weight,
                    budget.reserve(16 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let mut run = Some(run);
            let native = if attach_native {
                let catalogue = program
                    .engine
                    .front_native_definition_queries(
                        &definitions,
                        front.clone(),
                        lower.clone(),
                        weight,
                        budget.reserve(16 << 20).unwrap(),
                        errors,
                    )
                    .unwrap();
                Some(
                    program
                        .engine
                        .front_native_queries(
                            FrontNativePlan {
                                entry: program.entry.clone(),
                                roots: Vec::new(),
                                run: run.take().unwrap(),
                                definitions: catalogue,
                            },
                            weight,
                            budget.reserve(16 << 20).unwrap(),
                            errors,
                        )
                        .unwrap(),
                )
            } else {
                None
            };
            let export = if let Some(native) = &native {
                program
                    .engine
                    .front_proof_export_from_native(
                        &program.entry,
                        native,
                        headers,
                        budget.reserve(4 << 20).unwrap(),
                    )
                    .unwrap()
            } else {
                let run = run.take().unwrap();
                program
                    .engine
                    .front_proof_export(
                        &program.entry,
                        run,
                        headers,
                        budget.reserve(4 << 20).unwrap(),
                    )
                    .unwrap()
            };
            runtime
                .register(FRONT_RUN_KIND, export.run().factory())
                .unwrap();
            if let Some(native) = &native {
                assert_eq!(native.run_query(&program.entry), Some(export.query()));
                runtime
                    .register(FRONT_NATIVE_DEFINITION_KIND, native.definition_factory())
                    .unwrap();
                runtime
                    .register(FRONT_NATIVE_ROOT_KIND, native.factory())
                    .unwrap();
            }
            let query = export.query();
            let unknown = ProductRef {
                address: query.address,
                action: hirsel_artifact_store::Action(query.address.0),
                record: errors.protocol,
            };
            assert!(export.input(unknown.clone()).is_err());
            let request =
                runtime.request(&[native.as_ref().map_or(query, |native| native.query())]);
            let completed = runtime.wait(request).unwrap()[0].clone();
            let product = match &native {
                Some(native) => native.completed_run_product(completed.clone()).unwrap(),
                None => completed.clone(),
            };
            assert!(export.input(unknown.clone()).is_err());
            let before_lower = lower.work_snapshot();
            let before_front = front.work_snapshot();
            let mut owned = if admitted {
                let before_reads = counted.reads.load(Ordering::Relaxed);
                let required = export.input_bytes(product.clone()).unwrap();
                assert!(required > 0);
                assert_eq!(
                    counted.reads.load(Ordering::Relaxed),
                    before_reads,
                    "predecode sizing never opens a provider"
                );
                assert!(export.input_bytes(unknown).is_err());
                assert!(
                    export
                        .input_with_allowance(
                            product.clone(),
                            budget.reserve(required - 1).unwrap()
                        )
                        .is_err()
                );
                assert_eq!(
                    counted.reads.load(Ordering::Relaxed),
                    before_reads,
                    "N-1 export input refuses before any original decode"
                );
                Some(std::rc::Rc::new(
                    export
                        .input_with_allowance(product.clone(), budget.reserve(required).unwrap())
                        .unwrap(),
                ))
            } else {
                None
            };
            let ordinary = if owned.is_none() {
                Some(match &native {
                    Some(native) => export.input_for_native(native, completed).unwrap(),
                    None => export.input(product.clone()).unwrap(),
                })
            } else {
                None
            };
            let input = owned
                .as_ref()
                .map(|owned| owned.input())
                .or(ordinary.as_ref())
                .unwrap();
            assert!(!input.annotations.is_empty());
            assert!(input.annotations.iter().all(|a| a.location.line != 0));
            assert!(input.signatures.values().any(|s| s.owner_view.is_some()));
            let mut unused = program.entry.clone();
            unused.0.name = "unused".into();
            assert!(
                input.definitions.contains_key(&unused),
                "PS9 includes unused executable roots of the entry package"
            );
            let unbuilt = (!build).then(|| hirsel_proof_export::export(input));
            let body_buffers = consuming.then(|| {
                input
                    .definitions
                    .iter()
                    .map(|(path, definition)| (path.clone(), definition.bodies.as_ptr() as usize))
                    .collect::<std::collections::BTreeMap<_, _>>()
            });
            let built = if consuming {
                let owned = owned.take().unwrap();
                let required = owned.type_capture_bytes().unwrap();
                let prepared = owned
                    .capture_types_with_allowance(budget.reserve(required).unwrap())
                    .unwrap();
                let required = prepared.owned_build_bytes().unwrap();
                let built = prepared
                    .build_owned_with_allowance(budget.reserve(required).unwrap())
                    .unwrap();
                for definition in &built.export().definitions {
                    assert_eq!(
                        body_buffers.as_ref().unwrap()[&definition.path],
                        definition.tir.bodies.as_ptr() as usize
                    );
                }
                assert!(!built.export().annotations.is_empty());
                assert!(
                    built
                        .export()
                        .signatures
                        .iter()
                        .any(|s| s.owner_view.is_some())
                );
                Some(built)
            } else if build {
                let owned = owned.as_ref().unwrap();
                let required = owned.type_capture_bytes().unwrap();
                assert!(
                    owned
                        .clone()
                        .capture_types_with_allowance(budget.reserve(required - 1).unwrap())
                        .is_err()
                );
                let prepared = std::rc::Rc::new(
                    owned
                        .clone()
                        .capture_types_with_allowance(budget.reserve(required).unwrap())
                        .unwrap(),
                );
                let required = prepared.build_bytes().unwrap();
                assert!(
                    prepared
                        .clone()
                        .build_with_allowance(budget.reserve(required - 1).unwrap())
                        .is_err()
                );
                Some(
                    prepared
                        .clone()
                        .build_with_allowance(budget.reserve(required).unwrap())
                        .unwrap(),
                )
            } else {
                None
            };
            let encoded = built.as_ref().map(|built| {
                assert!(
                    built
                        .export()
                        .definitions
                        .iter()
                        .any(|section| !section.obligations.is_empty()),
                    "exercise a real obligation table"
                );
                let required = built.output_bytes().unwrap();
                assert!(
                    built
                        .encode_with_allowance(budget.reserve(required - 1).unwrap())
                        .is_err()
                );
                let before = budget.reserved();
                let encoded = built
                    .encode_with_allowance(budget.reserve(required).unwrap())
                    .unwrap();
                assert_eq!(
                    budget.reserved(),
                    before + encoded.retained_bytes(),
                    "temporary encoding charge is released before return"
                );
                encoded
            });
            let actual = encoded
                .as_ref()
                .map(|text| text.text().to_owned())
                .unwrap_or_else(|| unbuilt.unwrap());
            assert_eq!(lower.work_snapshot(), before_lower);
            assert_eq!(front.work_snapshot(), before_front);
            let expected = program.engine.proof_export_input(&program.entry).unwrap();
            assert_eq!(actual, hirsel_proof_export::export(&expected));
            let read_rows = export.reads(product.clone()).unwrap();
            assert!(read_rows.contains_key(&unused));
            drop(owned);
            drop(ordinary);
            drop(built);
            runtime.shutdown();
            drop(program);
            assert_eq!(
                hirsel_proof_export::export(&export.input(product).unwrap()),
                actual,
                "the exact export remains available after loader/runtime shutdown"
            );
            if let Some(encoded) = encoded {
                let detached = encoded.clone();
                assert_eq!(detached.text(), actual);
                let before = budget.reserved();
                let retained = encoded.retained_bytes();
                drop(encoded);
                assert_eq!(
                    budget.reserved(),
                    before,
                    "detached clone keeps output admission"
                );
                drop(detached);
                assert_eq!(
                    budget.reserved(),
                    before - retained,
                    "last clone releases the one output grant"
                );
            }
        }
    });
}
#[test]
fn source_export_matches_full_ps9_package_closure_private_views_annotations_and_positions() {
    export_case(
        "@analysis(example: retain = yes)\npub opaque type Hidden: () uses {} { value:u64 }\nfn read(h:Hidden)->u64{h.value}\nfn unused(x:u64)->u64{x+1u64}\nconst A:u8=comptime 1u8; pub fn main()->u8{A}",
        false,
        false,
        false,
    );
}

#[test]
fn source_export_shares_exact_native_selected_run_and_completed_originals() {
    export_case(
        "@analysis(example: retain = yes)\npub opaque type Hidden: () uses {} { value:u64 }\nfn read(h:Hidden)->u64{h.value}\nfn unused(x:u64)->u64{x+1u64}\nconst A:u8=comptime 1u8; pub fn main()->u8{A}",
        true,
        false,
        false,
    );
}

#[test]
fn complete_ps9_input_admission_precedes_body_reads_and_matches_scalar_export() {
    export_case(
        "@analysis(example: retain = yes)\npub opaque type Hidden: () uses {} { value:u64 }\nfn read(h:Hidden)->u64{h.value}\nfn unused(x:u64)->u64{x+1u64}\nconst A:u8=comptime 1u8; pub fn main()->u8{A}",
        true,
        true,
        false,
    );
}

#[test]
fn full_ps9_build_and_text_admission_preserve_scalar_and_detached_ownership() {
    export_case(
        "@analysis(example: retain = yes)\npub opaque type Hidden: () uses {} { value:u64 }\nfn read(h:Hidden)->u64{h.value}\nfn unused(x:u64)->u64{x+1u64}\nconst A:u8=comptime 1u8; pub fn main()->u8{A}",
        true,
        true,
        true,
    );
}

#[test]
fn consuming_full_ps9_native_input_preserves_original_buffers_and_detached_canonical_text() {
    export_case_with_ownership(
        "@analysis(example: retain = yes)\npub opaque type Hidden: () uses {} { value:u64 }\nfn read(h:Hidden)->u64{h.value}\nfn unused(x:u64)->u64{x+1u64}\nconst A:u8=comptime 1u8; pub fn main()->u8{A}",
        true,
        true,
        true,
        true,
    );
}
