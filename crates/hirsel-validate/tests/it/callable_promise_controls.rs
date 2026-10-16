//! Written callable guarantees survive storage, independently of source checking.
use crate::support::*;
use hirsel_syntax::PassingMode;
use hirsel_tir::*;
use hirsel_types::*;
use hirsel_validate::ViolationKind;

#[derive(Clone, Copy)]
enum Guarantee {
    Captures,
    AdvanceOnly,
}

fn callback(f: &mut Fixture, guarantee: Guarantee, promised: bool) -> FunctionType {
    let element = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::U64)));
    let iterator = f.definition.types.push(ty(TypeKind::Builtin {
        kind: BuiltinType::Iter,
        arguments: vec![GenericArgument::Type(element)].into(),
    }));
    let mut result = f.definition.types.get(iterator).unwrap().clone();
    if matches!(guarantee, Guarantee::Captures) && promised {
        result.erased_captures = Some(ErasedCaptures::FromArguments(
            vec![ParameterRef::bound(ParamId(0)).into()].into(),
        ));
    }
    let result = f.definition.types.push(result);
    FunctionType {
        generics: vec![].into(),
        parameters: vec![Parameter {
            name: None,
            label: None,
            passing: match guarantee {
                Guarantee::Captures => PassingMode::Take,
                Guarantee::AdvanceOnly => PassingMode::Mut,
            },
            ty: iterator,
        }]
        .into(),
        result: if matches!(guarantee, Guarantee::Captures) {
            result
        } else {
            f.body().result
        },
        error: None,
        allocation: Allocation::MayAllocate,
        authority: AuthoritySummary::Unknown,
        uses: Uses::Unrestricted,
        contracts: vec![].into(),
        state_promises: if matches!(guarantee, Guarantee::AdvanceOnly) && promised {
            vec![StatePromise {
                selector: ParameterRef::bound(ParamId(0)).into(),
                advance_only: true,
                preserve_captures: true,
            }]
            .into()
        } else {
            vec![].into()
        },
        incomplete: false,
        foreign: None,
    }
}

fn declaration(f: &mut Fixture, name: &str, function: FunctionType) {
    let mut signature = function_signature(name, vec![], TypeKind::Tuple(vec![].into()));
    signature.types = f.definition.types.clone();
    signature.kind = SignatureKind::Function(function);
    f.signatures.insert(signature.definition.clone(), signature);
}

/// A call result is held by an annotated local, directly or inside a tuple.
/// The declaration, call result and source slot all agree on the actual promise;
/// only the receiving storage asks for more in the rejected control.
fn captures_storage(promised: bool, tuple: bool, usage: UseKind) -> Fixture {
    let mut f = Fixture::unit();
    let actual = callback(&mut f, Guarantee::Captures, promised);
    let expected = callback(&mut f, Guarantee::Captures, true);
    let mut actual_kind = TypeKind::Function(actual);
    let mut expected_kind = TypeKind::Function(expected);
    if tuple {
        let actual = f.definition.types.push(ty(actual_kind));
        let expected = f.definition.types.push(ty(expected_kind));
        let tag = f
            .definition
            .types
            .push(ty(TypeKind::Primitive(Primitive::U64)));
        actual_kind = TypeKind::Tuple(vec![actual, tag].into());
        expected_kind = TypeKind::Tuple(vec![expected, tag].into());
    }
    let source = f.slot(actual_kind, Storage::Temporary);
    let held = f.slot(expected_kind, Storage::Local);
    let SignatureKind::Function(mut factory) =
        function_signature("mk", vec![], TypeKind::Tuple(vec![].into())).kind
    else {
        unreachable!()
    };
    factory.result = f.body().places[source as usize].ty;
    declaration(&mut f, "mk", factory);
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: path("mk"),
            arguments: vec![],
        },
        arguments: vec![],
        returns: Some(CallReturn {
            destination: PlaceId(source),
            next: BlockId(1),
        }),
        on_failure: CleanupId(0),
    };
    let failure = f.cleanup(vec![drop(source)]);
    let cleanup = f.cleanup(vec![drop(source), drop(held)]);
    f.assign(held, Rvalue::Use(operand(source, usage)), failure);
    let assignment = f.body().blocks[0].instructions.pop().unwrap();
    f.body()
        .blocks
        .push(block(vec![assignment], ret(0, cleanup)));
    owner_signature(&mut f, PassingMode::Take);
    f
}

/// A named function is materialized at its declaration's actual contract before
/// being assigned to a holder requiring advance-only state preservation.
fn state_storage(promised: bool, usage: UseKind) -> Fixture {
    let mut f = Fixture::unit();
    let actual = callback(&mut f, Guarantee::AdvanceOnly, promised);
    let expected = callback(&mut f, Guarantee::AdvanceOnly, true);
    let source = f.slot(TypeKind::Function(actual.clone()), Storage::Temporary);
    let held = f.slot(TypeKind::Function(expected), Storage::Local);
    declaration(&mut f, "plain", actual);
    f.assign(
        source,
        Rvalue::FunctionReference {
            definition: path("plain"),
            arguments: vec![],
        },
        0,
    );
    let failure = f.cleanup(vec![drop(source)]);
    f.assign(held, Rvalue::Use(operand(source, usage)), failure);
    let cleanup = f.cleanup(vec![drop(source), drop(held)]);
    f.body().blocks[0].terminator = ret(0, cleanup);
    owner_signature(&mut f, PassingMode::Take);
    f
}

/// Replacing a live holder takes the same callable-comparison path in flow.
fn replacement(promised: bool, guarantee: Guarantee) -> Fixture {
    let mut f = Fixture::unit();
    let actual = callback(&mut f, guarantee, promised);
    let expected = callback(&mut f, guarantee, true);
    let held = f.slot(TypeKind::Function(expected.clone()), Storage::Local);
    let source = f.slot(TypeKind::Function(actual.clone()), Storage::Temporary);
    let displaced = f.slot(TypeKind::Function(expected.clone()), Storage::Temporary);
    declaration(&mut f, "initial", expected);
    declaration(&mut f, "incoming", actual);
    f.assign(
        held,
        Rvalue::FunctionReference {
            definition: path("initial"),
            arguments: vec![],
        },
        0,
    );
    let failure = f.cleanup(vec![drop(held)]);
    f.assign(
        source,
        Rvalue::FunctionReference {
            definition: path("incoming"),
            arguments: vec![],
        },
        failure,
    );
    f.emit(InstructionKind::Replace {
        destination: PlaceId(held),
        value: take(source),
        displaced: PlaceId(displaced),
        if_live: false,
    });
    f.emit(InstructionKind::Drop {
        place: PlaceId(displaced),
        if_live: false,
    });
    f.body().blocks[0]
        .instructions
        .last_mut()
        .unwrap()
        .on_failure = CleanupId(failure);
    f.body().blocks[0].terminator = ret(0, failure);
    owner_signature(&mut f, PassingMode::Take);
    f
}

#[test]
fn let_rejects_missing_callable_result_capture_selection() {
    for usage in [UseKind::Take, UseKind::Copy] {
        captures_storage(false, false, usage).broken(vec![ViolationKind::TypeMismatch {
            entity: "assignment",
        }]);
    }
}

#[test]
fn let_accepts_the_declared_callable_result_capture_selection() {
    for usage in [UseKind::Take, UseKind::Copy] {
        captures_storage(true, false, usage).valid();
    }
}

#[test]
fn assignment_rejects_missing_callable_state_promise() {
    for usage in [UseKind::Take, UseKind::Copy] {
        state_storage(false, usage).broken(vec![ViolationKind::TypeMismatch {
            entity: "assignment",
        }]);
    }
}

#[test]
fn assignment_accepts_the_declared_callable_state_promise() {
    for usage in [UseKind::Take, UseKind::Copy] {
        state_storage(true, usage).valid();
    }
}

#[test]
fn tuple_storage_rejects_missing_callable_result_capture_selection() {
    for usage in [UseKind::Take, UseKind::Copy] {
        captures_storage(false, true, usage).broken(vec![ViolationKind::TypeMismatch {
            entity: "assignment",
        }]);
    }
}

#[test]
fn tuple_storage_accepts_the_declared_callable_result_capture_selection() {
    for usage in [UseKind::Take, UseKind::Copy] {
        captures_storage(true, true, usage).valid();
    }
}

#[test]
fn replacement_rejects_missing_callable_guarantees() {
    for guarantee in [Guarantee::Captures, Guarantee::AdvanceOnly] {
        replacement(false, guarantee).broken(vec![ViolationKind::TypeMismatch {
            entity: "replacement",
        }]);
    }
}

#[test]
fn replacement_accepts_the_declared_callable_guarantees() {
    for guarantee in [Guarantee::Captures, Guarantee::AdvanceOnly] {
        replacement(true, guarantee).valid();
    }
}
