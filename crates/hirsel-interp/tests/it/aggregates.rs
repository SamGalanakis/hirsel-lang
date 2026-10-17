use crate::support;
use hirsel_diag::RuleId;
use hirsel_interp::{aggregate::*, observation::*, *};
use hirsel_syntax::BinaryOp as B;
use hirsel_tir::*;
use hirsel_types::{ParamId, Primitive as P, TypeKind, UseKind};
use support::*;
fn integer(n: u128) -> Value {
    Value::Integer(Integer::new(P::U64, n).unwrap())
}
fn fields(values: Vec<PlaceId>) -> Vec<ConstructField> {
    values
        .into_iter()
        .enumerate()
        .map(|(i, p)| ConstructField {
            field: i as u32,
            value: take(p),
        })
        .collect()
}
fn run_runtime(
    definitions: Vec<Definition>,
    runtime: &RuntimeTypes,
    arguments: Vec<Value>,
) -> Execution {
    assert_fixtures(&definitions, runtime);
    let configuration = RunConfiguration {
        entry: definitions[0].path.clone(),
        hash_seed: 42,
        inputs: vec![],
    };
    run_with_runtime(
        &mut Provider {
            definitions,
            queries: vec![],
        },
        &sources(),
        &configuration,
        arguments,
        Limits::default(),
        &mut NoOperations,
        runtime,
    )
    .unwrap()
}
fn returned(e: Execution) -> Value {
    match e.outcome {
        Outcome::Returned { value, .. } => value,
        _ => panic!("{:?}", e.outcome),
    }
}
fn nominal(name: &str, count: usize, finalizer: bool) -> Nominal {
    Nominal {
        definition: path(name),
        resource: true,
        distinct: false,
        finalizer: finalizer.then(|| (path(&format!("{name}_drop")), BodyId(0))),
        fields: count,
    }
}
fn resource(b: &mut Builder, name: &str, parts: Vec<PlaceId>) -> PlaceId {
    let p = b.local(
        TypeKind::Named {
            definition: path(name),
            arguments: vec![].into(),
        },
        Storage::Local,
    );
    b.assign(
        p,
        Rvalue::Construct {
            constructor: Constructor::Record(path(name)),
            fields: fields(parts),
            destination: hirsel_types::Destination::Inferred,
        },
        2,
        CleanupId(0),
    );
    p
}
fn finalizer(name: &str, fail: bool) -> Definition {
    let mut b = Builder::new();
    b.borrowed_parameter(TypeKind::Named {
        definition: path(name),
        arguments: vec![].into(),
    });
    let u = b.unit();
    if fail {
        b.block(
            Terminator::Fail {
                rule: const { RuleId::of("FAIL-1") },
                cleanup: CleanupId(0),
            },
            9,
        );
        b.definition(&format!("{name}_drop"))
    } else {
        b.finish(&format!("{name}_drop"), u)
    }
}
fn finalizers(e: &Execution) -> Vec<String> {
    e.trace
        .iter()
        .filter_map(|t| match t {
            TraceEvent::Effect(OrderedEffect {
                effect: Effect::Finalizer { definition },
                ..
            }) => Some(definition.0.name.clone()),
            _ => None,
        })
        .collect()
}
#[test]
fn resource_finalizer_precedes_fields_and_failed_finalizer_releases_every_field() {
    for fail in [false, true] {
        let mut b = Builder::new();
        let a = resource(&mut b, "A", vec![]);
        let z = resource(&mut b, "Z", vec![]);
        let outer = resource(&mut b, "Outer", vec![a, z]);
        let c = b.cleanup(vec![drop(outer)]);
        let u = b.unit();
        b.block(
            Terminator::Return {
                value: take(u),
                cleanup: c,
            },
            4,
        );
        let runtime = RuntimeTypes {
            trace_mode: Default::default(),
            nominals: vec![
                nominal("Outer", 2, true),
                nominal("A", 0, true),
                nominal("Z", 0, true),
            ],
            ..RuntimeTypes::default()
        };
        let e = run_runtime(
            vec![
                b.definition("main"),
                finalizer("Outer", fail),
                finalizer("A", false),
                finalizer("Z", false),
            ],
            &runtime,
            vec![],
        );
        assert_eq!(finalizers(&e), ["Outer", "Z", "A"]);
        assert_eq!(matches!(e.outcome, Outcome::Failed(_)), fail);
    }
}
#[test]
fn failure_preserves_original_cause_then_records_cleanup_failures_in_order() {
    let mut b = Builder::new();
    let a = resource(&mut b, "A", vec![]);
    let z = resource(&mut b, "Z", vec![]);
    let c = b.cleanup(vec![drop(z), drop(a)]);
    b.block(
        Terminator::Fail {
            rule: const { RuleId::of("FACT-1") },
            cleanup: c,
        },
        6,
    );
    let runtime = RuntimeTypes {
        trace_mode: Default::default(),
        nominals: vec![nominal("A", 0, true), nominal("Z", 0, true)],
        ..RuntimeTypes::default()
    };
    let e = run_runtime(
        vec![
            b.definition("main"),
            finalizer("A", true),
            finalizer("Z", true),
        ],
        &runtime,
        vec![],
    );
    assert_eq!(finalizers(&e), ["Z", "A"]);
    assert_eq!(e.failures.len(), 3);
    assert_eq!(e.failures[0].failure.rule.as_str(), "FACT-1");
    assert_eq!(e.failures[0].cause, Some(1));
    assert_eq!(e.failures[1].cause, Some(2));
    assert_eq!(e.observable(&sources()).unwrap().failures.len(), 3);
}
#[test]
fn forget_suppresses_outer_finalizer_and_all_owned_fields() {
    let mut b = Builder::new();
    let inner = resource(&mut b, "A", vec![]);
    let outer = resource(&mut b, "Outer", vec![inner]);
    let result = b.local(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    b.block(
        Terminator::Call {
            callee: Callee::Definition {
                path: path("forget"),
                arguments: vec![],
            },
            arguments: vec![CallArgument {
                parameter: ParamId(0),
                value: take(outer),
            }],
            returns: Some(CallReturn {
                destination: result,
                next: BlockId(1),
            }),
            on_failure: CleanupId(0),
        },
        3,
    );
    let runtime = RuntimeTypes {
        trace_mode: Default::default(),
        nominals: vec![nominal("A", 0, true), nominal("Outer", 1, true)],
        forget: Some(path("forget")),
        checked: None,
    };
    let e = run_runtime(vec![b.finish("main", result)], &runtime, vec![]);
    assert!(finalizers(&e).is_empty());
    assert!(matches!(&e.trace.last(), Some(TraceEvent::Return { .. })));
    assert!(e.trace.iter().any(|t| matches!(
        t,
        TraceEvent::Effect(OrderedEffect {
            effect: Effect::Forgotten { .. },
            ..
        })
    )));
}
#[test]
fn direct_mut_call_writes_back_even_on_failure() {
    for fail in [false, true] {
        let mut callee = Builder::new();
        let n = callee.parameter(TypeKind::Primitive(P::U64));
        let replacement = callee.int(P::U64, 9);
        callee.replace(n, take(replacement), 2);
        let u = callee.unit();
        let callee = if fail {
            callee.block(
                Terminator::Fail {
                    rule: const { RuleId::of("FAIL-1") },
                    cleanup: CleanupId(0),
                },
                3,
            );
            callee.definition("update")
        } else {
            callee.finish("update", u)
        };
        let mut b = Builder::new();
        let n = b.int(P::U64, 1);
        let result = b.local(TypeKind::Tuple(vec![].into()), Storage::Temporary);
        let loan = LoanId(0);
        b.emit(
            InstructionKind::BeginLoan {
                id: loan,
                place: n,
                mutable: true,
                if_inactive: false,
            },
            2,
            CleanupId(0),
        );
        let access = b.through_loan(n, loan);
        let failure = b.cleanup(vec![CleanupAction::EndLoan {
            loan: loan,
            if_active: false,
        }]);
        b.block(
            Terminator::Call {
                callee: Callee::Definition {
                    path: path("update"),
                    arguments: vec![],
                },
                arguments: vec![CallArgument {
                    parameter: ParamId(0),
                    value: Operand {
                        place: access,
                        usage: UseKind::Mut,
                    },
                }],
                returns: Some(CallReturn {
                    destination: result,
                    next: BlockId(1),
                }),
                on_failure: failure,
            },
            3,
        );
        b.emit(
            InstructionKind::EndLoan {
                loan: loan,
                if_active: false,
            },
            3,
            CleanupId(0),
        );
        let e = execute(vec![b.finish("main", n), callee], vec![], Limits::default());
        if !fail {
            assert_eq!(returned(e), integer(9));
        } else {
            assert!(matches!(e.outcome, Outcome::Failed(_)));
        }
    }
}
#[test]
fn projection_ignores_instrumentation_and_is_identical_over_one_hundred_runs() {
    let build = || {
        let mut b = Builder::new();
        let a = resource(&mut b, "A", vec![]);
        let c = b.cleanup(vec![drop(a), drop(a)]);
        let u = b.unit();
        b.block(
            Terminator::Return {
                value: take(u),
                cleanup: c,
            },
            4,
        );
        vec![b.definition("main"), finalizer("A", false)]
    };
    let runtime = RuntimeTypes {
        trace_mode: Default::default(),
        nominals: vec![nominal("A", 0, true)],
        ..RuntimeTypes::default()
    };
    let e = run_runtime(build(), &runtime, vec![]);
    let projection = e.observable(&sources()).unwrap();
    for _ in 0..100 {
        let next = run_runtime(build(), &runtime, vec![]);
        assert_eq!(next.trace, e.trace);
        assert_eq!(next.observable(&sources()).unwrap(), projection);
    }
    let mut changed = e.clone();
    changed.steps += 100;
    changed.trace.retain(|e| matches!(e, TraceEvent::Effect(_)));
    assert_eq!(changed.observable(&sources()).unwrap(), projection);
}
#[test]
fn capability_effects_are_ordered_with_resource_effects_and_exact_output() {
    struct Fake;
    impl Operations for Fake {
        fn call(
            &mut self,
            call: OperationCall<'_>,
            ctx: &mut OperationContext<'_>,
        ) -> Result<Option<OperationResult>, Error> {
            ctx.consume(CapabilityEvent {
                capability: call.path.clone(),
                operation: "read".into(),
                input: vec![0, 255],
                output: vec![1],
            })?;
            ctx.stdout.extend_from_slice(b"a\0b\n");
            Ok(Some(OperationResult::returned(Value::Unit)))
        }
    }
    let mut b = Builder::new();
    let a = resource(&mut b, "A", vec![]);
    let result = b.local(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    b.block(
        Terminator::Call {
            callee: Callee::Definition {
                path: path("cap"),
                arguments: vec![],
            },
            arguments: vec![],
            returns: Some(CallReturn {
                destination: result,
                next: BlockId(1),
            }),
            on_failure: CleanupId(0),
        },
        3,
    );
    let c = b.cleanup(vec![drop(a)]);
    b.block(
        Terminator::Return {
            value: take(result),
            cleanup: c,
        },
        4,
    );
    let e = run_with_runtime(
        &mut Provider {
            definitions: vec![b.definition("main"), finalizer("A", false)],
            queries: vec![],
        },
        &sources(),
        &RunConfiguration {
            entry: path("main"),
            hash_seed: 0,
            inputs: vec![],
        },
        vec![],
        Limits::default(),
        &mut Fake,
        &RuntimeTypes {
            trace_mode: Default::default(),
            nominals: vec![nominal("A", 0, true)],
            ..RuntimeTypes::default()
        },
    )
    .unwrap();
    let p = e.observable(&sources()).unwrap();
    assert_eq!(p.output, b"a\0b\n");
    assert_eq!(p.effects.len(), 3);
    assert!(matches!(
        p.effects[0].effect,
        Effect::ResourceCreated { .. }
    ));
    assert!(matches!(p.effects[1].effect, Effect::CapabilityConsumed(_)));
    assert!(matches!(p.effects[2].effect, Effect::Finalizer { .. }));
}
#[test]
fn instruction_failure_paths_release_temporaries_then_locals_in_list_order() {
    for kind in 0..3 {
        let mut b = Builder::new();
        let named = resource(&mut b, "Named", vec![]);
        let temporary = resource(&mut b, "Temporary", vec![]);
        let c = b.cleanup(vec![drop(temporary), drop(named)]);
        if kind == 0 {
            let f = b.constant(TypeKind::Primitive(P::Bool), Constant::Bool(false));
            b.emit(
                InstructionKind::Check {
                    condition: copy(f),
                    rule: const { RuleId::of("LANG-9") },
                },
                7,
                c,
            );
        } else if kind == 1 {
            let a = b.int(P::U8, 255);
            let one = b.int(P::U8, 1);
            b.binary(B::Add, a, one, TypeKind::Primitive(P::U8), 7, c);
        } else {
            b.emit(InstructionKind::RunCleanup(c), 7, CleanupId(0));
        }
        let u = b.unit();
        let e = run_runtime(
            vec![
                b.finish("main", u),
                finalizer("Named", false),
                finalizer("Temporary", false),
            ],
            &RuntimeTypes {
                trace_mode: Default::default(),
                nominals: vec![nominal("Named", 0, true), nominal("Temporary", 0, true)],
                ..RuntimeTypes::default()
            },
            vec![],
        );
        assert_eq!(finalizers(&e), ["Temporary", "Named"]);
        assert_eq!(matches!(e.outcome, Outcome::Failed(_)), kind < 2);
    }
}
#[test]
fn suspended_call_failure_drops_transferred_argument_before_callers_local() {
    let mut callee = Builder::new();
    let p = callee.parameter(TypeKind::Named {
        definition: path("Argument"),
        arguments: vec![].into(),
    });
    let c = callee.cleanup(vec![drop(p)]);
    callee.block(
        Terminator::Fail {
            rule: const { RuleId::of("FAIL-1") },
            cleanup: c,
        },
        8,
    );
    let mut b = Builder::new();
    let local = resource(&mut b, "Local", vec![]);
    let argument = resource(&mut b, "Argument", vec![]);
    let c = b.cleanup(vec![drop(local), drop(argument)]);
    let result = b.local(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    b.block(
        Terminator::Call {
            callee: Callee::Definition {
                path: path("callee"),
                arguments: vec![],
            },
            arguments: vec![CallArgument {
                parameter: ParamId(0),
                value: take(argument),
            }],
            returns: Some(CallReturn {
                destination: result,
                next: BlockId(1),
            }),
            on_failure: c,
        },
        3,
    );
    let e = run_runtime(
        vec![
            b.finish("main", result),
            callee.definition("callee"),
            finalizer("Local", false),
            finalizer("Argument", false),
        ],
        &RuntimeTypes {
            trace_mode: Default::default(),
            nominals: vec![nominal("Local", 0, true), nominal("Argument", 0, true)],
            ..RuntimeTypes::default()
        },
        vec![],
    );
    assert_eq!(finalizers(&e), ["Argument", "Local"]);
    assert_eq!(e.failures[0].anchor.definition, path("callee"));
}
