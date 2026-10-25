use hirsel_driver::*;
use hirsel_engine::{runtime::*, store::MemStore, *};
use std::sync::Arc;

fn export_case(source: &str, attach_native: bool, admitted: bool) {
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
            assert!(export.input(unknown).is_err());
            let request =
                runtime.request(&[native.as_ref().map_or(query, |native| native.query())]);
            let completed = runtime.wait(request).unwrap()[0];
            let product = match &native {
                Some(native) => native.completed_run_product(completed).unwrap(),
                None => completed,
            };
            assert!(export.input(unknown).is_err());
            let before_lower = lower.work_snapshot();
            let before_front = front.work_snapshot();
            let owned = if admitted {
                let before_reads = counted.reads.load(Ordering::Relaxed);
                let required = export.input_bytes(product).unwrap();
                assert!(required > 0);
                assert_eq!(
                    counted.reads.load(Ordering::Relaxed),
                    before_reads,
                    "predecode sizing never opens a provider"
                );
                assert!(export.input_bytes(unknown).is_err());
                assert!(
                    export
                        .input_with_allowance(product, budget.reserve(required - 1).unwrap())
                        .is_err()
                );
                assert_eq!(
                    counted.reads.load(Ordering::Relaxed),
                    before_reads,
                    "N-1 export input refuses before any original decode"
                );
                Some(
                    export
                        .input_with_allowance(product, budget.reserve(required).unwrap())
                        .unwrap(),
                )
            } else {
                None
            };
            let ordinary = if owned.is_none() {
                Some(match &native {
                    Some(native) => export.input_for_native(native, completed).unwrap(),
                    None => export.input(product).unwrap(),
                })
            } else {
                None
            };
            let input = owned
                .as_ref()
                .map(FrontProofInput::input)
                .or(ordinary.as_ref())
                .unwrap();
            let actual = hirsel_proof_export::export(input);
            assert_eq!(lower.work_snapshot(), before_lower);
            assert_eq!(front.work_snapshot(), before_front);
            assert!(!input.annotations.is_empty());
            assert!(input.annotations.iter().all(|a| a.location.line != 0));
            assert!(input.signatures.values().any(|s| s.owner_view.is_some()));
            let mut unused = program.entry.clone();
            unused.0.name = "unused".into();
            assert!(
                input.definitions.contains_key(&unused),
                "PS9 includes unused executable roots of the entry package"
            );
            let expected = program.engine.proof_export_input(&program.entry).unwrap();
            assert_eq!(actual, hirsel_proof_export::export(&expected));
            let read_rows = export.reads(product).unwrap();
            assert!(read_rows.contains_key(&unused));
            drop(owned);
            drop(ordinary);
            runtime.shutdown();
            drop(program);
            assert_eq!(
                hirsel_proof_export::export(&export.input(product).unwrap()),
                actual,
                "the exact export remains available after loader/runtime shutdown"
            );
        }
    });
}
#[test]
fn source_export_matches_full_ps9_package_closure_private_views_annotations_and_positions() {
    export_case(
        "@analysis(example: retain = yes)\npub opaque type Hidden: () uses {} { value:u64 }\nfn read(h:Hidden)->u64{h.value}\nfn unused(x:u64)->u64{x+1u64}\nconst A:u8=comptime 1u8; pub fn main()->u8{A}",
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
    );
}

#[test]
fn complete_ps9_input_admission_precedes_body_reads_and_matches_scalar_export() {
    export_case(
        "@analysis(example: retain = yes)\npub opaque type Hidden: () uses {} { value:u64 }\nfn read(h:Hidden)->u64{h.value}\nfn unused(x:u64)->u64{x+1u64}\nconst A:u8=comptime 1u8; pub fn main()->u8{A}",
        true,
        true,
    );
}
