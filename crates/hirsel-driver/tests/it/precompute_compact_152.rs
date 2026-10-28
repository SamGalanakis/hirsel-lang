use hirsel_driver::{COMPILER, CompilerRequestsExt, requests::*};
use hirsel_engine::{
    CheckCompletion, CompileConfig, LoweringPolicy, QueryError, QueryKind, ToolError,
    store::{self, ResultStore},
    with_compiler_stack,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct Observed {
    inner: store::DirStore,
    mode_reads: AtomicUsize,
    compact_fault: AtomicUsize,
}
impl ResultStore for Observed {
    fn get(&self, key: &store::EntryKey) -> Option<store::Bytes> {
        self.inner.get(key)
    }
    fn put(&self, key: &store::EntryKey, value: &[u8]) {
        self.inner.put(key, value);
    }
    fn get_content(&self, reference: store::Reference) -> Option<store::Bytes> {
        if reference.domain == store::product_domain(QueryKind::Modes) {
            self.mode_reads.fetch_add(1, Ordering::SeqCst);
        }
        if reference.domain.kind == 0x4600_0037 {
            match self.compact_fault.load(Ordering::SeqCst) {
                1 => return None,
                2 => return Some(vec![0xff; 8].into()),
                3 => panic!("oversized compact content must be refused before fetching"),
                _ => {}
            }
        }
        self.inner.get_content(reference)
    }
    fn put_content(&self, domain: store::Domain, bytes: &[u8]) -> Option<store::Reference> {
        self.inner.put_content(domain, bytes)
    }
    fn lookup_product(&self, address: store::Address) -> Option<store::Association> {
        let mut selected = self.inner.lookup_product(address)?;
        if selected.reference.domain.kind == 0x4600_0037
            && self.compact_fault.load(Ordering::SeqCst) == 3
        {
            selected.reference.len = u64::MAX;
        }
        Some(selected)
    }
    fn associate_product(
        &self,
        address: store::Address,
        action: store::Action,
        reference: store::Reference,
    ) {
        self.inner.associate_product(address, action, reference);
    }
    fn read_scope(&self) -> Option<Box<dyn Send + Sync>> {
        self.inner.read_scope()
    }
    fn local_product_store(&self) -> Option<hirsel_artifact_store::Store> {
        self.inner.local_product_store()
    }
    fn verifier(&self) -> Option<Arc<hirsel_validate::VerifierCache>> {
        self.inner.verifier()
    }
    fn name(&self) -> &'static str {
        "observed-source-products"
    }
}

#[test]
fn installed_compact_source_checks_skip_modes_bodies_and_refuse_stale_missing_corrupt_or_oversized_facts()
 {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("main.hir");
        let manifest = directory.path().join("hirsel.json");
        let valid = "pub fn first()->u64{7u64}\npub fn second()->u64{first()}\n";
        std::fs::write(&source, valid).unwrap();
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"compact_source","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[]}"#).unwrap();
        let products = directory.path().join("products");
        COMPILER.precompute_package(&manifest, &products).unwrap();
        let observed = Arc::new(Observed {
            inner: store::DirStore::new(directory.path().join("reader"))
                .with_read_only_root(products),
            mode_reads: AtomicUsize::new(0),
            compact_fault: AtomicUsize::new(0),
        });
        let execute = || {
            let config = CompileConfig {
                store: observed.clone(),
                hold_bytes: 0,
                jobs: hirsel_intern::jobs::Jobs::ONE,
                ..Default::default()
            };
            CompilerRequests::for_host(COMPILER.with_config(config.clone()), config).execute(
                &manifest,
                CompileOperation::Check {
                    lowering: LoweringPolicy::Comptime,
                },
            )
        };
        let first = execute().unwrap();
        let CompileResult::Check(checked) = &first.result else {
            panic!("expected Check")
        };
        assert_eq!(checked.completion, CheckCompletion::Complete);
        assert!(checked.diagnostics.is_empty());
        assert_eq!(observed.mode_reads.load(Ordering::SeqCst), 0);
        drop(first);
        for fault in [1, 2, 3] {
            observed.compact_fault.store(fault, Ordering::SeqCst);
            let before = observed.mode_reads.load(Ordering::SeqCst);
            let fallback = execute().unwrap();
            let CompileResult::Check(checked) = &fallback.result else {
                panic!("expected Check")
            };
            assert_eq!(checked.completion, CheckCompletion::Complete);
            assert!(checked.diagnostics.is_empty());
            assert!(
                observed.mode_reads.load(Ordering::SeqCst) > before,
                "fault {fault}"
            );
        }
        observed.compact_fault.store(0, Ordering::SeqCst);
        std::fs::write(source, valid.replace("{7u64}", "{false}")).unwrap();
        match execute() {
            Ok(output) => {
                let CompileResult::Check(checked) = &output.result else {
                    panic!("expected Check")
                };
                assert!(
                    checked
                        .diagnostics
                        .iter()
                        .any(|d| checked.sources.slice(d.primary) == Some("false"))
                );
            }
            Err(ToolError::Query {
                error: QueryError::Diagnostics(diagnostics),
                sources,
                ..
            }) => {
                assert!(
                    diagnostics
                        .iter()
                        .any(|d| sources.slice(d.primary) == Some("false"))
                );
            }
            Err(error) => panic!("expected current source diagnostics, got {error:?}"),
        }
    });
}

#[test]
fn cached_front_retains_no_more_metadata_than_the_same_cold_front() {
    use hirsel_engine::runtime::*;
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(
            directory.path().join("main.hir"),
            "pub fn first()->u64{7u64}\npub fn second()->u64{first()+first()}\n",
        )
        .unwrap();
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"compact_source","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"second"}}"#).unwrap();
        let products = directory.path().join("products");
        COMPILER.precompute_package(&manifest, &products).unwrap();
        let execute = |store: Arc<dyn ResultStore>| {
            let mut program = COMPILER
                .with_config(CompileConfig {
                    store,
                    jobs: hirsel_intern::jobs::Jobs::ONE,
                    ..Default::default()
                })
                .with_owned_engines()
                .load_program(&manifest)
                .unwrap();
            let paths = program
                .engine
                .indexed_definitions()
                .into_iter()
                .filter(|p| p.0.package.name == "compact_source")
                .collect::<Vec<_>>();
            assert_eq!(paths.len(), 2);
            let budget = ProcessBudget::new(1 << 30);
            let errors = crate::scheduled_front::errors();
            let front = program
                .engine
                .front_body_queries(
                    &paths,
                    MemoryWeight {
                        retained: 1 << 20,
                        scratch: 8 << 20,
                        stack: 0,
                    },
                    budget.reserve(8 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let mut runtime = AdmittedRuntime::new(
                hirsel_intern::jobs::Jobs::ONE,
                budget.reserve(512 << 20).unwrap(),
                errors,
            )
            .unwrap();
            runtime
                .register(hirsel_engine::FRONT_BODY_KIND, front.factory())
                .unwrap();
            let roots = paths
                .iter()
                .map(|p| front.query(p).unwrap())
                .collect::<Vec<_>>();
            let request = runtime.request(&roots);
            assert_eq!(runtime.wait(request).unwrap().len(), paths.len());
            let retained = front.retained_bytes();
            let recomputed = paths
                .iter()
                .map(|p| front.recompute_count(p, QueryKind::Modes))
                .sum::<u64>();
            let adopted = front.store_adoptions();
            runtime.shutdown();
            drop(front);
            assert_eq!(budget.reserved(), 0);
            (retained, recomputed, adopted)
        };
        let cold = execute(Arc::new(store::MemStore::default()));
        let warm = execute(Arc::new(
            store::DirStore::new(directory.path().join("reader")).with_read_only_root(products),
        ));
        assert_eq!(cold.1, 2);
        assert_eq!(warm.1, 0);
        assert_eq!(warm.2, 2);
        assert!(warm.0 <= cold.0, "warm={} cold={}", warm.0, cold.0);
    });
}
