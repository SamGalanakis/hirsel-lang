use hirsel_driver::*;
use hirsel_engine::{runtime::ProcessBudget, store::ResultStore, *};
use hirsel_syntax::ContentHash;
use native::queries::{
    declarations::NativeDeclarations,
    demand::DemandInstances,
    source_inputs::{NativeInstanceInputs, NativeSourceProvider},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Default)]
struct WithoutVerifier(store::MemStore);
impl ResultStore for WithoutVerifier {
    fn get(&self, key: &store::EntryKey) -> Option<store::Bytes> {
        self.0.get(key)
    }
    fn put(&self, key: &store::EntryKey, value: &[u8]) {
        self.0.put(key, value)
    }
    fn name(&self) -> &'static str {
        "ordinary-only-control@1"
    }
}
const SOURCE: &str = "resource type R[T:copyable] {n:T} drop(x) {let _=x.n}\nfn outer[T](take x:T)->T {let f:once fn()->T=fn()->T{take x};f()}\npub fn main()->u64 {let r=R(n:1u64);outer(take 7u64)}";

#[test]
fn ordinary_source_handoff_executes_without_an_engine_for_verify_off_recompute_and_absent_verifier()
{
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let cases: Vec<(Arc<dyn ResultStore>, Verify, store::VerifyStore)> = vec![
            (
                Arc::new(store::NoneStore),
                Verify::Off,
                store::VerifyStore::Off,
            ),
            (
                Arc::new(WithoutVerifier::default()),
                Verify::On,
                store::VerifyStore::Off,
            ),
            (
                Arc::new(store::DirStore::new(directory.path().join("recompute"))),
                Verify::On,
                store::VerifyStore::Recompute,
            ),
        ];
        for (backend, verify, recompute) in cases {
            let ordinary = verify == Verify::Off || backend.verifier().is_none();
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
                .with_result_store(backend.clone())
                .with_verify_store(recompute);
            engine.verify = verify;
            let file = engine
                .sources
                .add("ordinary-source.hir", SOURCE.into())
                .unwrap();
            engine.parse_file(file, &default_context()).unwrap();
            let mut cache = NativeDiscoveryCache::new(engine.types().clone(), backend.clone());
            let declarations = NativeDeclarations::collect(&mut engine, &mut cache, &[]).unwrap();
            let budget = ProcessBudget::new(128 << 20);
            let transport: Arc<dyn ResultStore> = Arc::new(store::RequestStore::new(
                backend,
                budget.reserve(128 << 20).unwrap(),
            ));
            let inputs =
                NativeInstanceInputs::complete(&mut engine, declarations, transport.clone())
                    .unwrap();
            let main = entry_path(&default_context());
            let original = engine.lower_to_tir(&main).unwrap().value;
            let weak = Arc::downgrade(&original);
            drop(original);
            drop(engine);
            assert!(
                weak.upgrade().is_none(),
                "ordinary transport retains no original Definition"
            );
            fn send_sync<T: Send + Sync>(_: &T) {}
            send_sync(&inputs);
            let mut provider = inputs.provider();
            assert_eq!(provider.verification(), verify);
            assert!(
                !provider.query_policy().reuse,
                "ordinary or forced source production is never an authority shortcut"
            );
            let observation = provider.source_observation(&main).unwrap();
            assert_eq!(observation.2.is_none(), ordinary);
            assert_eq!(observation.3.is_some(), ordinary);
            let trace = hirsel_lir::construction::ValueTrace::default();
            let _guard = trace.enter();
            let options = native::Options::new(hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu());
            let mut graph = DemandInstances::new_from_source(
                &provider,
                transport.clone(),
                options.target.clone(),
                ContentHash::of_bytes(b"ordinary native source control"),
            );
            let mut pending = graph.roots_from(&provider).unwrap();
            assert_eq!(trace.totals(), Default::default());
            let mut seen = BTreeSet::new();
            while let Some(key) = pending.pop() {
                if !seen.insert(key.instance.encoding().to_vec()) {
                    continue;
                }
                let output = graph.instance_from(&mut provider, &key, &options).unwrap();
                pending.extend(output.dependencies.iter().cloned());
            }
            assert!(graph.imports >= 5);
            assert_eq!(graph.imports, graph.recipe_decodes);
            assert_eq!(
                graph.counts().validations,
                graph.imports,
                "actual C judges every native miss"
            );
            assert!(provider.body_demands > 0);
            if ordinary && verify == Verify::On {
                assert_eq!(
                    provider.source_verifications, provider.body_demands,
                    "ordinary transported artifacts run actual independent V"
                );
            } else {
                assert_eq!(provider.source_verifications, 0);
            }
            drop(graph);
            drop(provider);
            drop(inputs);
            drop(transport);
            assert_eq!(budget.reserved(), 0);
        }
    });
}

#[test]
fn ordinary_verify_off_outputs_do_not_supply_current_source_v_or_native_c_on_verify_on() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let backend: Arc<dyn ResultStore> =
            Arc::new(store::DirStore::new(directory.path().join("products")));
        let mut previous: BTreeMap<Vec<u8>, native::queries::ProductRef> = BTreeMap::new();
        for verify in [Verify::Off, Verify::On] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
                .with_result_store(backend.clone());
            engine.verify = verify;
            let file = engine
                .sources
                .add("ordinary-upgrade.hir", "pub fn main()->u64 {7u64}".into())
                .unwrap();
            engine.parse_file(file, &default_context()).unwrap();
            let mut cache = NativeDiscoveryCache::new(engine.types().clone(), backend.clone());
            let declarations = NativeDeclarations::collect(&mut engine, &mut cache, &[]).unwrap();
            let budget = ProcessBudget::new(32 << 20);
            let transport: Arc<dyn ResultStore> = Arc::new(store::RequestStore::new(
                backend.clone(),
                budget.reserve(32 << 20).unwrap(),
            ));
            let inputs =
                NativeInstanceInputs::complete(&mut engine, declarations, transport.clone())
                    .unwrap();
            if verify == Verify::On {
                assert!(engine.body_validation_runs(&entry_path(&default_context())) > 0);
            }
            drop(engine);
            let mut provider = inputs.provider();
            let observation = provider
                .source_observation(&entry_path(&default_context()))
                .unwrap();
            assert_eq!(observation.2.is_some(), verify == Verify::On);
            assert_eq!(observation.3.is_some(), verify == Verify::Off);
            let options = native::Options::new(hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu());
            let mut graph = DemandInstances::new_from_source(
                &provider,
                transport.clone(),
                options.target.clone(),
                ContentHash::of_bytes(b"ordinary VerifyOff upgrade"),
            );
            let mut pending = graph.roots_from(&provider).unwrap();
            let mut actual = BTreeMap::new();
            while let Some(key) = pending.pop() {
                if actual.contains_key(key.instance.encoding()) {
                    continue;
                }
                let output = graph.instance_from(&mut provider, &key, &options).unwrap();
                pending.extend(output.dependencies.iter().cloned());
                actual.insert(key.instance.encoding().to_vec(), output.product.product);
            }
            assert_eq!(graph.imports as usize, actual.len());
            assert_eq!(graph.counts().validations, graph.imports);
            if verify == Verify::On {
                for (key, output) in &actual {
                    assert_ne!(output.action, previous[key].action);
                }
            }
            previous = actual;
        }
    });
}
