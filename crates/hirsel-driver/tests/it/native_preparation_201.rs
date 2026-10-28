use hirsel_driver::{COMPILER, native::queries::preparation::*};
use hirsel_engine::{runtime::*, store::*, *};
use hirsel_intern::jobs::Jobs;
use std::{
    sync::{Arc, Mutex},
    task::Poll,
};

#[derive(Default)]
struct FaultStore {
    inner: MemStore,
    damaged: Mutex<Option<(Reference, Option<Bytes>)>>,
}
impl ResultStore for FaultStore {
    fn name(&self) -> &'static str {
        "native-preparation-faults"
    }
    fn verifier(&self) -> Option<Arc<hirsel_validate::VerifierCache>> {
        self.inner.verifier()
    }
    fn get(&self, key: &EntryKey) -> Option<Bytes> {
        self.inner.get(key)
    }
    fn put(&self, key: &EntryKey, bytes: &[u8]) {
        self.inner.put(key, bytes);
    }
    fn get_content(&self, reference: Reference) -> Option<Bytes> {
        if let Some((selected, bytes)) = &*self.damaged.lock().unwrap()
            && *selected == reference
        {
            return bytes.clone();
        }
        self.inner.get_content(reference)
    }
    fn put_content(&self, domain: Domain, bytes: &[u8]) -> Option<Reference> {
        self.inner.put_content(domain, bytes)
    }
    fn lookup_product(&self, address: Address) -> Option<Association> {
        self.inner.lookup_product(address)
    }
    fn associate_product(&self, address: Address, action: Action, reference: Reference) {
        self.inner.associate_product(address, action, reference);
    }
}
struct Fixture {
    runtime: AdmittedRuntime,
    source: Arc<FrontNativeDefinitionQueries>,
    paths: Vec<DefPath>,
    store: Arc<FaultStore>,
    budget: Arc<ProcessBudget>,
    errors: RuntimeErrors,
    weight: MemoryWeight,
}
fn fixture(text: &str, verify: Verify, jobs: usize) -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let manifest = directory.path().join("hirsel.json");
    std::fs::write(directory.path().join("main.hir"), text).unwrap();
    std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"native_preparation","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
    let store = Arc::new(FaultStore::default());
    let mut program = COMPILER
        .with_config(CompileConfig {
            jobs: Jobs::new(jobs),
            store: store.clone(),
            verify,
            hold_bytes: 0,
            ..Default::default()
        })
        .with_owned_engines()
        .load_program(&manifest)
        .unwrap();
    let paths = program.engine.front_source_roots();
    let budget = ProcessBudget::new(512 << 20);
    let errors = crate::scheduled_front::errors();
    let weight = MemoryWeight {
        retained: 1 << 20,
        scratch: 16 << 20,
        stack: 0,
    };
    let mut runtime =
        AdmittedRuntime::new(Jobs::new(jobs), budget.reserve(256 << 20).unwrap(), errors).unwrap();
    let front = program
        .engine
        .front_body_queries(&paths, weight, budget.reserve(24 << 20).unwrap(), errors)
        .unwrap();
    let lower = program
        .engine
        .front_lower_queries(
            &paths,
            front.clone(),
            weight,
            paths.len() * 3,
            budget.reserve(24 << 20).unwrap(),
            errors,
        )
        .unwrap();
    let source = program
        .engine
        .front_native_definition_queries(
            &paths,
            front.clone(),
            lower.clone(),
            weight,
            budget.reserve(24 << 20).unwrap(),
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
        .register(FRONT_NATIVE_DEFINITION_KIND, source.factory())
        .unwrap();
    Fixture {
        runtime,
        source,
        paths,
        store,
        budget,
        errors,
        weight,
    }
}
impl Fixture {
    fn path(&self, name: &str) -> DefPath {
        self.paths
            .iter()
            .find(|path| path.0.name == name)
            .unwrap()
            .clone()
    }
    fn preparation(&self, bytes: u64) -> Arc<NativePreparationQueries> {
        NativePreparationQueries::new(
            self.source.clone(),
            &self.paths,
            self.store.clone(),
            self.weight,
            self.budget.reserve(bytes).unwrap(),
            self.errors,
        )
        .unwrap()
    }
    fn complete(&mut self, preparation: &Arc<NativePreparationQueries>, name: &str) -> ProductRef {
        self.runtime
            .register(PREPARATION_KIND, preparation.factory())
            .unwrap();
        let query = preparation.query(&self.path(name)).unwrap();
        let request = self.runtime.request(&[query, query]);
        let outputs = self
            .runtime
            .wait(request)
            .unwrap_or_else(|error| panic!("{error:?}"));
        assert_eq!(outputs[0], outputs[1]);
        outputs[0]
    }
}
const SOURCE: &str = "type Token {value:u64}\nfn helper(x:u64)->u64{Token(value:x).value+1u64}\nfn idle()->u64{5u64}\npub fn main()->u64{let f=fn(x:u64)->u64{helper(x)};f(2u64)}";

#[test]
fn preparation_demands_one_source_and_keeps_closure_and_layout_views_after_registry_drop() {
    with_compiler_stack(|| {
        for jobs in [1, 2] {
            let generic = SOURCE.replace(
                "fn helper(x:u64)->u64{Token(value:x).value+1u64}",
                "fn helper[T](take x:T)->T{x}",
            );
            let mut f = fixture(&generic, Verify::On, jobs);
            let budget = f.budget.clone();
            let preparation = f.preparation(1 << 20);
            let source_bytes = budget.reserved() - (256 << 20) - (1 << 20);
            let main = f.path("main");
            let output = f.complete(&preparation, "main");
            let facts = preparation.outcome(output).unwrap();
            assert!(Arc::ptr_eq(&facts, &preparation.outcome(output).unwrap()));
            let declaration = facts.declaration().unwrap();
            assert_eq!(declaration.declaration.definition, main);
            assert_eq!(
                declaration.declaration.signature,
                facts.definition().signature.fingerprint
            );
            assert_eq!(
                declaration.declaration.body_count, 2,
                "entry and lexical closure"
            );
            assert!(
                facts
                    .requirements()
                    .iter()
                    .any(|requirement| requirement.callee.0.name == "helper")
            );
            assert!(!facts.anchors().is_empty());
            assert!(facts.definition().lower.is_some());
            let idle = preparation.query(&f.path("idle")).unwrap();
            assert!(
                preparation
                    .outcome(ProductRef {
                        address: idle.address,
                        ..output
                    })
                    .is_none()
            );
            // Nominal layout facts are separately demanded and have no entry.
            let token = preparation.query(&f.path("Token")).unwrap();
            let request = f.runtime.request(&[token]);
            let token = f.runtime.wait(request).unwrap()[0];
            let layout = preparation.outcome(token).unwrap();
            assert!(layout.declaration().is_none());
            assert!(layout.definition().representation.is_some());
            let physical_bytes = facts.retained_metadata_bytes() + layout.retained_metadata_bytes();
            let reader = f
                .source
                .original_context()
                .reader(
                    [facts.original().unwrap().as_ref().clone()],
                    f.store.clone(),
                )
                .unwrap();
            let body = reader.read(&main).unwrap().definition().clone();
            let weak = Arc::downgrade(&body);
            drop(body);
            drop(reader);
            assert!(
                weak.upgrade().is_none(),
                "prepared views retain no original Definition"
            );
            drop(f);
            drop(preparation);
            assert!(budget.reserved() >= source_bytes + physical_bytes);
            assert_eq!(facts.definition().definition, main);
            drop(layout);
            assert!(budget.reserved() >= source_bytes + facts.retained_metadata_bytes());
            drop(facts);
            assert_eq!(budget.reserved(), 0);
        }
    });
}
struct Selected {
    query: QueryId,
    product: Option<ProductRef>,
}
impl QueryContext for Selected {
    fn read(&mut self, query: QueryId) -> Poll<Result<ProductRef, Reference>> {
        assert_eq!(query, self.query);
        self.product
            .map_or(Poll::Pending, |product| Poll::Ready(Ok(product)))
    }
}
#[test]
fn preparation_enforces_exact_view_admission_and_refuses_crossed_or_damaged_products() {
    with_compiler_stack(|| {
        let mut f = fixture(SOURCE, Verify::On, 1);
        let preparation = f.preparation(1 << 20);
        let main = f.path("main");
        let query = preparation.query(&main).unwrap();
        let mut task = preparation.factory().create(query).unwrap();
        let mut selected = Selected {
            query: f.source.query(&main).unwrap(),
            product: None,
        };
        assert!(
            matches!(task.poll(&mut selected), QueryPoll::Waiting(ids) if ids == vec![selected.query])
        );
        drop(task);
        let output = f.complete(&preparation, "main");
        let facts = preparation.outcome(output).unwrap();
        let required = preparation.input_bytes() + facts.retained_metadata_bytes();
        selected.product = Some(facts.source_product());
        for (bytes, succeeds) in [(required - 1, false), (required, true)] {
            let candidate = f.preparation(bytes);
            let mut task = candidate
                .factory()
                .create(candidate.query(&main).unwrap())
                .unwrap();
            match task.poll(&mut selected) {
                QueryPoll::Complete(product) => {
                    assert!(succeeds);
                    assert!(candidate.outcome(product).is_some());
                }
                QueryPoll::Failed(error) => {
                    assert!(!succeeds);
                    assert_eq!(error, f.errors.resource);
                }
                _ => panic!("settled source must finish preparation"),
            }
        }
        assert!(
            NativePreparationQueries::new(
                f.source.clone(),
                &f.paths,
                f.store.clone(),
                f.weight,
                f.budget.reserve(preparation.input_bytes() - 1).unwrap(),
                f.errors
            )
            .is_err()
        );
        let mut crossed = output;
        crossed.action.0 = hirsel_syntax::ContentHash::of_bytes(b"another action");
        assert!(preparation.outcome(crossed).is_none());
        assert!(preparation.outcome(facts.source_product()).is_none());
        for damaged in [None, Some(Bytes::from(b"corrupt preparation".as_slice()))] {
            *f.store.damaged.lock().unwrap() = Some((output.record, damaged));
            assert!(preparation.outcome(output).is_none());
        }
        *f.store.damaged.lock().unwrap() = None;
        let candidate = f.preparation(1 << 20);
        selected.product = Some(output);
        let mut task = candidate
            .factory()
            .create(candidate.query(&main).unwrap())
            .unwrap();
        assert!(
            matches!(task.poll(&mut selected), QueryPoll::Failed(error) if error == f.errors.protocol)
        );
    });
}
#[test]
fn preparation_binds_body_signature_and_private_layout_edits_without_verify_off_authority() {
    with_compiler_stack(|| {
        let mut observations = Vec::new();
        for (text, verify, selected) in [
            (SOURCE.to_string(), Verify::On, "helper"),
            (
                SOURCE.replace("value+1u64", "value+3u64"),
                Verify::On,
                "helper",
            ),
            (
                SOURCE
                    .replace(
                        "helper(x:u64)->u64{Token(value:x).value+1u64}",
                        "helper(x:u64)->u32{3u32}",
                    )
                    .replace("main()->u64", "main()->u32")
                    .replace("fn(x:u64)->u64", "fn(x:u64)->u32"),
                Verify::On,
                "helper",
            ),
            (SOURCE.to_string(), Verify::Off, "helper"),
        ] {
            let mut f = fixture(&text, verify, 1);
            let preparation = f.preparation(1 << 20);
            let product = f.complete(&preparation, selected);
            let facts = preparation.outcome(product).unwrap();
            if verify == Verify::Off {
                assert!(facts.definition().lower.is_none());
                assert!(facts.original().unwrap().verified.is_none());
            }
            observations.push(product);
        }
        for changed in &observations[1..] {
            assert_ne!(observations[0].action, changed.action);
            assert_ne!(observations[0].record, changed.record);
        }
        let mut layouts = Vec::new();
        for text in [
            SOURCE.to_string(),
            SOURCE
                .replace("Token {value:u64}", "Token {value:u64, other:u64}")
                .replace("Token(value:x)", "Token(value:x,other:0u64)"),
        ] {
            let mut f = fixture(&text, Verify::On, 1);
            let preparation = f.preparation(1 << 20);
            layouts.push(f.complete(&preparation, "Token"));
        }
        assert_ne!(layouts[0].action, layouts[1].action);
        assert_ne!(layouts[0].record, layouts[1].record);
    });
}
