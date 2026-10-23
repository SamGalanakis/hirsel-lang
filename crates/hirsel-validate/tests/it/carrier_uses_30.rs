//! Selected aggregate authority covers its callback parts, never another input.
use crate::support::*;
use hirsel_syntax::PassingMode;
use hirsel_tir::LocalId;
use hirsel_tir::*;
use hirsel_types::*;
use hirsel_validate::ViolationKind;

fn selected(ordinals: &[u32]) -> AuthoritySummary {
    AuthoritySummary::Known(
        ordinals
            .iter()
            .map(|&p| AuthorityTerm::Parameter(ParameterRef::bound(ParamId(p))))
            .collect(),
    )
}

fn invocation(allowed: &[u32], staged: bool, with_argument: bool) -> Fixture {
    let mut f = Fixture::unit();
    let unit = f.body().result;
    let mut callback = fn_type(vec![], unit);
    callback.authority = AuthoritySummary::Unknown;
    callback.uses = Uses::Unrestricted;
    let sibling_type = interner().intern(ty(TypeKind::Function(callback.clone())));
    if with_argument {
        callback.parameters = vec![Parameter {
            name: None,
            label: None,
            passing: PassingMode::Borrow,
            ty: sibling_type,
        }]
        .into();
    }
    let callback = interner().intern(ty(TypeKind::Function(callback)));
    let holder = f.slot(
        TypeKind::Record(
            vec![Field {
                name: "call".into(),
                ty: callback,
                growable: false,
            }]
            .into(),
        ),
        Storage::Parameter,
    );
    let sibling = f.slot(
        TypeKind::Function(match &interner().get(sibling_type).unwrap().kind {
            TypeKind::Function(function) => function.clone(),
            _ => unreachable!(),
        }),
        Storage::Parameter,
    );
    // Borrowed input content and its projected callback have the same local
    // view. This fixture does not forge a local projection of global storage.
    for root in [holder, sibling] {
        let mut row = interner()
            .get(f.body().locals[root as usize].ty)
            .unwrap()
            .clone();
        row.modes.relation = Relation::environment();
        let local = f.definition.types.push(row);
        f.body().locals[root as usize].ty = local;
        f.body().places[root as usize].ty = local;
    }
    f.body().parameters = vec![LocalId(holder), LocalId(sibling)];
    owner_signature(&mut f, PassingMode::Borrow);
    let SignatureKind::Function(owner) = &mut f.signatures.get_mut(id("main")).unwrap().kind else {
        unreachable!()
    };
    owner.uses = Uses::Only(selected(allowed));
    let mut field = ty(interner().get(callback).unwrap().kind.clone());
    field.modes.relation = Relation::environment();
    let field_ty = f.definition.types.push(field.clone());
    let projected = f.body().places.len() as u32;
    f.body().places.push(Place {
        root: LocalId(holder),
        projection: vec![Projection::Field(0)].into(),
        loan: None,
        binding: None,
        ty: field_ty,
    });
    let mut cleanup = 0;
    let callee = if staged {
        let local = f.slot(field.kind, Storage::Local);
        let root = f.body().places[local as usize].root;
        f.body().locals[root.0 as usize].ty = field_ty;
        f.body().places[local as usize].ty = field_ty;
        f.assign(local, Rvalue::Use(copy(projected)), 0);
        cleanup = f.cleanup(vec![drop(local)]);
        local
    } else {
        projected
    };
    let result = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Indirect {
            forward_caller: false,
            value: copy(callee),
            arguments: vec![],
        },
        arguments: if with_argument {
            vec![CallArgument {
                parameter: ParamId(0),
                value: copy(sibling),
            }]
        } else {
            vec![]
        },
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
fn carrier_uses_30_selected_projection_and_staging() {
    for staged in [false, true] {
        invocation(&[0], staged, false).valid();
    }
}

#[test]
fn carrier_uses_30_body_uses_cannot_exceed_selected_parameter() {
    for staged in [false, true] {
        for allowed in [&[][..], &[1][..]] {
            let kinds = invocation(allowed, staged, false).kinds();
            assert!(!kinds.is_empty(), "staged={staged}, allowed={allowed:?}");
            assert!(
                kinds.iter().all(|k| *k
                    == ViolationKind::TypeMismatch {
                        entity: "body uses"
                    }),
                "{kinds:?}"
            );
        }
    }
}

#[test]
fn carrier_uses_30_selecting_callee_does_not_select_callback_argument() {
    for staged in [false, true] {
        invocation(&[0, 1], staged, true).valid();
        let kinds = invocation(&[0], staged, true).kinds();
        assert!(!kinds.is_empty());
        assert!(
            kinds.iter().all(|k| *k
                == ViolationKind::TypeMismatch {
                    entity: "body uses"
                }),
            "{kinds:?}"
        );
    }
}
