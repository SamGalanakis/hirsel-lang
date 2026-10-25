//! CAP/uses call-site controls: no host effects are performed.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_types::QueryError;

fn check(source: &str, expected: Option<&str>) {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add("authority-call-22.hir", source.into())
            .unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert_eq!(
            output.diagnostics.first().map(|d| d.rule.as_str()),
            expected,
            "{source}\n{:?}",
            output.diagnostics
        );
        if let Some(rule) = expected {
            let site = engine.sources.slice(output.diagnostics[0].primary).unwrap();
            assert!(
                if rule == "CLO-4" {
                    site == "effect"
                } else {
                    site.starts_with("effect(") || site.starts_with("f(")
                },
                "{rule} at {site:?}"
            );
        }
        if expected.is_none() {
            assert_eq!(output.completion, CheckCompletion::Complete);
            match engine.lower_to_tir(&entry_path(&default_context())) {
                Ok(_) => {
                    let input = engine
                        .export_native_input()
                        .unwrap_or_else(|e| panic!("{source}\n{e:?}"));
                    let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
                    for signature in input.signatures.values() {
                        signatures.insert(signature.definition, (**signature).clone());
                    }
                    let definition = input
                        .definitions
                        .get(&entry_path(&default_context()))
                        .unwrap();
                    hirsel_validate::assert_valid(definition, engine.types(), &signatures);
                }
                Err(QueryError::Diagnostics(d)) => panic!("{source}\n{d:?}"),
                Err(e) => panic!("{source}\n{e:?}"),
            }
        }
    });
}

#[test]
fn authority_call_22_explicit_callable_seed() {
    for (binding, expected) in [
        ("let f=effect", None),
        ("let f:fn(arg:Dir)->() uses {arg}=effect", None),
        ("let f:fn(arg:Dir)->() uses {Dir}=effect", None),
        ("let f:fn(arg:Dir)->() uses {}=effect", Some("CLO-4")),
    ] {
        check(
            &format!(
                "fn effect(d:Dir) uses {{d}} {{}}\npub fn main(d:Dir) uses {{d}} {{{binding};f(d)}}"
            ),
            expected,
        );
    }
}

#[test]
fn authority_call_22_call_forms() {
    for (definition, binding, call) in [
        ("fn effect(d:Dir,n:Net) uses {d,n} {}", "", "effect(d,n)"),
        (
            "fn effect(d:Dir,n:Net) uses {d,n} {}",
            "let f=effect;",
            "f(d,n)",
        ),
        (
            "fn effect(d d:Dir,n n:Net) uses {d,n} {}",
            "",
            "effect(n:n,d:d)",
        ),
        ("fn effect[A,B](d:A,n:B) uses {d,n} {}", "", "effect(d,n)"),
        (
            "fn effect[A,B](d:A,n:B) uses {d,n} {}",
            "let f:fn(Dir,Net)->()=effect;",
            "f(d,n)",
        ),
    ] {
        for (allowed, expected) in [
            ("Dir,Net", None),
            ("Dir", Some("CAP-1")),
            ("Net", Some("CAP-1")),
            ("", Some("CAP-1")),
        ] {
            check(
                &format!(
                    "{definition}\npub fn main(d:Dir,n:Net) uses {{{allowed}}} {{{binding}{call}}}"
                ),
                expected,
            );
        }
    }
}

#[test]
fn authority_call_22_mixed_record_authority() {
    for (binding, call) in [
        ("", "effect(record)"),
        ("let f:fn(Packet)->()=effect;", "f(record)"),
    ] {
        for (allowed, expected) in [
            ("Dir,Net", None),
            ("Dir", Some("CAP-1")),
            ("Net", Some("CAP-1")),
            ("", Some("CAP-1")),
        ] {
            check(
                &format!(
                    "type Packet {{tag:u64,d:Dir,n:Net}}\nfn effect[S](record:S) uses {{record}} {{}}\npub fn main(d:Dir,n:Net) uses {{{allowed}}} {{let record=Packet(tag:0u64,d:d,n:n);{binding}{call}}}"
                ),
                expected,
            );
        }
    }
}

#[test]
fn authority_call_22_selected_generic_does_not_cover_known_sibling() {
    for (binding, call) in [
        ("", "effect(state,n)"),
        ("let f:fn(S,Net)->()=effect;", "f(state,n)"),
    ] {
        for (allowed, expected) in [
            ("state,Net", None),
            ("state", Some("CAP-1")),
            ("Net", Some("CAP-1")),
        ] {
            check(
                &format!(
                    "fn effect[S](state:S,n:Net) uses {{state,n}} {{}}\npub fn main[S](state:S,n:Net) uses {{{allowed}}} {{{binding}{call}}}"
                ),
                expected,
            );
        }
    }
}

#[test]
fn authority_call_22_only_the_selected_actual_is_charged() {
    for (declaration, binding, call) in [
        (
            "fn effect[A,B](d d:A,n n:B) uses {d} {}",
            "",
            "effect(n:n,d:d)",
        ),
        (
            "fn effect[A,B](d:A,n:B) uses {d} {}",
            "let f:fn(Dir,Net)->()=effect;",
            "f(d,n)",
        ),
    ] {
        for (allowed, expected) in [("Dir", None), ("Net", Some("CAP-1"))] {
            check(
                &format!(
                    "{declaration}\npub fn main(d:Dir,n:Net) uses {{{allowed}}} {{{binding}{call}}}"
                ),
                expected,
            );
        }
    }
}
