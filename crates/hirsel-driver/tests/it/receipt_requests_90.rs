use hirsel_driver::native::queries::{
    declarations::NativeDeclarations,
    demand::DemandInstances,
    instance::InstanceQueries,
    recipes::SourceRecipe,
    source_inputs::{NativeInstanceInputs, NativeSourceProvider},
};
use hirsel_driver::{
    compiler_adapters,
    native::receipt_source::{
        factory::{NamedInput, named_graph},
        provider::{Front, Provider},
    },
};
use hirsel_engine::store::ResultStore;
use hirsel_engine::*;
use hirsel_layout::layout::LayoutBuilder;
use hirsel_lir::validate::{Inputs, NativeEnvironment};
use hirsel_rc::instance::Actuals;
use hirsel_rc::instance::Instances;
use hirsel_syntax::ContentHash;
use std::sync::Arc;

#[test]
fn aware_source_request_restores_a_borrowed_heap_parameter_and_replays_current_c() {
    borrowed_request(false, false);
}

#[test]
fn aware_source_request_forwards_paired_heap_companions_through_rc_and_reuse() {
    borrowed_request(true, false);
}

#[test]
fn aware_source_request_transfers_taken_heap_companions_through_rc_and_reuse() {
    borrowed_request(true, true);
}

fn borrowed_request(forward: bool, take: bool) {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let store: Arc<dyn ResultStore> =
            Arc::new(store::DirStore::new(directory.path().join("products")));
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        let file = engine
            .sources
            .add(
                "custody.hir",
                custody_source(
                    r#"
fn read(root: List[u64]) -> u64 {
  let mut r=arena.with(fn(a:Arena)->Region[List[u64]] {
    let independent:List[u64] in heap=copy_to(heap,root)
    arena.seal(take a,take independent)
  })
  region.with(mut r,fn(v:List[u64])->u64 {list.len(v)})
}
fn forward(root: List[u64]) -> u64 { read(root) }
fn forward_again(root: List[u64]) -> u64 { forward(root) }
pub fn inspect(root: List[u64]) -> u64 { forward(root)+forward_again(root) }

"#,
                    take,
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
        let front_context = Arc::new(engine.native_original_context(&paths).unwrap());
        let records = NativeRecords::new(engine.types().clone(), store.clone());
        let mut products = std::collections::BTreeMap::new();
        for path in &paths {
            let front = Front::new(&reader);
            let original = front.original(path).unwrap();
            let bytes = records.encode(&engine, original.definition()).unwrap();
            products.insert(
                path.clone(),
                NativeOriginalProduct {
                    definition: path.clone(),
                    implementation_revision: original.implementation_revision(),
                    signature: original.definition().signature_hash,
                    body: store.put_content(NATIVE_ORIGINAL_BODY, &bytes).unwrap(),
                    verified: original.output().cloned(),
                },
            );
        }
        let front = Front::new(&reader);
        let mut inspect = entry_path(&default_context());
        inspect.0.name = "inspect".into();
        let original = front.original(&inspect).unwrap();
        let roots = vec![NamedInput {
            base: hirsel_rc::instance::key_of(
                front.types(),
                &inspect,
                original.definition().entry,
                &Actuals::none(),
                None,
                None,
            )
            .unwrap(),
            actuals: Actuals::none(),
        }];
        let mut read = inspect.clone();
        read.0.name = if forward { "forward_again" } else { "forward" }.into();
        let original = declarations.body(&mut engine, &read).unwrap();
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
        let declaration = &declarations.context.sources[&read];
        let (id, _) = instances
            .register_function(
                &declarations.program,
                declaration.declaration.clone(),
                declaration.entry,
                Actuals::none(),
                &mut layouts,
            )
            .unwrap();
        let (mut instances, _) = hirsel_rc::instance::discover_instance(
            &declarations.program,
            &declarations.context,
            &mut layouts,
            instances,
            id,
            original,
        )
        .unwrap();
        let graph =
            named_graph(&front, roots.clone(), &mut |_, _, _| panic!("data ports")).unwrap();
        let selected = graph
            .requests
            .iter()
            .find(|r| r.ordinary_key().definition() == &read)
            .unwrap();
        let lexical = instances.view(id).key.clone();
        instances
            .bind_receipt_request(id, selected, &mut layouts)
            .unwrap();
        assert_ne!(instances.view(id).key, lexical);
        assert_eq!(instances.lexical_key(id), &lexical);
        assert_eq!(instances.receipt_source(id).unwrap().profiles.len(), 2);
        drop(layouts);
        drop(owners);
        assert_eq!(
            SourceRecipe::capture(&instances, id)
                .unwrap()
                .key()
                .unwrap(),
            lexical
        );
        drop(front);
        let inputs = NativeInstanceInputs::from_products(declarations, reader).unwrap();
        if forward {
            use hirsel_driver::native::queries::instance_tasks::{
                INSTANCE_INPUT_KIND, InstanceTaskFactory, InstanceTasks,
            };
            use hirsel_driver::native::queries::source_inputs::NativeFrontLower;
            use runtime::{
                MemoryWeight, ProcessBudget, ProductRef, QueryFactory, QueryId, QueryPoll,
            };
            let bindings = products
                .iter()
                .map(|(path, product)| {
                    (
                        path.clone(),
                        QueryId {
                            kind: 0x7c90,
                            address: store::Address(product.body.output.0),
                        },
                    )
                })
                .collect::<std::collections::BTreeMap<_, _>>();
            let rows = bindings
                .iter()
                .map(|(path, query)| {
                    (
                        *query,
                        ProductRef {
                            address: query.address,
                            action: store::Action(query.address.0),
                            record: products[path].body,
                        },
                    )
                })
                .collect();
            let mut delayed = read.clone();
            delayed.0.name = "read".into();
            let mut cx = LowerReads {
                rows,
                waiting: Some(bindings[&delayed]),
            };
            let budget = ProcessBudget::new(64 << 20);
            let tasks = InstanceTasks::new(
                inputs.clone(),
                store.clone(),
                hirsel_driver::native::Options::new(target.clone()),
                ContentHash::of_bytes(b"receipt worker source dependencies"),
                MemoryWeight {
                    scratch: 16 << 20,
                    retained: 1 << 20,
                    stack: 0,
                },
                budget.reserve(64 << 20).unwrap(),
            )
            .unwrap()
            .with_front_lower(NativeFrontLower {
                context: front_context,
                bindings,
                products: Arc::new(LowerRows(products)),
            })
            .unwrap();
            let owner = tasks
                .roots()
                .iter()
                .find(|key| key.instance.definition() == &read)
                .unwrap();
            let factory = InstanceTaskFactory::new(tasks.clone());
            let mut task = factory
                .create(QueryId {
                    kind: INSTANCE_INPUT_KIND,
                    address: owner.address(INSTANCE_INPUT_KIND),
                })
                .unwrap();
            assert!(
                matches!(task.poll(&mut cx), QueryPoll::Waiting(ref waits) if waits == &[cx.waiting.unwrap()])
            );
            cx.waiting = None;
            assert!(matches!(task.poll(&mut cx), QueryPoll::Complete(_)));
            assert_eq!(tasks.imports.load(std::sync::atomic::Ordering::Relaxed), 0);
        }
        let mut source = inputs.provider();
        let mut options = hirsel_driver::native::Options::new(target.clone());
        options.validation = hirsel_driver::native::Validation::Ready;
        let mut demand = DemandInstances::new_from_source(
            &source,
            store.clone(),
            target.clone(),
            ContentHash::of_bytes(b"custody request"),
        );
        let addresses = demand.roots_from(&source).unwrap();
        let address = addresses
            .into_iter()
            .find(|a| a.instance.definition() == &read)
            .unwrap();
        assert_ne!(address.instance, lexical);
        let output = demand
            .instance_from(&mut source, &address, &options)
            .unwrap();
        assert_eq!(demand.imports, 1);
        let mut current = source.declarations().program.operation_registry();
        let selected = hirsel_lir::ir::Symbol {
            kind: hirsel_lir::ir::SymbolKind::Function,
            definition: read.clone(),
            instance: Some(address.instance.digest()),
        };
        let (evidence, _) = source
            .source_invocations(
                &read,
                &selected,
                hirsel_tir::BodyId(address.instance.body()),
            )
            .unwrap();
        current
            .invocations
            .insert(evidence.body.definition.clone(), evidence.invocations);
        current.invocation_bodies.push(evidence.body);
        let operations = demand.operation_declarations(Arc::new(current));
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
        let entry = unit.tables.receipt_entry.as_ref().unwrap();
        assert_eq!(entry.parameters.len(), 1);
        assert_eq!(
            unit.values[entry.parameters[0].version.index()].holding,
            hirsel_lir::ir::Holding::Plain
        );
        if forward {
            assert!(unit.tables.calls.iter().any(|call| {
                call.receipts
                    .as_ref()
                    .is_some_and(|receipts| receipts.arguments.len() == 1)
            }));
        }
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
            hirsel_rc::placement::place(unit, &binding.table, &Default::default()).unwrap();
        let rc = hirsel_rc::insert::insert(placed, &binding.table, &Default::default()).unwrap();
        let rc = hirsel_lir::validate::validate_rc(rc, &check)
            .unwrap()
            .into_raw();
        let reuse = hirsel_rc::reuse::reuse(
            rc,
            &binding.table,
            &hirsel_rc::reuse::Options { reuse: true },
        )
        .unwrap();
        hirsel_lir::validate::validate_reuse(reuse, &check).unwrap();
        drop(check);
        drop(operations);
        let imports = demand.imports;
        let trace = hirsel_lir::construction::ValueTrace::default();
        let observe = trace.enter();
        let replay = demand
            .instance_from(&mut source, &address, &options)
            .unwrap();
        assert_eq!(replay.product.product, output.product.product);
        assert_eq!(demand.imports, imports);
        assert_eq!(
            trace.totals(),
            hirsel_lir::construction::ValueCounts::default()
        );
        drop(observe);
        let mut changed = QueryEngine::new(default_environment(), compiler_adapters())
            .with_types(engine.types().clone())
            .with_result_store(store.clone());
        let file = changed
            .sources
            .add(
                "custody.hir",
                custody_source(
                    r#"
fn read(root:List[u64])->u64 {list.len(root)}
fn forward(root:List[u64])->u64 {read(root)}
fn forward_again(root:List[u64])->u64 {forward(root)}
pub fn inspect(root:List[u64])->u64 {forward(root)+forward_again(root)}
"#,
                    take,
                ),
            )
            .unwrap();
        let checked = check_progress(&mut changed, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let mut discovery = NativeDiscoveryCache::new(changed.types().clone(), store.clone());
        let declarations = NativeDeclarations::collect(&mut changed, &mut discovery, &[]).unwrap();
        let paths = declarations
            .context
            .sources
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let reader = changed
            .native_original_reader(&paths, store.clone())
            .unwrap();
        let changed = NativeInstanceInputs::from_products(declarations, reader).unwrap();
        let stale = demand
            .instance_from(&mut changed.provider(), &address, &options)
            .err()
            .unwrap();
        assert!(
            stale.contains("receipt request changed with current original source"),
            "{stale}"
        );
        assert_eq!(demand.imports, imports);
        // A fresh producer uses only persisted discovery and current original V.
        let mut fresh = DemandInstances::new_from_source(&source, store, target, address.policy);
        assert!(fresh.roots_from(&source).unwrap().contains(&address));
        let replay = fresh
            .instance_from(&mut source, &address, &options)
            .unwrap();
        assert_eq!(replay.product.product, output.product.product);
        assert_eq!(fresh.imports, 0);
    });
}

struct LowerRows(std::collections::BTreeMap<DefPath, NativeOriginalProduct>);
impl hirsel_driver::native::queries::source_inputs::NativeLowerProducts for LowerRows {
    fn lower_product(
        &self,
        path: &DefPath,
        selected: runtime::ProductRef,
    ) -> Result<NativeOriginalProduct, String> {
        self.0
            .get(path)
            .filter(|product| product.body == selected.record)
            .cloned()
            .ok_or_else(|| "unselected test Lower".into())
    }
}
struct LowerReads {
    rows: Vec<(runtime::QueryId, runtime::ProductRef)>,
    waiting: Option<runtime::QueryId>,
}
impl runtime::QueryContext for LowerReads {
    fn read(
        &mut self,
        query: runtime::QueryId,
    ) -> std::task::Poll<Result<runtime::ProductRef, store::Reference>> {
        if self.waiting == Some(query) {
            return std::task::Poll::Pending;
        }
        std::task::Poll::Ready(Ok(self.rows.iter().find(|(id, _)| *id == query).unwrap().1))
    }
}

fn custody_source(text: &str, take: bool) -> String {
    if !take {
        return text.into();
    }
    text.replace("root: List[u64]", "take root: List[u64]")
        .replace("root:List[u64]", "take root:List[u64]")
        .replace(
            "forward(root)+forward_again(root)",
            "forward_again(take root)",
        )
        .replace("read(root)", "read(take root)")
        .replace("forward(root)", "forward(take root)")
}
