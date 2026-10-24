use hirsel_driver::*;
use hirsel_engine::{store::ResultStore, *};
use hirsel_lir::{
    ir::{Callee, Symbol, SymbolKind, UnitOrigin},
    products::{Binding, LayoutClosure, UnitBody},
    stage::Value,
    validate::{Inputs, NativeEnvironment},
};
use hirsel_rc::closure_calls::{CallableEntry, rewrite_instance};
use hirsel_syntax::ContentHash;
use native::queries::{
    declarations::NativeDeclarations, demand::DemandInstances, instance::InstanceQueries,
    source_inputs::NativeInstanceInputs,
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
};

#[test]
fn local_callable_rewrite_requests_templates_before_opening_their_values_and_keeps_refusal_exact() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(store::DirStore::new(directory.path().to_path_buf()));
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        let file = engine.sources.add("callable-local.hir", "fn plus(n:u64)->u64 uses {} {n+1u64}\nfn invoke(n:u64,f:fn(u64)->u64 uses {})->u64 uses {f} {f(n)}\npub fn main()->u64 {invoke(1u64,plus)}".into()).unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let mut source_queries = NativeDiscoveryCache::new(engine.types().clone(), store.clone());
        let declarations =
            NativeDeclarations::collect(&mut engine, &mut source_queries, &[]).unwrap();
        let source =
            NativeInstanceInputs::complete(&mut engine, declarations, store.clone()).unwrap();
        drop(engine);
        let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
        let options = native::Options::new(target.clone());
        let mut provider = source.provider();
        let mut graph = DemandInstances::new_from_source(
            &provider,
            store.clone(),
            target.clone(),
            ContentHash::of_bytes(b"local callable rewrite"),
        );
        let mut pending = graph.roots_from(&provider).unwrap();
        let mut seen = BTreeSet::new();
        let mut completed = BTreeMap::new();
        while let Some(key) = pending.pop() {
            if !seen.insert(key.instance.encoding().to_vec()) {
                continue;
            }
            let output = graph.instance_from(&mut provider, &key, &options).unwrap();
            pending.extend(output.dependencies.iter().cloned());
            completed.insert(key.instance.definition().0.name.to_string(), (key, output));
        }
        let operations = source.declarations.program.operation_registry();
        let manifest = hirsel_core_contract::Manifest::skeleton(
            source.declarations.program.operations.values().cloned(),
        );
        let environment = NativeEnvironment {
            target: &target,
            manifest: &manifest,
            operations: &operations,
            discharge: Some(&options.discharge),
        };
        let mut queries = InstanceQueries::new(store.clone());
        let mut closures = Vec::new();
        let mut observed = Vec::new();
        for (key, output) in completed.values() {
            assert!(
                queries
                    .lookup(key, output.product.product.action, &environment)
                    .is_some()
            );
            let checked = queries
                .linkage(key, output.product.product.action, &environment)
                .unwrap();
            observed.push((key, output, checked.abi()));
            let bytes = store.get_content(output.product.layout_closure).unwrap();
            closures.push(
                hirsel_codec::from_text::<LayoutClosure>(std::str::from_utf8(&bytes).unwrap())
                    .unwrap(),
            );
        }
        let binding = Binding::new(&target, &closures.iter().collect::<Vec<_>>()).unwrap();
        let entries = observed
            .into_iter()
            .map(|(key, output, abi)| {
                (
                    Symbol {
                        kind: SymbolKind::Function,
                        definition: key.instance.definition().clone(),
                        instance: Some(key.instance.digest()),
                    },
                    CallableEntry {
                        key: key.instance.clone(),
                        abi: binding.abi(abi).unwrap(),
                        origin: if output.templates.library {
                            UnitOrigin::Library
                        } else {
                            UnitOrigin::Source
                        },
                        op_free: false,
                    },
                )
            })
            .collect::<HashMap<_, _>>();
        let inputs = Inputs {
            memo: Default::default(),
            target: &target,
            layouts: &binding.table,
            operations: &operations,
            manifest: &manifest,
            discharge: Some(&options.discharge),
        };
        let mut value_reads = 0;
        let mut read = |name: &str| {
            value_reads += 1;
            let bytes = store.get_content(completed[name].1.product.value).unwrap();
            let body: UnitBody =
                hirsel_codec::from_text(std::str::from_utf8(&bytes).unwrap()).unwrap();
            binding.materialize::<Value>(&body).unwrap()
        };
        let original = read("main");
        let mut refused = original.clone();
        let mut requests = Vec::new();
        rewrite_instance(
            &mut refused,
            &binding.table,
            &entries,
            &[],
            0,
            |entry, request| {
                assert_eq!(entry.key.definition().0.name, "invoke");
                requests.push(request.clone());
                false
            },
        );
        assert_eq!(requests.len(), 1);
        assert_eq!(
            hirsel_codec::to_text(&refused),
            hirsel_codec::to_text(&original)
        );
        let mut accepted = original;
        let mut admitted = Vec::new();
        rewrite_instance(
            &mut accepted,
            &binding.table,
            &entries,
            &[],
            0,
            |_, request| {
                admitted.push(request.clone());
                true
            },
        );
        assert_eq!(admitted.len(), 1);
        let request = admitted.pop().unwrap();
        assert_eq!(request.key, requests[0].key);
        assert!(
            request
                .key
                .is_same_or_derived_from(&completed["invoke"].0.instance)
        );
        assert_eq!(request.seeds.len(), 1);
        assert_eq!(request.seeds[0].1.symbol.definition.0.name, "plus");
        assert!(!request.seeds[0].1.environment);
        assert!(accepted.tables.calls.iter().any(|call| match call.callee {
            Callee::Direct(symbol) =>
                accepted.tables.symbols[symbol.index()].instance == Some(request.key.digest()),
            _ => false,
        }));
        hirsel_lir::validate::validate(accepted, &inputs).unwrap();
        // Only after the request is known does the consumer open its selected
        // original template. All other Values remain unopened throughout.
        let mut template = read("invoke");
        assert!(
            template
                .tables
                .calls
                .iter()
                .any(|call| matches!(call.callee, Callee::Indirect(_)))
        );
        template.header.instance = request.key.digest();
        let inline = rewrite_instance(
            &mut template,
            &binding.table,
            &entries,
            &request.seeds,
            request.spawn_flags,
            |_, _| panic!("scalar callback introduces no further specialization"),
        );
        assert!(inline.is_empty());
        assert!(
            template
                .tables
                .calls
                .iter()
                .all(|call| !matches!(call.callee, Callee::Indirect(_)))
        );
        assert!(template.tables.calls.iter().any(|call| match call.callee {
            Callee::Direct(symbol) =>
                template.tables.symbols[symbol.index()].definition.0.name == "plus",
            _ => false,
        }));
        hirsel_lir::validate::validate(template, &inputs).unwrap();
        assert_eq!(value_reads, 2);
    });
}
