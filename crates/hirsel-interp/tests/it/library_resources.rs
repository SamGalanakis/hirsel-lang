//! Operations own their generated/pulled temporaries; callback parameters borrow.
use crate::support;
use hirsel_interp::{
    library::{collection, *},
    *,
};
use hirsel_tir::*;
use hirsel_types::*;
use support::*;
#[derive(Clone, Copy, Debug)]
enum Ending {
    Return,
    Fail,
    Loop,
}
fn nominal(name: &str) -> TypeKind {
    TypeKind::Named {
        definition: support::path(name),
        arguments: vec![].into(),
    }
}
fn closure(name: &str) -> Value {
    Value::Closure {
        invoked: Default::default(),
        sizes: Default::default(),
        type_arguments: Default::default(),
        definition: support::path(name),
        body: BodyId(0),
        captures: vec![],
        owned_captures: vec![],
    }
}
fn resource() -> Value {
    Value::Resource {
        sizes: Default::default(),
        definition: support::path("R"),
        fields: vec![],
    }
}
fn finalizer() -> Definition {
    let mut b = Builder::new();
    b.borrowed_parameter(nominal("R"));
    let u = b.unit();
    b.finish("R_drop", u)
}
fn runtime() -> aggregate::RuntimeTypes {
    aggregate::RuntimeTypes {
        trace_mode: Default::default(),
        nominals: vec![aggregate::Nominal {
            definition: support::path("R"),
            resource: true,
            distinct: false,
            finalizer: Some((support::path("R_drop"), BodyId(0))),
            fields: 0,
            representation: None,
        }],
        forget: None,
        checked: None,
        serialization: Default::default(),
        budgets: Default::default(),
    }
}
fn callback(name: &str, end: Ending, boolean: bool) -> Definition {
    let mut b = Builder::new();
    b.borrowed_parameter(nominal("R"));
    match end {
        Ending::Return => {
            let v = if boolean {
                b.constant(TypeKind::Primitive(Primitive::Bool), Constant::Bool(true))
            } else {
                b.int(Primitive::U64, 42)
            };
            b.finish(name, v)
        }
        Ending::Fail => {
            b.body.result = b.type_id(TypeKind::Primitive(if boolean {
                Primitive::Bool
            } else {
                Primitive::U64
            }));
            b.block(
                Terminator::Fail {
                    rule: const { hirsel_diag::RuleId::of("LANG-9") },
                    cleanup: CleanupId(0),
                },
                5,
            );
            b.definition(name)
        }
        Ending::Loop => {
            b.body.result = b.type_id(TypeKind::Primitive(if boolean {
                Primitive::Bool
            } else {
                Primitive::U64
            }));
            b.block(Terminator::Goto(BlockId(0)), 5);
            b.definition(name)
        }
    }
}
fn finals(execution: &Execution) -> Vec<usize> {
    execution.trace.iter().enumerate().filter_map(|(i,event)|if matches!(event,TraceEvent::Effect(observation::OrderedEffect {effect:observation::Effect::Finalizer {definition},..}) if *definition==support::path("R")){Some(i)}else{None}).collect()
}
#[test]
fn pulled_resource_mapper_finalizes_once_and_failed_saved_iterator_does_not_own_it_again() {
    mapper_cases(true);
}
#[test]
fn dynamically_pulled_resource_finalizes_after_mapper_success_failure_and_abandonment() {
    mapper_cases(false);
}
fn mapper_cases(materialized: bool) {
    for end in [Ending::Return, Ending::Fail, Ending::Loop] {
        let run = RunConfiguration {
            entry: support::path("main"),
            hash_seed: 42,
            inputs: vec![],
        };
        let mut library = Library::new(&run, Default::default()).unwrap();
        let mut types = TypeStore::default();
        let dummy = types.push(ty(TypeKind::Tuple(vec![].into())));
        let sources = sources();
        let mut stdout = vec![];
        let mut trace = vec![];
        let mut next = Builder::new();
        next.borrowed_parameter(TypeKind::Primitive(Primitive::U64));
        let r = next.local(nominal("R"), Storage::Temporary);
        next.assign(
            r,
            Rvalue::Construct {
                constructor: Constructor::Record(support::path("R")),
                fields: vec![],
                destination: Destination::Inferred,
            },
            3,
            CleanupId(0),
        );
        let r_ty = next.body.places[r.0 as usize].ty;
        let result = next.local(
            TypeKind::Named {
                definition: hirsel_syntax::DefPath::core("Option"),
                arguments: vec![GenericArgument::Type(r_ty)].into(),
            },
            Storage::Temporary,
        );
        next.assign(
            result,
            Rvalue::Construct {
                constructor: Constructor::Variant {
                    definition: hirsel_syntax::DefPath::core("Option"),
                    variant: 1,
                },
                fields: vec![ConstructField {
                    field: 0,
                    value: take(r),
                }],
                destination: Destination::Inferred,
            },
            3,
            CleanupId(0),
        );
        let next = next.finish("next", result);
        let it = if materialized {
            collection::sequence(BuiltinType::Iter, [resource()])
        } else {
            let OperationResult::Complete { value: it, .. } = library
                .call(
                    OperationCall {
                        runtime: &Default::default(),
                        path: &hirsel_syntax::DefPath::core("iter.from"),
                        arguments: &mut [u64_value(0), closure("next")],
                        generics: &[],
                        result_type: dummy,
                        types: &types,
                        site: site(7),
                    },
                    &mut OperationContext {
                        configuration: &run,
                        sources: &sources,
                        stdout: &mut stdout,
                        trace: &mut trace,
                        trace_mode: Default::default(),
                        anchor: None,
                    },
                )
                .unwrap()
                .unwrap()
            else {
                panic!()
            };
            it
        };
        let OperationResult::Complete { value: it, .. } = library
            .call(
                OperationCall {
                    runtime: &Default::default(),
                    path: &hirsel_syntax::DefPath::core("iter.map"),
                    arguments: &mut [it, closure("mapper")],
                    generics: &[],
                    result_type: dummy,
                    types: &types,
                    site: site(7),
                },
                &mut OperationContext {
                    configuration: &run,
                    sources: &sources,
                    stdout: &mut stdout,
                    trace: &mut trace,
                    trace_mode: Default::default(),
                    anchor: None,
                },
            )
            .unwrap()
            .unwrap()
        else {
            panic!()
        };
        let mut b = Builder::new();
        let n = b.type_id(TypeKind::Primitive(Primitive::U64));
        let it_slot = b.parameter(TypeKind::Builtin {
            kind: BuiltinType::Iter,
            arguments: vec![GenericArgument::Type(n)].into(),
        });
        let cleanup = b.cleanup(vec![support::drop(it_slot)]);
        let result = b.local(
            TypeKind::Named {
                definition: hirsel_syntax::DefPath::core("Option"),
                arguments: vec![GenericArgument::Type(n)].into(),
            },
            Storage::Temporary,
        );
        b.block(
            Terminator::Call {
                callee: Callee::Definition {
                    path: hirsel_syntax::DefPath::core("iter.next"),
                    arguments: vec![],
                },
                arguments: vec![CallArgument {
                    parameter: ParamId(0),
                    value: Operand {
                        place: it_slot,
                        usage: UseKind::Mut,
                    },
                }],
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
                value: support::take(result),
                cleanup,
            },
            8,
        );
        let mut runtime = runtime();
        runtime.nominals.push(aggregate::Nominal {
            definition: hirsel_syntax::DefPath::core("Option"),
            resource: false,
            distinct: false,
            finalizer: None,
            fields: 1,
            representation: None,
        });
        let e = run_with_runtime(
            &mut Provider {
                definitions: vec![
                    b.definition("main"),
                    callback("mapper", end, false),
                    finalizer(),
                    next,
                ],
                queries: vec![],
            },
            &sources,
            &run,
            vec![it],
            Limits {
                steps: Some(1000),
                call_depth: 3,
            },
            &mut library,
            &runtime,
        )
        .unwrap();
        assert_eq!(
            finals(&e).len(),
            1,
            "owned input must never reappear in saved iterator state: {end:?}"
        );
        match end {
            Ending::Return => assert!(matches!(e.outcome, Outcome::Returned { .. })),
            Ending::Fail => assert!(matches!(e.outcome, Outcome::Failed(_))),
            Ending::Loop => assert!(matches!(e.outcome, Outcome::ToolingLimit(_))),
        }
    }
}
