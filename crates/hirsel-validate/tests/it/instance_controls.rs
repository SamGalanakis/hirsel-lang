//! CT-5 instance-body tables, hand-built: a generic generated declaration
//! `id[T](x: T) -> T` with its instance at u64.
use crate::support::*;
use hirsel_syntax::{DefId, DefPath, PassingMode};
use hirsel_tir::*;
use hirsel_types::*;
use hirsel_validate::{InstanceViolation, validate_instance_bodies};
use std::collections::BTreeMap;
use std::sync::Arc;

const U64: TypeKind = TypeKind::Primitive(Primitive::U64);

fn declaration() -> Signature {
    let mut s = function_signature(
        "id",
        vec![(
            PassingMode::Borrow,
            TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0))),
        )],
        TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0))),
    );
    let SignatureKind::Function(f) = &mut s.kind else {
        unreachable!()
    };
    f.generics = vec![GenericParameter::Type {
        name: "T".into(),
        bounds: vec![].into(),
    }]
    .into();
    s
}

struct Instances {
    bodies: InstanceBodies,
    signatures: BTreeMap<DefPath, SignatureRef>,
    definitions: BTreeMap<DefPath, DefinitionRef>,
}

/// The declaration, its instance `id$instance` at u64 whose header is
/// `instance_result`, and a caller `main` that references `id[u64]`.
fn table(instance_result: TypeKind) -> Instances {
    let types = interner();
    let argument = vec![GenericArgument::Type(types.push(ty(U64)))];
    let key = InstanceBodies::key(types, &argument).unwrap();
    let instance = path("id$instance");
    let mut bodies = InstanceBodies::default();
    bodies.declarations.insert(id("id"));
    bodies.instances.entry(id("id")).or_default().insert(
        key,
        InstanceBody {
            arguments: argument,
            definition: DefId::intern(&instance),
        },
    );
    let mut instance_definition = Fixture::unit().definition;
    instance_definition.path = DefId::intern(&instance);
    let mut caller = Fixture::unit();
    let u64 = caller.definition.types.push(ty(U64));
    caller.body().rvalues.push(Rvalue::FunctionReference {
        definition: path("id"),
        arguments: vec![GenericArgument::Type(u64)],
    });
    caller.definition.path = id("main");
    Instances {
        bodies,
        signatures: BTreeMap::from([
            (path("id"), Arc::new(declaration())),
            (
                instance.clone(),
                Arc::new(function_signature(
                    "id$instance",
                    vec![(PassingMode::Borrow, U64)],
                    instance_result,
                )),
            ),
            (
                path("main"),
                Arc::new(function_signature(
                    "main",
                    vec![],
                    TypeKind::Tuple(vec![].into()),
                )),
            ),
        ]),
        definitions: BTreeMap::from([
            (instance, Arc::new(instance_definition)),
            (path("main"), Arc::new(caller.definition)),
        ]),
    }
}

fn violations(t: &Instances) -> Vec<InstanceViolation> {
    validate_instance_bodies(interner(), &t.bodies, &t.signatures, &t.definitions)
}

#[test]
fn an_instance_header_other_than_the_declaration_at_its_arguments_is_rejected() {
    let v = violations(&table(TypeKind::Primitive(Primitive::U32)));
    assert!(
        matches!(&v[..], [InstanceViolation::Header { declaration, .. }] if *declaration == path("id")),
        "{v:?}"
    );
}

#[test]
fn a_closed_reference_to_a_declaration_without_its_row_is_rejected() {
    let mut t = table(U64);
    t.bodies.instances.clear();
    let v = violations(&t);
    assert!(
        matches!(&v[..], [InstanceViolation::Unselected { owner, declaration }]
            if *owner == path("main") && *declaration == path("id")),
        "{v:?}"
    );
}

#[test]
fn a_declaration_with_a_definition_of_its_own_is_rejected() {
    let mut t = table(U64);
    let mut direct = Fixture::unit().definition;
    direct.path = id("id");
    t.definitions.insert(path("id"), Arc::new(direct));
    let v = violations(&t);
    assert!(
        matches!(&v[..], [InstanceViolation::Declaration(declaration)] if *declaration == path("id")),
        "{v:?}"
    );
}
