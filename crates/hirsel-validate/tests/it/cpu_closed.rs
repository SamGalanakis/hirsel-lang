//! V reconstructs the CPU proof from local bodies, without source checker facts.
use crate::support::*;
use hirsel_syntax::{DefPath, PassingMode};
use hirsel_tir::*;
use hirsel_types::*;
use hirsel_validate::ViolationKind;

fn fixture() -> Fixture {
    let mut f = Fixture::unit();
    let unit = f.body().result;
    let mut callback = fn_type(vec![], unit);
    callback.uses = Uses::Only(AuthoritySummary::inert());
    let callback_kind = TypeKind::Function(callback);
    let child = Fixture::unit().definition.bodies.remove(0);
    f.definition.bodies.push(child.clone());
    f.definition.bodies.push(child);
    let prepare = f.slot(callback_kind.clone(), Storage::Local);
    let map = f.slot(callback_kind.clone(), Storage::Local);
    let prepare_cleanup = f.cleanup(vec![drop(prepare)]);
    for (index, slot) in [prepare, map].into_iter().enumerate() {
        f.assign(
            slot,
            Rvalue::Closure {
                context: CaptureContext::OwningScoped,
                body: BodyId(index as u32 + 1),
                captures: vec![],
                destination: Destination::Inferred,
            },
            if index == 0 { 0 } else { prepare_cleanup },
        );
    }
    let mut signature = function_signature(
        "cpu",
        vec![
            (PassingMode::Borrow, TypeKind::Tuple(vec![].into())),
            (PassingMode::Borrow, TypeKind::Tuple(vec![].into())),
            (PassingMode::Take, callback_kind.clone()),
            (PassingMode::Take, callback_kind),
        ],
        TypeKind::Tuple(vec![].into()),
    );
    let cpu = DefPath::core("cpu.map_scoped");
    signature.definition = (&cpu).into();
    f.signatures.insert(signature.definition, signature);
    let result = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    let cleanup = f.cleanup(vec![drop(map), drop(prepare)]);
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: cpu,
            arguments: vec![],
        },
        arguments: [copy(0), copy(0), take(prepare), take(map)]
            .into_iter()
            .enumerate()
            .map(|(i, value)| CallArgument {
                parameter: ParamId(i as u32),
                value,
            })
            .collect(),
        returns: Some(CallReturn {
            destination: PlaceId(result),
            next: BlockId(1),
        }),
        on_failure: CleanupId(cleanup),
    };
    f.body().blocks.push(block(vec![], ret(result, cleanup)));
    f
}

#[test]
fn cpu_proof_rejects_external_calls_inside_a_nested_callback() {
    let mut f = fixture();
    f.valid();
    let child = f.definition.bodies[1].clone();
    f.definition.bodies.push(child);
    // An unused nested closure is still part of the closed body. Its call
    // cannot hide behind a core operation accepting a callback.
    let body = &mut f.definition.bodies[1];
    body.rvalues.push(Rvalue::Closure {
        context: CaptureContext::OwningScoped,
        body: BodyId(3),
        captures: vec![],
        destination: Destination::Inferred,
    });
    let signature = function_signature("outside", vec![], TypeKind::Tuple(vec![].into()));
    f.signatures.insert(signature.definition, signature);
    f.definition.bodies[3].blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: path("outside"),
            arguments: vec![],
        },
        arguments: vec![],
        returns: None,
        on_failure: CleanupId(0),
    };
    assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
        entity: "unproved external CPU helper"
    }));
}

#[test]
fn cpu_operation_cannot_escape_as_an_indirect_function_value() {
    let mut f = fixture();
    f.valid();
    f.body().rvalues.push(Rvalue::FunctionReference {
        definition: DefPath::core("cpu.map_scoped"),
        arguments: vec![],
    });
    let value = f.body().rvalues.last().unwrap().clone();
    f.body().rvalues.pop();
    f.assign(1, value, 0);
    assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
        entity: "indirect CPU operation"
    }));
}
