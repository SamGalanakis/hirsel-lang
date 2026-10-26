use hirsel_driver::*;
use hirsel_engine::{runtime::*, store::MemStore, *};
use std::sync::Arc;
#[test]
fn ordered_check_gates_lowering_and_preserves_file_failure_prefixes() {
    with_compiler_stack(|| {
        for (source, reject_growth, fail_lower) in [
            (
                "const A:u8=comptime 1u8;\nconst B:u8=comptime A;\npub fn main()->u8{B}\n",
                false,
                false,
            ),
            (
                "const A:u8=comptime 1u8;\nfn grow[T: copyable](x:T){grow((copy x,copy x))}\npub fn main(){}\n",
                true,
                false,
            ),
            (
                "const BAD:u8=comptime(1u8/0u8);\nconst LATER:u8=comptime 2u8;\npub fn main()->u8{0u8}\n",
                false,
                true,
            ),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("main.hir");
            std::fs::write(&path, source).unwrap();
            for jobs in [1, 2] {
                for policy in [LoweringPolicy::Comptime, LoweringPolicy::Reachable] {
                    check_case(&path, reject_growth, fail_lower, jobs, policy);
                }
            }
        }
    });
}

fn check_case(
    path: &std::path::Path,
    reject_growth: bool,
    fail_lower: bool,
    jobs: usize,
    policy: LoweringPolicy,
) {
    // These local vectors exercise source-order publication and the original
    // Lower grants. Standalone .hir loading would also import std's unrelated
    // specialization declarations, which need an accepted namespace.
    let manifest = path.with_extension("json");
    std::fs::write(
        &manifest,
        serde_json::json!({
            "schema":1,"package":{"name":"scheduled_check","version":"1"},
            "sources":[{"path":path.file_name().unwrap().to_str().unwrap(),"module":[]}],
            "dependencies":[],"entry":{"module":[],"name":"main"}
        })
        .to_string(),
    )
    .unwrap();
    let store = Arc::new(MemStore::default());
    let mut program = COMPILER
        .with_config(CompileConfig {
            store: store.clone(),
            jobs: hirsel_intern::jobs::Jobs::new(jobs),
            ..Default::default()
        })
        .with_owned_engines()
        .load_program(&manifest)
        .unwrap();
    assert_eq!(program.files.len(), 1, "the complete local source vector");
    let mut definitions = Vec::new();
    for (file, context) in program.files.clone() {
        definitions.extend(
            program
                .engine
                .parse_file(file, &context)
                .unwrap()
                .value
                .items
                .iter()
                .map(|syntax| syntax.item.definition.clone()),
        );
    }
    let budget = ProcessBudget::new(1 << 30);
    let errors = crate::scheduled_front::errors();
    let weight = MemoryWeight {
        retained: 2 << 20,
        scratch: 8 << 20,
        stack: 0,
    };
    let mut runtime = AdmittedRuntime::with_query_capacity(
        hirsel_intern::jobs::Jobs::new(jobs),
        budget.reserve(512 << 20).unwrap(),
        errors,
        definitions.len() * 3 + 2,
    )
    .unwrap();
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
            32,
            budget.reserve(32 << 20).unwrap(),
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
    // Match package checking: the requested policy applies to the root
    // package; dependencies lower only their compile-time obligations.
    let root = program.entry.0.package.clone();
    let plan = FrontCheckPlan {
        files: program
            .files
            .iter()
            .map(|(file, context)| {
                let lowering = if context.package == root {
                    policy
                } else {
                    LoweringPolicy::Comptime
                };
                (*file, context.clone(), lowering)
            })
            .collect(),
        front: front.clone(),
        lower: lower.clone(),
        growth: growth.clone(),
        unsafe_ffi: program.unsafe_ffi.clone(),
        lint_package: None,
    };
    let check = program
        .engine
        .front_check_queries(plan, weight, budget.reserve(16 << 20).unwrap(), errors)
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
    let request = runtime.request(&[check.query()]);
    let actual = runtime
        .wait(request)
        .map(|products| check.outcome(products[0]).unwrap())
        .map_err(|reference| {
            check
                .failure(reference)
                .map(|(error, _)| error)
                .or_else(|| lower.failure(reference))
                .unwrap_or_else(|| panic!("unowned failure {reference:?}"))
        });
    if reject_growth {
        let mut constant = program.entry.clone();
        constant.0.name = "A".into();
        assert_eq!(
            lower.work(&constant).initializers,
            0,
            "GEN5 rejection withholds S1"
        );
    }
    if fail_lower {
        let mut later = program.entry.clone();
        later.0.name = "LATER".into();
        assert_eq!(
            lower.work(&later).initializers,
            0,
            "failed earlier Lower withholds suffix"
        );
    }
    let expected = program.check(policy);
    match (actual, expected) {
        (Ok(actual), Ok(expected)) => {
            assert_eq!(actual.checked.completion, expected.completion);
            assert_eq!(
                actual.checked.diagnostics, expected.diagnostics,
                "jobs={jobs},policy={policy:?}"
            );
        }
        (Err(actual), Err(expected)) => assert_eq!(actual, expected),
        (actual, expected) => panic!(
            "different acceptance: actual={},expected={:?}",
            actual.is_ok(),
            expected.err()
        ),
    }
    runtime.shutdown();
}

#[test]
fn ordered_reachable_check_fits_original_lower_metadata_grant() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.hir");
        std::fs::write(
            &path,
            "const A:u8=comptime 1u8;\nconst B:u8=comptime A;\npub fn main()->u8{B}\n",
        )
        .unwrap();
        check_case(&path, false, false, 1, LoweringPolicy::Reachable);
    });
}

#[test]
fn settled_front_check_does_not_decode_an_evicted_modes_body() {
    use hirsel_engine::store::{Bytes, EntryKey, Reference, ResultStore};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    #[derive(Default)]
    struct EvictModes {
        store: MemStore,
        evicted: AtomicBool,
        reads_after_eviction: AtomicUsize,
    }
    impl ResultStore for EvictModes {
        fn get(&self, key: &EntryKey) -> Option<Bytes> {
            self.store.get(key)
        }
        fn put(&self, key: &EntryKey, value: &[u8]) {
            self.store.put(key, value);
        }
        fn get_content(&self, reference: Reference) -> Option<Bytes> {
            if reference.domain.kind == QueryKind::Modes as u32 + 16
                && self.evicted.load(Ordering::Relaxed)
            {
                self.reads_after_eviction.fetch_add(1, Ordering::Relaxed);
                return None;
            }
            self.store.get_content(reference)
        }
        fn name(&self) -> &'static str {
            "evicted-modes-control"
        }
    }
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.hir");
        std::fs::write(&path, "pub fn main()->u8{7u8}\n").unwrap();
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(
            &manifest,
            serde_json::json!({
                "schema":1,"package":{"name":"compact_front","version":"1"},
                "sources":[{"path":"main.hir","module":[]}],"dependencies":[],
                "entry":{"module":[],"name":"main"}
            })
            .to_string(),
        )
        .unwrap();
        let store = Arc::new(EvictModes::default());
        let mut program = COMPILER
            .with_config(CompileConfig {
                store: store.clone(),
                hold_bytes: 0,
                ..Default::default()
            })
            .with_owned_engines()
            .load_program(&manifest)
            .unwrap();
        let definitions = vec![program.entry.clone()];
        let budget = ProcessBudget::new(1 << 30);
        let errors = crate::scheduled_front::errors();
        let weight = MemoryWeight {
            retained: 2 << 20,
            scratch: 8 << 20,
            stack: 0,
        };
        let mut runtime = AdmittedRuntime::new(
            hirsel_intern::jobs::Jobs::new(1),
            budget.reserve(512 << 20).unwrap(),
            errors,
        )
        .unwrap();
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
                32,
                budget.reserve(32 << 20).unwrap(),
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
        let plan = FrontCheckPlan {
            files: program
                .files
                .iter()
                .map(|(file, context)| (*file, context.clone(), LoweringPolicy::Comptime))
                .collect(),
            front: front.clone(),
            lower: lower.clone(),
            growth: growth.clone(),
            unsafe_ffi: program.unsafe_ffi.clone(),
            lint_package: None,
        };
        let check = program
            .engine
            .front_check_queries(plan, weight, budget.reserve(16 << 20).unwrap(), errors)
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
        let request = runtime.request(&[front.query(&program.entry).unwrap()]);
        runtime.wait(request).unwrap();
        assert_eq!(front.live_body_count(), 0, "settled P owns no body");
        store.evicted.store(true, Ordering::Relaxed);
        let request = runtime.request(&[check.query()]);
        let products = runtime
            .wait(request)
            .expect("accepted front facts survive body eviction");
        let output = check.outcome(products[0]).unwrap();
        assert_eq!(output.checked.completion, CheckCompletion::Complete);
        assert!(output.checked.diagnostics.is_empty());
        assert_eq!(store.reads_after_eviction.load(Ordering::Relaxed), 0);
        for kind in [QueryKind::Resolve, QueryKind::Check, QueryKind::Modes] {
            assert_eq!(front.recompute_count(&program.entry, kind), 1);
        }
        assert_eq!(front.live_body_count(), 0);
        runtime.shutdown();
    });
}
