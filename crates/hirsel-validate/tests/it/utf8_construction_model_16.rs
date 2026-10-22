//! A plain branded scalar models StrPos's identity/lifetime obligation.
//! These controls bypass source checking and never manufacture a runtime StrPos.
use crate::support::*;
use hirsel_tir::{LocalId, *};
use hirsel_types::*;
use hirsel_validate::ViolationKind;

fn live_position() -> Fixture {
    let mut f = Fixture::unit();
    f.body().blocks[0].instructions.clear();
    f.body().rvalues.clear();
    let source = f.slot(TypeKind::Primitive(Primitive::String), Storage::Parameter);
    let other = f.slot(TypeKind::Primitive(Primitive::String), Storage::Parameter);
    let position = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Parameter);
    let mut row = ty(TypeKind::Primitive(Primitive::U64));
    row.brand.alternatives.push(Origin {
        root: OriginRoot::Parameter(ParameterRef::body(f.body().parameter_scope, ParamId(0))),
        fields: vec![].into(),
    });
    let branded = f.definition.types.push(row);
    f.body().locals[position as usize].ty = branded;
    f.body().places[position as usize].ty = branded;
    f.body().parameters = vec![LocalId(source), LocalId(other), LocalId(position)];
    owner_signature(&mut f, PassingMode::Borrow);
    let mut declared = interner().get(branded).unwrap().clone();
    declared.brand.alternatives[0].root = OriginRoot::Parameter(ParameterRef::bound(ParamId(0)));
    if let SignatureKind::Function(fun) = &mut f.signatures.get_mut(&path("main")).unwrap().kind {
        fun.parameters[2].ty = interner().intern(declared);
    }
    let mut callee = function_signature(
        "at",
        vec![
            (PassingMode::Borrow, TypeKind::Primitive(Primitive::String)),
            (PassingMode::Borrow, TypeKind::Primitive(Primitive::U64)),
        ],
        TypeKind::Tuple(vec![].into()),
    );
    let mut row = ty(TypeKind::Primitive(Primitive::U64));
    row.brand.alternatives.push(Origin {
        root: OriginRoot::Parameter(ParameterRef::bound(ParamId(0))),
        fields: vec![].into(),
    });
    if let SignatureKind::Function(fun) = &mut callee.kind {
        fun.parameters[1].ty = interner().intern(row);
    }
    f.signatures.insert(id("at"), callee);
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: path("at"),
            arguments: vec![],
        },
        arguments: vec![
            CallArgument {
                parameter: ParamId(0),
                value: operand(source, UseKind::Borrow),
            },
            CallArgument {
                parameter: ParamId(1),
                value: operand(position, UseKind::Borrow),
            },
        ],
        returns: Some(CallReturn {
            destination: PlaceId(0),
            next: BlockId(1),
        }),
        on_failure: CleanupId(0),
    };
    f.body().blocks.push(block(vec![], ret(0, 0)));
    f
}

#[test]
fn same_live_source_position_is_accepted() {
    live_position().valid();
}

#[test]
fn wrong_live_source_is_rejected_independently() {
    let mut f = live_position();
    f.valid();
    let Terminator::Call { arguments, .. } = &mut f.body().blocks[0].terminator else {
        unreachable!()
    };
    arguments[0].value.place = PlaceId(2);
    let kinds = f.kinds();
    assert!(
        kinds.contains(&ViolationKind::TypeMismatch {
            entity: "call argument"
        }),
        "{kinds:?}"
    );
}

#[test]
fn ended_source_position_is_rejected_independently() {
    let mut f = live_position();
    // EndOrigin ends the explicitly bound identity, not an unbound borrowed
    // storage name. Lowering installs this binding for a source position.
    f.body().places[1].binding = Some(Origin {
        root: OriginRoot::Parameter(ParameterRef::body(f.body().parameter_scope, ParamId(0))),
        fields: vec![].into(),
    });
    f.valid();
    f.emit(InstructionKind::EndOrigin(PlaceId(1)));
    let kinds = f.kinds();
    assert!(
        kinds.contains(&ViolationKind::InvalidOrigin(PlaceId(3))),
        "{kinds:?}"
    );
}
