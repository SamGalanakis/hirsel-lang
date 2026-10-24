use super::*;

fn program(invoke: bool, fail: bool) -> Vec<Definition> {
    let mut callback = Builder::new();
    callback.local(kind(), Storage::Capture);
    let result = callback.unit();
    let callback = if fail {
        callback.block(
            Terminator::Fail {
                rule: const { hirsel_diag::RuleId::of("FAIL-1") },
                cleanup: CleanupId(0),
            },
            9,
        );
        callback.definition("callback")
    } else {
        callback.finish("callback", result)
    };
    let mut builder = Builder::new();
    let resource = token(&mut builder);
    let unit = builder.type_id(TypeKind::Tuple(vec![].into()));
    let function_kind = function_type(unit, vec![]);
    let closure = builder.local(function_kind.clone(), Storage::Local);
    let mut row = crate::support::ty(function_kind);
    row.modes.multiplicity = hirsel_types::Multiplicity::Once;
    let function = builder.types.intern(row);
    let local = builder.body.places[closure.0 as usize].root;
    builder.body.locals[local.0 as usize].ty = function;
    builder.body.places[closure.0 as usize].ty = function;
    builder.assign(
        closure,
        Rvalue::Closure {
            context: hirsel_types::CaptureContext::OwningScoped,
            body: BodyId(1),
            captures: vec![take(resource)],
            destination: Destination::Heap,
        },
        3,
        CleanupId(0),
    );
    let result = if invoke {
        let result = builder.local(TypeKind::Tuple(vec![].into()), Storage::Temporary);
        builder.block(
            Terminator::Call {
                callee: Callee::Indirect {
                    forward_caller: false,
                    value: take(closure),
                    arguments: vec![],
                },
                arguments: vec![],
                returns: Some(CallReturn {
                    destination: result,
                    next: BlockId(1),
                }),
                on_failure: CleanupId(0),
            },
            4,
        );
        result
    } else {
        builder.unit()
    };
    let mut main = builder.finish("main", result);
    crate::support::append_body(&mut main, &callback);
    vec![main, finalizer(false)]
}

#[test]
fn ownership_closure_drop_releases_the_actual_uninvoked_owned_capture() {
    let traced = observed(program(false, false), &runtime_types());
    let trace = traced.trace.unwrap();
    let analysis = trace.analyze([9; 32], 100).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (1, 1, 0)
    );
    assert!(
        trace
            .events
            .iter()
            .any(|e| !e.action.owner().path.is_empty())
    );
}

#[test]
fn ownership_closure_once_entry_moves_its_capture_into_the_actual_body_on_all_exits() {
    for fail in [false, true] {
        let traced = observed(program(true, fail), &runtime_types());
        assert_eq!(matches!(traced.execution.outcome, Outcome::Failed(_)), fail);
        let trace = traced.trace.unwrap();
        let analysis = trace.analyze([9; 32], 100).unwrap();
        assert_eq!(
            (analysis.created, analysis.dropped, analysis.live.len()),
            (1, 1, 0)
        );
        assert!(trace.events.iter().any(|e| match &e.action {
            Action::Move { from, owner, .. } =>
                !from.path.is_empty()
                    && matches!(owner.storage, TraceStorage::Slot { frame: 1, .. }),
            _ => false,
        }));
    }
}
