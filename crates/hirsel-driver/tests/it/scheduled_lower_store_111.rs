use hirsel_driver::*;
use hirsel_engine::store::ResultStore;
use hirsel_engine::{runtime::*, store::MemStore, *};
use std::sync::Arc;

struct FreshBytes {
    bytes: MemStore,
    verifier: Arc<hirsel_validate::VerifierCache>,
}
impl store::ResultStore for FreshBytes {
    fn verifier(&self) -> Option<Arc<hirsel_validate::VerifierCache>> {
        Some(self.verifier.clone())
    }
    fn get(&self, key: &store::EntryKey) -> Option<store::Bytes> {
        self.bytes.get(key)
    }
    fn put(&self, key: &store::EntryKey, value: &[u8]) {
        self.bytes.put(key, value);
    }
    fn name(&self) -> &'static str {
        "mem@1"
    }
}

#[test]
fn stored_lower_reuses_unchanged_inputs_and_rechecks_edited_constant_dependencies() {
    check_lower_reuse(false);
}

#[test]
fn carried_lower_republishes_type_components_into_a_fresh_store() {
    check_lower_reuse(true);
}

fn check_lower_reuse(carry_across_stores: bool) {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("main.hir");
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"lower_reuse","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        for verify in [Verify::On, Verify::Off] {
            for jobs in [1, 2] {
                let store: Arc<dyn store::ResultStore> = Arc::new(MemStore::default());
                let carry_budget = ProcessBudget::new(32 << 20);
                let carry = LowerCarry::new(carry_budget.reserve(32 << 20).unwrap())
                    .ok()
                    .unwrap();
                for revision in 0..4 {
                    let store: Arc<dyn store::ResultStore> = if carry_across_stores {
                        Arc::new(FreshBytes {
                            bytes: MemStore::default(),
                            verifier: store.verifier().unwrap(),
                        })
                    } else {
                        store.clone()
                    };
                    let value = if revision == 3 { 2 } else { 1 };
                    let extra = if revision >= 2 { "+0u8" } else { "" };
                    std::fs::write(&source, format!("const A:u8=comptime {value}u8;\nfn helper()->u8{{A}}\npub fn main()->u8{{comptime helper(){extra}}}\n")).unwrap();
                    let mut program = COMPILER
                        .with_config(CompileConfig {
                            store: store.clone(),
                            verify,
                            jobs: hirsel_intern::jobs::Jobs::new(jobs),
                            ..Default::default()
                        })
                        .with_owned_engines()
                        .load_program(&manifest)
                        .unwrap();
                    let definitions: Vec<_> = ["A", "helper", "main"]
                        .iter()
                        .map(|name| {
                            let mut path = program.entry.clone();
                            path.0.name = (*name).into();
                            path
                        })
                        .collect();
                    let errors = crate::scheduled_front::errors();
                    let budget = ProcessBudget::new(1 << 30);
                    let mut runtime = AdmittedRuntime::new(
                        hirsel_intern::jobs::Jobs::new(jobs),
                        budget.reserve(512 << 20).unwrap(),
                        errors,
                    )
                    .unwrap();
                    let weight = MemoryWeight {
                        retained: 2 << 20,
                        scratch: 8 << 20,
                        stack: 0,
                    };
                    let front = program
                        .engine
                        .front_body_queries(
                            &definitions,
                            weight,
                            budget.reserve(16 << 20).unwrap(),
                            errors,
                        )
                        .unwrap();
                    let lower = program
                        .engine
                        .front_lower_queries(
                            &definitions,
                            front.clone(),
                            weight,
                            64,
                            budget.reserve(32 << 20).unwrap(),
                            errors,
                        )
                        .unwrap();
                    if carry_across_stores {
                        lower.install_carry(carry.clone()).ok().unwrap();
                    }
                    runtime.register(FRONT_BODY_KIND, front.factory()).unwrap();
                    runtime
                        .register_local(FRONT_LOWER_KIND, lower.factory())
                        .unwrap();
                    runtime
                        .register_local(FRONT_EVALUATE_KIND, lower.factory())
                        .unwrap();
                    let request = runtime.request(&[lower.query(&program.entry).unwrap()]);
                    let products = runtime.wait(request).unwrap_or_else(|error| {
                        panic!(
                            "revision={revision}, jobs={jobs}, verify={verify:?}: {:?}",
                            lower.failure(error)
                        )
                    });
                    let actual = lower.outcome((products[0]).clone()).unwrap();
                    let scalar = program.engine.lower_to_tir(&program.entry).unwrap();
                    let records = NativeRecords::new(program.engine.types().clone(), store.clone());
                    assert_eq!(
                        records.encode(&program.engine, &actual.output.value),
                        records.encode(&program.engine, &scalar.value)
                    );
                    assert_eq!(actual.verified.is_some(), verify == Verify::On);
                    let mut live_grant = budget.reserve(16 << 20).unwrap();
                    let live_lower = actual
                        .into_live((products[0]).clone(), &mut live_grant)
                        .unwrap();
                    assert_eq!(live_lower.address(), products[0].address);
                    assert_eq!(live_lower.action(), products[0].action);
                    assert_eq!(live_lower.output().value.path.path(), &program.entry);
                    let front_request = runtime.request(&[front.query(&program.entry).unwrap()]);
                    let front_product = runtime.wait(front_request).unwrap()[0].clone();
                    let live_summary = front.live_summary(front_product, &mut live_grant).unwrap();
                    assert!(live_summary.equivalent(&live_summary.clone()));
                    if revision == 0 {
                        let mut scratch = budget.reserve(8 << 20).unwrap();
                        let archive = live_lower.archive(&mut scratch, 8 << 20).unwrap();
                        assert_eq!(archive.reference().domain, LIVE_LOWER);
                        assert_eq!(
                            store.put_content(LIVE_LOWER, archive.bytes()),
                            Some(archive.reference())
                        );
                        assert_eq!(
                            live_lower
                                .archive(&mut scratch, u64::MAX)
                                .unwrap()
                                .reference(),
                            archive.reference()
                        );
                    }
                    let work = lower.work_snapshot();
                    let lowerings: u64 = work.iter().map(|(_, w)| w.lowerings).sum();
                    let initializers: u64 = work.iter().map(|(_, w)| w.initializers).sum();
                    match revision {
                        0 => assert!(lowerings > 0 && initializers > 0),
                        1 => assert_eq!(
                            (lowerings, initializers),
                            (0, 0),
                            "fresh graph must adopt ordinary Lower products"
                        ),
                        2 => {
                            assert_eq!(lower.work(&definitions[0]).initializers, 0);
                            assert_eq!(lower.work(&definitions[1]).lowerings, 0);
                            assert_eq!(lower.work(&definitions[2]).lowerings, 1);
                        }
                        3 => assert!(initializers > 0, "edited constant must be evaluated again"),
                        _ => unreachable!(),
                    }
                    runtime.shutdown();
                    drop(runtime);
                    drop(front);
                    drop(lower);
                    drop(program);
                    drop(live_grant);
                    assert_eq!(
                        live_lower.output().value.path,
                        live_summary.value().definition
                    );
                    assert!(budget.reserved() > 0);
                    drop(live_lower);
                    drop(live_summary);
                    assert_eq!(
                        budget.reserved(),
                        0,
                        "last typed views release the complete producer grants"
                    );
                }
            }
        }
    });
}

#[test]
fn live_lower_retains_complete_observations_under_exact_admission() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.hir");
        std::fs::write(&path, "pub fn main()->u8{comptime 7u8}\n").unwrap();
        for verify in [Verify::On, Verify::Off] {
            let store: Arc<dyn ResultStore> = Arc::new(MemStore::default());
            let mut program = COMPILER
                .with_config(CompileConfig {
                    store: store.clone(),
                    verify,
                    ..Default::default()
                })
                .with_owned_engines()
                .load_program(&path)
                .unwrap();
            let budget = ProcessBudget::new(1 << 30);
            let errors = crate::scheduled_front::errors();
            let mut runtime = AdmittedRuntime::new(
                hirsel_intern::jobs::Jobs::ONE,
                budget.reserve(512 << 20).unwrap(),
                errors,
            )
            .unwrap();
            let weight = MemoryWeight {
                retained: 2 << 20,
                scratch: 8 << 20,
                stack: 0,
            };
            let definitions = [program.entry.clone()];
            let source_bytes = program
                .engine
                .front_input_footprint(&definitions)
                .unwrap()
                .source_headers;
            let source_admission = Arc::new(budget.reserve(source_bytes).unwrap());
            let source_owner = Arc::downgrade(&source_admission);
            program.engine.sources =
                std::mem::take(&mut program.engine.sources).replace_owner(source_admission);
            let front = program
                .engine
                .front_body_queries(
                    &definitions,
                    weight,
                    budget.reserve(16 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let lower = program
                .engine
                .front_lower_queries(
                    &definitions,
                    front.clone(),
                    weight,
                    64,
                    budget.reserve(32 << 20).unwrap(),
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
            let request = runtime.request(&[lower.query(&program.entry).unwrap()]);
            let product = runtime.wait(request).unwrap().remove(0);
            let actual = lower.outcome(product.clone()).unwrap();
            assert!(
                !actual.materialized.is_empty(),
                "exercise a real staged value"
            );
            assert_eq!(actual.verified.is_some(), verify == Verify::On);
            let expected = actual.clone();
            let required = LiveLowerProduct::required_bytes(&actual).unwrap();
            let mut short = budget.reserve(required - 1).unwrap();
            assert!(
                actual
                    .clone()
                    .into_live(product.clone(), &mut short)
                    .is_none()
            );
            assert_eq!(short.bytes(), required - 1);
            drop(short);
            let mut exact = budget.reserve(required).unwrap();
            let live = actual.into_live(product, &mut exact).unwrap();
            assert_eq!(exact.bytes(), 0);
            assert_eq!(live.retained_bytes(), required);
            let observations = live.observations();
            assert_eq!(
                observations.implementation_revision,
                expected.implementation_revision
            );
            assert_eq!(observations.signature, expected.signature);
            assert_eq!(observations.verified, expected.verified);
            assert_eq!(
                observations.execution.is_some(),
                expected.execution.is_some()
            );
            assert_eq!(observations.execution_subject, expected.execution_subject);
            assert_eq!(observations.executed, expected.executed);
            assert_eq!(observations.selections, expected.selections);
            assert_eq!(observations.acceptance, expected.acceptance);
            assert_eq!(live.reads(), &expected.reads);
            assert_eq!(live.depth().first, expected.depth.first);
            let records = NativeRecords::new(program.engine.types().clone(), store);
            let mut scratch = budget.reserve(8 << 20).unwrap();
            let archive = live.archive(&mut scratch, 8 << 20).unwrap();
            let (
                output,
                reads,
                depth,
                materialized,
                implementation,
                signature,
                executed,
                selections,
            ): (
                hirsel_types::QueryOutput<hirsel_tir::DefinitionRef>,
                Reads,
                memo::DepthEvidence<hirsel_span::Span>,
                Vec<(hirsel_syntax::ExprId, hirsel_tir::Materialized)>,
                hirsel_syntax::ContentHash,
                hirsel_syntax::ContentHash,
                bool,
                Vec<(hirsel_syntax::DefId, Vec<u64>)>,
            ) = records.decode(&program.engine, archive.bytes()).unwrap();
            assert_eq!(output.value, expected.output.value);
            assert_eq!(output.diagnostics, expected.output.diagnostics);
            assert_eq!(output.dependencies, expected.output.dependencies);
            assert_eq!(reads, expected.reads);
            assert_eq!(depth.first, expected.depth.first);
            assert_eq!(implementation, expected.implementation_revision);
            assert_eq!(signature, expected.signature);
            assert_eq!(executed, expected.executed);
            assert_eq!(selections, expected.selections);
            assert_eq!(
                records.encode(&program.engine, &materialized),
                records.encode(&program.engine, &expected.materialized)
            );
            let producer = Arc::downgrade(&lower);
            let front_producer = Arc::downgrade(&front);
            drop(expected);
            drop(program);
            runtime.shutdown();
            drop(runtime);
            drop(front);
            drop(lower);
            assert!(
                producer.upgrade().is_none(),
                "a live value does not retain its producer table"
            );
            assert!(
                front_producer.upgrade().is_none(),
                "Lower retains source backing independently of P"
            );
            drop(scratch);
            drop(exact);
            assert!(
                source_owner.upgrade().is_some(),
                "the source grant survives its graph"
            );
            assert!(
                budget.reserved() > required,
                "live output pins its producer backing"
            );
            assert!(budget.reserved() >= required + source_bytes);
            drop(live);
            assert!(source_owner.upgrade().is_none());
            assert_eq!(budget.reserved(), archive.retained_bytes());
            drop(archive);
            assert_eq!(budget.reserved(), 0);
        }
    });
}
