//! Recursive physical child transport under an actual borrowed root.
use crate::support::*;
use hirsel_syntax::PassingMode;
use hirsel_tir::LocalId;
use hirsel_tir::*;
use hirsel_types::*;

#[test]
fn borrow_cursor_8_recursive_child_loop() {
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
    let shape = interner().intern(ty(TypeKind::Variant(
        vec![
            Variant {
                name: "Leaf".into(),
                fields: vec![(None, number)].into(),
            },
            Variant {
                name: "Branch".into(),
                fields: vec![(None, boxed), (None, boxed)].into(),
            },
        ]
        .into(),
    )));
    let mut signature = function_signature("Tree", vec![], TypeKind::Tuple(vec![].into()));
    signature.kind = SignatureKind::Type(TypeDefinition {
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
    });
    f.signatures.insert(id("Tree"), signature);
    let source = f.slot(TypeKind::Box(tree), Storage::Parameter);
    let cursor = f.slot(TypeKind::Box(tree), Storage::Temporary);
    let rhs = f.slot(TypeKind::Box(tree), Storage::Temporary);
    let retired = f.slot(TypeKind::Box(tree), Storage::Temporary);
    let condition = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Parameter);
    f.body().parameters = vec![LocalId(source), LocalId(condition)];
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
        root: LocalId(cursor),
        projection: vec![
            Projection::Field(0),
            Projection::VariantField {
                variant: 1,
                field: 0,
            },
        ]
        .into(),
        loan: None,
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
        projection: vec![Projection::Field(0)].into(),
        loan: None,
        binding: None,
        ty: tree,
    });
    f.body().blocks.push(block(
        vec![],
        Terminator::Switch {
            value: operand(payload.0, UseKind::Borrow),
            arms: vec![(0, BlockId(3)), (1, BlockId(2))],
            otherwise: BlockId(3),
        },
    ));
    f.body()
        .blocks
        .push(block(advance, Terminator::Goto(BlockId(1))));
    f.body().blocks.push(block(
        vec![inst(
            InstructionKind::EndLoan {
                loan: LoanId(0),
                if_active: false,
            },
            0,
        )],
        ret(0, 0),
    ));
    f.valid();
}
