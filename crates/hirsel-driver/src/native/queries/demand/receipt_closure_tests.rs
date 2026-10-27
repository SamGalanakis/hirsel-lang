use crate::{
    compiler_adapters,
    native::{
        queries::declarations::NativeDeclarations,
        receipt_source::{
            factory::{NamedInput, named_graph},
            provider::Front,
        },
    },
};
use hirsel_engine::*;
use hirsel_layout::layout::LayoutBuilder;
use hirsel_rc::instance::{Actuals, Instances};
use std::sync::Arc;

#[test]
fn demanded_closure_rechecks_current_lexical_source_before_cache_replay() {
    bind(false);
}

fn bind(captured: bool) {
    with_compiler_stack(|| {
        let store: Arc<dyn store::ResultStore> = Arc::new(store::MemStore::default());
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        let file = engine
            .sources
            .add(
                "closure.hir",
                r#"
fn read(root: List[u64]) -> u64 {
  let mut r=arena.with(fn(a:Arena)->Region[List[u64]] {
    let independent:List[u64] in heap=copy_to(heap,root)
    arena.seal(take a,take independent)
  })
  region.with(mut r,fn(v:List[u64])->u64 {list.len(v)})
}
pub fn inspect(take root: List[u64]) -> u64 {
  let f:fn(List[u64])->u64=fn(x:List[u64])->u64 {read(x)}
  f(root)
}
"#
                .replace(
                    "  let f:",
                    if captured {
                        "  let captured=copy root; let f:"
                    } else {
                        "  let f:"
                    },
                )
                .replace(
                    "{read(x)}",
                    if captured {
                        "{let _=list.len(captured);read(x)}"
                    } else {
                        "{read(x)}"
                    },
                ),
            )
            .unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let mut discovery = NativeDiscoveryCache::new(engine.types().clone(), store.clone());
        let mut declarations =
            NativeDeclarations::collect(&mut engine, &mut discovery, &[]).unwrap();
        let paths = declarations
            .context
            .sources
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let reader = engine
            .native_original_reader(&paths, store.clone())
            .unwrap();
        let front = Front::new(&reader);
        let mut inspect = entry_path(&default_context());
        inspect.0.name = "inspect".into();
        let original = declarations.body(&mut engine, &inspect).unwrap();
        let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
        let owners = hirsel_rc::library::owners(
            &declarations.program.types,
            &declarations.program.signatures,
        )
        .over(&declarations.program.owners);
        let mut layouts = LayoutBuilder::new(&target, &owners);
        layouts.prepare_native_controls();
        let mut instances = Instances::new(
            declarations.program.types.clone(),
            declarations.program.memo.clone(),
        );
        let declaration = &declarations.context.sources[&inspect];
        let (parent, _) = instances
            .register_function(
                &declarations.program,
                declaration.declaration.clone(),
                declaration.entry,
                Actuals::none(),
                &mut layouts,
            )
            .unwrap();
        let base = instances.view(parent).key.clone();
        let (mut instances, _) = hirsel_rc::instance::discover_instance(
            &declarations.program,
            &declarations.context,
            &mut layouts,
            instances,
            parent,
            original.clone(),
        )
        .unwrap();
        let graph = named_graph(
            &front,
            [NamedInput {
                base,
                actuals: Actuals::none(),
            }],
            &mut |_, _, _| panic!("data ports"),
        )
        .unwrap();
        let selected = graph
            .requests
            .iter()
            .find(|request| {
                request.ordinary_key().definition() == &inspect
                    && request.ordinary_key().body() != 0
                    && request.aware_key().is_some()
            })
            .unwrap();
        let id = instances
            .ids()
            .find(|&id| {
                instances.view(id).key.definition() == &inspect
                    && instances.view(id).body.0 == selected.ordinary_key().body()
            })
            .unwrap();
        let lexical = instances.view(id).key.clone();
        assert_ne!(&lexical, selected.ordinary_key());
        assert!(
            instances
                .receipt_lexical_matches(id, selected, &mut layouts)
                .unwrap()
        );
        let environment = instances.environments[id.index()];
        let scalar = declarations
            .program
            .types
            .intern(hirsel_types::Type::with_kind(
                hirsel_types::TypeKind::Primitive(hirsel_types::Primitive::U64),
            ));
        instances.environments[id.index()] =
            Some(layouts.layout(&declarations.program.types, scalar).unwrap());
        assert!(
            !instances
                .receipt_lexical_matches(id, selected, &mut layouts)
                .unwrap()
        );
        assert!(
            instances
                .bind_receipt_request(id, selected, &mut layouts)
                .is_err()
        );
        assert_eq!(instances.view(id).key, lexical);
        instances.environments[id.index()] = environment;
        instances
            .bind_receipt_request(id, selected, &mut layouts)
            .unwrap();
        assert_eq!(instances.lexical_key(id), &lexical);
        assert_eq!(instances.view(id).key, *selected.aware_key().unwrap());
        let policy = hirsel_syntax::ContentHash::of_bytes(b"closure lexical replay");
        let table = layouts.finish();
        drop(owners);
        drop(front);
        let inputs =
            super::super::source_inputs::NativeInstanceInputs::from_products(declarations, reader)
                .unwrap();
        let mut source = inputs.provider();
        let mut demand =
            super::DemandInstances::new_from_source(&source, store.clone(), target.clone(), policy);
        let roots = demand.roots_from(&source).unwrap();
        let root = roots
            .iter()
            .find(|a| a.instance.definition() == &inspect)
            .unwrap();
        let receipt_roots = demand
            .requests
            .get(root.instance.encoding())
            .unwrap()
            .receipt_roots;
        // Admit the independently bound closure directly. Its parent retains a
        // separate held-code inventory obligation owned by source expansion.
        let request = demand
            .request(&source, &instances, id, &table, receipt_roots)
            .unwrap();
        let child = demand.admit(request).unwrap();
        let mut options = crate::native::Options::new(target);
        options.validation = crate::native::Validation::Ready;
        if captured {
            assert!(
                demand
                    .instance_from(&mut source, &child, &options)
                    .err()
                    .expect("captured metadata remains closed")
                    .contains("capture")
            );
        } else {
            let output = demand.instance_from(&mut source, &child, &options).unwrap();
            let imports = demand.imports;
            let trace = hirsel_lir::construction::ValueTrace::default();
            let observe = trace.enter();
            let replay = demand.instance_from(&mut source, &child, &options).unwrap();
            assert_eq!(replay.product.product, output.product.product);
            assert_eq!(demand.imports, imports);
            assert_eq!(
                trace.totals(),
                hirsel_lir::construction::ValueCounts::default()
            );
            drop(observe);
        }
    });
}
