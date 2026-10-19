use crate::support;
use hirsel_interp::{aggregate::*, observation::*, *};
use hirsel_tir::*;
use hirsel_types::{ParamId, Primitive as P, TypeKind};
use support::*;
struct Repeat {
    callback: Value,
    resumes: u64,
    mutating: bool,
}
impl Operations for Repeat {
    fn call(
        &mut self,
        _: OperationCall<'_>,
        _: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        Ok(Some(OperationResult::Call {
            callee: self.callback.clone(),
            arguments: vec![Value::Integer(Integer::new(P::U64, 1).unwrap())],
            mut_arguments: if self.mutating { vec![0] } else { vec![] },
            writebacks: vec![],
            updates: vec![],
            owned: vec![],
            consumed: vec![],
            continuation: 0,
        }))
    }
    fn resume(
        &mut self,
        token: u64,
        result: OperationResult,
        _arguments: &mut [Value],
        ctx: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        assert_eq!(ctx.anchor().unwrap().definition, path("main"));
        self.resumes += 1;
        if let OperationResult::Failed(_) = result {
            return Ok(OperationResult::returned(Value::Unit));
        } // Scheduler must preserve fail-stop semantics.
        if self.mutating {
            assert!(
                matches!(result,OperationResult::Complete {arguments,..} if arguments[0].1==Value::Integer(Integer::new(P::U64,9).unwrap()))
            );
        }
        if token == 999 {
            Ok(OperationResult::returned(Value::Unit))
        } else {
            Ok(OperationResult::Call {
                callee: self.callback.clone(),
                arguments: vec![Value::Integer(Integer::new(P::U64, 1).unwrap())],
                mut_arguments: if self.mutating { vec![0] } else { vec![] },
                writebacks: vec![],
                updates: vec![],
                owned: vec![],
                consumed: vec![],
                continuation: token + 1,
            })
        }
    }
}
fn fixture(failing: bool, mutating: bool) -> Definition {
    let mut callback = Builder::new();
    let n = callback.parameter(TypeKind::Primitive(P::U64));
    if mutating {
        let replacement = callback.int(P::U64, 9);
        callback.replace(n, take(replacement), 1);
    }
    let u = callback.unit();
    let d = if failing {
        callback.block(
            Terminator::Fail {
                rule: const { hirsel_diag::RuleId::of("LANG-9") },
                cleanup: CleanupId(0),
            },
            5,
        );
        callback.definition("callback")
    } else {
        callback.finish("callback", u)
    };
    let mut b = Builder::new();
    let unit = b.type_id(TypeKind::Tuple(vec![].into()));
    let scalar = b.type_id(TypeKind::Primitive(P::U64));
    let mut kind = function_type(unit, vec![scalar]);
    if mutating {
        if let TypeKind::Function(f) = &mut kind {
            f.parameters[0].passing = hirsel_syntax::PassingMode::Mut;
        }
    }
    let f = b.local(kind, Storage::Local);
    b.assign(
        f,
        Rvalue::Closure {
            body: BodyId(1),
            captures: vec![],
            destination: hirsel_types::Destination::Heap,
        },
        1,
        CleanupId(0),
    );
    let result = b.local(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    b.block(
        Terminator::Call {
            callee: Callee::Definition {
                path: path("repeat"),
                arguments: vec![],
            },
            arguments: vec![CallArgument {
                parameter: ParamId(0),
                value: take(f),
            }],
            returns: Some(CallReturn {
                destination: result,
                next: BlockId(1),
            }),
            on_failure: CleanupId(0),
        },
        2,
    );
    let mut dmain = b.finish("main", result);
    support::append_body(&mut dmain, &d);
    dmain
}
fn run_repeat(failing: bool, mutating: bool, limits: Limits) -> (Execution, u64) {
    let mut operations = Repeat {
        callback: Value::Closure {
            invoked: Default::default(),
            sizes: Default::default(),
            type_arguments: Default::default(),
            definition: path("main"),
            body: BodyId(1),
            captures: vec![],
            owned_captures: vec![],
        },
        resumes: 0,
        mutating,
    };
    let definitions = vec![fixture(failing, mutating)];
    assert_fixtures(&definitions, &RuntimeTypes::default());
    let e = run_with_operations(
        &mut Provider {
            definitions,
            queries: vec![],
        },
        &sources(),
        &RunConfiguration {
            entry: path("main"),
            hash_seed: 0,
            inputs: vec![],
        },
        vec![],
        limits,
        &mut operations,
    )
    .unwrap();
    (e, operations.resumes)
}
#[test]
fn operation_callbacks_use_frames_and_resume_a_thousand_times_with_mut_results() {
    for mutating in [false, true] {
        let (e, resumes) = run_repeat(
            false,
            mutating,
            Limits {
                steps: Some(20_000),
                call_depth: 2,
            },
        );
        assert!(matches!(e.outcome, Outcome::Returned { .. }));
        assert_eq!(resumes, 1000);
        assert_eq!(
            e.trace
                .iter()
                .filter(|e| matches!(e, TraceEvent::Enter { .. }))
                .count(),
            1001
        );
    }
}
#[test]
fn callback_failure_is_delivered_to_resume_but_cannot_be_swallowed() {
    let (e, resumes) = run_repeat(true, false, Limits::default());
    assert_eq!(resumes, 1);
    assert!(matches!(e.outcome, Outcome::Failed(_)));
    assert_eq!(e.failures.len(), 1);
    assert_eq!(e.failures[0].failure.line, 5);
}
#[test]
fn callback_step_and_depth_limits_remain_tooling_outcomes() {
    for limits in [
        Limits {
            steps: None,
            call_depth: 1,
        },
        Limits {
            steps: Some(2),
            call_depth: 20,
        },
        Limits {
            steps: Some(100),
            call_depth: 1,
        },
    ] {
        let (e, _) = run_repeat(false, false, limits);
        assert!(matches!(e.outcome, Outcome::ToolingLimit(_)));
        assert!(e.observable(&sources()).is_none());
        assert!(e.failures.is_empty());
    }
}
#[test]
fn handle_cleanup_hook_runs_high_to_low_with_the_parent_cleanup_anchor() {
    struct Hooks {
        seen: Vec<u64>,
    }
    impl Operations for Hooks {
        fn call(
            &mut self,
            _: OperationCall<'_>,
            _: &mut OperationContext<'_>,
        ) -> Result<Option<OperationResult>, Error> {
            Ok(None)
        }
        fn drop_handle(
            &mut self,
            kind: &str,
            id: u64,
            ctx: &mut OperationContext<'_>,
        ) -> Result<(), Error> {
            assert_eq!(kind, "File");
            assert!(ctx.anchor().unwrap().cleanup.is_some());
            self.seen.push(id);
            Ok(())
        }
    }
    let mut b = Builder::new();
    let p = b.parameter(TypeKind::Named {
        definition: path("Files"),
        arguments: vec![].into(),
    });
    let c = b.cleanup(vec![drop(p)]);
    let u = b.unit();
    b.block(
        Terminator::Return {
            value: take(u),
            cleanup: c,
        },
        2,
    );
    let mut hooks = Hooks { seen: vec![] };
    let e = run_with_runtime(
        &mut Provider {
            definitions: vec![b.definition("main")],
            queries: vec![],
        },
        &sources(),
        &RunConfiguration {
            entry: path("main"),
            hash_seed: 0,
            inputs: vec![],
        },
        vec![Value::Resource {
            sizes: Default::default(),
            definition: path("Files"),
            fields: vec![
                Some(Value::Handle {
                    kind: "File".into(),
                    id: 1,
                }),
                Some(Value::Handle {
                    kind: "File".into(),
                    id: 2,
                }),
            ],
        }],
        Limits::default(),
        &mut hooks,
        &RuntimeTypes::default(),
    )
    .unwrap();
    assert_eq!(hooks.seen, [2, 1]);
    let effects = e.observable(&sources()).unwrap().effects;
    assert!(matches!(
        effects[0].effect,
        Effect::HandleDropped { id: 2, .. }
    ));
    assert!(matches!(
        effects[1].effect,
        Effect::HandleDropped { id: 1, .. }
    ));
}

#[test]
fn callback_owned_temporaries_finalize_before_resume_on_success_failure_and_abandonment() {
    struct Owned {
        resumes: usize,
        native_fails: bool,
    }
    impl Operations for Owned {
        fn call(
            &mut self,
            call: OperationCall<'_>,
            _: &mut OperationContext<'_>,
        ) -> Result<Option<OperationResult>, Error> {
            if call.path == &path("native") {
                return Ok(Some(if self.native_fails {
                    OperationResult::Failed(hirsel_interp::Failure {
                        rule: const { hirsel_diag::RuleId::of("LANG-9") },
                        site: call.site,
                        line: 5,
                        kind: "native".into(),
                    })
                } else {
                    OperationResult::returned(Value::Unit)
                }));
            }
            Ok(Some(OperationResult::Call {
                callee: call.arguments[0].clone(),
                arguments: vec![call.arguments[1].clone()],
                mut_arguments: vec![],
                writebacks: vec![],
                updates: vec![],
                owned: call.arguments[1..].to_vec(),
                consumed: vec![1, 2],
                continuation: 7,
            }))
        }
        fn resume(
            &mut self,
            token: u64,
            result: OperationResult,
            _arguments: &mut [Value],
            ctx: &mut OperationContext<'_>,
        ) -> Result<OperationResult, Error> {
            assert_eq!(token, 7);
            let names = ctx
                .trace
                .iter()
                .filter_map(|t| match t {
                    TraceEvent::Effect(OrderedEffect {
                        effect: Effect::Finalizer { definition },
                        ..
                    }) => Some(definition.0.name.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(names, ["B", "A"]);
            self.resumes += 1;
            ctx.stdout.extend_from_slice(b"resume\n");
            if matches!(result, OperationResult::Failed(_)) {
                return Ok(OperationResult::returned(Value::Unit));
            }
            Ok(OperationResult::returned(Value::Unit))
        }
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
                    rule: const { hirsel_diag::RuleId::of("FAIL-1") },
                    cleanup: CleanupId(0),
                },
                8,
            );
            b.definition(&format!("{name}_drop"))
        } else {
            b.finish(&format!("{name}_drop"), u)
        }
    }
    for native in [false, true] {
        for (callback_fails, finalizer_fails, tooling) in [
            (false, false, false),
            (true, false, false),
            (false, true, false),
            (true, true, false),
            (false, false, true),
        ] {
            let mut nested = Builder::new();
            nested.borrowed_parameter(TypeKind::Named {
                definition: path("A"),
                arguments: vec![].into(),
            });
            let u = nested.unit();
            let body = if callback_fails {
                nested.block(
                    Terminator::Fail {
                        rule: const { hirsel_diag::RuleId::of("LANG-9") },
                        cleanup: CleanupId(0),
                    },
                    5,
                );
                nested.definition("body")
            } else {
                nested.finish("body", u)
            };
            let mut b = Builder::new();
            let unit = b.type_id(TypeKind::Tuple(vec![].into()));
            let a_ty = b.type_id(TypeKind::Named {
                definition: path("A"),
                arguments: vec![].into(),
            });
            let f = b.local(function_type(unit, vec![a_ty]), Storage::Local);
            b.assign(
                f,
                if native {
                    Rvalue::FunctionReference {
                        definition: path("native"),
                        arguments: vec![],
                    }
                } else {
                    Rvalue::Closure {
                        body: BodyId(1),
                        captures: vec![],
                        destination: hirsel_types::Destination::Heap,
                    }
                },
                1,
                CleanupId(0),
            );
            let mut values = vec![f];
            for name in ["A", "B"] {
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
                        fields: vec![],
                        destination: hirsel_types::Destination::Inferred,
                    },
                    1,
                    CleanupId(0),
                );
                values.push(p);
            }
            let result = b.slot(unit, Storage::Temporary);
            b.block(
                Terminator::Call {
                    callee: Callee::Definition {
                        path: path("owned"),
                        arguments: vec![],
                    },
                    arguments: values
                        .into_iter()
                        .enumerate()
                        .map(|(i, p)| CallArgument {
                            parameter: ParamId(i as u32),
                            value: take(p),
                        })
                        .collect(),
                    returns: Some(CallReturn {
                        destination: result,
                        next: BlockId(1),
                    }),
                    on_failure: CleanupId(0),
                },
                2,
            );
            let mut main = b.finish("main", result);
            if !native {
                support::append_body(&mut main, &body);
            }
            let mut ops = Owned {
                resumes: 0,
                native_fails: callback_fails,
            };
            let e = run_with_runtime(
                &mut Provider {
                    definitions: vec![main, finalizer("A", false), finalizer("B", finalizer_fails)],
                    queries: vec![],
                },
                &sources(),
                &RunConfiguration {
                    entry: path("main"),
                    hash_seed: 0,
                    inputs: vec![],
                },
                vec![],
                Limits {
                    steps: Some(100),
                    call_depth: if tooling { 1 } else { 10 },
                },
                &mut ops,
                &RuntimeTypes {
                    trace_mode: Default::default(),
                    nominals: ["A", "B"]
                        .into_iter()
                        .map(|name| Nominal {
                            definition: path(name),
                            resource: true,
                            distinct: false,
                            finalizer: Some((path(&format!("{name}_drop")), BodyId(0))),
                            fields: 0,
                            representation: None,
                        })
                        .collect(),
                    ..RuntimeTypes::default()
                },
            )
            .unwrap();
            if tooling {
                assert!(matches!(e.outcome, Outcome::ToolingLimit(_)));
                assert_eq!(ops.resumes, 0);
                assert!(e.stdout.is_empty());
            } else {
                assert_eq!(ops.resumes, 1);
                assert_eq!(e.stdout, b"resume\n");
                assert_eq!(
                    matches!(e.outcome, Outcome::Failed(_)),
                    callback_fails || finalizer_fails
                );
                assert_eq!(
                    e.failures.len(),
                    usize::from(callback_fails) + usize::from(finalizer_fails)
                );
                if callback_fails {
                    assert_eq!(e.failures[0].failure.rule.as_str(), "LANG-9");
                }
            }
            let effects = e
                .trace
                .iter()
                .filter_map(|t| match t {
                    TraceEvent::Effect(OrderedEffect {
                        effect: Effect::Finalizer { definition },
                        ..
                    }) => Some(definition.0.name.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(effects, ["B", "A"]);
        }
    }
}

#[test]
fn pre_callback_updates_remove_transferred_items_before_failure_or_abandonment_restores_mut_state()
{
    struct Pull {
        resumes: usize,
    }
    impl Operations for Pull {
        fn call(
            &mut self,
            call: OperationCall<'_>,
            _: &mut OperationContext<'_>,
        ) -> Result<Option<OperationResult>, Error> {
            let Value::Sequence { kind, elements, .. } = &call.arguments[0] else {
                panic!()
            };
            let item = elements[0].as_ref().unwrap().clone();
            Ok(Some(OperationResult::Call {
                callee: call.arguments[1].clone(),
                arguments: vec![item.clone()],
                mut_arguments: vec![],
                writebacks: vec![],
                updates: vec![(
                    0,
                    Value::Sequence {
                        kind: *kind,
                        elements: vec![None].into(),
                        arena: None,
                    },
                )],
                owned: vec![item],
                consumed: vec![],
                continuation: 11,
            }))
        }
        fn resume(
            &mut self,
            token: u64,
            _: OperationResult,
            _arguments: &mut [Value],
            ctx: &mut OperationContext<'_>,
        ) -> Result<OperationResult, Error> {
            assert_eq!(token, 11);
            assert_eq!(
                ctx.trace
                    .iter()
                    .filter(|t| matches!(
                        t,
                        TraceEvent::Effect(OrderedEffect {
                            effect: Effect::Finalizer { .. },
                            ..
                        })
                    ))
                    .count(),
                1
            );
            self.resumes += 1;
            Ok(OperationResult::returned(Value::Unit))
        }
    }
    for (failing, abandoned) in [(false, false), (true, false), (false, true)] {
        let mut nested = Builder::new();
        nested.borrowed_parameter(TypeKind::Named {
            definition: path("Item"),
            arguments: vec![].into(),
        });
        let u = nested.unit();
        let nested = if failing {
            nested.block(
                Terminator::Fail {
                    rule: const { hirsel_diag::RuleId::of("LANG-9") },
                    cleanup: CleanupId(0),
                },
                6,
            );
            nested.definition("body")
        } else {
            nested.finish("body", u)
        };
        let mut b = Builder::new();
        let item = b.type_id(TypeKind::Named {
            definition: path("Item"),
            arguments: vec![].into(),
        });
        let source = b.parameter(TypeKind::Builtin {
            kind: hirsel_types::BuiltinType::Array,
            arguments: vec![hirsel_types::GenericArgument::Type(item)].into(),
        });
        let unit = b.type_id(TypeKind::Tuple(vec![].into()));
        let f = b.local(function_type(unit, vec![item]), Storage::Local);
        b.assign(
            f,
            Rvalue::Closure {
                body: BodyId(1),
                captures: vec![],
                destination: hirsel_types::Destination::Heap,
            },
            1,
            CleanupId(0),
        );
        let loan = LoanId(0);
        b.emit(
            InstructionKind::BeginLoan {
                id: loan,
                place: source,
                mutable: true,
                if_inactive: false,
            },
            2,
            CleanupId(0),
        );
        let access = b.through_loan(source, loan);
        let failure = b.cleanup(vec![
            CleanupAction::EndLoan {
                loan: loan,
                if_active: false,
            },
            drop(f),
            drop(source),
        ]);
        let result = b.slot(unit, Storage::Temporary);
        b.block(
            Terminator::Call {
                callee: Callee::Definition {
                    path: path("pull"),
                    arguments: vec![],
                },
                arguments: vec![
                    CallArgument {
                        parameter: ParamId(0),
                        value: hirsel_tir::Operand {
                            place: access,
                            usage: hirsel_types::UseKind::Mut,
                        },
                    },
                    CallArgument {
                        parameter: ParamId(1),
                        value: take(f),
                    },
                ],
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
            4,
            CleanupId(0),
        );
        let mut main = b.finish("main", result);
        support::append_body(&mut main, &nested);
        let mut finalizer = Builder::new();
        finalizer.borrowed_parameter(TypeKind::Named {
            definition: path("Item"),
            arguments: vec![].into(),
        });
        let u = finalizer.unit();
        let finalizer = finalizer.finish("Item_drop", u);
        let mut ops = Pull { resumes: 0 };
        let runtime = RuntimeTypes {
            trace_mode: Default::default(),
            nominals: vec![Nominal {
                definition: path("Item"),
                resource: true,
                distinct: false,
                finalizer: Some((path("Item_drop"), BodyId(0))),
                fields: 0,
                representation: None,
            }],
            ..RuntimeTypes::default()
        };
        let definitions = vec![main, finalizer];
        assert_fixtures(&definitions, &runtime);
        let e = run_with_runtime(
            &mut Provider {
                definitions,
                queries: vec![],
            },
            &sources(),
            &RunConfiguration {
                entry: path("main"),
                hash_seed: 0,
                inputs: vec![],
            },
            vec![Value::Sequence {
                kind: hirsel_types::BuiltinType::Array,
                elements: vec![Some(Value::Resource {
                    sizes: Default::default(),
                    definition: path("Item"),
                    fields: vec![],
                })]
                .into(),
                arena: None,
            }],
            Limits {
                steps: Some(100),
                call_depth: if abandoned { 1 } else { 10 },
            },
            &mut ops,
            &runtime,
        )
        .unwrap();
        assert_eq!(
            e.trace
                .iter()
                .filter(|t| matches!(
                    t,
                    TraceEvent::Effect(OrderedEffect {
                        effect: Effect::Finalizer { .. },
                        ..
                    })
                ))
                .count(),
            1
        );
        assert_eq!(ops.resumes, usize::from(!abandoned));
        if abandoned {
            assert!(matches!(e.outcome, Outcome::ToolingLimit(_)));
        } else {
            assert_eq!(matches!(e.outcome, Outcome::Failed(_)), failing);
        }
    }
}

#[test]
fn nested_native_named_callbacks_resume_mut_updates_and_preserve_failure_and_depth_limits() {
    struct Native {
        reference: Value,
        failing: bool,
        calls: usize,
        resumed: Vec<u64>,
    }
    impl Operations for Native {
        fn call(
            &mut self,
            call: OperationCall<'_>,
            _: &mut OperationContext<'_>,
        ) -> Result<Option<OperationResult>, Error> {
            self.calls += 1;
            if call.path == &path("leaf") {
                return Ok(Some(if self.failing {
                    OperationResult::Failed(hirsel_interp::Failure {
                        rule: const { hirsel_diag::RuleId::of("FAIL-1") },
                        line: 1,
                        site: call.site,
                        kind: "leaf".into(),
                    })
                } else {
                    OperationResult::Complete {
                        value: Value::Unit,
                        arguments: vec![(0, Value::Integer(Integer::new(P::U64, 9).unwrap()))],
                        consumed: vec![],
                        released: vec![],
                    }
                }));
            }
            let mut callee = self.reference.clone();
            let Value::FunctionReference(reference) = &mut callee else {
                panic!()
            };
            reference.definition = if call.path == &path("outer") {
                path("middle")
            } else {
                path("leaf")
            };
            Ok(Some(OperationResult::Call {
                callee,
                arguments: vec![Value::Integer(Integer::new(P::U64, 1).unwrap())],
                mut_arguments: vec![0],
                writebacks: vec![],
                updates: vec![],
                owned: vec![],
                consumed: vec![],
                continuation: if call.path == &path("outer") { 1 } else { 2 },
            }))
        }
        fn resume(
            &mut self,
            token: u64,
            result: OperationResult,
            _arguments: &mut [Value],
            _: &mut OperationContext<'_>,
        ) -> Result<OperationResult, Error> {
            self.resumed.push(token);
            if self.failing {
                assert!(matches!(result, OperationResult::Failed(_)));
                return Ok(OperationResult::returned(Value::Unit)); // Cannot turn a failure into success.
            }
            assert!(
                matches!(result, OperationResult::Complete { ref arguments, .. } if arguments == &vec![(0, Value::Integer(Integer::new(P::U64,9).unwrap()))])
            );
            Ok(OperationResult::Complete {
                value: Value::Unit,
                arguments: if token == 2 {
                    vec![(0, Value::Integer(Integer::new(P::U64, 9).unwrap()))]
                } else {
                    vec![]
                },
                consumed: vec![],
                released: vec![],
            })
        }
    }
    for (failing, depth) in [(false, 256), (true, 256), (false, 2)] {
        let mut b = Builder::new();
        let unit = b.type_id(TypeKind::Tuple(vec![].into()));
        let scalar = b.type_id(TypeKind::Primitive(P::U64));
        let mut kind = function_type(unit, vec![scalar]);
        if let TypeKind::Function(function) = &mut kind {
            function.parameters[0].passing = hirsel_syntax::PassingMode::Mut;
        }
        let reference = b.type_id(kind);
        let result = b.slot(unit, Storage::Temporary);
        b.block(
            Terminator::Call {
                callee: Callee::Definition {
                    path: path("outer"),
                    arguments: vec![],
                },
                arguments: vec![],
                returns: Some(CallReturn {
                    destination: result,
                    next: BlockId(1),
                }),
                on_failure: CleanupId(0),
            },
            1,
        );
        let definition = b.finish("main", result);
        let mut operations = Native {
            reference: Value::FunctionReference(hirsel_interp::value::NamedFunction {
                definition: path("middle"),
                arguments: vec![],
                types: std::sync::Arc::new(definition.types.clone()),
                ty: reference,
            }),
            failing,
            calls: 0,
            resumed: vec![],
        };
        let mut provider = Provider {
            definitions: vec![definition],
            queries: vec![],
        };
        let execution = run_with_operations(
            &mut provider,
            &sources(),
            &RunConfiguration {
                entry: path("main"),
                inputs: vec![],
                hash_seed: 0,
            },
            vec![],
            Limits {
                call_depth: depth,
                ..Limits::default()
            },
            &mut operations,
        )
        .unwrap();
        assert_eq!(provider.queries, [path("main")]); // Native names never request forwarding bodies.
        if depth == 2 {
            assert!(matches!(execution.outcome, Outcome::ToolingLimit(_)));
            assert_eq!(operations.calls, 2);
            assert!(operations.resumed.is_empty());
        } else {
            assert_eq!(operations.calls, 3);
            assert_eq!(operations.resumed, [2, 1]);
            if failing {
                assert!(matches!(execution.outcome, Outcome::Failed(_)));
                assert_eq!(execution.failures.len(), 1);
            } else {
                assert!(matches!(execution.outcome, Outcome::Returned { .. }));
            }
        }
    }
}

#[test]
fn blocked_stop_retains_the_prefix_and_runs_no_language_cleanup() {
    struct Block {
        drops: usize,
    }
    impl Operations for Block {
        fn call(
            &mut self,
            _: OperationCall<'_>,
            cx: &mut OperationContext<'_>,
        ) -> Result<Option<OperationResult>, Error> {
            cx.stdout.extend_from_slice(b"prefix");
            cx.consume(CapabilityEvent {
                capability: path("Net"),
                operation: "confirmed-prefix".into(),
                input: vec![],
                output: vec![],
            })?;
            Err(Error::Blocked(vec![BlockedWait {
                spawn_path: vec![],
                anchor: cx.anchor().unwrap().clone(),
                kind: "net.read".into(),
                targets: vec![1],
            }]))
        }
        fn drop_handle(
            &mut self,
            _: &str,
            _: u64,
            _: &mut OperationContext<'_>,
        ) -> Result<(), Error> {
            self.drops += 1;
            Ok(())
        }
    }
    let mut b = Builder::new();
    let handle = b.parameter(TypeKind::Named {
        definition: path("Files"),
        arguments: vec![].into(),
    });
    let cleanup = b.cleanup(vec![drop(handle)]);
    let unit = b.unit();
    let result = b.local(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    b.block(
        Terminator::Call {
            callee: Callee::Definition {
                path: path("block"),
                arguments: vec![],
            },
            arguments: vec![],
            returns: Some(CallReturn {
                destination: result,
                next: BlockId(1),
            }),
            on_failure: cleanup,
        },
        7,
    );
    b.block(
        Terminator::Return {
            value: take(unit),
            cleanup,
        },
        8,
    );
    let mut operations = Block { drops: 0 };
    let sources = sources();
    let mut e = run_with_runtime(
        &mut Provider {
            definitions: vec![b.definition("main")],
            queries: vec![],
        },
        &sources,
        &RunConfiguration {
            entry: path("main"),
            hash_seed: 0,
            inputs: vec![],
        },
        vec![Value::Resource {
            sizes: Default::default(),
            definition: path("Files"),
            fields: vec![Some(Value::Handle {
                kind: "Stream".into(),
                id: 1,
            })],
        }],
        Limits::default(),
        &mut operations,
        &aggregate::RuntimeTypes {
            nominals: vec![aggregate::Nominal {
                definition: path("Files"),
                resource: true,
                distinct: false,
                finalizer: None,
                fields: 1,
                representation: None,
            }],
            ..Default::default()
        },
    )
    .unwrap();
    assert!(matches!(e.outcome, Outcome::Blocked(_)));
    assert!(e.failures.is_empty());
    assert_eq!(operations.drops, 0);
    assert_eq!(e.stdout, b"prefix");
    let observation = e.observable(&sources).unwrap();
    let waits = observation.blocked.unwrap();
    assert_eq!(waits.len(), 1);
    assert_eq!(waits[0].site.line, 7);
    assert_eq!(waits[0].site.definition, path("main"));
    assert_eq!(waits[0].targets, vec![1]);
    assert_eq!(observation.effects.len(), 1);
    let Outcome::Blocked(original) = &e.outcome else {
        unreachable!()
    };
    let mut later = original[0].clone();
    later.spawn_path = vec![2];
    let mut earlier = later.clone();
    earlier.spawn_path = vec![1];
    e.outcome = Outcome::Blocked(vec![later, earlier]);
    let ordered = e.observable(&sources).unwrap().blocked.unwrap();
    assert_eq!(
        ordered
            .iter()
            .map(|w| w.spawn_path.clone())
            .collect::<Vec<_>>(),
        [vec![1], vec![2]]
    );
}
