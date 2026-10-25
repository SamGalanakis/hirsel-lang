//! List transfer and malformed-call controls for the synchronous completion.
use crate::support;
use hirsel_interp::{library::*, *};
use hirsel_tir::RunConfiguration;
use hirsel_types::{BuiltinType, TypeKind};

fn push(arguments: &mut [Value]) -> Result<OperationResult, Error> {
    let configuration = RunConfiguration {
        entry: support::path("main"),
        hash_seed: 42,
        inputs: vec![],
    };
    let mut library = Library::new(&configuration, Default::default()).unwrap();
    let runtime = support::runtime();
    let types = support::session();
    let sources = support::sources();
    let mut stdout = vec![];
    let mut trace = vec![];
    let result = library.call(
        OperationCall {
            runtime: &runtime,
            path: &hirsel_syntax::DefPath::core("list.push"),
            generics: &[],
            arguments,
            result_type: types.intern(support::ty(TypeKind::Tuple(vec![].into()))),
            types: &types,
            site: support::site(7),
        },
        &mut OperationContext {
            configuration: &configuration,
            sources: &sources,
            stdout: &mut stdout,
            trace: &mut trace,
            trace_mode: TraceMode::Full,
            anchor: None,
        },
    );
    assert!(stdout.is_empty());
    assert!(trace.is_empty());
    result.map(|result| result.expect("list.push is handled"))
}

#[test]
fn interp_list_cost_5_transfers_exactly_the_item_without_replacing_the_owner() {
    for item in [
        Value::Unit,
        u64_value(9),
        Value::String("owned".into()),
        Value::Handle {
            kind: "fixture-resource".into(),
            id: 7,
        },
        collection::sequence(BuiltinType::List, [Value::String("nested".into())]),
    ] {
        let original = collection::sequence(BuiltinType::List, [u64_value(1)]);
        let mut arguments = [original.clone(), item.clone(), u64_value(99)];
        assert_eq!(
            push(&mut arguments).unwrap(),
            OperationResult::Complete {
                value: Value::Unit,
                arguments: vec![],
                consumed: vec![1],
                released: vec![],
            }
        );
        assert_eq!(arguments[1], Value::Unit);
        assert_eq!(arguments[2], u64_value(99));
        assert_eq!(
            collection::elements(&arguments[0], BuiltinType::List).unwrap(),
            [u64_value(1), item]
        );
        assert_eq!(
            collection::elements(&original, BuiltinType::List).unwrap(),
            [u64_value(1)]
        );
    }
}

#[test]
fn interp_list_cost_5_missing_item_precedes_owner_validation() {
    for mut arguments in [vec![], vec![Value::Unit]] {
        let original = arguments.clone();
        assert!(
            matches!(push(&mut arguments), Err(Error::InvalidTir(message))
            if message == "missing builtin argument")
        );
        assert_eq!(arguments, original);
    }
}

#[test]
fn interp_list_cost_5_wrong_owner_keeps_the_original_taken_argument_state() {
    for owner in [
        Value::Unit,
        collection::sequence(BuiltinType::Array, []),
        Value::map(vec![]),
    ] {
        let mut arguments = [owner.clone(), Value::String("taken".into())];
        assert!(
            matches!(push(&mut arguments), Err(Error::InvalidTir(message))
            if message == "list.push requires List")
        );
        assert_eq!(arguments, [owner, Value::Unit]);
    }
}

#[test]
fn interp_list_cost_5_every_step_cut_preserves_the_mutation_trace_and_failure_site() {
    use hirsel_tir::*;
    use hirsel_types::{GenericArgument, ParamId, Primitive, UseKind};
    use support::{Builder, Provider, drop, take};

    let mut b = Builder::new();
    let string = b.type_id(TypeKind::Primitive(Primitive::String));
    let list = b.parameter(TypeKind::Builtin {
        kind: BuiltinType::List,
        arguments: vec![GenericArgument::Type(string)].into(),
    });
    let item = b.parameter(TypeKind::Primitive(Primitive::String));
    let result = b.local(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    let loan = LoanId(0);
    b.emit(
        InstructionKind::BeginLoan {
            id: loan,
            place: list,
            mutable: true,
            if_inactive: false,
        },
        2,
        CleanupId(0),
    );
    let access = b.through_loan(list, loan);
    let failure = b.cleanup(vec![
        CleanupAction::EndLoan {
            loan,
            if_active: false,
        },
        drop(item),
        drop(list),
    ]);
    b.block(
        Terminator::Call {
            callee: Callee::Definition {
                path: hirsel_syntax::DefPath::core("list.push"),
                arguments: vec![],
            },
            arguments: vec![
                CallArgument {
                    parameter: ParamId(0),
                    value: Operand {
                        place: access,
                        usage: UseKind::Mut,
                    },
                },
                CallArgument {
                    parameter: ParamId(1),
                    value: take(item),
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
            loan,
            if_active: false,
        },
        4,
        CleanupId(0),
    );
    let definition = b.finish("main", list);
    let configuration = RunConfiguration {
        entry: support::path("main"),
        hash_seed: 42,
        inputs: vec![],
    };
    let mut runtime = support::runtime();
    runtime.trace_mode = TraceMode::Full;
    support::assert_fixtures(std::slice::from_ref(&definition), &runtime);
    let snapshot = collection::sequence(BuiltinType::List, [Value::String("borrowed".into())]);
    let execute = |steps| {
        run_with_runtime(
            &mut Provider {
                definitions: vec![definition.clone()],
                queries: vec![],
            },
            &support::sources(),
            &configuration,
            vec![snapshot.clone(), Value::String("owned".into())],
            Limits {
                steps,
                call_depth: 8,
                source: None,
            },
            &mut Library::new(&configuration, Default::default()).unwrap(),
            &runtime,
        )
        .unwrap()
    };
    let full = execute(None);
    assert_eq!(
        full.outcome,
        Outcome::Returned {
            value: collection::sequence(
                BuiltinType::List,
                [
                    Value::String("borrowed".into()),
                    Value::String("owned".into())
                ]
            ),
            exit_code: 0,
        }
    );
    assert!(full.steps > 0);
    assert_eq!(execute(Some(full.steps)), full);
    for cut in 0..full.steps {
        let short = execute(Some(cut));
        let Outcome::ToolingLimit(limit) = &short.outcome else {
            panic!("cut {cut}: {short:?}")
        };
        assert_eq!(limit.kind, ToolingLimitKind::Steps);
        assert_eq!(short.steps, cut);
        let at = short
            .trace
            .iter()
            .position(|event| matches!(event, TraceEvent::ToolingLimit(_)))
            .unwrap();
        assert_eq!(short.trace[..at], full.trace[..at], "cut {cut}");
        let next = full
            .trace
            .iter()
            .filter_map(|event| match event {
                TraceEvent::Instruction { site, .. } | TraceEvent::Terminator { site, .. } => {
                    Some(*site)
                }
                _ => None,
            })
            .nth(cut as usize);
        assert_eq!(limit.site, next, "cut {cut}");
    }
    assert_eq!(
        collection::elements(&snapshot, BuiltinType::List).unwrap(),
        [Value::String("borrowed".into())]
    );
}
