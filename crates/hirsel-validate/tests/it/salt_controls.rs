//! Oracle O1 for V: verdicts do not depend on how session ids are assigned.
use crate::support::*;
use hirsel_syntax::PassingMode;
use hirsel_tir::*;
use hirsel_types::*;

/// Fixtures that reach V's flow, typing, generic, closure, loan, call and
/// type-graph checks, accepted and rejected.
fn fixtures() -> Vec<(String, Fixture)> {
    let mut out = vec![
        ("unit".to_owned(), Fixture::unit()),
        ("string".to_owned(), Fixture::string()),
        ("loan".to_owned(), Fixture::loan()),
        ("call".to_owned(), Fixture::call()),
    ];
    for returning in [false, true] {
        out.push((
            format!("result_call returning={returning}"),
            Fixture::result_call(TypeKind::Primitive(Primitive::String), returning),
        ));
    }
    for outer in [false, true] {
        for inner in [false, true] {
            for alias in [false, true] {
                out.push((
                    format!("captured_generic_copy {outer} {inner} {alias}"),
                    captured_generic_copy(outer, inner, alias),
                ));
            }
        }
    }
    let mut f = Fixture::call();
    let signature = f.signatures.get_mut(id("callee")).unwrap();
    if let SignatureKind::Function(fun) = &mut signature.kind {
        fun.parameters[0].ty = interner().intern(ty(TypeKind::Primitive(Primitive::Bool)));
        fun.parameters[0].passing = PassingMode::Borrow;
    }
    out.push(("call with a mismatched parameter".to_owned(), f));
    let mut f = Fixture::string();
    f.body().blocks[0].terminator = ret(0, 0);
    out.push(("string leaked".to_owned(), f));
    let mut f = Fixture::unit();
    f.definition.types = interner()
        .intern_graph(1, |ids| vec![ty(TypeKind::Box(ids[0]))])
        .into();
    let cycle = f.definition.types[0];
    f.body().result = cycle;
    f.body().locals[0].ty = cycle;
    f.body().places[0].ty = cycle;
    out.push(("type cycle".to_owned(), f));
    out
}

/// Each fixture's verdict, with session ids left out.
fn verdicts() -> String {
    let mut out = String::new();
    let fixtures = fixtures();
    let fixture_count = fixtures.len();
    for (name, f) in fixtures {
        let verdict = hirsel_validate::validate(
            &tabled(&f.definition),
            interner(),
            &f.signatures,
            &mut vec![],
        );
        out += &format!("{name}: {}\n", without_ids(&format!("{verdict:?}")));
    }
    let mutations = crate::checks::mutation_verdicts();
    let mutation_count = mutations.lines().count();
    assert!(fixture_count > 0 && mutation_count > 0);
    out += &without_ids(&mutations);
    out += &format!("executed {fixture_count} fixtures and {mutation_count} mutations\n");
    out
}

/// `text` with the number in every `TypeId(n)` dropped.
fn without_ids(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("TypeId(") {
        out += &rest[..at + "TypeId(".len()];
        rest = rest[at + "TypeId(".len()..].trim_start_matches(|c: char| c.is_ascii_digit());
    }
    out + rest
}

#[test]
fn verdicts_do_not_depend_on_id_assignment() {
    let Some(runs) = hirsel_intern::under_salts(
        "salt_controls::verdicts_do_not_depend_on_id_assignment",
        &[1, 2, 0x5eed],
        verdicts,
    ) else {
        return;
    };
    assert_eq!(runs.len(), 3, "all requested subprocess salts executed");
    assert!(runs.iter().all(|run| *run == runs[0]), "{runs:#?}");
    assert_eq!(
        runs[0],
        verdicts(),
        "salted runs agree with the unsalted one"
    );
    eprintln!(
        "V salt differential: 1, 2, 0x5eed and unsalted; {}",
        runs[0].lines().last().unwrap()
    );
}
