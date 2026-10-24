use hirsel_driver::*;
use hirsel_engine::{store::ResultStore, *};
use hirsel_lir::{
    products::Binding,
    stage::Value,
    validate::{Inputs, NativeEnvironment},
};
use hirsel_syntax::ContentHash;
use native::queries::{
    INSTANCE_KIND,
    declarations::NativeDeclarations,
    demand::DemandInstances,
    instance::{CALLABLE_VALUE_KIND, InstanceQueries},
    source_inputs::NativeInstanceInputs,
};
use std::sync::Arc;

#[test]
fn imported_and_callable_value_queries_keep_separate_associations_and_actual_c_authority() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(store::DirStore::new(directory.path().to_path_buf()));
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        let file = engine
            .sources
            .add("value-stages.hir", "pub fn main()->u64 {7u64}".into())
            .unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let mut discovery = NativeDiscoveryCache::new(engine.types().clone(), store.clone());
        let declarations = NativeDeclarations::collect(&mut engine, &mut discovery, &[]).unwrap();
        let source =
            NativeInstanceInputs::complete(&mut engine, declarations, store.clone()).unwrap();
        drop(engine);
        let target = native::TargetSpec::x86_64_unknown_linux_gnu();
        let options = native::Options::new(target.clone());
        let mut provider = source.provider();
        let mut graph = DemandInstances::new_from_source(
            &provider,
            store.clone(),
            target.clone(),
            ContentHash::of_bytes(b"Value phase authority control"),
        );
        let owner = graph
            .roots_from(&provider)
            .unwrap()
            .into_iter()
            .find(|owner| owner.instance.definition().0.name == "main")
            .unwrap();
        let imported = graph
            .instance_from(&mut provider, &owner, &options)
            .unwrap();
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
        let mut base = InstanceQueries::new(store.clone());
        let mut callable = InstanceQueries::new_callable(store.clone());
        assert_eq!(base.address(&owner), owner.address(INSTANCE_KIND));
        assert_eq!(callable.address(&owner), owner.address(CALLABLE_VALUE_KIND));
        assert_ne!(base.address(&owner), callable.address(&owner));
        let action = imported.product.product.action;
        assert!(base.lookup(&owner, action, &environment).is_some());
        assert!(callable.lookup(&owner, action, &environment).is_none());
        // Copying ordinary associations and identical body references cannot
        // copy the other stage's C-owned acceptance, even with the same action.
        store.associate_product(
            callable.address(&owner),
            action,
            imported.product.product.record,
        );
        assert!(callable.candidate(&owner).is_some());
        assert!(callable.lookup(&owner, action, &environment).is_none());
        assert!(
            callable
                .read(&owner, &imported.product, &environment)
                .is_err()
        );
        let (closure, body) = base.read(&owner, &imported.product, &environment).unwrap();
        let binding = Binding::new(&target, &[&closure]).unwrap();
        let unit = binding.materialize::<Value>(&body).unwrap();
        let inputs = Inputs {
            memo: Default::default(),
            target: &target,
            layouts: &binding.table,
            operations: &operations,
            manifest: &manifest,
            discharge: Some(&options.discharge),
        };
        let (_, checked) = callable
            .publish(
                &owner,
                action,
                unit,
                &inputs,
                &[],
                imported.product.discovery,
                imported.product.statics_finalizers,
            )
            .unwrap();
        assert_eq!(callable.counts.validations, 1);
        assert_eq!(checked.product.address, callable.address(&owner));
        assert!(base.lookup(&owner, action, &environment).is_some());
        assert_eq!(
            store
                .lookup_product(base.address(&owner))
                .unwrap()
                .reference,
            imported.product.product.record
        );
        assert_eq!(
            callable
                .lookup(&owner, action, &environment)
                .unwrap()
                .product,
            checked.product
        );
        let trace = hirsel_lir::construction::ValueTrace::default();
        let _guard = trace.enter();
        let mut fresh = InstanceQueries::new_callable(store.clone());
        assert_eq!(
            fresh.lookup(&owner, action, &environment).unwrap().product,
            checked.product
        );
        assert_eq!(fresh.counts.validations, 0);
        assert_eq!(
            trace.totals(),
            Default::default(),
            "stage hit does not decode or construct a Value"
        );
        // Passing a current C envelope from the other stage to the selected
        // body's miss reader is still a descriptor mismatch.
        assert!(base.read(&owner, &checked, &environment).is_err());
    });
}
