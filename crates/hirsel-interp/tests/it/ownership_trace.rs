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
fn ownership_reference_unsupported_nested_resource_transport_never_claims_completeness() {
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
    assert!(matches!(traced.trace, Err(TraceError::Unsupported(_))));
}
