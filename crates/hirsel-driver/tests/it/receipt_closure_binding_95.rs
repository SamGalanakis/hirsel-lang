use hirsel_driver::{
    compiler_adapters,
    native::{
        queries::{declarations::NativeDeclarations, recipes::SourceRecipe},
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
fn original_closure_request_binds_exact_native_convention_and_environment() {
    for captured in [false, true] {
        bind(captured);
    }
}

fn bind(captured: bool) {
    with_compiler_stack(|| {
        let budget = runtime::ProcessBudget::new(64 << 20);
        let store: Arc<dyn store::ResultStore> = Arc::new(store::RequestStore::new(
            Arc::new(store::MemStore::default()),
            budget.reserve(64 << 20).unwrap(),
        ));
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
        let reader = engine.native_original_reader(&paths).unwrap();
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
        let recipe = SourceRecipe::capture(&instances, id).unwrap();
        assert_eq!(recipe.key().unwrap(), lexical);
        let (restored, id) = recipe
            .restore_selected(
                &declarations.program,
                &declarations.context,
                original,
                Some(selected),
                &mut layouts,
            )
            .unwrap();
        assert_eq!(restored.lexical_key(id), &lexical);
        assert_eq!(restored.view(id).key, *selected.aware_key().unwrap());
    });
}
