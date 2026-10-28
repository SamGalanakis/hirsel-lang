//! Field then enum payload reachability, reconstructed without producer decisions.
use crate::support::*;
use hirsel_syntax::PassingMode;
use hirsel_tir::LocalId;
use hirsel_tir::*;
use hirsel_types::*;
use hirsel_validate::{ViolationKind, validate};

fn field_match(foreign: bool) -> Fixture {
    let mut f = Fixture::unit();
    let tree = f.definition.types.push(ty(TypeKind::Named {
        definition: id("Tree"),
        arguments: vec![].into(),
    }));
    let boxed = f.definition.types.push(ty(TypeKind::Box(tree)));
    let number = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::U64)));
    let option = interner().intern(ty(TypeKind::Variant(
        vec![
            Variant {
                name: "Some".into(),
                fields: vec![(None, boxed)].into(),
            },
            Variant {
                name: "None".into(),
                fields: vec![].into(),
            },
        ]
        .into(),
    )));
    let shape = interner().intern(ty(TypeKind::Record(
        vec![
            Field {
                name: "value".into(),
                growable: false,
                ty: number,
            },
            Field {
                name: "left".into(),
                growable: false,
                ty: option,
            },
        ]
        .into(),
    )));
    let mut signature = function_signature("Tree", vec![], TypeKind::Tuple(vec![].into()));
    signature.kind = SignatureKind::Type(Box::new(TypeDefinition {
        loop_source: None,
        format: None,
        generics: vec![],
        representation: Some(shape),
        bounds: vec![],
        authority: AuthoritySummary::inert(),
        capability: None,
        implementation_authority: None,
        serializable: false,
        serializable_check: None,
        layout: Default::default(),
        distinct: false,
        resource: false,
        drop: None,
        fact: None,
        captures: None,
        destructor_allocation: Allocation::NoAlloc,
    }));
    f.signatures.insert(id("Tree"), signature);
    let source = f.slot(TypeKind::Box(tree), Storage::Parameter);
    let cursor = f.slot(TypeKind::Box(tree), Storage::Temporary);
    let rhs = f.slot(TypeKind::Box(tree), Storage::Temporary);
    let retired = f.slot(TypeKind::Box(tree), Storage::Temporary);
    let other = f.slot(TypeKind::Box(tree), Storage::Parameter);
    f.body().parameters = vec![LocalId(source), LocalId(other)];
    f.body().scopes.push(hirsel_tir::Scope {
        parent: Some(ScopeId(0)),
        span: Default::default(),
    });
    f.body().locals[retired as usize].scope = ScopeId(1);
    owner_signature(&mut f, PassingMode::Borrow);
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(0),
        place: PlaceId(source),
        mutable: false,
        if_inactive: false,
    });
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(1),
        place: PlaceId(other),
        mutable: false,
        if_inactive: false,
    });
    let loaned = f.through_loan(source, 0);
    let initializer = RvalueId(f.body().rvalues.len() as u32);
    f.assign(cursor, Rvalue::Use(operand(loaned, UseKind::Borrow)), 0);
    f.body().borrow_cursors.push(BorrowCursor {
        carrier: LocalId(cursor),
        initializer,
        cover: None,
    });
    let child = f.body().places.len() as u32;
    f.body().places.push(Place {
        root: LocalId(if foreign { other } else { cursor }),
        projection: vec![
            Projection::Field(0),
            Projection::Field(1),
            Projection::VariantField {
                variant: 0,
                field: 0,
            },
        ]
        .into(),
        loan: foreign.then_some(LoanId(1)),
        binding: None,
        ty: boxed,
    });
    let preheader = std::mem::take(&mut f.body().blocks[0].instructions);
    f.assign(rhs, Rvalue::Use(operand(child, UseKind::Borrow)), 0);
    f.assign(retired, Rvalue::Use(take(cursor)), 0);
    f.assign(cursor, Rvalue::Use(take(rhs)), 0);
    f.emit(InstructionKind::EndScope(ScopeId(1)));
    let advance = std::mem::take(&mut f.body().blocks[0].instructions);
    f.body().blocks[0] = block(preheader, Terminator::Goto(BlockId(1)));
    let payload = PlaceId(f.body().places.len() as u32);
    f.body().places.push(Place {
        root: LocalId(cursor),
        projection: vec![Projection::Field(0), Projection::Field(1)].into(),
        loan: None,
        binding: None,
        ty: option,
    });
    f.body().blocks.push(block(
        vec![],
        Terminator::Switch {
            value: operand(payload.0, UseKind::Borrow),
            arms: vec![(0, BlockId(2)), (1, BlockId(3))],
            otherwise: BlockId(3),
        },
    ));
    f.body()
        .blocks
        .push(block(advance, Terminator::Goto(BlockId(1))));
    f.body().blocks.push(block(
        vec![
            inst(
                InstructionKind::EndLoan {
                    loan: LoanId(0),
                    if_active: false,
                },
                0,
            ),
            inst(
                InstructionKind::EndLoan {
                    loan: LoanId(1),
                    if_active: false,
                },
                0,
            ),
        ],
        ret(0, 0),
    ));
    f
}

#[test]
fn field_match_cursor_v_accepts_recursive_field_payload_loop() {
    field_match(false).valid();
}

#[test]
fn field_match_cursor_v_rejects_foreign_field_payload_loop() {
    let f = field_match(true);
    let verdict = validate(
        &tabled(&f.definition),
        interner(),
        &f.signatures,
        &mut vec![],
    );
    assert!(
        verdict
            .violations
            .iter()
            .any(|v| v.block == Some(BlockId(2))
                && v.instruction == Some(2)
                && v.kind == ViolationKind::InvalidOrigin(PlaceId(2))),
        "{verdict:?}"
    );
}
