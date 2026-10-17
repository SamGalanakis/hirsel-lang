use crate::support;
use hirsel_interp::{aggregate::*, *};
use hirsel_tir::*;
use hirsel_types::{ParamId, Primitive as P, TypeKind};
use support::*;

struct Writeback {
    callback: Value,
    native: bool,
    invalid: bool,
}
impl Operations for Writeback {
    fn call(
        &mut self,
        call: OperationCall<'_>,
        context: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        if call.path == &path("too_deep")
            || call.path == &path("step")
            || call.path == &path("even_deeper")
        {
            return Ok(None);
        }
        if call.path == &path("observe") {
            let Value::Integer(n) = &call.arguments[0] else {
                panic!()
            };
            context
                .stdout
                .extend_from_slice(format!("{}\n", n.bits()).as_bytes());
            return Ok(Some(OperationResult::returned(Value::Unit)));
        }
        let middle = call.path == &path("middle");
        let mut callee = self.callback.clone();
        if self.native && !middle {
            let Value::FunctionReference(reference) = &mut callee else {
                panic!()
            };
            reference.definition = path("middle");
        }
        let value = if middle {
            call.arguments[0].clone()
        } else {
            let Value::Resource { fields, .. } = &call.arguments[0] else {
                panic!()
            };
            fields[0].as_ref().unwrap().clone()
        };
        Ok(Some(OperationResult::Call {
            callee,
            arguments: vec![value],
            mut_arguments: vec![0],
            writebacks: vec![CallbackWriteback {
                parameter: 0,
                argument: 0,
                fields: if middle {
                    vec![]
                } else if self.invalid {
                    vec![1]
                } else {
                    vec![0]
                },
            }],
            updates: vec![],
            owned: vec![],
            consumed: vec![],
            continuation: if middle { 1 } else { 0 },
        }))
    }
    fn resume(
        &mut self,
        token: u64,
        result: OperationResult,
        _: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        if token == 1 {
            // The explicit writeback has already published this value even if
            // this success-only continuation is skipped by failure or a limit.
            if let OperationResult::Complete { arguments, .. } = result {
                return Ok(OperationResult::Complete {
                    value: Value::Unit,
                    arguments,
                    consumed: vec![],
                    released: vec![],
                });
            }
        }
        Ok(OperationResult::returned(Value::Unit))
    }
}
#[test]
fn callback_writebacks_restore_updated_fields_on_return_failure_and_tooling_exit() {
    for native in [false, true] {
        for ending in [1, 0, 2] {
            let execution = execute_writeback(native, ending, false).unwrap();
            assert_eq!(execution.stdout, b"9\n", "native={native}, ending={ending}");
            match ending {
                0 => assert!(matches!(execution.outcome, Outcome::Returned { .. })),
                1 => assert!(
                    matches!(execution.outcome, Outcome::Failed(ref failure) if failure.rule.as_str() == "SEM-1")
                ),
                2 => assert!(matches!(execution.outcome, Outcome::ToolingLimit(_))),
                _ => unreachable!(),
            }
        }
    }
}

#[test]
fn a_callback_writeback_cannot_target_a_missing_field() {
    assert!(matches!(execute_writeback(false, 0, true), Err(error)
        if matches!(error.error, Error::InvalidTir(ref message) if message == "callback writeback field")));
}

fn execute_writeback(native: bool, ending: u8, invalid: bool) -> Result<Execution, ExecutionError> {
    let state_kind = || TypeKind::Named {
        definition: path("State"),
        arguments: vec![].into(),
    };
    let mut callback = Builder::new();
    let n = callback.borrowed_parameter(TypeKind::Primitive(P::U64));
    let nine = callback.int(P::U64, 9);
    callback.replace(n, take(nine), 5);
    let u = callback.unit();
    let callback = match ending {
        0 => callback.finish("step", u),
        1 => {
            callback.block(
                Terminator::Fail {
                    rule: const { hirsel_diag::RuleId::of("SEM-1") },
                    cleanup: CleanupId(0),
                },
                6,
            );
            callback.definition("step")
        }
        2 => {
            let result = callback.local(TypeKind::Tuple(vec![].into()), Storage::Temporary);
            callback.block(
                Terminator::Call {
                    callee: Callee::Definition {
                        path: path("too_deep"),
                        arguments: vec![],
                    },
                    arguments: vec![],
                    returns: Some(CallReturn {
                        destination: result,
                        next: BlockId(1),
                    }),
                    on_failure: CleanupId(0),
                },
                6,
            );
            callback.finish("step", result)
        }
        _ => unreachable!(),
    };
    let mut main = Builder::new();
    let state = main.local(state_kind(), Storage::Local);
    let initial = main.int(P::U64, 1);
    main.assign(
        state,
        Rvalue::Construct {
            constructor: Constructor::Record(path("State")),
            fields: vec![ConstructField {
                field: 0,
                value: take(initial),
            }],
            destination: hirsel_types::Destination::Heap,
        },
        1,
        CleanupId(0),
    );
    let unit = main.type_id(TypeKind::Tuple(vec![].into()));
    let scalar = main.type_id(TypeKind::Primitive(P::U64));
    let mut kind = function_type(unit, vec![scalar]);
    if let TypeKind::Function(function) = &mut kind {
        function.parameters[0].passing = hirsel_syntax::PassingMode::Mut;
    }
    let callback_type = main.type_id(kind);
    let result = main.slot(unit, Storage::Temporary);
    let loan = LoanId(0);
    main.emit(
        InstructionKind::BeginLoan {
            id: loan,
            place: state,
            mutable: true,
            if_inactive: false,
        },
        1,
        CleanupId(0),
    );
    let access = main.through_loan(state, loan);
    let failure = main.cleanup(vec![
        CleanupAction::EndLoan {
            loan,
            if_active: false,
        },
        drop(state),
    ]);
    main.block(
        Terminator::Call {
            callee: Callee::Definition {
                path: path("pull"),
                arguments: vec![],
            },
            arguments: vec![CallArgument {
                parameter: ParamId(0),
                value: Operand {
                    place: access,
                    usage: hirsel_types::UseKind::Mut,
                },
            }],
            returns: Some(CallReturn {
                destination: result,
                next: BlockId(1),
            }),
            on_failure: failure,
        },
        2,
    );
    main.emit(
        InstructionKind::EndLoan {
            loan,
            if_active: false,
        },
        3,
        CleanupId(0),
    );
    let main = main.finish("main", result);
    let mut finalizer = Builder::new();
    let state = finalizer.borrowed_parameter(state_kind());
    let scalar = finalizer.type_id(TypeKind::Primitive(P::U64));
    let n = finalizer.project(state, Projection::Field(0), scalar);
    let result = finalizer.local(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    finalizer.block(
        Terminator::Call {
            callee: Callee::Definition {
                path: path("observe"),
                arguments: vec![],
            },
            arguments: vec![CallArgument {
                parameter: ParamId(0),
                value: copy(n),
            }],
            returns: Some(CallReturn {
                destination: result,
                next: BlockId(1),
            }),
            on_failure: CleanupId(0),
        },
        7,
    );
    let finalizer = finalizer.finish("State_drop", result);
    let runtime = RuntimeTypes {
        trace_mode: Default::default(),
        nominals: vec![Nominal {
            definition: path("State"),
            resource: true,
            distinct: false,
            finalizer: Some((path("State_drop"), BodyId(0))),
            fields: 1,
        }],
        ..RuntimeTypes::default()
    };
    let mut ops = Writeback {
        callback: if native {
            Value::FunctionReference(hirsel_interp::value::NamedFunction {
                definition: path("step"),
                arguments: vec![],
                types: std::sync::Arc::new(main.types.clone()),
                ty: callback_type,
            })
        } else {
            Value::Closure {
                invoked: Default::default(),
                definition: path("step"),
                body: BodyId(0),
                captures: vec![],
                owned_captures: vec![],
                sizes: Default::default(),
            }
        },
        native,
        invalid,
    };
    let mut too_deep = Builder::new();
    let result = too_deep.local(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    too_deep.block(
        Terminator::Call {
            callee: Callee::Definition {
                path: path("even_deeper"),
                arguments: vec![],
            },
            arguments: vec![],
            returns: Some(CallReturn {
                destination: result,
                next: BlockId(1),
            }),
            on_failure: CleanupId(0),
        },
        8,
    );
    let mut even_deeper = Builder::new();
    let u = even_deeper.unit();
    let definitions = vec![
        main,
        callback,
        finalizer,
        too_deep.finish("too_deep", result),
        even_deeper.finish("even_deeper", u),
    ];
    assert_fixtures(&definitions, &runtime);
    run_with_runtime(
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
        Limits {
            call_depth: if ending == 2 {
                if native { 3 } else { 2 }
            } else {
                256
            },
            ..Limits::default()
        },
        &mut ops,
        &runtime,
    )
}
