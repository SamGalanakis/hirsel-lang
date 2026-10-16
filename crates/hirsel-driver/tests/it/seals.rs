use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_types::{AuthoritySummary, AuthorityTerm, SignatureKind};

fn checked(source: &str) -> QueryEngine {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("seal.hir", source.into()).unwrap();
    let result = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(
        result.diagnostics.is_empty(),
        "{source}: {:?}",
        result.diagnostics
    );
    engine
}

fn path(name: &str) -> DefPath {
    let mut path = entry_path(&default_context());
    path.0.name = name.into();
    path
}

#[test]
fn seals_keep_public_nominal_authority_and_private_ceilings_in_separate_graphs() {
    with_compiler_stack(|| {
        let mut engine =
            checked("@capability(global) pub opaque type ReadDir: plain uses {Dir} { d: Dir }");
        let public = engine.item_signature(&path("ReadDir")).unwrap().value;
        let SignatureKind::Type(d) = &public.kind else {
            panic!("type")
        };
        assert!(public.types.is_empty());
        assert!(d.representation.is_none());
        assert!(d.implementation_authority.is_none());
        assert_eq!(
            d.authority,
            AuthoritySummary::Known(vec![AuthorityTerm::Capability(path("ReadDir"))].into())
        );
        let owner = engine
            .owner_representation(&path("ReadDir"), &path("ReadDir"))
            .unwrap();
        assert!(!owner.types.is_empty());
    });
}

#[test]
fn private_ceiling_edits_change_owner_revisions_but_preserve_public_fingerprints() {
    with_compiler_stack(|| {
        let source = "@capability(global) pub opaque type ReadDir: plain uses {Dir} { d: Dir }\nfn get(r: ReadDir) -> Dir { r.d }";
        let mut engine = checked(source);
        let public = engine.item_signature(&path("ReadDir")).unwrap().value;
        let owner = engine
            .owner_representation(&path("ReadDir"), &path("ReadDir"))
            .unwrap();
        engine.lower_to_tir(&path("get")).unwrap();
        let count = engine.recompute_count(&path("get"), QueryKind::Lower);
        let file = engine
            .sources
            .add(
                "seal.hir",
                source.replace("uses {Dir}", "uses {Dir, Tasks}"),
            )
            .unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let new_public = engine.item_signature(&path("ReadDir")).unwrap().value;
        let new_owner = engine
            .owner_representation(&path("ReadDir"), &path("ReadDir"))
            .unwrap();
        assert_eq!(public.fingerprint, new_public.fingerprint);
        assert_eq!(public.types, new_public.types);
        assert_ne!(
            owner.representation_revision,
            new_owner.representation_revision
        );
        engine.lower_to_tir(&path("get")).unwrap();
        assert!(engine.recompute_count(&path("get"), QueryKind::Lower) > count);
    });
}

#[test]
fn seal_captures_and_projected_values_keep_distinct_authority() {
    with_compiler_stack(|| {
        for (capture, allowed, expected) in [
            ("r", "ReadDir", None),
            ("r", "", Some("CLO-2")),
            ("d", "ReadDir", Some("CLO-2")),
            ("d", "Dir", None),
        ] {
            let source = format!(
                "@capability(global) pub opaque type ReadDir: plain uses {{Dir}} {{ d: Dir }}\nfn restrict(body: fn() -> () uses {{{allowed}}}) {{}}\nfn probe(r: ReadDir) {{ let d = r.d; restrict(fn() {{ let _ = {capture} }}) }}"
            );
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("seal.hir", source.clone()).unwrap();
            let result = check_progress(&mut engine, file, &default_context()).unwrap();
            assert_eq!(
                result.diagnostics.first().map(|d| d.rule.as_str()),
                expected,
                "{source}: {:?}",
                result.diagnostics
            );
            if expected.is_none() {
                engine.lower_to_tir(&path("probe")).unwrap();
            }
        }
    });
}

#[test]
fn hidden_tasks_cannot_enter_an_inert_callback() {
    with_compiler_stack(|| {
        for (payload, ceiling, allowed, expected) in [
            ("Tasks", "Tasks", "", Some("CLO-2")),
            ("Tasks", "Tasks", "Hidden", None),
            ("u64", "", "", Some("CLO-2")),
        ] {
            let source = format!(
                "@capability(global) pub opaque type Hidden: plain uses {{{ceiling}}} {{ payload: {payload} }}\nfn restrict(body: fn() -> () uses {{{allowed}}}) {{}}\nfn probe(h: Hidden) {{ restrict(fn() {{ let _ = h }}) }}"
            );
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("seal.hir", source.clone()).unwrap();
            let result = check_progress(&mut engine, file, &default_context()).unwrap();
            assert_eq!(
                result.diagnostics.first().map(|d| d.rule.as_str()),
                expected,
                "{source}: {:?}",
                result.diagnostics
            );
            if expected.is_none() {
                engine.lower_to_tir(&path("probe")).unwrap();
            }
        }
        let mut engine = checked(
            "fn restrict(body: fn() -> () uses {}) {}\nfn probe(n: u64) { restrict(fn() { let _ = n }) }",
        );
        engine.lower_to_tir(&path("probe")).unwrap();
    });
}

#[test]
fn owner_operations_use_values_derived_from_seal_parameters() {
    with_compiler_stack(|| {
        let mut engine = checked(
            "@capability(global) pub opaque type ReadDir: plain uses {Dir} { d: Dir }\nfn raw(r: ReadDir) -> Dir uses {r} { r.d }\nfn read(r: ReadDir, p: Path) -> Result[Bytes, FileError] uses {r} { let d = r.d; file.read_bytes(d, p) }",
        );
        for name in ["raw", "read"] {
            engine.lower_to_tir(&path(name)).unwrap();
        }
    });
}

#[test]
fn scoped_seals_cannot_escape_through_generic_holders() {
    with_compiler_stack(|| {
        for (kind, result, body, expected) in [
            ("local", "Local", "Local(n: 0)", Some("MODE-6")),
            ("global", "Local", "Local(n: 0)", None),
            (
                "local",
                "Wrap[Local]",
                "Wrap(value: Local(n: 0))",
                Some("MODE-6"),
            ),
            ("global", "Wrap[Local]", "Wrap(value: Local(n: 0))", None),
        ] {
            let source = format!(
                "@capability({kind}) pub opaque type Local: () uses {{}} {{ n: u64 }}\ntype Wrap[T] {{ value: T }}\nfn probe() -> {result} {{ {body} }}"
            );
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("seal.hir", source.clone()).unwrap();
            let result = check_progress(&mut engine, file, &default_context()).unwrap();
            assert_eq!(
                result.diagnostics.first().map(|d| d.rule.as_str()),
                expected,
                "{source}: {:?}",
                result.diagnostics
            );
            if expected.is_none() {
                engine.lower_to_tir(&path("probe")).unwrap();
            }
        }
    });
}

#[test]
fn erased_callable_authority_cannot_hide_a_scoped_sibling() {
    with_compiler_stack(|| {
        for (kind, expected) in [("local", Some("MODE-6")), ("global", None)] {
            let source = format!(
                "@capability({kind}) pub opaque type Local: () uses {{}} {{ n: u64 }}\ntype Wrap[T] {{ value: T }}\nfn probe(take f: fn() -> ()) -> Wrap[(Local, fn() -> ())] {{ Wrap(value: (Local(n: 0), f)) }}"
            );
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("seal.hir", source.clone()).unwrap();
            let result = check_progress(&mut engine, file, &default_context()).unwrap();
            assert_eq!(
                result.diagnostics.first().map(|d| d.rule.as_str()),
                expected,
                "{source}: {:?}",
                result.diagnostics
            );
            if expected.is_none() {
                engine.lower_to_tir(&path("probe")).unwrap();
            }
        }
    });
}

#[test]
fn global_seals_cannot_keep_scoped_callable_views() {
    with_compiler_stack(|| {
        for (passing, expected) in [("", Some("CLO-1")), ("take ", None)] {
            let source = format!(
                "@capability(global) pub opaque type Seal: () uses {{}} {{ body: fn() -> u64 uses {{}} }}\nfn probe({passing}f: fn() -> u64 uses {{}}) -> Seal {{ Seal(body: f) }}"
            );
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("seal.hir", source.clone()).unwrap();
            let result = check_progress(&mut engine, file, &default_context()).unwrap();
            assert_eq!(
                result.diagnostics.first().map(|d| d.rule.as_str()),
                expected,
                "{source}: {:?}",
                result.diagnostics
            );
            if expected.is_none() {
                engine.lower_to_tir(&path("probe")).unwrap();
            }
        }
    });
}

#[test]
fn generic_seal_instantiations_share_one_nominal_permission() {
    with_compiler_stack(|| {
        let mut engine = checked(
            "@capability(global) pub opaque type Facet[T: plain]: plain uses {T} { value: T }\nfn restrict(body: fn() -> () uses {Facet}) {}\nfn probe(a: Facet[u64], b: Facet[Dir]) { restrict(fn() { let _ = a; let _ = b }) }",
        );
        engine.lower_to_tir(&path("probe")).unwrap();
    });
}

#[test]
fn a_private_ceiling_can_conservatively_name_its_own_seal() {
    with_compiler_stack(|| {
        let mut engine = checked(
            "@capability(global) pub opaque type Facet: plain uses {Facet} { n: u64 }\nfn probe(n: u64) -> Facet { Facet(n: n) }",
        );
        engine.lower_to_tir(&path("probe")).unwrap();
    });
}
