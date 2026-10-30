//! Production demand import and C replay of original constructor obligations.
use hirsel_driver::{
    compiler_adapters,
    native::queries::{
        declarations::NativeDeclarations,
        demand::DemandInstances,
        instance::InstanceQueries,
        source_inputs::{NativeInstanceInputs, NativeSourceProvider},
    },
};
use hirsel_engine::*;
use hirsel_lir::validate::{Inputs, NativeEnvironment};
use hirsel_syntax::ContentHash;
use std::sync::Arc;

#[test]
fn aware_inventory_import_preserves_dead_original_payload_and_rechecks_c_on_replay() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let budget = runtime::ProcessBudget::new(64 << 20);
        let store: Arc<dyn store::ResultStore> = Arc::new(store::RequestStore::new(
            Arc::new(store::DirStore::new(directory.path().join("products"))),
            budget.reserve(64 << 20).unwrap(),
        ));
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        let file = engine
            .sources
            .add(
                "inventory-request.hir",
                r#"
fn read(root:List[u64])->u64 {
  let mut r=arena.with(fn(a:Arena)->Region[List[u64]] {
    let independent:List[u64] in heap=copy_to(heap,root)
    arena.seal(take a,take independent)
  })
  region.with(mut r,fn(v:List[u64])->u64 {list.len(v)})
}
pub fn forward(root:List[u64])->u64 {
  let dead:List[u64] in heap=[11u64,12u64]
  read(root)
}
"#
                .into(),
            )
            .unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let mut discovery = NativeDiscoveryCache::new(engine.types().clone(), store.clone());
        let declarations = NativeDeclarations::collect(&mut engine, &mut discovery, &[]).unwrap();
        let paths = declarations
            .context
            .sources
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let reader = engine
            .native_original_reader(&paths, store.clone())
            .unwrap();
        let inputs = NativeInstanceInputs::from_products(declarations, reader).unwrap();
        let mut source = inputs.provider();
        let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
        let mut options = hirsel_driver::native::Options::new(target.clone());
        options.validation = hirsel_driver::native::Validation::Ready;
        let mut demand = DemandInstances::new_from_source(
            &source,
            store.clone(),
            target.clone(),
            ContentHash::of_bytes(b"inventory request 97"),
        );
        let address = demand
            .roots_from(&source)
            .unwrap()
            .into_iter()
            .find(|address| address.instance.definition().0.name.as_str() == "forward")
            .unwrap();
        let output = demand
            .instance_from(&mut source, &address, &options)
            .unwrap();
        let mut ordinary = source.declarations().program.operation_registry();
        let symbol = hirsel_lir::ir::Symbol {
            kind: hirsel_lir::ir::SymbolKind::Function,
            definition: address.instance.definition().clone(),
            instance: Some(address.instance.digest()),
        };
        let (evidence, _) = source
            .source_invocations(
                address.instance.definition(),
                &symbol,
                hirsel_tir::BodyId(address.instance.body()),
            )
            .unwrap();
        ordinary
            .invocations
            .insert(evidence.body.definition.clone(), evidence.invocations);
        ordinary.invocation_bodies.push(evidence.body);
        let operations = demand.operation_declarations(Arc::new(ordinary));
        let manifest = source.declarations().manifest().unwrap();
        let environment = NativeEnvironment {
            target: &target,
            operations: operations.as_ref(),
            manifest: &manifest,
            discharge: Some(&options.discharge),
        };
        let (closure, body) = InstanceQueries::new(store.clone())
            .read(&address, &output.product, &environment)
            .unwrap();
        let binding = hirsel_lir::products::Binding::new(&target, &[&closure]).unwrap();
        let unit = binding
            .materialize::<hirsel_lir::stage::Value>(&body)
            .unwrap();
        assert_eq!(
            unit.tables
                .seal_payloads
                .iter()
                .map(|table| table.bindings.len())
                .sum::<usize>(),
            1
        );
        let expected = operations
            .seal_payloads(address.instance.definition(), address.instance.digest())
            .unwrap();
        assert_eq!(expected.len(), 1);
        let check = Inputs {
            memo: Default::default(),
            target: &target,
            layouts: &binding.table,
            operations: operations.as_ref(),
            manifest: &manifest,
            discharge: Some(&options.discharge),
        };
        hirsel_lir::validate::validate_value(unit.clone(), &check).unwrap();
        let placed =
            hirsel_rc::placement::place(unit.clone(), &binding.table, &Default::default()).unwrap();
        let rc = hirsel_rc::insert::insert(placed, &binding.table, &Default::default()).unwrap();
        hirsel_lir::validate::validate_rc(rc, &check).unwrap();
        let mut omitted = unit;
        omitted.tables.seal_payloads.clear();
        assert!(hirsel_lir::validate::validate_value(omitted, &check).is_err());
        drop(check);
        drop(operations);
        let imports = demand.imports;
        let trace = hirsel_lir::construction::ValueTrace::default();
        let observing = trace.enter();
        let replay = demand
            .instance_from(&mut source, &address, &options)
            .unwrap();
        assert_eq!(replay.product.product, output.product.product);
        assert_eq!(demand.imports, imports);
        assert_eq!(
            trace.totals(),
            hirsel_lir::construction::ValueCounts::default()
        );
        drop(observing);
    });
}
