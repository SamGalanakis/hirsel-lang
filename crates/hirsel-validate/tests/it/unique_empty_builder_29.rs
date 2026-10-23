//! Independent freshness adversaries: no source ownership facts reach V.
use crate::support::*;
use hirsel_syntax::PassingMode;
use hirsel_tir::*;
use hirsel_types::*;
use hirsel_validate::ViolationKind;

// mode 0 builds []; mode 1 substitutes a counted copy of an aliased input;
// mode 2 takes that input; mode 3 copies an input that entered unique.
fn initializer(mode: u8) -> Fixture {
    let mut f = Fixture::unit();
    let string = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::String)));
    let list = TypeKind::Builtin {
        kind: BuiltinType::List,
        arguments: vec![GenericArgument::Type(string)].into(),
    };
    let source = param(&mut f, list.clone());
    if mode == 3 {
        let mut row = interner().row(f.body().places[source as usize].ty).clone();
        row.modes.uniqueness = Uniqueness::Unique;
        let unique = f.definition.types.push(row);
        f.body().places[source as usize].ty = unique;
        f.body().locals[source as usize].ty = unique;
    }
    let values = f.slot(list.clone(), Storage::Local);
    let mut row = ty(list);
    row.modes.uniqueness = Uniqueness::Unique;
    let unique = f.definition.types.push(row);
    // The receiving row claims unique in every twin; only flow can prove it.
    f.body().locals[values as usize].ty = unique;
    f.body().places[values as usize].ty = unique;
    f.body().result = unique;
    f.body().locals[0].ty = unique;
    f.body().places[0].ty = unique;
    f.body().blocks[0].instructions.clear();
    f.body().rvalues.clear();
    let rv = match mode {
        0 => Rvalue::Construct {
            constructor: Constructor::Sequence,
            fields: vec![],
            destination: Destination::Inferred,
        },
        2 => Rvalue::Use(take(source)),
        _ => Rvalue::Use(copy(source)),
    };
    let cleanup = f.cleanup(vec![drop(source)]);
    f.assign(values, rv, cleanup);
    f.body().blocks[0].terminator = ret(values, cleanup);
    owner_sig(&mut f, &[PassingMode::Take]);
    f
}

#[test]
fn unique_empty_builder_29_fresh_empty_initializer_proves_unique() {
    initializer(0).valid();
}

#[test]
fn unique_empty_builder_29_nonfresh_initializers_cannot_forge_unique() {
    for mode in 1..=3 {
        let violations = initializer(mode).kinds();
        assert!(
            violations
                .iter()
                .any(|kind| matches!(kind, ViolationKind::TypeMismatch { .. })),
            "mode {mode}: {violations:?}"
        );
    }
}
