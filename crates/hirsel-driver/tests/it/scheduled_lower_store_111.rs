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
                    let actual = lower.outcome(products[0]).unwrap();
                    let scalar = program.engine.lower_to_tir(&program.entry).unwrap();
                    let records = NativeRecords::new(program.engine.types().clone(), store.clone());
                    assert_eq!(
                        records.encode(&program.engine, &actual.output.value),
                        records.encode(&program.engine, &scalar.value)
                    );
                    assert_eq!(actual.verified.is_some(), verify == Verify::On);
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
                }
            }
        }
    });
}
