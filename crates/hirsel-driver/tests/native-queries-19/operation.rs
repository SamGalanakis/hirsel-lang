use hirsel_driver::*;
use hirsel_engine::{store::ResultStore, *};
use hirsel_lir::validate::NativeEnvironment;
use hirsel_syntax::ContentHash;
use native::queries::{
    declarations::NativeDeclarations,
    demand::DemandInstances,
    operation::{OperationCounts, settle_operations},
    source_inputs::NativeInstanceInputs,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[test]
fn actual_operation_components_cut_off_before_values_and_propagate_recursive_local_refusal() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(store::DirStore::new(directory.path().to_path_buf()));
        let target = native::TargetSpec::x86_64_unknown_linux_gnu();
        let options = native::Options::new(target.clone());
        let policy = ContentHash::of_bytes(b"operation component control");
        let mut previous = BTreeMap::new();
        let mut previous_graph: Option<native::queries::operation::SettledOperations> = None;
        for (iteration, (literal, heap, fresh)) in [
            (7u64, false, false),
            (7, false, true),
            (8, false, false),
            (8, true, false),
            (8, true, true),
        ]
        .into_iter()
        .enumerate()
        {
            let text = fixture(literal, heap);
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
                .with_result_store(store.clone());
            let file = engine.sources.add("op-component.hir", text).unwrap();
            engine.parse_file(file, &default_context()).unwrap();
            if fresh {
                engine.adapters.lowering = None;
            }
            let mut queries = NativeDiscoveryCache::new(engine.types().clone(), store.clone());
            let declarations = NativeDeclarations::collect(&mut engine, &mut queries, &[]).unwrap();
            let source =
                NativeInstanceInputs::complete(&mut engine, declarations, store.clone()).unwrap();
            drop(engine);
            let trace = hirsel_lir::construction::ValueTrace::default();
            let _guard = trace.enter();
            let mut provider = source.provider();
            let mut instances =
                DemandInstances::new_from_source(&provider, store.clone(), target.clone(), policy);
            let mut pending = instances.roots_from(&provider).unwrap();
            let mut seen = BTreeSet::new();
            let mut completed = BTreeMap::new();
            while let Some(owner) = pending.pop() {
                if !seen.insert(owner.instance.encoding().to_vec()) {
                    continue;
                }
                let output = instances
                    .instance_from(&mut provider, &owner, &options)
                    .unwrap();
                pending.extend(output.dependencies.iter().cloned());
                completed.insert(
                    owner.instance.definition().0.name.to_string(),
                    (owner, output),
                );
            }
            assert_eq!(
                instances.imports,
                if fresh {
                    0
                } else if iteration == 0 {
                    completed.len() as u64
                } else {
                    1
                }
            );
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
            let before = trace.totals();
            let mut counts = OperationCounts::default();
            let settled =
                settle_operations(store.as_ref(), &instances, &environment, &mut counts).unwrap();
            assert_eq!(
                trace.totals(),
                before,
                "operation settlement never opens a Value"
            );
            assert!(settled.matches_inputs(&instances, &environment).unwrap());
            if let Some(old) = &previous_graph {
                assert_eq!(
                    old.matches_inputs(&instances, &environment).unwrap(),
                    fresh,
                    "retained graph must match every actual admitted base input, even when facet outputs are equal"
                );
            }
            for name in ["first", "second", "main"] {
                let mut wrong = completed[name].0.clone();
                wrong.policy = ContentHash::of_bytes(b"different operation policy");
                assert_eq!(settled.get(&wrong), None);
                wrong = completed[name].0.clone();
                wrong.target_abi = ContentHash::of_bytes(b"different operation target");
                assert_eq!(settled.get(&wrong), None);
            }

            for name in ["first", "second", "main"] {
                assert_eq!(
                    settled.get(&completed[name].0),
                    Some(!heap),
                    "{name} local/recursive propagation"
                );
                let component = settled
                    .components
                    .iter()
                    .find(|c| c.members.contains(&completed[name].0))
                    .unwrap();
                if let Some(old) = previous.insert(name, component.product) {
                    if fresh || !heap {
                        assert_eq!(
                            component.product, old,
                            "equal equations cut off body-only edit"
                        );
                    } else {
                        assert_ne!(component.product.record, old.record);
                    }
                }
            }
            let recursive = settled
                .components
                .iter()
                .find(|component| component.members.contains(&completed["first"].0))
                .unwrap();
            assert_eq!(recursive.members.len(), 2);
            assert!(recursive.members.contains(&completed["second"].0));
            let main = settled
                .components
                .iter()
                .find(|component| component.members.contains(&completed["main"].0))
                .unwrap();
            assert_eq!(
                main.reads,
                vec![recursive.product],
                "caller reads actual settled recursive result"
            );
            if iteration == 0 {
                assert_eq!(counts.publications, settled.components.len() as u64);
            } else if !fresh && heap {
                assert_eq!(counts.publications, 2);
            } else {
                assert_eq!(counts.publications, 0);
            }
            assert_eq!(counts.evaluations, settled.components.len() as u64);
            if fresh {
                assert_eq!(trace.totals(), Default::default());
                assert_eq!(provider.body_demands, 0);
            }
            // Ordinary associations cannot manufacture a true summary: even an
            // action-preserving forged output is independently equation-checked.
            let forged = store
                .put_content(main.product.record.domain, b"[]")
                .unwrap();
            store.associate_product(main.product.address, main.product.action, forged);
            let mut repair = OperationCounts::default();
            let repaired =
                settle_operations(store.as_ref(), &instances, &environment, &mut repair).unwrap();
            assert_eq!(repair.publications, 1);
            assert_eq!(
                repaired
                    .components
                    .iter()
                    .find(|c| c.members.contains(&completed["main"].0))
                    .unwrap()
                    .product,
                main.product
            );
            assert_eq!(trace.totals(), before);
            previous_graph = Some(settled);
        }
    });
}

pub(super) fn fixture(literal: u64, heap: bool) -> String {
    let base = if heap {
        format!("let b=box Item(n:{literal}u64);b.n")
    } else {
        format!("{literal}u64")
    };
    format!(
        "resource type Item {{n:u64}} drop(x) {{let _=x.n}}\nfn first(n:u64)->u64 uses {{}} {{if n==0u64 {{{base}}} else {{second(n-1u64)}}}}\nfn second(n:u64)->u64 uses {{}} {{first(n)}}\npub fn main()->u64 {{second(0u64)}}"
    )
}
