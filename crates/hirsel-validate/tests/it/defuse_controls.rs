//! Independent hand-built controls for explicit finalizer suppression (#97).
use super::*;

#[derive(Clone, Copy)]
enum Mutation {
    None,
    ForeignModule,
    Partial,
    NoFinalizer,
    Twice,
}

fn fixture(mutation: Mutation) -> Fixture {
    let mut f = Fixture::unit();
    let mut signature = function_signature("Token", vec![], TypeKind::Tuple(vec![].into()));
    let scalar = interner().intern(ty(TypeKind::Primitive(Primitive::U64)));
    let representation = interner().intern(ty(TypeKind::Record(
        vec![Field {
            name: "value".into(),
            ty: scalar,
            growable: false,
        }]
        .into(),
    )));
    signature.kind = SignatureKind::Type(Box::new(TypeDefinition {
        loop_source: None,
        format: None,
        destructor_allocation: Allocation::NoAlloc,
        generics: vec![],
        representation: Some(representation),
        bounds: vec![],
        authority: AuthoritySummary::inert(),
        capability: None,
        implementation_authority: None,
        serializable: false,
        serializable_check: None,
        layout: Layout::default(),
        distinct: false,
        resource: true,
        drop: if matches!(mutation, Mutation::NoFinalizer) {
            None
        } else {
            Some(path("Token_drop"))
        },
        fact: None,
        captures: None,
    }));
    f.signatures.insert(id("Token"), signature);
    let input = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Temporary);
    let parent = f.slot(
        TypeKind::Named {
            definition: id("Token"),
            arguments: vec![].into(),
        },
        Storage::Temporary,
    );
    let output = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Local);
    f.assign(input, Rvalue::Constant(Constant::Integer(5)), 0);
    f.assign(
        parent,
        Rvalue::Construct {
            constructor: Constructor::Record(path("Token")),
            fields: vec![ConstructField {
                field: 0,
                value: take(input),
            }],
            destination: Destination::Inferred,
        },
        0,
    );
    let root = f.body().places[parent as usize].root;
    let ty = f.body().places[input as usize].ty;
    let field = f.body().places.len() as u32;
    f.body().places.push(Place {
        loan: None,
        binding: None,
        root,
        projection: vec![Projection::Field(0)].into(),
        ty,
    });
    let cleanup = f.cleanup(vec![drop(parent)]);
    if matches!(mutation, Mutation::Partial) {
        f.assign(output, Rvalue::Use(take(field)), cleanup);
    }
    f.body().blocks[0].instructions.push(inst(
        InstructionKind::Defuse {
            place: PlaceId(parent),
        },
        cleanup,
    ));
    if matches!(mutation, Mutation::Twice) {
        f.body().blocks[0].instructions.push(inst(
            InstructionKind::Defuse {
                place: PlaceId(parent),
            },
            cleanup,
        ));
    }
    if !matches!(mutation, Mutation::Partial) {
        f.assign(output, Rvalue::Use(take(field)), cleanup);
    }
    f.body().blocks[0].terminator = ret(0, cleanup);
    if matches!(mutation, Mutation::ForeignModule) {
        {
            let mut path = f.definition.path.path().clone();
            path.0.modules = vec!["client".into()];
            f.definition.path = DefId::intern(&path);
        }
    }
    f
}

#[test]
fn defuse_requires_the_defining_module() {
    fixture(Mutation::None).valid();
    assert!(
        fixture(Mutation::ForeignModule)
            .kinds()
            .contains(&ViolationKind::InvalidUse(PlaceId(2)))
    );
}
#[test]
fn defuse_requires_all_fields_initialized() {
    fixture(Mutation::None).valid();
    assert!(fixture(Mutation::Partial).kinds().iter().any(|k| matches!(
        k,
        ViolationKind::DoubleMove(_) | ViolationKind::UseAfterMove(_)
    )));
}
#[test]
fn defuse_requires_a_declared_finalizer() {
    fixture(Mutation::None).valid();
    assert!(
        fixture(Mutation::NoFinalizer)
            .kinds()
            .contains(&ViolationKind::InvalidUse(PlaceId(2)))
    );
}
#[test]
fn defuse_cannot_run_twice() {
    fixture(Mutation::None).valid();
    assert!(
        fixture(Mutation::Twice)
            .kinds()
            .contains(&ViolationKind::InvalidUse(PlaceId(2)))
    );
}
