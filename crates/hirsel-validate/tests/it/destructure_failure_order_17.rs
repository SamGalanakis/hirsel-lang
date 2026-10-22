//! Independent TIR owners at an omitted-finalizer failure boundary.
use crate::support::*;
use hirsel_syntax::DefId;
use hirsel_tir::*;
use hirsel_types::*;
use hirsel_validate::ViolationKind;

#[derive(Clone, Copy, Debug)]
enum Mutation {
    None,
    LostSibling,
    ReversedSuffix,
    Duplicate,
    Foreign,
    Twice,
}

fn fixture(bound: usize, mutation: Mutation) -> Fixture {
    let mut f = Fixture::unit();
    let string = interner().intern(ty(TypeKind::Primitive(Primitive::String)));
    let representation = interner().intern(ty(TypeKind::Record(
        ["first", "middle", "last"]
            .into_iter()
            .map(|name| Field {
                name: name.into(),
                ty: string,
                growable: false,
            })
            .collect::<Vec<_>>()
            .into(),
    )));
    let mut declaration = capability_signature("Parent");
    let SignatureKind::Type(t) = &mut declaration.kind else {
        unreachable!()
    };
    t.capability = None;
    t.authority = AuthoritySummary::inert();
    t.resource = true;
    t.drop = Some(path("Parent_drop"));
    t.representation = Some(representation);
    f.signatures.insert(id("Parent"), declaration);
    let mut inputs = vec![];
    for name in ["first", "middle", "last"] {
        let slot = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
        let cleanup = f.cleanup(inputs.iter().rev().copied().map(drop).collect());
        f.assign(
            slot,
            Rvalue::Constant(Constant::String(name.into())),
            cleanup,
        );
        inputs.push(slot);
    }
    let parent = f.slot(
        TypeKind::Named {
            definition: id("Parent"),
            arguments: vec![].into(),
        },
        Storage::Temporary,
    );
    let before = f.cleanup(inputs.iter().rev().copied().map(drop).collect());
    f.assign(
        parent,
        Rvalue::Construct {
            constructor: Constructor::Record(path("Parent")),
            fields: inputs
                .iter()
                .enumerate()
                .map(|(i, &p)| ConstructField {
                    field: i as u32,
                    value: take(p),
                })
                .collect(),
            destination: Destination::Inferred,
        },
        before,
    );
    let parent_cleanup = f.cleanup(vec![drop(parent)]);
    f.body().blocks[0].instructions.push(inst(
        InstructionKind::Defuse {
            place: PlaceId(parent),
        },
        parent_cleanup,
    ));
    if matches!(mutation, Mutation::Twice) {
        f.body().blocks[0].instructions.push(inst(
            InstructionKind::Defuse {
                place: PlaceId(parent),
            },
            parent_cleanup,
        ));
    }
    // One bound local, then the omitted temporaries introduced last-first.
    let order: Vec<_> = [bound]
        .into_iter()
        .chain((0..3).rev().filter(|&i| i != bound))
        .collect();
    let mut owners = vec![];
    for (n, field) in order.into_iter().enumerate() {
        let projected = f.body().places.len() as u32;
        let root = f.body().places[parent as usize].root;
        f.body().places.push(Place {
            root,
            projection: vec![Projection::Field(field as u32)].into(),
            ty: string,
            loan: None,
            binding: None,
        });
        let owner = f.slot(
            TypeKind::Primitive(Primitive::String),
            if n == 0 {
                Storage::Local
            } else {
                Storage::Temporary
            },
        );
        let cleanup = f.cleanup(
            owners
                .iter()
                .rev()
                .copied()
                .map(drop)
                .chain([drop(parent)])
                .collect(),
        );
        f.assign(owner, Rvalue::Use(take(projected)), cleanup);
        owners.push(owner);
    }
    let cleanup = f.cleanup(owners.iter().rev().copied().map(drop).collect());
    f.body().blocks[0].instructions.push(inst(
        InstructionKind::Drop {
            place: PlaceId(parent),
            if_live: false,
        },
        cleanup,
    ));
    // A deterministic check stands at the first omitted field's fallible
    // finalizer boundary. V independently requires its complete live suffix.
    let flag = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
    f.assign(flag, Rvalue::Constant(Constant::Bool(false)), cleanup);
    let mut suffix = owners.iter().rev().copied().collect::<Vec<_>>();
    match mutation {
        Mutation::LostSibling => {
            suffix.remove(1);
        }
        Mutation::ReversedSuffix => suffix.swap(0, 1),
        Mutation::Duplicate => suffix.insert(1, suffix[0]),
        _ => (),
    }
    let actions = suffix
        .into_iter()
        .enumerate()
        .map(|(i, place)| {
            if matches!(mutation, Mutation::Duplicate) && i == 1 {
                CleanupAction::Drop {
                    place: PlaceId(place),
                    if_live: false,
                }
            } else {
                drop(place)
            }
        })
        .collect();
    let boundary = f.cleanup(actions);
    f.body().blocks[0].instructions.push(inst(
        InstructionKind::Check {
            condition: operand(flag, UseKind::Borrow),
            rule: const { hirsel_diag::RuleId::of("SEM-1") },
        },
        boundary,
    ));
    f.body().blocks[0].terminator = ret(0, cleanup);
    if matches!(mutation, Mutation::Foreign) {
        let mut path = f.definition.path.path().clone();
        path.0.modules = vec!["client".into()];
        f.definition.path = DefId::intern(&path);
    }
    f
}

#[test]
fn every_bound_position_has_a_complete_ordered_failure_suffix() {
    for bound in 0..3 {
        fixture(bound, Mutation::None).valid();
    }
}
#[test]
fn losing_reordering_or_duplicating_an_omitted_owner_is_rejected() {
    for bound in 0..3 {
        for mutation in [
            Mutation::LostSibling,
            Mutation::ReversedSuffix,
            Mutation::Duplicate,
        ] {
            let violations = fixture(bound, mutation).kinds();
            assert!(
                violations.iter().any(|v| matches!(
                    v,
                    ViolationKind::CleanupMismatch { .. }
                        | ViolationKind::CleanupOrder(_)
                        | ViolationKind::ReadBeforeInit(_)
                        | ViolationKind::DoubleMove(_)
                        | ViolationKind::UseAfterMove(_)
                )),
                "{bound} {mutation:?}: {violations:?}"
            );
        }
    }
}
#[test]
fn only_one_local_commit_can_suppress_the_parent() {
    for mutation in [Mutation::Foreign, Mutation::Twice] {
        assert!(
            fixture(1, mutation)
                .kinds()
                .iter()
                .any(|v| matches!(v, ViolationKind::InvalidUse(_)))
        );
    }
}
