//! V reconstructs data nonretention without trusting the selector claim.
use crate::support::*;
use hirsel_tir::LocalId;
use hirsel_tir::*;
use hirsel_types::*;

fn returned(scoped: bool, copied: bool, unary: bool) -> Fixture {
    let mut f = Fixture::unit();
    let kind = TypeKind::Primitive(Primitive::String);
    let root = f.slot(kind.clone(), Storage::Parameter);
    let result = f.slot(kind.clone(), Storage::Temporary);
    f.body().parameters = vec![LocalId(root)];
    if unary {
        let mut row = interner().row(f.body().locals[result as usize].ty).clone();
        row.modes.uniqueness = Uniqueness::Unique;
        let ty = f.definition.types.push(row);
        f.body().locals[result as usize].ty = ty;
        f.body().places[result as usize].ty = ty;
    }
    f.body().result = f.body().locals[result as usize].ty;
    let signature = function_signature("main", vec![(PassingMode::Borrow, kind.clone())], kind);
    f.signatures.insert(signature.definition, signature);
    if scoped {
        f.body().scoped_parameters = vec![ScopedParameter {
            parameter: 0,
            fields: vec![],
        }];
    }
    f.assign(
        result,
        if copied {
            if unary {
                Rvalue::Unary {
                    op: hirsel_syntax::UnaryOp::Unique,
                    value: copy(root),
                }
            } else {
                Rvalue::Use(copy(root))
            }
        } else {
            Rvalue::Constant(Constant::String("independent".into()))
        },
        0,
    );
    f.body().blocks[0].terminator = ret(result, 0);
    if unary {
        owner_signature(&mut f, PassingMode::Borrow);
        let SignatureKind::Function(function) = &mut f.signatures.get_mut(id("main")).unwrap().kind
        else {
            unreachable!()
        };
        function.allocation = Allocation::MayAllocate;
        if scoped {
            f.body().scoped_parameters = vec![ScopedParameter {
                parameter: 0,
                fields: vec![],
            }];
        }
    }
    f
}

fn qualified(f: &Fixture) -> Vec<hirsel_validate::ViolationKind> {
    f.valid();
    hirsel_validate::qualify_scoped_transport(
        &tabled(&f.definition),
        interner(),
        &f.signatures,
        &mut vec![],
    )
    .violations
    .into_iter()
    .map(|v| v.kind)
    .collect()
}

#[test]
fn a_data_contract_accepts_an_independent_result_and_rejects_a_retained_root() {
    assert_eq!(qualified(&returned(true, false, false)), []);
    for unary in [false, true] {
        assert_eq!(qualified(&returned(false, true, unary)), []);
        let kinds = qualified(&returned(true, true, unary));
        assert!(
            kinds.contains(&hirsel_validate::ViolationKind::TypeMismatch {
                entity: "scoped nonretaining result"
            }),
            "{kinds:?}"
        );
    }
}

#[test]
fn a_data_selector_cannot_decorate_a_taken_parameter() {
    let mut f = returned(true, false, false);
    f.valid();
    let signature = f.signatures.get_mut(id("main")).unwrap();
    let SignatureKind::Function(function) = &mut signature.kind else {
        unreachable!()
    };
    function.parameters = function
        .parameters
        .iter()
        .cloned()
        .map(|mut p| {
            p.passing = PassingMode::Take;
            p
        })
        .collect();
    assert!(
        f.kinds()
            .contains(&hirsel_validate::ViolationKind::TypeMismatch {
                entity: "scoped parameter contract"
            })
    );
}
