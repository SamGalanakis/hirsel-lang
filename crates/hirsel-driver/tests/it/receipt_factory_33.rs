//! Current-source factory assembly. These are original graph/profile controls;
//! no receipt-aware ABI, Some native provider or public seal is activated here.
use hirsel_driver::{
    compiler_adapters,
    native::receipt_source::{
        factory::{closure as factory_closure, *},
        provider::*,
    },
};
use hirsel_engine::*;
use hirsel_rc::{
    instance::{Actuals, key_of},
    sealed_source::SourceError,
};
use hirsel_syntax::{DefId, DefPath};
use hirsel_types::{GenericArgument, SignatureRef, TypeInterner};
use std::sync::Arc;

struct Simulated<'a> {
    front: &'a Front<'a>,
    unavailable: bool,
    redirect: Option<(DefId, DefId)>,
    signature_override: Option<SignatureRef>,
}
impl Provider for Simulated<'_> {
    fn types(&self) -> &TypeInterner {
        self.front.types()
    }
    fn current(&self, path: &DefPath) -> Result<Current, SourceError> {
        if self.unavailable {
            return Err(SourceError::Unsupported("simulated unknown source"));
        }
        self.front.current(path)
    }
    fn signature(&self, path: &DefPath) -> Result<SignatureRef, SourceError> {
        if let Some(signature) = &self.signature_override
            && signature.definition.path() == path
        {
            return Ok(signature.clone());
        }
        self.front.signature(path)
    }
    fn original(&self, path: &DefPath) -> Result<Arc<NativeOriginalBody>, SourceError> {
        self.front.original(path)
    }
}
impl SelectionProvider for Simulated<'_> {
    fn selected_instance(
        &self,
        declaration: DefId,
        arguments: &[GenericArgument],
    ) -> Result<Option<DefId>, SourceError> {
        if let Some((from, to)) = self.redirect
            && declaration == from
        {
            return Ok(Some(to));
        }
        self.front.selected_instance(declaration, arguments)
    }
}
fn source(text: &str) -> (NativeOriginalReader, DefPath) {
    source_at(text, "main")
}
fn source_at(text: &str, name: &str) -> (NativeOriginalReader, DefPath) {
    let store: Arc<dyn store::ResultStore> = Arc::new(store::MemStore::default());
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
        .with_result_store(store.clone());
    let file = engine
        .sources
        .add("receipt-factory.hir", text.into())
        .unwrap();
    let report = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    let mut path = entry_path(&default_context());
    path.0.name = name.into();
    let roots = engine.native_source_roots(&[path.clone()]).unwrap();
    let reader = engine
        .native_original_reader(&roots.definitions, store)
        .unwrap();
    (reader, path)
}
fn root(front: &Front<'_>, path: &DefPath) -> NamedInput {
    let original = front.original(path).unwrap();
    NamedInput {
        base: key_of(
            front.types(),
            path,
            original.definition().entry,
            &Actuals::none(),
            None,
            None,
        )
        .unwrap(),
        actuals: Actuals::none(),
    }
}

#[test]
fn source_factory_shares_original_named_contexts_and_keeps_ordinary_keys() {
    with_compiler_stack(|| {
        let (reader, path) = source(
            r#"
fn id[T](take x:T)->T { take x }
fn wrap[T](take x:T)->T { id(take x) }
pub fn main()->u64 {
  let a=wrap(7u64)
  let b=wrap(11u64)
  a+b
}
"#,
        );
        let front = Front::new(&reader);
        let graph = named_graph(
            &front,
            [root(&front, &path), root(&front, &path)],
            &mut |_, _, _| panic!("no held callable port"),
        )
        .unwrap();
        assert_eq!(graph.contexts.len(), 3);
        assert_eq!(graph.roots[0], graph.roots[1]);
        assert_eq!(graph.requests.len(), 3);
        for (context, request) in graph.contexts.iter().zip(&graph.requests) {
            assert_eq!(request.ordinary_key(), &context.input.base);
            assert!(request.aware_key().is_none());
            assert!(request.declaration().contract().is_none());
            assert_eq!(context.source.entry(), request.declaration().entry());
            assert_eq!(
                context.ports.len(),
                context.source.entry().parameters().len() + 1
            );
        }
        let mut simulated = Simulated {
            front: &front,
            unavailable: false,
            redirect: None,
            signature_override: None,
        };
        let same = named_graph(&simulated, [root(&front, &path)], &mut |_, _, _| {
            panic!("no callable profile query")
        })
        .unwrap();
        assert_eq!(
            graph
                .requests
                .iter()
                .map(|r| r.context_key())
                .collect::<Vec<_>>(),
            same.requests
                .iter()
                .map(|r| r.context_key())
                .collect::<Vec<_>>()
        );
        simulated.unavailable = true;
        assert!(matches!(
            named_graph(&simulated, [root(&front, &path)], &mut |_, _, _| Ok(vec![])),
            Err(SourceError::Unsupported("simulated unknown source"))
        ));
    });
}

#[test]
fn source_factory_resolves_a_recursive_original_graph_without_clone_limits() {
    with_compiler_stack(|| {
        let (reader, path) = source(
            r#"
fn count(n:u64)->u64 { if n==0u64 {0u64} else {count(n-1u64)} }
pub fn main()->u64 { count(3u64) }
"#,
        );
        let front = Front::new(&reader);
        let graph = named_graph(&front, [root(&front, &path)], &mut |_, _, _| {
            panic!("no held callable port")
        })
        .unwrap();
        assert_eq!(graph.contexts.len(), 2);
        assert_eq!(graph.requests.len(), 2);
        assert!(graph.requests.iter().all(|r| r.aware_key().is_none()));
        assert!(
            graph
                .contexts
                .iter()
                .any(|c| c.input.base.definition().0.name == "count")
        );
    });
}

#[test]
fn source_factory_refuses_mutated_original_callable_even_without_a_seal() {
    with_compiler_stack(|| {
        let (reader, path) = source(
            r#"
pub fn main()->u64 {
  let mut f:fn()->u64=fn()->u64 {7u64}
  let old=replace(mut f, fn()->u64 {11u64})
  f()
}
"#,
        );
        let front = Front::new(&reader);
        assert!(matches!(
            named_graph(&front, [root(&front, &path)], &mut |_, _, _| Ok(vec![])),
            Err(SourceError::Unsupported(
                "receipt indirect call needs complete original code producers"
            ))
        ));
    });
}

#[test]
fn source_factory_rechecks_actual_ct5_selections_and_complete_call_inventory() {
    with_compiler_stack(|| {
        let (reader, path) = source(
            r#"
type Point {x:u64}
fn make[T](s:TypeShape[T])->Result[Body[fn(T)->u64 uses {}],String] {
  Ok(codegen.body("fn(v: standalone@0/Point) -> u64 uses {} { v.x }"))
}
fn generated[T](v:T)->u64 uses {} = comptime make(shape(T))
fn other(v:Point)->u64 uses {} {v.x}
fn pick(flag:bool)->u64 {if flag {generated(Point(x:7u64))} else {other(Point(x:11u64))}}
pub fn main()->u64 {pick(true)}
"#,
        );
        let front = Front::new(&reader);
        let mut graph = named_graph(&front, [root(&front, &path)], &mut |_, _, _| {
            panic!("no held callable")
        })
        .unwrap();
        assert_eq!(graph.contexts.len(), 4);
        assert_eq!(graph.calls.len(), 3);
        let call = graph
            .calls
            .iter()
            .find(|call| call.selection.generated)
            .unwrap();
        assert!(call.selection.valid());
        let generated = DefId::intern(&call.selection.declaration);
        let other = graph
            .contexts
            .iter()
            .find(|context| context.input.base.definition().0.name == "other")
            .unwrap();
        let alternative = DefId::intern(other.input.base.definition());
        graph
            .recheck(&front, [root(&front, &path)], &mut |_, _, _| {
                panic!("no held callable")
            })
            .unwrap();
        let mut simulated = Simulated {
            front: &front,
            unavailable: false,
            redirect: None,
            signature_override: None,
        };
        graph
            .recheck(&simulated, [root(&front, &path)], &mut |_, _, _| {
                panic!("no held callable")
            })
            .unwrap();
        simulated.redirect = Some((generated, alternative));
        assert!(matches!(
            graph.recheck(&simulated, [root(&front, &path)], &mut |_, _, _| Ok(vec![])),
            Err(SourceError::Malformed(
                "receipt graph differs from current original observations"
            ))
        ));
        simulated.redirect = None;
        let saved = graph.calls.pop().unwrap();
        assert!(matches!(
            graph.recheck(&front, [root(&front, &path)], &mut |_, _, _| Ok(vec![])),
            Err(SourceError::Malformed(
                "receipt graph differs from current original observations"
            ))
        ));
        graph.calls.push(saved);
        graph
            .recheck(&front, [root(&front, &path)], &mut |_, _, _| Ok(vec![]))
            .unwrap();
        let saved_input = graph.contexts[1].input.clone();
        graph.contexts[1].input.base = root(&front, &path).base;
        assert!(matches!(
            graph.recheck(&front, [root(&front, &path)], &mut |_, _, _| Ok(vec![])),
            Err(SourceError::Malformed(
                "receipt graph differs from current original observations"
            ))
        ));
        graph.contexts[1].input = saved_input;
        let saved_roots = std::mem::take(&mut graph.roots);
        let saved_contexts = std::mem::take(&mut graph.contexts);
        let saved_requests = std::mem::take(&mut graph.requests);
        let saved_calls = std::mem::take(&mut graph.calls);
        assert!(matches!(
            graph.recheck(&front, [root(&front, &path)], &mut |_, _, _| Ok(vec![])),
            Err(SourceError::Malformed(
                "receipt graph differs from current original observations"
            ))
        ));
        graph.roots = saved_roots;
        graph.contexts = saved_contexts;
        graph.requests = saved_requests;
        graph.calls = saved_calls;
        simulated.unavailable = true;
        assert!(matches!(
            graph.recheck(&simulated, [root(&front, &path)], &mut |_, _, _| Ok(vec![])),
            Err(SourceError::Unsupported("simulated unknown source"))
        ));
    });
}

#[test]
fn source_factory_derives_nonempty_seal_demand_from_the_reviewed_original_operation() {
    with_compiler_stack(|| {
        let (reader, path) = source_at(
            r#"
pub fn pack(take a:Arena in a, take root:u64)->Region[u64] {
  arena.seal(take a, take root)
}
"#,
            "pack",
        );
        let front = Front::new(&reader);
        let graph = named_graph(&front, [root(&front, &path)], &mut |_, _, _| {
            panic!("scalar root has no held callable")
        })
        .unwrap();
        assert_eq!(graph.contexts.len(), 1);
        assert!(graph.calls.is_empty());
        assert_eq!(graph.operations.len(), 1);
        assert!(
            graph.operations[0]
                .operation
                .definition
                .is_core("arena.seal")
        );
        let request = &graph.requests[0];
        assert!(request.aware_key().is_some());
        let contract = request.declaration().contract().unwrap();
        assert_eq!(contract.parameters.len(), 1);
        assert_eq!(contract.parameters[0].source, 1);
        assert_eq!(
            contract.parameters[0].mode,
            hirsel_lir::sealed_payload_source::ReceiptMode::Take
        );
        assert_eq!(
            contract.inventory,
            [hirsel_lir::sealed_payload_source::ReceiptGrant::Parameter(
                0
            )]
        );
        assert!(contract.result.is_none());
        let mut simulated = Simulated {
            front: &front,
            unavailable: false,
            redirect: None,
            signature_override: None,
        };
        graph
            .recheck(&simulated, [root(&front, &path)], &mut |_, _, _| Ok(vec![]))
            .unwrap();
        let operation = DefPath::core("arena.seal");
        let original = front.signature(&operation).unwrap();
        for mutation in 0..3 {
            let mut signature = (*original).clone();
            let hirsel_types::SignatureKind::Function(function) = &mut signature.kind else {
                unreachable!()
            };
            match mutation {
                0 => function.parameters[0].passing = hirsel_types::PassingMode::Borrow,
                1 => function.allocation = hirsel_types::Allocation::NoAlloc,
                2 => {
                    signature.fingerprint =
                        hirsel_syntax::ContentHash::of_bytes(b"foreign core signature")
                }
                _ => unreachable!(),
            }
            simulated.signature_override = Some(Arc::new(signature));
            assert!(matches!(
                named_graph(&simulated, [root(&front, &path)], &mut |_, _, _| Ok(vec![])),
                Err(SourceError::Malformed(
                    "receipt operation declaration differs from reviewed core"
                ))
            ));
        }
    });
}

#[test]
fn source_factory_closure_context_keeps_original_capture_ports_and_rechecks_its_producer() {
    with_compiler_stack(|| {
        let (reader, path) = source(
            r#"
pub fn main()->u64 {
  let captured:u64=7u64
  let f=fn(x:u64)->u64 {captured+x}
  f(1u64)
}
"#,
        );
        let front = Front::new(&reader);
        let original = front.original(&path).unwrap();
        let mut selected = None;
        for (parent, body) in original.definition().bodies.iter().enumerate() {
            for (block, source) in body.blocks.iter().enumerate() {
                for (statement, instruction) in source.instructions.iter().enumerate() {
                    let hirsel_tir::InstructionKind::Assign { value, .. } = instruction.kind else {
                        continue;
                    };
                    let hirsel_tir::Rvalue::Closure { body: target, .. } =
                        &body.rvalues[value.0 as usize]
                    else {
                        continue;
                    };
                    assert!(selected.is_none());
                    selected = Some(ClosureInput {
                        base: hirsel_layout::instance::InstanceKey::new(
                            &path,
                            target.0,
                            &[],
                            front.types(),
                            None,
                            None,
                        )
                        .unwrap(),
                        parent: hirsel_tir::BodyId(parent as u32),
                        block: hirsel_tir::BlockId(block as u32),
                        statement: statement as u32,
                    });
                }
            }
        }
        let input = selected.expect("the original source closure assignment exists");
        let mut context = factory_closure(
            &front,
            input.clone(),
            &hirsel_types::TypeMemo::new(),
            &mut |_, _, _| panic!("only scalar ports"),
        )
        .unwrap();
        assert_eq!(context.source.entry().parameters().len(), 1);
        assert_eq!(context.source.entry().captures().len(), 1);
        assert_eq!(context.ports.len(), 3);
        let capture = &context.source.entry().captures()[0];
        assert_eq!(capture.ordinal, 0);
        let capture_port = hirsel_rc::sealed_source::contract::Port::Capture(0);
        assert_eq!(context.ports[&capture_port].root, capture.ty);
        assert!(matches!(
            context.source.entry().kind(),
            hirsel_rc::sealed_source::SourceEntryKind::Closure { .. }
        ));
        let mut simulated = Simulated {
            front: &front,
            unavailable: false,
            redirect: None,
            signature_override: None,
        };
        context
            .recheck(&simulated, input.clone(), &mut |_, _, _| {
                panic!("only scalar ports")
            })
            .unwrap();
        let saved_input = context.input.clone();
        context.input.statement = u32::MAX;
        assert!(matches!(
            context.recheck(&front, input.clone(), &mut |_, _, _| Ok(vec![])),
            Err(SourceError::Malformed(
                "receipt closure differs from current original observations"
            ))
        ));
        context.input = saved_input;
        let mut wrong = input.clone();
        wrong.statement = u32::MAX;
        assert!(matches!(
            factory_closure(
                &front,
                wrong,
                &hirsel_types::TypeMemo::new(),
                &mut |_, _, _| Ok(vec![])
            ),
            Err(SourceError::Malformed("original closure statement"))
        ));
        let saved = context.ports.remove(&capture_port).unwrap();
        assert!(matches!(
            context.context().canonical_bytes(),
            Err(SourceError::Malformed(
                "receipt context needs a complete ordered port-profile map"
            ))
        ));
        assert!(matches!(
            context.recheck(&front, input.clone(), &mut |_, _, _| Ok(vec![])),
            Err(SourceError::Malformed(
                "receipt closure differs from current original observations"
            ))
        ));
        context.ports.insert(capture_port, saved);
        simulated.unavailable = true;
        assert!(matches!(
            context.recheck(&simulated, input, &mut |_, _, _| Ok(vec![])),
            Err(SourceError::Unsupported("simulated unknown source"))
        ));
    });
}

#[test]
fn source_factory_discovers_literal_closures_and_named_function_values() {
    with_compiler_stack(|| {
        let (reader, path) = source(
            r#"
fn id(x:u64)->u64 {x}
pub fn main()->u64 {
  let f:fn(u64)->u64=id
  let captured=7u64
  let c=fn(x:u64)->u64 {id(x)+captured}
  let first=f(1u64)
  first+c(2u64)
}
"#,
        );
        let front = Front::new(&reader);
        let graph = named_graph(&front, [root(&front, &path)], &mut |_, _, _| {
            panic!("scalar entry and capture ports")
        })
        .unwrap();
        assert_eq!(graph.contexts.len(), 3);
        assert_eq!(graph.closures.len(), 1);
        assert_eq!(graph.targets.len(), 3);
        assert_eq!(graph.calls.len(), 2);
        let closure_context = &graph.contexts[graph.closures[0].target.index()];
        assert_eq!(closure_context.source.entry().captures().len(), 1);
        assert_eq!(closure_context.ports.len(), 3);
        assert!(
            graph
                .requests
                .iter()
                .all(|request| request.aware_key().is_none())
        );
        let simulated = Simulated {
            front: &front,
            unavailable: false,
            redirect: None,
            signature_override: None,
        };
        graph
            .recheck(&simulated, [root(&front, &path)], &mut |_, _, _| {
                panic!("scalar ports")
            })
            .unwrap();
    });
}

#[test]
fn source_factory_keeps_the_original_literal_closure_vector_as_an_accepted_graph() {
    with_compiler_stack(|| {
        let (reader, path) = source(
            r#"
pub fn main()->u64 {
  let f=fn()->u64 {7u64}
  f()
}
"#,
        );
        let front = Front::new(&reader);
        let graph = named_graph(&front, [root(&front, &path)], &mut |_, _, _| {
            panic!("no callable entry ports")
        })
        .unwrap();
        assert_eq!(graph.contexts.len(), 2);
        assert_eq!(graph.closures.len(), 1);
        assert_eq!(graph.targets.len(), 1);
        assert_eq!(graph.targets[0].targets, [graph.closures[0].target]);
        assert!(
            graph
                .requests
                .iter()
                .all(|request| request.aware_key().is_none())
        );
    });
}

#[test]
fn source_factory_expands_reviewed_arena_callbacks_and_keeps_scope_journal_demand() {
    with_compiler_stack(|| {
        for operation in ["arena.with", "arena.bounded"] {
            let (reader, path) = source(&format!(
                r#"
pub fn main()->{} {{
  {}({}fn(a:Arena)->Region[u64] {{
    let root=7u64
    arena.seal(take a,take root)
  }})
}}
"#,
                if operation == "arena.with" {
                    "Region[u64]"
                } else {
                    "Result[Region[u64],OutOfSpace]"
                },
                operation,
                if operation == "arena.with" {
                    ""
                } else {
                    "0u64,"
                }
            ));
            let front = Front::new(&reader);
            let mut graph = named_graph(&front, [root(&front, &path)], &mut |_, _, _| {
                panic!("scalar ports")
            })
            .unwrap();
            assert_eq!(graph.contexts.len(), 2);
            assert_eq!(graph.operations.len(), 2);
            assert_eq!(graph.closures.len(), 1);
            assert_eq!(graph.targets.len(), 1);
            let callback = &graph.requests[graph.closures[0].target.index()];
            assert_eq!(
                callback.declaration().contract().unwrap().inventory,
                [hirsel_lir::sealed_payload_source::ReceiptGrant::Parameter(
                    0
                )]
            );
            assert!(
                graph.requests[0].aware_key().is_some(),
                "scope must retain its internal journal demand"
            );
            let mut simulated = Simulated {
                front: &front,
                unavailable: false,
                redirect: None,
                signature_override: None,
            };
            graph
                .recheck(&simulated, [root(&front, &path)], &mut |_, _, _| {
                    panic!("scalar ports")
                })
                .unwrap();
            let saved = graph.targets.pop().unwrap();
            assert!(
                graph
                    .recheck(&front, [root(&front, &path)], &mut |_, _, _| Ok(vec![]))
                    .is_err()
            );
            graph.targets.push(saved);
            let original = front.signature(&DefPath::core(operation)).unwrap();
            let mut forged = (*original).clone();
            if let hirsel_types::SignatureKind::Function(function) = &mut forged.kind {
                function.allocation = hirsel_types::Allocation::MayAllocate;
            } else {
                unreachable!()
            }
            simulated.signature_override = Some(Arc::new(forged));
            assert!(matches!(
                named_graph(&simulated, [root(&front, &path)], &mut |_, _, _| Ok(vec![])),
                Err(SourceError::Malformed(
                    "receipt operation declaration differs from reviewed core"
                ))
            ));
        }
        let (reader, path) = source("pub fn main()->u64 { arena.with(fn(a:Arena)->u64 {7u64}) }");
        let front = Front::new(&reader);
        let graph = named_graph(&front, [root(&front, &path)], &mut |_, _, _| {
            panic!("scalar ports")
        })
        .unwrap();
        assert_eq!(graph.contexts.len(), 2);
        assert!(
            graph
                .requests
                .iter()
                .all(|request| request.aware_key().is_none())
        );
    });
}

#[test]
fn source_factory_expands_reviewed_sealed_read_callbacks_from_original_owner_calls() {
    with_compiler_stack(|| {
        let (reader, path) = source(
            r#"
pub fn main()->u64 {
  let mut r=arena.with(fn(a:Arena)->Region[u64] {
    let root=7u64
    arena.seal(take a,take root)
  })
  region.with(mut r,fn(root:u64)->u64 {root})
}
"#,
        );
        let front = Front::new(&reader);
        let mut graph = named_graph(&front, [root(&front, &path)], &mut |_, _, _| {
            panic!("scalar profiles")
        })
        .unwrap();
        assert_eq!(graph.contexts.len(), 3);
        assert_eq!(graph.closures.len(), 2);
        assert_eq!(graph.targets.len(), 2);
        assert_eq!(graph.operations.len(), 3);
        assert!(
            graph
                .operations
                .iter()
                .any(|operation| operation.operation.definition.is_core("region.with"))
        );
        let mut simulated = Simulated {
            front: &front,
            unavailable: false,
            redirect: None,
            signature_override: None,
        };
        graph
            .recheck(&simulated, [root(&front, &path)], &mut |_, _, _| {
                panic!("scalar profiles")
            })
            .unwrap();
        let saved = graph.operations.pop().unwrap();
        assert!(
            graph
                .recheck(&front, [root(&front, &path)], &mut |_, _, _| Ok(vec![]))
                .is_err()
        );
        graph.operations.push(saved);
        let operation = DefPath::core("region.with");
        let original = front.signature(&operation).unwrap();
        let mut forged = (*original).clone();
        if let hirsel_types::SignatureKind::Function(function) = &mut forged.kind {
            function.parameters[0].passing = hirsel_types::PassingMode::Borrow;
        } else {
            unreachable!()
        }
        simulated.signature_override = Some(Arc::new(forged));
        assert!(matches!(
            named_graph(&simulated, [root(&front, &path)], &mut |_, _, _| Ok(vec![])),
            Err(SourceError::Malformed(
                "receipt operation declaration differs from reviewed core"
            ))
        ));

        let (reader, path) = source_at(
            "pub fn inspect(mut r:Region[u64])->u64 { region.with(mut r,fn(root:u64)->u64 {root}) }",
            "inspect",
        );
        let front = Front::new(&reader);
        let graph = named_graph(&front, [root(&front, &path)], &mut |_, _, _| {
            panic!("scalar profiles")
        })
        .unwrap();
        assert_eq!(graph.contexts.len(), 2);
        assert!(
            graph
                .requests
                .iter()
                .all(|request| request.aware_key().is_none())
        );
    });
}

#[test]
fn source_factory_tracks_before_scope_payloads_and_mutated_arena_collections() {
    with_compiler_stack(|| {
        let (reader, path) = source(
            r#"
pub fn main()->u64 {
  let leaf:Bytes in heap=copy_to(heap,b"abc")
  let mut r=arena.with(fn(a:Arena)->Region[List[Bytes]] {
    let independent:Bytes in heap=copy_to(heap,leaf)
    let mut root:List[Bytes] in a=[copy independent]
    list.push(mut root,copy independent)
    arena.seal(take a,take root)
  })
  region.with(mut r,fn(root:List[Bytes])->u64 {
    list.len(root)+bytes.len(root[0])
  })
}
"#,
        );
        let front = Front::new(&reader);
        let graph = named_graph(&front, [root(&front, &path)], &mut |_, _, _| {
            panic!("noncallable ports")
        })
        .unwrap();
        assert_eq!(graph.contexts.len(), 3);
        assert_eq!(graph.operations.len(), 10);
        let constructor = graph
            .contexts
            .iter()
            .zip(&graph.requests)
            .find(|(context, _)| context.source.entry().arena_scope_grant().is_some())
            .unwrap();
        let contract = constructor.1.declaration().contract().unwrap();
        assert_eq!(contract.captures.len(), 1);
        assert_eq!(contract.captures[0].ordinal, 0);
        assert_eq!(
            contract.captures[0].mode,
            hirsel_lir::sealed_payload_source::ReceiptMode::Borrow
        );
        assert!(
            graph.requests[0]
                .declaration()
                .requirements()
                .payload_locals
                .len()
                > 1
        );
        let mut simulated = Simulated {
            front: &front,
            unavailable: false,
            redirect: None,
            signature_override: None,
        };
        graph
            .recheck(&simulated, [root(&front, &path)], &mut |_, _, _| {
                panic!("noncallable ports")
            })
            .unwrap();
        let original = front.signature(&DefPath::core("list.push")).unwrap();
        let mut forged = (*original).clone();
        if let hirsel_types::SignatureKind::Function(function) = &mut forged.kind {
            function.parameters[0].passing = hirsel_types::PassingMode::Borrow;
        } else {
            unreachable!()
        }
        simulated.signature_override = Some(Arc::new(forged));
        assert!(matches!(
            named_graph(&simulated, [root(&front, &path)], &mut |_, _, _| Ok(vec![])),
            Err(SourceError::Malformed(
                "receipt operation declaration differs from reviewed core"
            ))
        ));
    });
}

#[test]
fn source_factory_keeps_reviewed_scalar_observations_empty_without_a_seal() {
    with_compiler_stack(|| {
        let (reader, path) = source(
            r#"
pub fn main()->u64 {
  let bytes:Bytes in heap=copy_to(heap,b"abc")
  let mut xs:List[Bytes] in heap=[]
  list.push(mut xs,copy bytes)
  list.len(xs)+bytes.len(bytes)
}
"#,
        );
        let front = Front::new(&reader);
        let graph = named_graph(&front, [root(&front, &path)], &mut |_, _, _| {
            panic!("noncallable ports")
        })
        .unwrap_or_else(|error| {
            let original = front.original(&path).unwrap();
            let calls: Vec<_> = original
                .definition()
                .bodies
                .iter()
                .flat_map(|body| {
                    body.blocks
                        .iter()
                        .filter_map(|block| match &block.terminator {
                            hirsel_tir::Terminator::Call { callee, .. } => Some(callee.clone()),
                            _ => None,
                        })
                })
                .collect();
            panic!("{error:?}; original collection calls: {calls:?}")
        });
        assert_eq!(graph.operations.len(), 5);
        assert_eq!(graph.contexts.len(), 1);
        assert!(graph.requests[0].aware_key().is_none());
        assert!(
            graph.requests[0]
                .declaration()
                .requirements()
                .payload_locals
                .is_empty()
        );
    });
}

#[test]
fn source_factory_rejects_borrowed_before_scope_payload_without_detachment() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add(
                "borrowed-before-scope.hir",
                r#"
pub fn main()->u64 {
  let leaf:Bytes in heap=copy_to(heap,b"abc")
  let mut r=arena.with(fn(a:Arena)->Region[List[Bytes]] {
    let mut root:List[Bytes] in a=[copy leaf]
    list.push(mut root,copy leaf)
    arena.seal(take a,take root)
  })
  region.with(mut r,fn(root:List[Bytes])->u64 {
    list.len(root)+bytes.len(root[0])
  })
}
"#
                .into(),
            )
            .unwrap();
        let report = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.rule.as_str() == "MODE-1"),
            "{:?}",
            report.diagnostics
        );
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.rule.as_str() == "MEM-4"),
            "{:?}",
            report.diagnostics
        );
    });
}

#[test]
fn source_factory_publishes_only_rechecked_receipt_requests_to_c_declarations() {
    use hirsel_lir::operation::OperationDeclarations;
    with_compiler_stack(|| {
        let (reader, path) = source(
            r#"
pub fn main()->u64 {
  let mut r=arena.with(fn(a:Arena)->Region[u64] {
    let root=7u64
    arena.seal(take a,take root)
  })
  region.with(mut r,fn(root:u64)->u64 {root})
}
"#,
        );
        let front = Front::new(&reader);
        let mut graph = named_graph(&front, [root(&front, &path)], &mut |_, _, _| {
            panic!("scalar profiles")
        })
        .unwrap();
        let declarations = graph
            .declarations(
                &front,
                [root(&front, &path)],
                &mut |_, _, _| panic!("scalar profiles"),
                &(),
            )
            .unwrap();
        let mut published = 0;
        for request in &graph.requests {
            let expected = request.receipt_source().unwrap();
            if let Some(expected) = expected {
                let actual = declarations
                    .receipt_source(&expected.original.definition, expected.instance)
                    .unwrap();
                assert_eq!(actual, expected);
                assert_eq!(actual.profiles.len(), request.profiles().len());
                assert_ne!(actual.instance, actual.original.instance);
                assert!(
                    declarations
                        .receipt_source(&actual.original.definition, actual.original.instance)
                        .is_none()
                );
                assert!(
                    declarations
                        .receipt_source(&DefPath::core("unrelated"), actual.instance)
                        .is_none()
                );
                published += 1;
            }
        }
        assert!(published > 0);
        let mut simulated = Simulated {
            front: &front,
            unavailable: true,
            redirect: None,
            signature_override: None,
        };
        assert!(matches!(
            graph.declarations(
                &simulated,
                [root(&front, &path)],
                &mut |_, _, _| Ok(vec![]),
                &()
            ),
            Err(SourceError::Unsupported("simulated unknown source"))
        ));
        simulated.unavailable = false;
        graph.requests.swap(0, 1);
        assert!(matches!(
            graph.declarations(
                &simulated,
                [root(&front, &path)],
                &mut |_, _, _| Ok(vec![]),
                &()
            ),
            Err(SourceError::Malformed(
                "receipt graph differs from current original observations"
            ))
        ));
    });
}

#[test]
fn source_factory_receipt_declarations_refuse_an_older_v_source_selection() {
    with_compiler_stack(|| {
        let text = "pub fn main()->Region[u64] { arena.with(fn(a:Arena)->Region[u64] { let x=7u64; arena.seal(take a,take x) }) }";
        let (reader, path) = source(text);
        let front = Front::new(&reader);
        let graph = named_graph(&front, [root(&front, &path)], &mut |_, _, _| {
            panic!("scalar profiles")
        })
        .unwrap();
        let (current, _) = source(&text.replace("7u64", "11u64"));
        let current = Front::new(&current);
        assert!(matches!(
            graph.declarations(
                &current,
                [root(&current, &path)],
                &mut |_, _, _| panic!("scalar profiles"),
                &()
            ),
            Err(SourceError::Malformed(
                "receipt graph differs from current original observations"
            ))
        ));
        let current_graph = named_graph(&current, [root(&current, &path)], &mut |_, _, _| {
            panic!("scalar profiles")
        })
        .unwrap();
        current_graph
            .declarations(
                &current,
                [root(&current, &path)],
                &mut |_, _, _| panic!("scalar profiles"),
                &(),
            )
            .unwrap();
    });
}

#[test]
fn source_factory_expands_reviewed_task_callbacks_without_arena_authority() {
    with_compiler_stack(|| {
        let (reader, path) = source(
            r#"
pub fn main(tasks: Tasks)->u64 {
  let isolated=task.isolate(tasks,fn()->u64 {7u64})
  let got=match isolated {Ok(value)=>value, Error(_)=>0u64}
  task.scope(tasks,fn(s:Scope)->u64 {
    let child=task.async(s,fn()->u64 {got})
    task.await(s,take child)
  })
}
"#,
        );
        let front = Front::new(&reader);
        let graph = named_graph(&front, [root(&front, &path)], &mut |_, _, _| {
            panic!("no callable ports")
        })
        .unwrap();
        for name in ["task.isolate", "task.scope", "task.async", "task.await"] {
            assert!(
                graph
                    .operations
                    .iter()
                    .any(|op| op.operation.definition.is_core(name))
            );
        }
        assert!(
            graph.contexts.iter().all(|context| context
                .source
                .entry()
                .arena_scope_grant()
                .is_none())
        );
        assert!(
            graph
                .requests
                .iter()
                .all(|request| request.aware_key().is_none())
        );
        graph
            .recheck(&front, [root(&front, &path)], &mut |_, _, _| {
                panic!("no callable ports")
            })
            .unwrap();
        for name in ["task.isolate", "task.scope", "task.async"] {
            let original = front.signature(&DefPath::core(name)).unwrap();
            let mut forged = (*original).clone();
            let hirsel_types::SignatureKind::Function(function) = &mut forged.kind else {
                unreachable!()
            };
            function.parameters[1].passing = hirsel_types::PassingMode::Mut;
            let simulated = Simulated {
                front: &front,
                unavailable: false,
                redirect: None,
                signature_override: Some(Arc::new(forged)),
            };
            assert!(matches!(
                named_graph(&simulated, [root(&front, &path)], &mut |_, _, _| Ok(vec![])),
                Err(SourceError::Malformed(
                    "receipt operation declaration differs from reviewed core"
                ))
            ));
        }
    });
}

#[test]
fn source_factory_expands_the_sealed_task_program_from_original_source() {
    with_compiler_stack(|| {
        for name in ["MEM-4-sealed-task-unique-list-run.hir"] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../conformance/MEM")
                .join(name);
            let (reader, path) = source(&std::fs::read_to_string(path).unwrap());
            let front = Front::new(&reader);
            let graph = named_graph(&front, [root(&front, &path)], &mut |_, _, _| {
                panic!("no callable ports in {name}")
            })
            .unwrap_or_else(|error| panic!("{name}: {error:?}"));
            assert!(
                graph
                    .operations
                    .iter()
                    .any(|op| op.operation.definition.is_core("arena.seal"))
            );
            assert!(
                graph
                    .operations
                    .iter()
                    .any(|op| op.operation.definition.is_core("region.with"))
            );
            assert!(
                graph
                    .requests
                    .iter()
                    .any(|request| request.aware_key().is_some())
            );
            let _ = graph
                .declarations(
                    &front,
                    [root(&front, &path)],
                    &mut |_, _, _| panic!("no callable ports"),
                    &(),
                )
                .unwrap();
        }
    });
}

#[test]
fn source_factory_rechecks_shared_read_and_transfer_declarations() {
    with_compiler_stack(|| {
        let (reader, path) = source(
            r#"
pub fn main()->u64 {
  let built:unique Bytes=unique b"abc"
  let s=shared.freeze(take built)
  let n=shared.with(s,fn(b:Bytes)->u64 {bytes.len(b)})
  let ok=match shared.thaw(take s) {Ok(_)=>true,Error(_)=>false}
  failure.check(ok,Bounds())
  n
}
"#,
        );
        let front = Front::new(&reader);
        let graph = named_graph(&front, [root(&front, &path)], &mut |_, _, _| {
            panic!("no callable ports")
        })
        .unwrap();
        assert!(
            graph
                .requests
                .iter()
                .all(|request| request.aware_key().is_none())
        );
        for name in ["shared.freeze", "shared.thaw", "shared.with"] {
            assert!(
                graph
                    .operations
                    .iter()
                    .any(|op| op.operation.definition.is_core(name))
            );
            let original = front.signature(&DefPath::core(name)).unwrap();
            let mut forged = (*original).clone();
            let hirsel_types::SignatureKind::Function(function) = &mut forged.kind else {
                unreachable!()
            };
            function.parameters[0].passing = hirsel_types::PassingMode::Mut;
            let simulated = Simulated {
                front: &front,
                unavailable: false,
                redirect: None,
                signature_override: Some(Arc::new(forged)),
            };
            assert!(matches!(
                named_graph(&simulated, [root(&front, &path)], &mut |_, _, _| Ok(vec![])),
                Err(SourceError::Malformed(
                    "receipt operation declaration differs from reviewed core"
                ))
            ));
        }
    });
}
