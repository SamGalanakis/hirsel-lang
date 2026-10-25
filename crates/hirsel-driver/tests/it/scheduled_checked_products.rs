use hirsel_driver::*;
use hirsel_engine::{runtime::*, store::MemStore, *};
use std::sync::Arc;

fn products_case(source: &str, failed_front: bool, failed_lower: bool) {
    products_case_with_admission(source, failed_front, failed_lower, false);
}
fn products_case_with_admission(
    source: &str,
    failed_front: bool,
    failed_lower: bool,
    admitted: bool,
) {
    products_case_full_admission(source, failed_front, failed_lower, admitted, false);
}
fn products_case_full_admission(
    source: &str,
    failed_front: bool,
    failed_lower: bool,
    admitted: bool,
    finish: bool,
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
                    store: store.clone(),
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
            let headers = program
                .engine
                .front_checked_signatures(front.clone(), budget.reserve(4 << 20).unwrap())
                .unwrap();
            let other_front = program
                .engine
                .front_body_queries(
                    &definitions,
                    weight,
                    budget.reserve(8 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let wrong_headers = program
                .engine
                .front_checked_signatures(other_front, budget.reserve(4 << 20).unwrap())
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
            let query = check.query();
            let unknown = ProductRef {
                address: query.address,
                action: hirsel_artifact_store::Action(query.address.0),
                record: errors.protocol,
            };
            assert!(
                check
                    .completed_scope(unknown, budget.reserve(1 << 20).unwrap())
                    .is_err()
            );
            let request = runtime.request(&[query]);
            let product = runtime.wait(request).unwrap()[0];
            let output = check.outcome(product).unwrap();
            assert_eq!(output.checked.completion, CheckCompletion::Complete);
            assert_eq!(
                output
                    .checked
                    .diagnostics
                    .iter()
                    .any(|d| d.severity == hirsel_diag::Severity::Error),
                failed_front || failed_lower
            );
            assert!(
                check
                    .completed_scope(unknown, budget.reserve(1 << 20).unwrap())
                    .is_err()
            );
            assert!(
                check
                    .completed_scope(product, budget.reserve(0).unwrap())
                    .is_err()
            );
            let scope = Arc::new(
                check
                    .completed_scope(product, budget.reserve(1 << 20).unwrap())
                    .unwrap(),
            );
            assert!(!scope.reads().is_empty());
            assert!(scope.reads().values().all(|r| !r.is_empty()));
            if failed_front {
                assert!(
                    scope.reads().contains_key(&program.entry),
                    "a failed P root retains its negative reads"
                );
                assert!(scope.lowered().is_empty());
            } else if failed_lower {
                let mut bad = program.entry.clone();
                bad.0.name = "BAD".into();
                let mut later = program.entry.clone();
                later.0.name = "LATER".into();
                assert!(scope.failed_lowered().contains(&bad));
                assert!(scope.reads().contains_key(&bad));
                assert!(!scope.lowered().contains(&later));
                assert_eq!(lower.work(&later).initializers, 0);
                let before = lower.work(&bad);
                assert!(
                    scope.definition(&bad).is_err(),
                    "failed root cannot demand a body"
                );
                assert_eq!(lower.work(&bad), before);
            } else {
                let mut constant = program.entry.clone();
                constant.0.name = "A".into();
                assert!(scope.lowered().contains(&constant));
                assert!(scope.failed_lowered().is_empty());
                let before = lower.work(&constant);
                assert!(scope.definition(&constant).is_ok());
                assert_eq!(
                    lower.work(&constant),
                    before,
                    "inspection decodes only settled products"
                );
            }
            assert!(scope.definition(&DefPath::core("unselected")).is_err());
            assert_eq!(front.live_body_count(), 0);
            assert!(
                scope
                    .checked_products(&wrong_headers, &program.entry)
                    .is_err()
            );
            let before_lower = lower.work_snapshot();
            let before_front = front.work_snapshot();
            let owned = if admitted {
                let before_reads = counted.reads.load(Ordering::Relaxed);
                let required = scope
                    .checked_product_input_bytes(&headers, &program.entry)
                    .unwrap();
                assert!(required > 0);
                assert!(
                    scope
                        .checked_product_input_bytes(&wrong_headers, &program.entry)
                        .is_err()
                );
                assert_eq!(counted.reads.load(Ordering::Relaxed), before_reads);
                assert!(
                    scope
                        .checked_product_input_with_allowance(
                            headers.clone(),
                            &program.entry,
                            budget.reserve(required - 1).unwrap()
                        )
                        .is_err()
                );
                assert_eq!(
                    counted.reads.load(Ordering::Relaxed),
                    before_reads,
                    "N-1 refuses before original body I/O"
                );
                Some(std::rc::Rc::new(
                    scope
                        .checked_product_input_with_allowance(
                            headers.clone(),
                            &program.entry,
                            budget.reserve(required).unwrap(),
                        )
                        .unwrap(),
                ))
            } else {
                None
            };
            let actual_products = if finish {
                let owned = owned.as_ref().unwrap().clone();
                let needed = owned.type_capture_bytes().unwrap();
                assert!(
                    owned
                        .clone()
                        .capture_types_with_allowance(budget.reserve(needed - 1).unwrap())
                        .is_err()
                );
                let prepared = std::rc::Rc::new(
                    owned
                        .capture_types_with_allowance(budget.reserve(needed).unwrap())
                        .unwrap(),
                );
                let needed = prepared.build_bytes().unwrap();
                assert!(
                    prepared
                        .clone()
                        .build_with_allowance(budget.reserve(needed - 1).unwrap())
                        .is_err()
                );
                let built = prepared
                    .build_with_allowance(budget.reserve(needed).unwrap())
                    .unwrap();
                let output_bytes = built.output_bytes().unwrap();
                let receipt_bytes = built.checked_products_bytes().unwrap();
                assert!(
                    built
                        .checked_products_with_allowances(
                            budget.reserve(output_bytes).unwrap(),
                            budget.reserve(receipt_bytes - 1).unwrap()
                        )
                        .is_err()
                );
                assert!(
                    built
                        .checked_products_with_allowances(
                            budget.reserve(output_bytes - 1).unwrap(),
                            budget.reserve(receipt_bytes).unwrap()
                        )
                        .is_err()
                );
                built
                    .checked_products_with_allowances(
                        budget.reserve(output_bytes).unwrap(),
                        budget.reserve(receipt_bytes).unwrap(),
                    )
                    .unwrap()
            } else {
                Arc::new(scope.checked_products(&headers, &program.entry).unwrap())
            };
            if let Some(owned) = &owned {
                assert_eq!(
                    Some(hirsel_proof_export::export(owned.input()).as_str()),
                    actual_products.canonical.as_deref()
                );
            }
            assert_eq!(lower.work_snapshot(), before_lower);
            assert_eq!(front.work_snapshot(), before_front);
            let expected = program.check(LoweringPolicy::Comptime).unwrap();
            assert_eq!(expected.completion, output.checked.completion);
            let expected = program.engine.checked_products(&program.entry).unwrap();
            let canonical = |reads: &hirsel_engine::Reads| {
                reads
                    .iter()
                    .map(|(key, value)| {
                        let mut bytes = Vec::new();
                        key.encode(&mut bytes);
                        (bytes, *value)
                    })
                    .collect::<std::collections::BTreeMap<_, _>>()
            };
            let actual = scope
                .reads()
                .iter()
                .map(|(path, reads)| (path.clone(), canonical(reads)))
                .collect::<std::collections::BTreeMap<_, _>>();
            assert_eq!(
                actual, expected.reads,
                "the completed S5 scope preserves scalar complete reads, including failure obligations"
            );
            assert_eq!(actual_products.canonical, expected.canonical);
            assert_eq!(actual_products.digest, expected.digest);
            assert_eq!(
                actual_products.canonical_reads(),
                expected.canonical_reads()
            );
            let entry = program.entry.clone();
            runtime.shutdown();
            drop(program);
            if let Some(owned) = &owned {
                assert_eq!(
                    Some(hirsel_proof_export::export(owned.input()).as_str()),
                    actual_products.canonical.as_deref(),
                    "admitted input survives loader/runtime shutdown"
                );
            }
            assert_eq!(
                scope.checked_products(&headers, &entry).unwrap().digest,
                actual_products.digest,
                "formatting after shutdown retains its immutable sources"
            );
            if finish {
                let detached = actual_products.clone();
                assert!(Arc::ptr_eq(&detached, &actual_products));
                let before = budget.reserved();
                drop(actual_products);
                assert_eq!(budget.reserved(), before);
                drop(detached);
                assert!(
                    budget.reserved() < before,
                    "last receipt clone releases its physical output/map ownership"
                );
            }
        }
    });
}
#[test]
fn completed_checked_products_inspects_only_successful_required_lowerings() {
    products_case(
        "const A:u8=comptime 1u8; pub fn main()->u8{A}",
        false,
        false,
    );
}
#[test]
fn completed_checked_products_preserves_failed_front_negative_reads() {
    products_case("pub fn main()->u8{missing}", true, false);
}
#[test]
fn completed_checked_products_guards_failed_lower_and_withheld_suffix() {
    products_case(
        "const BAD:u8=comptime(1u8/0u8); const LATER:u8=comptime 2u8; pub fn main()->u8{0u8}",
        false,
        true,
    );
}

#[test]
fn completed_checked_products_preserve_private_headers_and_unused_runtime_bodies() {
    products_case(
        "pub opaque type Hidden: () uses {} { value:u64 }\nfn read(h:Hidden)->u64{h.value}\nconst A:u8=comptime 1u8; pub fn main()->u8{A}",
        false,
        false,
    );
}

#[test]
fn completed_checked_input_admission_refuses_before_body_reads_and_preserves_scalar_products() {
    products_case_with_admission(
        "pub opaque type Hidden: () uses {} { value:u64 }\nfn read(h:Hidden)->u64{h.value}\nconst A:u8=comptime 1u8; pub fn main()->u8{A}",
        false,
        false,
        true,
    );
}

#[test]
fn completed_checked_receipt_admission_preserves_scalar_bytes_reads_digest_and_detached_clones() {
    products_case_full_admission(
        "pub opaque type Hidden: () uses {} { value:u64 }\nfn read(h:Hidden)->u64{h.value}\nconst A:u8=comptime 1u8; pub fn main()->u8{A}",
        false,
        false,
        true,
        true,
    );
}

#[test]
fn completed_checked_receipt_admission_preserves_failed_front_negative_reads() {
    products_case_full_admission("pub fn main()->u8{missing}", true, false, true, true);
}
