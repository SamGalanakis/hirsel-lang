//! Actual reference execution of independently validated resource programs.
use crate::support::*;
use hirsel_interp::aggregate::{Nominal, RuntimeTypes};
use hirsel_interp::ownership_trace::{
    ResourceExecution, ResourceRequest, TraceError, run_resources,
};
use hirsel_interp::{Limits, NoOperations, Outcome, run_with_runtime};
use hirsel_run_schema::ownership_trace::{Action, Storage as TraceStorage};
use hirsel_syntax::DefId;
use hirsel_tir::*;
use hirsel_types::{Destination, ParamId, TypeKind};

fn kind() -> TypeKind {
    TypeKind::Named {
        definition: DefId::intern(&path("Token")),
        arguments: vec![].into(),
    }
}
fn token(builder: &mut Builder) -> PlaceId {
    let value = builder.local(kind(), Storage::Local);
    builder.assign(
        value,
        Rvalue::Construct {
            constructor: Constructor::Record(path("Token")),
            fields: vec![],
            destination: Destination::Inferred,
        },
        2,
        CleanupId(0),
    );
    value
}
fn finalizer(fail: bool) -> Definition {
    let mut builder = Builder::new();
    builder.borrowed_parameter(kind());
    let value = builder.unit();
    if fail {
        builder.block(
            Terminator::Fail {
                rule: const { hirsel_diag::RuleId::of("FAIL-1") },
                cleanup: CleanupId(0),
            },
            9,
        );
        builder.definition("Token_drop")
    } else {
        builder.finish("Token_drop", value)
    }
}
fn runtime_types() -> RuntimeTypes {
    RuntimeTypes {
        nominals: vec![Nominal {
            definition: path("Token"),
            resource: true,
            distinct: false,
            finalizer: Some((path("Token_drop"), BodyId(0))),
            fields: 0,
            representation: None,
        }],
        forget: Some(path("forget")),
        ..runtime()
    }
}
fn observed(definitions: Vec<Definition>, runtime: &RuntimeTypes) -> ResourceExecution {
    assert_fixtures(&definitions, runtime);
    let configuration = RunConfiguration {
        entry: definitions[0].path.path().clone(),
        hash_seed: 49,
        inputs: vec![],
    };
    let ordinary = run_with_runtime(
        &mut Provider {
            definitions: definitions.clone(),
            queries: vec![],
        },
        &sources(),
        &configuration,
        vec![],
        Limits::default(),
        &mut NoOperations,
        runtime,
    )
    .unwrap();
    let traced = run_resources(
        &mut Provider {
            definitions,
            queries: vec![],
        },
        &sources(),
        ResourceRequest {
            configuration: &configuration,
            program: [9; 32],
            event_limit: 100,
        },
        vec![],
        Limits::default(),
        &mut NoOperations,
        runtime,
    )
    .unwrap();
    assert_eq!(
        traced.execution, ordinary,
        "observation changed the actual execution"
    );
    traced
}

fn call(builder: &mut Builder, name: &str, value: Operand, result: TypeKind) -> PlaceId {
    let destination = builder.local(result, Storage::Temporary);
    builder.block(
        Terminator::Call {
            callee: Callee::Definition {
                path: path(name),
                arguments: vec![],
            },
            arguments: vec![CallArgument {
                parameter: ParamId(0),
                value,
            }],
            returns: Some(CallReturn {
                destination,
                next: BlockId(builder.body.blocks.len() as u32 + 1),
            }),
            on_failure: CleanupId(0),
        },
        3,
    );
    destination
}

#[test]
fn ownership_reference_calls_and_returns_preserve_the_same_resource_token() {
    let mut identity = Builder::new();
    let parameter = identity.parameter(kind());
    let identity = identity.finish("identity", parameter);
    let mut builder = Builder::new();
    let first = token(&mut builder);
    let second = call(&mut builder, "identity", take(first), kind());
    call(&mut builder, "identity", take(second), kind());
    let result = builder.unit();
    let traced = observed(
        vec![builder.finish("main", result), identity, finalizer(false)],
        &runtime_types(),
    );
    let trace = traced.trace.unwrap();
    let analysis = trace.analyze([9; 32], 100).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (1, 1, 0)
    );
    let tokens: std::collections::BTreeSet<_> =
        trace.events.iter().map(|e| e.action.token()).collect();
    assert_eq!(tokens.len(), 1);
    assert_eq!(
        trace
            .events
            .iter()
            .filter(|e| matches!(e.action, Action::Move { .. }))
            .count(),
        6
    );
}

#[test]
fn ownership_reference_borrowed_call_keeps_custody_with_the_caller() {
    let mut inspect = Builder::new();
    inspect.borrowed_parameter(kind());
    let result = inspect.unit();
    let inspect = inspect.finish("inspect", result);
    let mut builder = Builder::new();
    let resource = token(&mut builder);
    let loan = LoanId(0);
    builder.emit(
        InstructionKind::BeginLoan {
            id: loan,
            place: resource,
            mutable: false,
            if_inactive: false,
        },
        3,
        CleanupId(0),
    );
    let access = builder.through_loan(resource, loan);
    builder.body.set_cleanup(
        CleanupId(0),
        vec![
            CleanupAction::EndLoan {
                loan,
                if_active: true,
            },
            drop(resource),
        ],
    );
    let result = call(
        &mut builder,
        "inspect",
        Operand {
            place: access,
            usage: hirsel_types::UseKind::Borrow,
        },
        TypeKind::Tuple(vec![].into()),
    );
    builder.emit(
        InstructionKind::EndLoan {
            loan,
            if_active: false,
        },
        4,
        CleanupId(0),
    );
    let traced = observed(
        vec![builder.finish("main", result), inspect, finalizer(false)],
        &runtime_types(),
    );
    let trace = traced.trace.unwrap();
    let analysis = trace.analyze([9; 32], 100).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (1, 1, 0)
    );
    // Only cleanup custody and finalizer entry move the token; borrowing does not.
    assert_eq!(
        trace
            .events
            .iter()
            .filter(|e| matches!(e.action, Action::Move { .. }))
            .count(),
        2
    );
}

#[test]
fn ownership_reference_failed_callee_cleans_its_actual_taken_resource() {
    let mut fail = Builder::new();
    fail.parameter(kind());
    fail.block(
        Terminator::Fail {
            rule: const { hirsel_diag::RuleId::of("FAIL-1") },
            cleanup: CleanupId(0),
        },
        9,
    );
    let fail = fail.definition("fail");
    let mut builder = Builder::new();
    let resource = token(&mut builder);
    let result = call(
        &mut builder,
        "fail",
        take(resource),
        TypeKind::Tuple(vec![].into()),
    );
    let traced = observed(
        vec![builder.finish("main", result), fail, finalizer(false)],
        &runtime_types(),
    );
    assert!(matches!(traced.execution.outcome, Outcome::Failed(_)));
    let analysis = traced.trace.unwrap().analyze([9; 32], 100).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (1, 1, 0)
    );
}

#[test]
fn ownership_reference_equal_resources_remain_distinct_across_separate_calls() {
    let mut consume = Builder::new();
    consume.parameter(kind());
    let result = consume.unit();
    let consume = consume.finish("consume", result);
    let mut builder = Builder::new();
    let first = token(&mut builder);
    let second = token(&mut builder);
    call(
        &mut builder,
        "consume",
        take(first),
        TypeKind::Tuple(vec![].into()),
    );
    let result = call(
        &mut builder,
        "consume",
        take(second),
        TypeKind::Tuple(vec![].into()),
    );
    let traced = observed(
        vec![builder.finish("main", result), consume, finalizer(false)],
        &runtime_types(),
    );
    let analysis = traced.trace.unwrap().analyze([9; 32], 100).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (2, 2, 0)
    );
}

#[test]
fn ownership_reference_resource_without_user_finalizer_still_ends_its_lifetime() {
    let mut runtime = runtime_types();
    runtime.nominals[0].finalizer = None;
    let mut builder = Builder::new();
    token(&mut builder);
    let result = builder.unit();
    let traced = observed(vec![builder.finish("main", result)], &runtime);
    let trace = traced.trace.unwrap();
    let analysis = trace.analyze([9; 32], 100).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (1, 1, 0)
    );
    assert_eq!(trace.events.len(), 3);
}

#[test]
fn ownership_reference_callee_can_create_a_resource_and_return_it_to_its_actual_caller() {
    let mut runtime = runtime_types();
    runtime.nominals[0].finalizer = None;
    let mut create = Builder::new();
    create.parameter(TypeKind::Tuple(vec![].into()));
    let resource = token(&mut create);
    let create = create.finish("create", resource);
    let mut builder = Builder::new();
    let argument = builder.unit();
    call(&mut builder, "create", take(argument), kind());
    let result = builder.unit();
    let traced = observed(vec![builder.finish("main", result), create], &runtime);
    let trace = traced.trace.unwrap();
    let analysis = trace.analyze([9; 32], 100).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (1, 1, 0)
    );
    assert_eq!(trace.events.len(), 4);
}

#[test]
fn ownership_reference_and_checked_h_agree_on_actual_call_and_cleanup_lineage() {
    let mut identity = Builder::new();
    let parameter = identity.parameter(kind());
    let identity = identity.finish("identity", parameter);
    let mut builder = Builder::new();
    let resource = token(&mut builder);
    call(&mut builder, "identity", take(resource), kind());
    let result = builder.unit();
    let definitions = vec![builder.finish("main", result), identity, finalizer(false)];
    let mut runtime = runtime_types();
    let reference = observed(definitions.clone(), &runtime);
    runtime.checked = Some(hirsel_interp::checked::CheckedRuntime {
        operations: fixture_signatures(&definitions, &runtime)
            .into_iter()
            .map(|(path, signature)| (path, std::sync::Arc::new(signature)))
            .collect(),
        ..Default::default()
    });
    let checked = observed(definitions, &runtime);
    assert_eq!(reference.execution.outcome, checked.execution.outcome);
    let reference = reference.trace.unwrap();
    let checked = checked.trace.unwrap();
    assert_eq!(reference, checked);
    let analysis = checked.analyze([9; 32], 100).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (1, 1, 0)
    );
}

#[test]
fn ownership_reference_equal_pointer_free_resources_are_created_and_dropped_individually() {
    let mut builder = Builder::new();
    token(&mut builder);
    token(&mut builder);
    let result = builder.unit();
    let result = observed(
        vec![builder.finish("main", result), finalizer(false)],
        &runtime_types(),
    );
    let trace = result.trace.unwrap();
    let analysis = trace.analyze([9; 32], 100).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (2, 2, 0)
    );
    let creates: Vec<_> = trace
        .events
        .iter()
        .filter_map(|e| match &e.action {
            Action::Create { token, .. } => Some(token),
            _ => None,
        })
        .collect();
    assert_ne!(creates[0], creates[1]);
}

#[test]
fn ownership_reference_exact_local_move_chain_reaches_the_real_finalizer() {
    let mut builder = Builder::new();
    let first = token(&mut builder);
    let second = builder.local(kind(), Storage::Local);
    builder.assign(second, Rvalue::Use(take(first)), 3, CleanupId(0));
    let third = builder.local(kind(), Storage::Local);
    builder.assign(third, Rvalue::Use(take(second)), 4, CleanupId(0));
    let result = builder.unit();
    let traced = observed(
        vec![builder.finish("main", result), finalizer(false)],
        &runtime_types(),
    );
    let trace = traced.trace.unwrap();
    let analysis = trace.analyze([9; 32], 100).unwrap();
    assert_eq!((analysis.created, analysis.dropped), (1, 1));
    let actions: Vec<_> = trace.events.iter().map(|e| &e.action).collect();
    assert!(matches!(actions.first(), Some(Action::Create { .. })));
    assert!(matches!(actions.last(), Some(Action::Drop { .. })));
    assert!(
        trace
            .events
            .iter()
            .filter(|e| matches!(e.action, Action::Move { .. }))
            .count()
            >= 4
    );
}

#[test]
fn ownership_reference_forgotten_resource_is_live_at_scope_exit() {
    let mut builder = Builder::new();
    let resource = token(&mut builder);
    let result = builder.local(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    builder.block(
        Terminator::Call {
            callee: Callee::Definition {
                path: path("forget"),
                arguments: vec![],
            },
            arguments: vec![CallArgument {
                parameter: ParamId(0),
                value: take(resource),
            }],
            returns: Some(CallReturn {
                destination: result,
                next: BlockId(1),
            }),
            on_failure: CleanupId(0),
        },
        3,
    );
    let traced = observed(
        vec![builder.finish("main", result), finalizer(false)],
        &runtime_types(),
    );
    let trace = traced.trace.unwrap();
    let analysis = trace.analyze([9; 32], 100).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (1, 0, 1)
    );
    assert!(matches!(
        analysis.live[0].owner.as_ref().unwrap().storage,
        TraceStorage::Forgotten { .. }
    ));
}

#[test]
fn ownership_reference_failed_finalizer_still_finishes_that_resource_lifetime() {
    let mut builder = Builder::new();
    token(&mut builder);
    let result = builder.unit();
    let traced = observed(
        vec![builder.finish("main", result), finalizer(true)],
        &runtime_types(),
    );
    assert!(matches!(traced.execution.outcome, Outcome::Failed(_)));
    let analysis = traced.trace.unwrap().analyze([9; 32], 100).unwrap();
    assert_eq!((analysis.created, analysis.dropped), (1, 1));
}

#[test]
fn ownership_reference_tuple_fields_retain_their_existing_resource_identity() {
    let mut builder = Builder::new();
    let inner = token(&mut builder);
    let scalar = builder.type_id(kind());
    let outer = builder.local(TypeKind::Tuple(vec![scalar].into()), Storage::Local);
    builder.assign(
        outer,
        Rvalue::Construct {
            constructor: Constructor::Tuple,
            fields: vec![ConstructField {
                field: 0,
                value: take(inner),
            }],
            destination: Destination::Inferred,
        },
        4,
        CleanupId(0),
    );
    let result = builder.unit();
    let traced = observed(
        vec![builder.finish("main", result), finalizer(false)],
        &runtime_types(),
    );
    let analysis = traced.trace.unwrap().analyze([9; 32], 100).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (1, 1, 0)
    );
}

#[test]
fn ownership_reference_replacement_preserves_both_resource_lifetimes() {
    let mut builder = Builder::new();
    let first = token(&mut builder);
    let second = token(&mut builder);
    builder.replace(first, take(second), 4);
    let result = builder.unit();
    let traced = observed(
        vec![builder.finish("main", result), finalizer(false)],
        &runtime_types(),
    );
    let trace = traced.trace.unwrap();
    let analysis = trace.analyze([9; 32], 100).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (2, 2, 0)
    );
    let displaced = trace
        .events
        .iter()
        .find_map(|event| match &event.action {
            Action::Move { token, from, owner } if event.site.line == 4 => {
                Some((token.clone(), from.clone(), owner.clone()))
            }
            _ => None,
        })
        .unwrap();
    assert_ne!(displaced.1, displaced.2);
    assert!(trace.events.iter().any(|event| matches!(&event.action,
        Action::Move { token, owner, .. } if token != &displaced.0 && owner == &displaced.1)));
    assert!(trace.events.iter().any(|event| matches!(&event.action,
        Action::Move { token, from, .. } if token == &displaced.0 && from == &displaced.2)));
}

fn tuple(builder: &mut Builder, fields: &[PlaceId]) -> PlaceId {
    let types = fields
        .iter()
        .map(|p| builder.body.places[p.0 as usize].ty)
        .collect::<Vec<_>>();
    let destination = builder.local(TypeKind::Tuple(types.into()), Storage::Local);
    builder.assign(
        destination,
        Rvalue::Construct {
            constructor: Constructor::Tuple,
            fields: fields
                .iter()
                .enumerate()
                .map(|(i, p)| ConstructField {
                    field: i as u32,
                    value: take(*p),
                })
                .collect(),
            destination: Destination::Inferred,
        },
        4,
        CleanupId(0),
    );
    destination
}

#[test]
fn ownership_reference_nested_tuple_moves_and_partial_take_drop_each_leaf_once() {
    let mut builder = Builder::new();
    let first = token(&mut builder);
    let second = token(&mut builder);
    let inner = tuple(&mut builder, &[first, second]);
    let outer = tuple(&mut builder, &[inner]);
    let inner_type = builder.body.places[inner.0 as usize].ty;
    let token_type = builder.type_id(kind());
    let projected = builder.project(outer, Projection::Field(0), inner_type);
    let projected = builder.project(projected, Projection::Field(1), token_type);
    let destination = builder.local(kind(), Storage::Local);
    builder.assign(destination, Rvalue::Use(take(projected)), 5, CleanupId(0));
    let result = builder.unit();
    let traced = observed(
        vec![builder.finish("main", result), finalizer(false)],
        &runtime_types(),
    );
    let trace = traced.trace.unwrap();
    let analysis = trace.analyze([9; 32], 100).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (2, 2, 0)
    );
    assert!(
        trace.events.iter().any(
            |event| matches!(&event.action, Action::Move { from, .. } if from.path.len() == 2)
        )
    );
}

#[test]
fn ownership_reference_aggregate_call_and_return_move_all_descendant_tokens() {
    let mut builder = Builder::new();
    let first = token(&mut builder);
    let second = token(&mut builder);
    let aggregate = tuple(&mut builder, &[first, second]);
    let token_type = builder.type_id(kind());
    let tuple_type = TypeKind::Tuple(vec![token_type, token_type].into());
    let mut identity = Builder::new();
    let parameter = identity.parameter(tuple_type.clone());
    let identity = identity.finish("identity", parameter);
    call(&mut builder, "identity", take(aggregate), tuple_type);
    let result = builder.unit();
    let traced = observed(
        vec![builder.finish("main", result), identity, finalizer(false)],
        &runtime_types(),
    );
    let analysis = traced.trace.unwrap().analyze([9; 32], 100).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (2, 2, 0)
    );
}

#[test]
fn ownership_reference_parent_finalizer_finishes_before_each_real_child_finalizer() {
    for fail in [false, true] {
        let mut builder = Builder::new();
        let first = token(&mut builder);
        let second = token(&mut builder);
        let parent_kind = TypeKind::Named {
            definition: DefId::intern(&path("Parent")),
            arguments: vec![].into(),
        };
        let parent = builder.local(parent_kind.clone(), Storage::Local);
        builder.assign(
            parent,
            Rvalue::Construct {
                constructor: Constructor::Record(path("Parent")),
                fields: vec![
                    ConstructField {
                        field: 0,
                        value: take(first),
                    },
                    ConstructField {
                        field: 1,
                        value: take(second),
                    },
                ],
                destination: Destination::Inferred,
            },
            4,
            CleanupId(0),
        );
        let mut parent_drop = Builder::new();
        parent_drop.borrowed_parameter(parent_kind);
        let result = parent_drop.unit();
        let parent_drop = if fail {
            parent_drop.block(
                Terminator::Fail {
                    rule: const { hirsel_diag::RuleId::of("FAIL-1") },
                    cleanup: CleanupId(0),
                },
                9,
            );
            parent_drop.definition("Parent_drop")
        } else {
            parent_drop.finish("Parent_drop", result)
        };
        let mut runtime = runtime_types();
        runtime.nominals.push(Nominal {
            definition: path("Parent"),
            resource: true,
            distinct: false,
            finalizer: Some((path("Parent_drop"), BodyId(0))),
            fields: 2,
            representation: None,
        });
        let result = builder.unit();
        let traced = observed(
            vec![
                builder.finish("main", result),
                parent_drop,
                finalizer(false),
            ],
            &runtime,
        );
        assert_eq!(matches!(traced.execution.outcome, Outcome::Failed(_)), fail);
        let trace = traced.trace.unwrap();
        let analysis = trace.analyze([9; 32], 100).unwrap();
        assert_eq!(
            (analysis.created, analysis.dropped, analysis.live.len()),
            (3, 3, 0)
        );
        let drops = trace
            .events
            .iter()
            .filter_map(|event| match &event.action {
                Action::Drop { token, .. } => Some(token.sequence),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            drops,
            [2, 1, 0],
            "parent then reversed child declaration order"
        );
    }
}

#[test]
fn ownership_reference_defuse_retires_an_owned_resource_without_running_its_finalizer() {
    for fail in [false, true] {
        let mut builder = Builder::new();
        let token = token(&mut builder);
        builder.emit(InstructionKind::Defuse { place: token }, 5, CleanupId(0));
        let value = builder.unit();
        let main = if fail {
            builder.block(
                Terminator::Fail {
                    rule: const { hirsel_diag::RuleId::of("FAIL-1") },
                    cleanup: CleanupId(0),
                },
                9,
            );
            builder.definition("main")
        } else {
            builder.finish("main", value)
        };
        // This finalizer must remain suppressed by the actual Defuse.
        let traced = observed(vec![main, finalizer(true)], &runtime_types());
        assert_eq!(matches!(traced.execution.outcome, Outcome::Failed(_)), fail);
        let analysis = traced.trace.unwrap().analyze([9; 32], 100).unwrap();
        assert_eq!(
            (analysis.created, analysis.dropped, analysis.live.len()),
            (1, 1, 0)
        );
    }
}

#[test]
fn ownership_reference_defused_owned_parent_still_releases_its_actual_child() {
    for fail in [false, true] {
        let mut builder = Builder::new();
        let child = token(&mut builder);
        let parent_kind = TypeKind::Named {
            definition: DefId::intern(&path("Parent")),
            arguments: vec![].into(),
        };
        let parent = builder.local(parent_kind.clone(), Storage::Local);
        builder.assign(
            parent,
            Rvalue::Construct {
                constructor: Constructor::Record(path("Parent")),
                fields: vec![ConstructField {
                    field: 0,
                    value: take(child),
                }],
                destination: Destination::Inferred,
            },
            4,
            CleanupId(0),
        );
        builder.emit(InstructionKind::Defuse { place: parent }, 5, CleanupId(0));
        let mut parent_drop = Builder::new();
        parent_drop.borrowed_parameter(parent_kind);
        parent_drop.block(
            Terminator::Fail {
                rule: const { hirsel_diag::RuleId::of("FAIL-1") },
                cleanup: CleanupId(0),
            },
            9,
        );
        let parent_drop = parent_drop.definition("Parent_drop");
        let mut runtime = runtime_types();
        runtime.nominals.push(Nominal {
            definition: path("Parent"),
            resource: true,
            distinct: false,
            finalizer: Some((path("Parent_drop"), BodyId(0))),
            fields: 1,
            representation: None,
        });
        let result = builder.unit();
        let main = if fail {
            builder.block(
                Terminator::Fail {
                    rule: const { hirsel_diag::RuleId::of("FAIL-1") },
                    cleanup: CleanupId(0),
                },
                9,
            );
            builder.definition("main")
        } else {
            builder.finish("main", result)
        };
        let traced = observed(vec![main, parent_drop, finalizer(false)], &runtime);
        assert_eq!(matches!(traced.execution.outcome, Outcome::Failed(_)), fail);
        let trace = traced.trace.unwrap();
        let analysis = trace.analyze([9; 32], 100).unwrap();
        assert_eq!(
            (analysis.created, analysis.dropped, analysis.live.len()),
            (2, 2, 0)
        );
        let drops: Vec<_> = trace
            .events
            .iter()
            .filter_map(|e| match &e.action {
                Action::Drop { token, .. } => Some(token.sequence),
                _ => None,
            })
            .collect();
        assert_eq!(drops, [1, 0]);
    }
}

mod closures;

#[test]
fn ownership_reference_finalizer_local_cleanup_precedes_queued_aggregate_sibling() {
    for fail in [false, true] {
        let audit_kind = TypeKind::Named {
            definition: DefId::intern(&path("Audit")),
            arguments: vec![].into(),
        };
        let mut builder = Builder::new();
        let first = token(&mut builder);
        let second = token(&mut builder);
        let token_type = builder.type_id(kind());
        let pair = builder.local(
            TypeKind::Tuple(vec![token_type, token_type].into()),
            Storage::Local,
        );
        builder.assign(
            pair,
            Rvalue::Construct {
                constructor: Constructor::Tuple,
                fields: vec![
                    ConstructField {
                        field: 0,
                        value: take(first),
                    },
                    ConstructField {
                        field: 1,
                        value: take(second),
                    },
                ],
                destination: Destination::Inferred,
            },
            4,
            CleanupId(0),
        );
        let result = builder.unit();
        let main = builder.finish("main", result);
        let mut token_drop = Builder::new();
        token_drop.borrowed_parameter(kind());
        let audit = token_drop.local(audit_kind.clone(), Storage::Local);
        token_drop.assign(
            audit,
            Rvalue::Construct {
                constructor: Constructor::Record(path("Audit")),
                fields: vec![],
                destination: Destination::Inferred,
            },
            6,
            CleanupId(0),
        );
        let unit = token_drop.unit();
        let token_drop = if fail {
            token_drop.block(
                Terminator::Fail {
                    rule: const { hirsel_diag::RuleId::of("FAIL-1") },
                    cleanup: CleanupId(0),
                },
                9,
            );
            token_drop.definition("Token_drop")
        } else {
            token_drop.finish("Token_drop", unit)
        };
        let mut audit_drop = Builder::new();
        audit_drop.borrowed_parameter(audit_kind);
        let unit = audit_drop.unit();
        let audit_drop = audit_drop.finish("Audit_drop", unit);
        let mut runtime = runtime_types();
        runtime.nominals.push(Nominal {
            definition: path("Audit"),
            resource: true,
            distinct: false,
            finalizer: Some((path("Audit_drop"), BodyId(0))),
            fields: 0,
            representation: None,
        });
        let traced = observed(vec![main, token_drop, audit_drop], &runtime);
        assert_eq!(matches!(traced.execution.outcome, Outcome::Failed(_)), fail);
        let trace = traced.trace.unwrap();
        let analysis = trace.analyze([9; 32], 100).unwrap();
        assert_eq!(
            (analysis.created, analysis.dropped, analysis.live.len()),
            (4, 4, 0)
        );
        let drops = trace
            .events
            .iter()
            .filter_map(|event| match &event.action {
                Action::Drop { token, .. } => Some(token.sequence),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            drops,
            [2, 1, 3, 0],
            "each finalizer local dies before its own receiver; fail={fail}"
        );
    }
}
