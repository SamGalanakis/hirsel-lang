use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_interp::{Execution, Limits, Outcome, TraceEvent, Value};
use hirsel_tir::{DefinitionRef, RunConfiguration};

fn prepare(source: &str) -> (QueryEngine, DefinitionRef) {
    if !source.contains("std@1/") {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("source.hir", source.into()).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(
            output
                .diagnostics
                .iter()
                .all(|d| d.severity != hirsel_diag::Severity::Error),
            "{:?}",
            output.diagnostics
        );
        let main = engine
            .lower_to_tir(&entry_path(&default_context()))
            .unwrap()
            .value;
        return (engine, main);
    }
    let mut loaded = load_program_source(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/std_numeric.hir"),
        Some(source),
    )
    .unwrap();
    loaded
        .check_for_execution()
        .unwrap_or_else(|e| panic!("{source}\n{e:?}"));
    let mut engine = loaded.engine;
    let main = engine
        .lower_to_tir(&entry_path(&default_context()))
        .unwrap()
        .value;
    (engine, main)
}
struct RecordingLibrary {
    library: hirsel_interp::library::Library,
    finalizer_values: Vec<u128>,
}
impl hirsel_interp::Operations for RecordingLibrary {
    fn call(
        &mut self,
        call: hirsel_interp::OperationCall<'_>,
        context: &mut hirsel_interp::OperationContext<'_>,
    ) -> Result<Option<hirsel_interp::OperationResult>, hirsel_interp::Error> {
        // The std formatter executes source, so observe its conversion leaf.
        if call.path == &hirsel_syntax::DefPath::core("string.from_bytes") {
            if let Some(Value::Bytes(bytes)) = call.arguments.first() {
                let text = std::str::from_utf8(bytes).expect("numeric finalizer marker is ASCII");
                self.finalizer_values.push(
                    text.parse()
                        .expect("numeric finalizer marker is an integer"),
                );
            }
        }
        self.library.call(call, context)
    }
}
fn execute_recorded(engine: &mut QueryEngine, depth: usize) -> (Execution, Vec<u128>) {
    let sources = engine.sources.clone();
    let mut runtime = engine.runtime_types().unwrap();
    runtime.trace_mode = hirsel_interp::TraceMode::Full;
    let config = RunConfiguration {
        entry: entry_path(&default_context()),
        hash_seed: 0,
        inputs: vec![],
    };
    let mut library = RecordingLibrary {
        library: hirsel_interp::library::Library::new(&config, Default::default()).unwrap(),
        finalizer_values: vec![],
    };
    let execution = hirsel_interp::run_with_runtime(
        engine,
        &sources,
        &config,
        vec![],
        Limits {
            steps: Some(5_000_000),
            call_depth: depth,
        },
        &mut library,
        &runtime,
    )
    .unwrap();
    (execution, library.finalizer_values)
}
fn execute(engine: &mut QueryEngine, depth: usize) -> Execution {
    execute_recorded(engine, depth).0
}
fn returned(execution: &Execution, expected: u64) {
    assert!(
        matches!(&execution.outcome, Outcome::Returned { value: Value::Integer(n), .. } if n.bits() == expected as u128),
        "{:?}",
        execution.outcome
    );
}
fn function(engine: &mut QueryEngine, name: &str) -> DefinitionRef {
    let mut path = entry_path(&default_context());
    path.0.name = name.into();
    engine.lower_to_tir(&path).unwrap().value
}
#[test]
fn counted_borrow_arguments_release_previous_and_final_internal_owners() {
    with_compiler_stack(|| {
        let (mut engine, _) = prepare(
            "fn descend(n:u64,text:String)->u64 { if n == 0u64 { string.byte_len(text) } else { descend(n-1u64,std@1/u64.to_string(n)) } }\nfn main()->u64 {let text:String=\"start\";descend(1000u64,text)}",
        );
        let descend = function(&mut engine, "descend");
        assert_eq!(descend.bodies[0].tail_sites.len(), 1, "{descend:#?}");
        assert_eq!(
            descend.bodies[0].tail_sites[0].owned_loop_arguments.len(),
            1
        );
        let execution = execute(&mut engine, 8);
        returned(&execution, 1);
        let accounting: Vec<_> = execution
            .trace
            .iter()
            .filter_map(|e| {
                if let TraceEvent::LoopOwnership { released, live } = e {
                    Some((*released, *live))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(accounting.iter().map(|e| e.0).sum::<usize>(), 1000);
        assert_eq!(accounting.iter().map(|e| e.1).max(), Some(1));
        assert_eq!(accounting.last().unwrap().1, 0);
    });
}
#[test]
fn moved_resource_tail_arguments_finalize_once_at_the_return() {
    with_compiler_stack(|| {
        let (mut engine, _) = prepare(
            "resource type R {n:u64} drop(x) {}\nfn descend(n:u64,take r:R)->u64 { if n == 0u64 { r.n } else { descend(n-1u64,take r) } }\nfn main()->u64 {descend(1000u64,R(n:7u64))}",
        );
        let descend = function(&mut engine, "descend");
        assert_eq!(descend.bodies[0].tail_sites.len(), 1, "{descend:#?}");
        let execution = execute(&mut engine, 8);
        returned(&execution, 7);
        assert_eq!(execution.trace.iter().filter(|e|matches!(e, TraceEvent::Effect(e) if matches!(e.effect,hirsel_interp::observation::Effect::Finalizer{..}))).count(), 1);
    });
}
#[test]
fn pending_resource_finalization_keeps_ordinary_frames_and_the_tooling_limit() {
    with_compiler_stack(|| {
        let source = "resource type R {n:u64} drop(x) { let _ = std@1/u64.to_string(x.n) }\nfn descend(n:u64)->u64 { let r = R(n:n); if n == 0u64 { r.n } else { descend(n-1u64) } }\nfn main()->u64 {descend(5u64)}";
        let (mut engine, _) = prepare(source);
        assert!(
            function(&mut engine, "descend").bodies[0]
                .tail_sites
                .is_empty()
        );
        let (execution, finalizer_values) = execute_recorded(&mut engine, 16);
        assert_eq!(finalizer_values, vec![0, 1, 2, 3, 4, 5]);
        returned(&execution, 0);
        let finalizers: Vec<_> = execution
            .trace
            .iter()
            .filter_map(|e| {
                if let TraceEvent::Effect(e) = e {
                    if matches!(
                        e.effect,
                        hirsel_interp::observation::Effect::Finalizer { .. }
                    ) {
                        Some(e.anchor.clone())
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(finalizers.len(), 6);
        assert!(finalizers.windows(2).all(|a| a[0].body == a[1].body));
        let (mut engine, _) = prepare(&source.replace("descend(5u64)", "descend(1000u64)"));
        assert!(matches!(
            execute(&mut engine, 8).outcome,
            Outcome::ToolingLimit(hirsel_tir::ToolingLimit {
                kind: hirsel_tir::ToolingLimitKind::CallDepth,
                ..
            })
        ));
    });
}

#[test]
fn current_frame_borrow_and_post_call_contract_retain_ordinary_calls() {
    with_compiler_stack(|| {
        for source in [
            "fn descend(n:u64,text:String)->u64 { let next = std@1/u64.to_string(n); if n == 0u64 { string.byte_len(text) } else { descend(n-1u64,next) } }\nfn main()->u64 {let text:String=\"start\";descend(1000u64,text)}",
            "fn descend(n:u64)->u64 ensures result == 0u64 { if n == 0u64 {0u64} else {descend(n-1u64)} }\nfn main()->u64 {descend(1000u64)}",
        ] {
            let (mut engine, _) = prepare(source);
            assert!(
                function(&mut engine, "descend").bodies[0]
                    .tail_sites
                    .is_empty()
            );
            assert!(matches!(
                execute(&mut engine, 8).outcome,
                Outcome::ToolingLimit(_)
            ));
        }
    });
}
#[test]
fn generic_size_bindings_survive_self_tail_restarts() {
    with_compiler_stack(|| {
        let (mut engine, _) = prepare(
            "fn descend[n:size](left:u64,xs:Inline[u8,n])->u64 { if left == 0u64 {n} else {descend(left-1u64,xs)} }\nfn main()->u64 {let xs:Inline[u8,7]=[1u8,2u8,3u8,4u8,5u8,6u8,7u8]; descend(1000u64,xs)}",
        );
        assert_eq!(
            function(&mut engine, "descend").bodies[0].tail_sites.len(),
            1
        );
        returned(&execute(&mut engine, 8), 7);
    });
}
#[test]
fn independent_ir_validation_rejects_forged_tail_obligations_and_continuations() {
    with_compiler_stack(|| {
        let (mut engine, _) = prepare(
            "fn descend(n:u64)->u64 {if n == 0u64 {0u64} else {descend(n-1u64)}}\nfn main()->u64 {descend(10u64)}",
        );
        let input = engine.export_native_input().unwrap();
        let mut path = entry_path(&default_context());
        path.0.name = "descend".into();
        let base = input.definitions.get(&path).unwrap().clone();
        let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
        signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
        assert!(
            hirsel_validate::validate(&base, engine.types(), &signatures, &mut vec![])
                .violations
                .is_empty()
        );
        let rejected = |definition: &hirsel_tir::Definition| {
            assert!(
                hirsel_validate::validate(definition, engine.types(), &signatures, &mut vec![])
                    .violations
                    .iter()
                    .any(|v| matches!(
                        v.kind,
                        hirsel_validate::ViolationKind::WrongKind {
                            entity: "tail witness",
                            ..
                        }
                    ))
            );
        };
        let mut bad = (*base).clone();
        bad.bodies[0].tail_sites[0]
            .owned_loop_arguments
            .push((hirsel_types::ParamId(0), hirsel_tir::PlaceId(0)));
        rejected(&bad);
        let mut bad = (*base).clone();
        let body = &mut bad.bodies[0];
        let hirsel_tir::Terminator::Call {
            returns: Some(hirsel_tir::CallReturn { destination, next }),
            ..
        } = body.blocks[body.tail_sites[0].call_block.0 as usize].terminator
        else {
            panic!()
        };
        let span = body.blocks[next.0 as usize].span;
        body.blocks[next.0 as usize].instructions.insert(
            0,
            hirsel_tir::Instruction {
                kind: hirsel_tir::InstructionKind::Check {
                    condition: hirsel_tir::Operand {
                        place: destination,
                        usage: hirsel_types::UseKind::Copy,
                    },
                    rule: const { hirsel_diag::RuleId::of("FACT-1") },
                },
                span,
                on_failure: hirsel_tir::CleanupId(0),
            },
        );
        rejected(&bad);
        let mut forged = input;
        forged.definitions.insert(path, std::sync::Arc::new(bad));
        assert!(engine.import_native_input(forged).is_err());
    });
}

#[test]
fn generic_plain_and_nominal_declarations_prove_tail_cleanup_independently() {
    with_compiler_stack(|| {
        for source in [
            "fn descend[T:plain](n:u64,x:T)->u64 {let alias=x; if n == 0u64 {7u64} else {descend(n-1u64,alias)}}\nfn main()->u64 {descend(1000u64,1u8)}",
            "type W {n:u64}\nfn descend(n:u64)->u64 {let w=W(n:n); if n == 0u64 {7u64} else {descend(n-1u64)}}\nfn main()->u64 {descend(1000u64)}",
            "type W {text:String}\nfn descend(n:u64,x:W)->u64 {if n == 0u64 {7u64} else {descend(n-1u64,W(text:std@1/u64.to_string(n)))}}\nfn main()->u64 {descend(1000u64,W(text:\"start\"))}",
            "fn descend[T:data](n:u64,x:T)->u64 {if n == 0u64 {7u64} else {descend(n-1u64,copy x)}}\nfn main()->u64 {let text:String=\"start\";descend(1000u64,text)}",
        ] {
            let (mut engine, _) = prepare(source);
            let descend = function(&mut engine, "descend");
            assert_eq!(
                descend.bodies[0].tail_sites.len(),
                1,
                "{source}\n{descend:#?}"
            );
            returned(&execute(&mut engine, 8), 7);
        }
    });
}

#[test]
fn a_diverging_self_tail_call_is_a_checked_loop_site() {
    with_compiler_stack(|| {
        // `fn serve() -> Never { ..; serve() }`: the loop's only exit is the failure
        // edge, reached after more restarts than the call depth allows.
        let (mut engine, _) = prepare(
            "fn serve(n:u64, take text:String)->Never { let _ = 10u64 / n; serve(n-1u64, take text) }\nfn main()->u64 {serve(1000u64,\"x\")}",
        );
        let execution = execute(&mut engine, 64);
        assert!(
            matches!(&execution.outcome, Outcome::Failed(f) if f.rule.as_str() == "SEM-1"),
            "{:?}",
            execution.outcome
        );
        let input = engine.export_native_input().unwrap();
        let mut path = entry_path(&default_context());
        path.0.name = "serve".into();
        let base = input.definitions.get(&path).unwrap().clone();
        let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
        signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
        let body = &base.bodies[0];
        let [site] = body.tail_sites.as_slice() else {
            panic!("{:?}", body.tail_sites)
        };
        let hirsel_tir::Terminator::Call {
            returns: None,
            arguments,
            ..
        } = &body.blocks[site.call_block.0 as usize].terminator
        else {
            panic!("not a diverging call site")
        };
        assert!(
            hirsel_validate::validate(&base, engine.types(), &signatures, &mut vec![])
                .violations
                .is_empty()
        );
        // The frame may keep nothing past a diverging site: a failure cleanup that
        // still owes the moved String is not a loop.
        let text = arguments[1].value.place;
        let mut bad = (*base).clone();
        let body = &mut bad.bodies[0];
        let owing = body.add_cleanup(vec![hirsel_tir::CleanupAction::Drop {
            place: text,
            if_live: false,
        }]);
        let hirsel_tir::Terminator::Call { on_failure, .. } =
            &mut body.blocks[site.call_block.0 as usize].terminator
        else {
            panic!()
        };
        *on_failure = owing;
        bad.types = hirsel_tir::definition_types(engine.types(), &bad);
        assert!(
            hirsel_validate::validate(&bad, engine.types(), &signatures, &mut vec![])
                .violations
                .iter()
                .any(|v| matches!(
                    v.kind,
                    hirsel_validate::ViolationKind::WrongKind {
                        entity: "tail witness",
                        ..
                    }
                )),
        );
        let mut forged = input;
        forged.definitions.insert(path, std::sync::Arc::new(bad));
        assert!(engine.import_native_input(forged).is_err());
    });
}
