//! The IR checker's own judgment on rules the source phases also enforce:
//! each test lowers accepted source and validates it against a signature
//! universe or a body the source phases would have rejected.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_validate::{ViolationKind, validate};
use std::collections::BTreeMap;

type Signatures = hirsel_validate::SignatureMap;

/// One interner for every engine here, so signatures from separate
/// lowerings combine in one universe.
fn types() -> &'static std::sync::Arc<hirsel_types::TypeInterner> {
    static TYPES: std::sync::OnceLock<std::sync::Arc<hirsel_types::TypeInterner>> =
        std::sync::OnceLock::new();
    TYPES.get_or_init(hirsel_engine::registry::session_types)
}
fn new_engine() -> QueryEngine {
    QueryEngine::new(default_environment(), compiler_adapters()).with_types(types().clone())
}

fn lowered(source: &str, name: &str) -> (hirsel_tir::Definition, Signatures) {
    let mut engine = new_engine();
    let file = engine.sources.add("agreement.hir", source.into()).unwrap();
    let result = check_progress(&mut engine, file, &default_context())
        .unwrap_or_else(|e| panic!("{source}\n{e:?}"));
    assert!(
        result.diagnostics.is_empty(),
        "{source}\n{:?}",
        result.diagnostics
    );
    let input = engine.export_native_input().unwrap();
    let mut path = entry_path(&default_context());
    path.0.name = name.into();
    let mut signatures = Signatures::new(types().clone());
    signatures.extend(input.signatures.iter().map(|(p, s)| (p, (**s).clone())));
    ((*input.definitions[&path]).clone(), signatures)
}
/// `signatures` with the signature of `name` replaced by the one `source` declares.
fn with(signatures: &Signatures, source: &str, name: &str) -> Signatures {
    let (definition, replacement) = lowered(source, name);
    let mut signatures = signatures.clone();
    signatures.insert(
        definition.path.clone(),
        replacement.get(definition.path).unwrap().clone(),
    );
    signatures
}
fn kinds(definition: &hirsel_tir::Definition, signatures: &Signatures) -> Vec<ViolationKind> {
    // A forged body's own type closure, as lowering would have recorded it.
    let mut definition = definition.clone();
    definition.types = hirsel_tir::definition_types(types(), &definition);
    validate(&definition, types(), signatures, &mut vec![])
        .violations
        .into_iter()
        .map(|v| v.kind)
        .collect()
}

#[test]
fn a_mixed_local_result_retains_only_its_local_argument_storage() {
    with_compiler_stack(|| {
        for expression in [
            "pick(xs, b\"fallback\")",
            "(fn() { pick(xs, b\"fallback\") })()",
        ] {
            let source = format!(
                "fn pick(xs: List[Bytes], x: Bytes) -> Bytes in {{xs, x}} {{ x }}\nfn f(xs: List[Bytes]) -> Bytes in xs {{ {expression} }}\nfn main() {{}}"
            );
            let (f, signatures) = lowered(&source, "f");
            assert_eq!(kinds(&f, &signatures), []);
        }

        let source = "fn pick(xs: List[Bytes], x: Bytes) -> Bytes in {xs, x} { x }\nfn f(xs: List[Bytes], x: Bytes) -> Bytes in {xs, x} { pick(xs, x) }\nfn main() {}";
        let (mut forged, signatures) = lowered(source, "f");
        // Hand-built control: keep the actual call and its Local second input,
        // but promise a result dependent only on xs, as the rejected twin does.
        let mut result = types().get(forged.bodies[0].result).unwrap().clone();
        result.modes.relation =
            hirsel_types::Relation::parameters([hirsel_types::ParameterRef::body(
                forged.bodies[0].parameter_scope,
                hirsel_types::ParamId(0),
            )]);
        forged.bodies[0].result = types().intern(result);
        let signatures = with(
            &signatures,
            "fn f(xs: List[Bytes], x: Bytes) -> Bytes in xs { xs[0] }\nfn main() {}",
            "f",
        );
        assert!(
            kinds(&forged, &signatures)
                .iter()
                .any(|kind| matches!(kind, ViolationKind::InvalidResultSource(_)))
        );

        let rejected = source.replace(
            "fn f(xs: List[Bytes], x: Bytes) -> Bytes in {xs, x}",
            "fn f(xs: List[Bytes], x: Bytes) -> Bytes in xs",
        );
        let mut engine = new_engine();
        let file = engine
            .sources
            .add("rejected.hir", rejected.clone())
            .unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        let diagnostic = &output.diagnostics[0];
        assert_eq!(diagnostic.rule.as_str(), "MODE-1");
        let start = output.sources.resolve(diagnostic.primary).start as usize;
        assert!(rejected[start..].starts_with("pick(xs, x)"));
    });
}

#[test]
fn an_indexed_write_detaches_a_copied_containers_identity() {
    with_compiler_stack(|| {
        let header = "type R { n: u64 }\nfn make(r: List[R]) -> Option[u8 of r] { None }\nfn inspect(r: List[R], p: Option[u8 of r]) {}";
        for receipt in ["copy r0", "(fn() { r0 })()"] {
            let source = format!(
                "{header}\nfn probe(take r0: List[R]) {{ let mut r = {receipt}; r[0].n = 3u64; let fresh = make(r); inspect(r, fresh) }}\nfn main() {{}}"
            );
            let (probe, signatures) = lowered(&source, "probe");
            assert_eq!(kinds(&probe, &signatures), []);
        }
        for (owner, accepted) in [("r0", true), ("r", false)] {
            let source = format!(
                "{header}\nfn probe(take r0: List[R]) {{ let mut r = copy r0; let old = make(r); r[0].n = 3u64; inspect({owner}, old) }}\nfn main() {{}}"
            );
            let mut engine = new_engine();
            let file = engine.sources.add("indexed.hir", source.clone()).unwrap();
            let output = check_progress(&mut engine, file, &default_context()).unwrap();
            if accepted {
                assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
            } else {
                let diagnostic = &output.diagnostics[0];
                assert_eq!(diagnostic.rule.as_str(), "PROV-1");
                let start = output.sources.resolve(diagnostic.primary).start as usize;
                assert!(source[start..].starts_with("inspect(r, old)"));
            }
        }
    });
}

#[test]
fn renewed_container_brands_agree_with_independent_validation() {
    with_compiler_stack(|| {
        // Keep the exact current-main accepted vectors, not only reduced
        // versions of their mutation patterns, under independent V as well.
        for source in [
            include_str!("../../../../conformance/PROV/PROV-1-index-container-ok.hir"),
            include_str!("../../../../conformance/PROV/PROV-1-field-index-container-ok.hir"),
            include_str!("../../../../conformance/PROV/PROV-1-for-mut-container-ok.hir"),
            include_str!("../../../../conformance/PROV/PROV-1-refreshed-owner-loop-ok.hir"),
        ] {
            let source = format!("{source}\nfn main() {{}}");
            let (probe, signatures) = lowered(&source, "probe");
            assert_eq!(kinds(&probe, &signatures), [], "{source}");
        }
        // Each original regression gets the same stale, wrong-owner,
        // replacement and copied-owner controls. Loop cases replay the whole
        // sequence so a fresh result must survive the next header judgment.
        for (owner, mutation, replacement, looped) in [
            ("List[u64]", "r[0] = 3u64", "[0u64]", false),
            ("R", "r.xs[0] = 3u64", "R(xs: [0u64], other: 0u64)", false),
            ("List[u64]", "for x in mut r { x = 3u64 }", "[0u64]", false),
            ("R", "touch(mut r)", "R(xs: [0u64], other: 0u64)", true),
        ] {
            let header = format!(
                "type R {{ xs: List[u64], other: u64 }}\nfn make(r: {owner}) -> Option[u8 of r] {{ None }}\nfn inspect(r: {owner}, p: Option[u8 of r]) {{}}\nfn touch(mut r: R) {{ r.xs[0] = 3u64 }}\nfn relay[T](take p: T) -> T {{ take p }}"
            );
            for (receipt, extra, use_value, accepted) in [
                ("r0", "", "inspect(r, fresh)", true),
                ("r0", "", "inspect(r, old)", false),
                ("r0", "", "inspect(other, fresh)", false),
                ("r0", "replace", "inspect(r, fresh)", false),
                ("r0", "again", "inspect(r, newest)", true),
                ("r0", "again", "inspect(r, fresh)", false),
                ("r0", "", "inspect(r, relay(take old))", false),
                ("copy r0", "", "inspect(r, fresh)", true),
                ("copy r0", "", "inspect(r, old)", false),
                ("copy r0", "", "inspect(r0, original)", true),
                ("copy r0", "", "inspect(r0, fresh)", false),
            ] {
                let extra = match extra {
                    "replace" => format!("r = {replacement};"),
                    "again" => format!("{mutation}; let newest = make(r);"),
                    _ => String::new(),
                };
                let body = format!(
                    "let old = make(r); {mutation}; let fresh = make(r); {extra} {use_value}"
                );
                let body = if looped {
                    format!("for i in 0u8..2u8 {{ {body} }}")
                } else {
                    body
                };
                let source = format!(
                    "{header}\nfn probe(take r0: {owner}, other: {owner}) {{ let original = make(r0); let mut r = {receipt}; {body} }}\nfn main() {{}}"
                );
                if accepted {
                    let (probe, signatures) = lowered(&source, "probe");
                    assert_eq!(kinds(&probe, &signatures), [], "{source}");
                } else {
                    let mut engine = new_engine();
                    let file = engine.sources.add("renewal.hir", source.clone()).unwrap();
                    let output = check_progress(&mut engine, file, &default_context()).unwrap();
                    assert!(
                        output
                            .diagnostics
                            .iter()
                            .any(|d| d.rule.as_str() == "PROV-1"),
                        "{source}: {:?}",
                        output.diagnostics,
                    );
                }
            }
        }
        // An indexed write renews the container and its ancestors, preserving
        // the identity of its untouched sibling field.
        for (argument, accepted) in [("same", true), ("whole", false)] {
            let source = format!(
                "type R {{ xs: List[u64], other: u64 }}\nfn sibling(r: R) -> Option[u8 of r.other] {{ None }}\nfn brand(r: R) -> Option[u8 of r] {{ None }}\nfn inspect(r: R, p: Option[u8 of r.other]) {{}}\nfn probe(take r0: R) {{ let mut r = r0; let same = sibling(r); r.xs[0] = 3u64; let whole = brand(r); inspect(r, {argument}) }}\nfn main() {{}}"
            );
            if accepted {
                let (probe, signatures) = lowered(&source, "probe");
                assert_eq!(kinds(&probe, &signatures), []);
            } else {
                let mut engine = new_engine();
                let file = engine.sources.add("sibling.hir", source).unwrap();
                let output = check_progress(&mut engine, file, &default_context()).unwrap();
                assert!(
                    output
                        .diagnostics
                        .iter()
                        .any(|d| d.rule.as_str() == "PROV-1")
                );
            }
        }
    });
}

#[test]
fn renewed_container_brands_survive_continuing_joins() {
    with_compiler_stack(|| {
        for expression in [
            "copy fresh",
            "if true { fresh } else { make(r) }",
            "if true { return } else { fresh }",
        ] {
            let source = format!(
                "fn make(r:List[u64])->Option[u8 of r]{{None}}\nfn inspect(r:List[u64],p:Option[u8 of r]){{}}\nfn probe(take r0:List[u64]){{let mut r=r0;let old=make(r);r[0]=3u64;let fresh=make(r);let selected={expression};inspect(r,selected)}}\nfn main(){{}}"
            );
            let (probe, signatures) = lowered(&source, "probe");
            assert_eq!(kinds(&probe, &signatures), [], "{source}");
            let stale = source.replace("inspect(r,selected)", "inspect(r,old)");
            let mut engine = new_engine();
            let file = engine.sources.add("stale-join.hir", stale.clone()).unwrap();
            let output = check_progress(&mut engine, file, &default_context()).unwrap();
            assert!(
                output
                    .diagnostics
                    .iter()
                    .any(|d| d.rule.as_str() == "PROV-1"),
                "{stale}: {:?}",
                output.diagnostics
            );
        }
    });
}

#[test]
fn forged_container_brands_after_an_indexed_write_are_rejected() {
    with_compiler_stack(|| {
        let header = "type R { n: u64 }\nfn make(r: List[R]) -> Option[u8 of r] { None }\nfn inspect(r: List[R], p: Option[u8 of r]) {}";
        for (receipt, old_owner) in [("copy r0", true), ("(fn() { r0 })()", false)] {
            let source = format!(
                "{header}\nfn probe(take r0: List[R]) {{ let mut r = {receipt}; r[0].n = 3u64; let fresh = make(r); inspect(r, fresh) }}\nfn main() {{}}"
            );
            // Assemble the fixture before publication so this control also runs
            // when the independent indexed-write rule is removed.
            let adapters = compiler_adapters();
            let lower = adapters.lowering.clone().unwrap();
            let mut engine =
                QueryEngine::new(default_environment(), adapters).with_types(types().clone());
            let file = engine.sources.add("forged.hir", source).unwrap();
            let parsed = engine.parse_file(file, &default_context()).unwrap();
            let probe = parsed
                .value
                .items
                .iter()
                .find(|syntax| syntax.item.definition.0.name == "probe")
                .unwrap();
            let indexed = probe
                .tree
                .expressions
                .iter()
                .enumerate()
                .find_map(|(index, e)| {
                    matches!(e.kind, hirsel_syntax::ExprKind::Index { .. })
                        .then_some(hirsel_syntax::ExprId(index as u32))
                })
                .unwrap();
            let received = if receipt == "copy r0" {
                hirsel_types::OriginRoot::Local(probe.owner(), hirsel_types::LocalId(0))
            } else {
                let call = probe
                    .tree
                    .expressions
                    .iter()
                    .enumerate()
                    .find_map(|(index, e)| {
                        matches!(e.kind, hirsel_syntax::ExprKind::Call { .. })
                            .then_some(hirsel_syntax::ExprId(index as u32))
                    })
                    .unwrap();
                hirsel_types::OriginRoot::Expression(probe.owner(), call)
            };
            let mut signatures = hirsel_validate::SignatureMap::new(types().clone());
            signatures.extend_signatures(
                hirsel_engine::registry::BuiltinRegistry::new()
                    .signatures()
                    .map(|s| (**s).clone()),
            );
            for root in &parsed.value.items {
                let path = &root.item.definition;
                let signature = engine.item_signature(path).unwrap();
                signatures.insert(path.clone(), (*signature.value).clone());
            }
            let mut path = entry_path(&default_context());
            path.0.name = "probe".into();
            let analyzed = engine.mode_definition(&path).unwrap();
            assert!(analyzed.diagnostics.is_empty());
            let declared = signatures.get(&path).unwrap();
            let comptime = engine.prepare_comptime(&analyzed.value, declared).unwrap();
            let mut forged = lower
                .lower(
                    types(),
                    engine.type_memo(),
                    &analyzed.value,
                    &comptime,
                    declared,
                )
                .unwrap()
                .value;
            // Hand-built control: every receiving row claims that the post-write
            // result belongs to r0 or the indexed element, rather than the live
            // container. The storage instructions remain honest.
            let scope = forged.bodies[0].parameter_scope;
            let roots: std::collections::HashSet<_> = forged
                .bodies
                .iter()
                .flat_map(|body| {
                    body.locals
                        .iter()
                        .map(|local| local.ty)
                        .chain(body.places.iter().map(|place| place.ty))
                        .chain([body.result])
                })
                .collect();
            let remapped: std::collections::HashMap<_, _> = roots
                .into_iter()
                .map(|root| {
                    let mapped = hirsel_types::map_type_origins(types(), root, |origin, _| {
                        let mut origin = origin.clone();
                        if origin.root == received {
                            origin.root = if old_owner {
                                hirsel_types::OriginRoot::Parameter(
                                    hirsel_types::ParameterRef::body(
                                        scope,
                                        hirsel_types::ParamId(0),
                                    ),
                                )
                            } else {
                                hirsel_types::OriginRoot::Expression(probe.owner(), indexed)
                            };
                            if receipt != "copy r0" {
                                origin.fields.insert(0, hirsel_types::FieldId(0));
                            }
                        }
                        Ok(origin)
                    })
                    .unwrap();
                    (root, mapped)
                })
                .collect();
            for body in &mut forged.bodies {
                for local in &mut body.locals {
                    local.ty = remapped[&local.ty];
                }
                for place in &mut body.places {
                    place.ty = remapped[&place.ty];
                }
                body.result = remapped[&body.result];
            }
            let violations = kinds(&forged, &signatures);
            assert!(
                violations.contains(&ViolationKind::TypeMismatch {
                    entity: "call result"
                }),
                "{receipt} old_owner={old_owner}: {violations:?}"
            );
        }
    });
}

#[test]
fn an_unplaced_take_parameter_refuses_an_arena_value_and_a_borrow_accepts_it() {
    with_compiler_stack(|| {
        let caller = "fn f(a: Arena, take x: Bytes in a) -> () { g(a, take x) }\nfn main() {}";
        let (f, placed) = lowered(
            &format!("fn g(a: Arena, take y: Bytes in a) -> () {{}}\n{caller}"),
            "f",
        );
        assert_eq!(kinds(&f, &placed), []);
        let unplaced = with(
            &placed,
            "fn g(a: Arena, take y: Bytes) -> () {}\nfn main() {}",
            "g",
        );
        assert!(kinds(&f, &unplaced).contains(&ViolationKind::TypeMismatch {
            entity: "call argument"
        }));
        let caller = "fn f(a: Arena, take x: Bytes in a) -> () { g(a, x) }\nfn main() {}";
        let (f, placed) = lowered(
            &format!("fn g(a: Arena, y: Bytes in a) -> () {{}}\n{caller}"),
            "f",
        );
        let unplaced = with(
            &placed,
            "fn g(a: Arena, y: Bytes) -> () {}\nfn main() {}",
            "g",
        );
        assert_eq!(kinds(&f, &unplaced), []);
    });
}

#[test]
fn a_local_result_outlives_only_the_storage_of_a_taken_parameter_it_names() {
    with_compiler_stack(|| {
        let source = "fn view(a: Arena, x: Bytes in a) -> Bytes in x { x }\nfn keep(a: Arena, take x: Bytes in a, take y: Bytes in a) -> Bytes in x { view(a, x) }\nfn main() {}";
        let (keep, signatures) = lowered(source, "keep");
        assert_eq!(kinds(&keep, &signatures), []);
        // The same body, with its result declared local to the other parameter.
        let mut forged = keep.clone();
        let body = &forged.bodies[0];
        let mut result = types().get(body.result).unwrap().clone();
        let Some(sources) = result.modes.relation.named_parameters() else {
            panic!("{result:?}")
        };
        let hirsel_types::ParameterRef::Body { scope, .. } = sources[0] else {
            panic!("{sources:?}")
        };
        // Keep the arena placement; only the lifetime source moves from x to y.
        result.modes.relation = result.modes.relation.map(|r| match r {
            hirsel_types::Reach::Place(hirsel_types::Origin {
                root:
                    hirsel_types::OriginRoot::Parameter(hirsel_types::ParameterRef::Body {
                        parameter: hirsel_types::ParamId(1),
                        ..
                    }),
                fields,
            }) => vec![hirsel_types::Reach::Place(hirsel_types::Origin {
                root: hirsel_types::OriginRoot::Parameter(hirsel_types::ParameterRef::body(
                    scope,
                    hirsel_types::ParamId(2),
                )),
                fields: fields.clone(),
            })],
            r => vec![r.clone()],
        });
        forged.bodies[0].result = types().intern(result);
        let signatures = with(
            &signatures,
            "fn keep(a: Arena, take x: Bytes in a, take y: Bytes in a) -> Bytes in y { y }\nfn main() {}",
            "keep",
        );
        let violations = kinds(&forged, &signatures);
        assert!(
            violations
                .iter()
                .any(|v| matches!(v, ViolationKind::LoanOutlivesSource { .. })),
            "{violations:?}"
        );
    });
}

#[test]
fn a_function_value_copied_from_borrowed_storage_is_an_owned_share_only_if_global() {
    with_compiler_stack(|| {
        let source = "fn first() -> u64 { let fs: Array[fn() -> u64 uses {}] = [fn() -> u64 { 1 }]; (fn() { fs[0]() })() }\nfn main() {}";
        let (first, signatures) = lowered(source, "first");
        assert_eq!(kinds(&first, &signatures), []);
        // The same copy out of an element typed as a scoped function value,
        // received as a global one. The closure reads `fs` as a borrowed
        // view, so its element is already scoped and only the receiver lies.
        let mut forged = first.clone();
        let mut scoped = std::collections::HashMap::new();
        let mut global = std::collections::HashMap::new();
        for body in &mut forged.bodies {
            let function = |ty| {
                matches!(
                    types().get(ty).unwrap().kind,
                    hirsel_types::TypeKind::Function(_)
                )
            };
            if !body
                .places
                .iter()
                .any(|place| !place.projection.is_empty() && function(place.ty))
            {
                continue;
            }
            for index in 0..body.places.len() {
                let ty = body.places[index].ty;
                let row = types().get(ty).unwrap().clone();
                if !matches!(row.kind, hirsel_types::TypeKind::Function(_)) {
                    continue;
                }
                let projected = !body.places[index].projection.is_empty();
                let locality = if projected {
                    hirsel_types::Relation::environment()
                } else if row.modes.relation.has_environment() {
                    hirsel_types::Relation::global()
                } else {
                    continue;
                };
                let forged_rows = if projected { &mut scoped } else { &mut global };
                let forged_ty = *forged_rows.entry(ty).or_insert_with(|| {
                    let mut row = row.clone();
                    row.modes.relation = locality;
                    types().intern(row)
                });
                body.places[index].ty = forged_ty;
                if !projected {
                    let root = body.places[index].root;
                    body.locals[root.0 as usize].ty = forged_ty;
                }
            }
        }
        assert!(!scoped.is_empty() && !global.is_empty());
        // Typing leaves a copy out of borrowed storage to flow's current
        // content proof, which finds the element bounded by the view.
        let violations = kinds(&forged, &signatures);
        assert!(
            violations
                .iter()
                .any(|v| matches!(v, ViolationKind::InvalidResultSource(_))),
            "{violations:?}"
        );
    });
}

#[test]
fn an_unbranded_parameter_borrows_a_branded_data_value_and_a_branded_one_requires_its_brand() {
    with_compiler_stack(|| {
        let rest = "fn fresh() -> List[u64] { [] }\nfn mk(r: List[u64]) -> Option[u8 of r] { None }\nfn probe() -> u8 { let r = fresh(); let q = fresh(); count(q, mk(r)) }\nfn main() {}";
        let (probe, signatures) = lowered(
            &format!("fn count(q: List[u64], v: Option[u8]) -> u8 {{ 0 }}\n{rest}"),
            "probe",
        );
        assert_eq!(kinds(&probe, &signatures), []);
        let branded = with(
            &signatures,
            "fn count(q: List[u64], v: Option[u8 of q]) -> u8 { 0 }\nfn main() {}",
            "count",
        );
        let violations = kinds(&probe, &branded);
        assert!(
            violations.contains(&ViolationKind::TypeMismatch {
                entity: "call argument"
            }),
            "{violations:?}"
        );
    });
}

#[test]
fn a_written_field_of_a_computed_binding_is_named_by_the_binding_and_not_by_a_sibling() {
    with_compiler_stack(|| {
        let rest = "type R { n: u64, other: u64 }\nfn id(x: R) -> R { copy x }\nfn make_n(r: R) -> Option[u8 of r.n] { None }\nfn probe(x: R) { let mut r = id(x); r.n = 3; let q = make_n(r); r.other = 4; inspect(r, q) }\nfn main() {}";
        let (probe, signatures) = lowered(
            &format!("fn inspect(r: R, p: Option[u8 of r.n]) {{}}\n{rest}"),
            "probe",
        );
        assert_eq!(kinds(&probe, &signatures), []);
        let sibling = with(
            &signatures,
            "type R { n: u64, other: u64 }\nfn inspect(r: R, p: Option[u8 of r.other]) {}\nfn main() {}",
            "inspect",
        );
        let violations = kinds(&probe, &sibling);
        assert!(
            violations.contains(&ViolationKind::TypeMismatch {
                entity: "call argument"
            }),
            "{violations:?}"
        );
    });
}

#[test]
fn a_parameter_branded_by_an_uncaptured_outer_owner_carries_that_name_and_no_other() {
    with_compiler_stack(|| {
        let source = "fn outer(x: List[u64], w: List[u64]) {\n  fn inner(y: Option[u8 of x], z: Option[u8 of w]) -> Option[u8 of x] { y }\n}\nfn main() {}";
        let (mut outer, signatures) = lowered(source, "outer");
        assert_eq!(kinds(&outer, &signatures), []);
        // The sole mutation returns the parameter branded by the other owner.
        let body = &mut outer.bodies[1];
        let (y, z) = (body.parameters[0], body.parameters[1]);
        let other = body
            .places
            .iter()
            .position(|p| p.root == z && p.projection.is_empty() && p.loan.is_none())
            .unwrap();
        let mut changed = 0;
        for value in &mut body.rvalues {
            if let hirsel_tir::Rvalue::Use(operand) = value {
                if body.places[operand.place.0 as usize].root == y {
                    operand.place = hirsel_tir::PlaceId(other as u32);
                    changed += 1;
                }
            }
        }
        assert_eq!(changed, 1);
        assert_ne!(kinds(&outer, &signatures), []);
    });
}

#[test]
fn borrowed_capture_physical_elements_keep_the_declared_callable_header() {
    with_compiler_stack(|| {
        let source = "fn first()->u64{let fs:Array[fn()->u64 uses {}]=[fn()->u64{1}];(fn(){fs[0]()})()}\nfn main(){}";
        let (first, signatures) = lowered(source, "first");
        assert_eq!(kinds(&first, &signatures), []);
        let mut forged = first.clone();
        let mut wrong_result = types().get(first.bodies[0].result).unwrap().clone();
        wrong_result.kind = hirsel_types::TypeKind::Primitive(hirsel_types::Primitive::U32);
        let wrong_result = types().intern(wrong_result);
        let mut changed = 0;
        for body in &mut forged.bodies {
            for place in &mut body.places {
                if place.projection.is_empty() {
                    continue;
                }
                let mut row = types().get(place.ty).unwrap().clone();
                if let hirsel_types::TypeKind::Function(fun) = &mut row.kind {
                    fun.result = wrong_result;
                    place.ty = types().intern(row);
                    changed += 1;
                }
            }
        }
        assert!(changed > 0);
        assert!(
            kinds(&forged, &signatures)
                .iter()
                .any(|v| matches!(v, ViolationKind::InvalidProjection(_)))
        );
    });
}

#[test]
fn a_global_environment_cannot_own_a_part_placed_in_scoped_storage() {
    with_compiler_stack(|| {
        // The source phases reject the arena form of this program (PROV-4 at
        // the closure literal); the heap form lowers, and placing the tuple's
        // string part in a region forges the rejected environment.
        let source = "fn probe()->fn()->u64 {let x:String in heap=\"x\";let pair=(x,1u8);fn(){let (s,n)=pair;string.byte_len(s)}}\nfn main(){}";
        let (probe, signatures) = lowered(source, "probe");
        assert_eq!(kinds(&probe, &signatures), []);
        let mut forged = probe.clone();
        let mut retargeted = std::collections::HashMap::new();
        for body in &probe.bodies {
            for ty in body.locals.iter().map(|l| l.ty) {
                let row = types().get(ty).unwrap();
                let hirsel_types::TypeKind::Tuple(parts) = &row.kind else {
                    continue;
                };
                let mut string = types().get(parts[0]).unwrap().clone();
                if string.kind != hirsel_types::TypeKind::Primitive(hirsel_types::Primitive::String)
                {
                    continue;
                }
                string.modes.relation =
                    string.modes.relation.union(&hirsel_types::Relation::new([
                        hirsel_types::Reach::Region(hirsel_types::RegionId(0)),
                    ]));
                let string = types().intern(string);
                let mut tuple = row.clone();
                tuple.kind = hirsel_types::TypeKind::Tuple(vec![string, parts[1]].into());
                let tuple = types().intern(tuple);
                retargeted.entry(ty).or_insert(tuple);
            }
        }
        assert!(!retargeted.is_empty());
        for body in &mut forged.bodies {
            for local in &mut body.locals {
                if let Some(ty) = retargeted.get(&local.ty) {
                    local.ty = *ty;
                }
            }
            for place in &mut body.places {
                if let Some(ty) = retargeted.get(&place.ty) {
                    place.ty = *ty;
                }
            }
        }
        assert!(
            kinds(&forged, &signatures).contains(&ViolationKind::TypeMismatch {
                entity: "closure capture placement"
            }),
            "{:?}",
            kinds(&forged, &signatures)
        );
    });
}

/// Forges every closure type of `definition` whose result is local into one
/// that promises a global result.
fn promise_global_closure_results(definition: &hirsel_tir::Definition) -> hirsel_tir::Definition {
    let mut forged = definition.clone();
    let mut promised = std::collections::HashMap::new();
    for body in &mut forged.bodies {
        for index in 0..body.places.len() {
            let ty = body.places[index].ty;
            let row = types().get(ty).unwrap().clone();
            let hirsel_types::TypeKind::Function(f) = &row.kind else {
                continue;
            };
            let result = types().get(f.result).unwrap().clone();
            if !result.modes.relation.has_environment() {
                continue;
            }
            let forged_ty = *promised.entry(ty).or_insert_with(|| {
                let mut result = result.clone();
                result.modes.relation = hirsel_types::Relation::global();
                let mut f = f.clone();
                f.result = types().intern(result);
                let mut row = row.clone();
                row.kind = hirsel_types::TypeKind::Function(f);
                types().intern(row)
            });
            body.places[index].ty = forged_ty;
            if body.places[index].projection.is_empty() {
                let root = body.places[index].root;
                body.locals[root.0 as usize].ty = forged_ty;
            }
        }
    }
    assert!(!promised.is_empty());
    forged
}

#[test]
fn a_closure_result_is_global_only_over_a_global_environment() {
    // CLO-1: a local body result is local to the environment; the closure
    // type may promise a global one only when every captured source is global
    // and no borrowed non-plain parameter can feed the result.
    with_compiler_stack(|| {
        let global = "fn probe(take x: Bytes) -> Bytes { (fn() { x })() }\nfn main() {}";
        let (probe, signatures) = lowered(global, "probe");
        assert_eq!(kinds(&probe, &signatures), []);
        for source in [
            // A local captured source.
            "fn view(x: Bytes) -> Bytes in x { copy x }\nfn probe(x: Bytes) { let v = view(x); let _ = (fn() { v })(); () }\nfn main() {}",
            // A borrowed non-plain parameter.
            "fn probe(take x: Bytes) { let _ = (fn(y: Bytes) { x })(b\"y\"); () }\nfn main() {}",
        ] {
            let (probe, signatures) = lowered(source, "probe");
            assert_eq!(kinds(&probe, &signatures), [], "{source}");
            let violations = kinds(&promise_global_closure_results(&probe), &signatures);
            assert!(
                violations.contains(&ViolationKind::TypeMismatch {
                    entity: "closure result"
                }),
                "{source}: {violations:?}"
            );
        }
        // A replaced local root whose current content only flow can judge.
        let replaced = "fn view(x: Bytes) -> Bytes in x { copy x }\nfn probe(x: Bytes) { let mut s = view(x); s = view(x); let _ = (fn() { s })(); () }\nfn main() {}";
        let (probe, signatures) = lowered(replaced, "probe");
        assert_eq!(kinds(&probe, &signatures), []);
        let violations = kinds(&promise_global_closure_results(&probe), &signatures);
        assert!(
            violations.contains(&ViolationKind::TypeMismatch {
                entity: "closure capture"
            }),
            "{violations:?}"
        );
    });
}

#[test]
fn a_captured_callable_keeps_the_authority_creation_fixed_and_no_more() {
    // CLO-1: a capture slot is the closure's view of a value whose authority
    // creation fixed, and a computed callee's row records it likewise, so
    // calling either is as effectful as its row says.
    with_compiler_stack(|| {
        for (source, storage) in [
            (
                "fn probe() -> fn() -> u64 {\n let x: String in heap = \"x\"\n let pair = (x, 1u8)\n let inner: fn() -> u64 = fn() { let (s, n) = pair; string.byte_len(s) }\n fn() { (fn() { inner() })() }\n}\nfn main() {}",
                hirsel_tir::Storage::Capture,
            ),
            (
                "fn probe() -> fn() -> u64 {\n let x: String in heap = \"x\"\n let pair = (x, 1u8)\n let inner: fn() -> u64 = fn() { let (s, n) = pair; string.byte_len(s) }\n fn() { (fn() { (fn() { inner })()() })() }\n}\nfn main() {}",
                hirsel_tir::Storage::Temporary,
            ),
        ] {
            let (probe, signatures) = lowered(source, "probe");
            assert_eq!(kinds(&probe, &signatures), []);
            // The same call through a callee whose row admits any authority.
            let mut forged = probe.clone();
            let mut unknown = std::collections::HashMap::new();
            for body in &mut forged.bodies {
                for index in 0..body.places.len() {
                    let place = &body.places[index];
                    if !place.projection.is_empty()
                        || body.locals[place.root.0 as usize].storage != storage
                    {
                        continue;
                    }
                    let ty = place.ty;
                    let row = types().get(ty).unwrap().clone();
                    let hirsel_types::TypeKind::Function(f) = &row.kind else {
                        continue;
                    };
                    let forged_ty = *unknown.entry(ty).or_insert_with(|| {
                        let mut f = f.clone();
                        f.authority = hirsel_types::AuthoritySummary::Unknown;
                        let mut row = row.clone();
                        row.kind = hirsel_types::TypeKind::Function(f);
                        types().intern(row)
                    });
                    let root = body.places[index].root;
                    body.places[index].ty = forged_ty;
                    body.locals[root.0 as usize].ty = forged_ty;
                }
            }
            assert!(!unknown.is_empty());
            let violations = kinds(&forged, &signatures);
            assert!(
                violations.contains(&ViolationKind::TypeMismatch {
                    entity: "body uses"
                }),
                "{source}: {violations:?}"
            );
        }
    });
}

#[test]
fn a_specialised_function_result_is_sendable_only_over_a_sendable_environment() {
    // A function value's sendability is its environment's: a share of a
    // sendable captured function is sendable, of an unsendable one is not.
    with_compiler_stack(|| {
        let source = "fn library(d: Dir, p: Path) -> Result[bool, FileError] {\n let read = file.read_bytes\n let same = (fn() { read })()(d, p)? == read(d, p)?\n Ok(same)\n}\nfn main() {}";
        let (library, signatures) = lowered(source, "library");
        assert_eq!(kinds(&library, &signatures), []);
        let mut forged = library.clone();
        let mut unsendable = std::collections::HashMap::new();
        let body = &mut forged.bodies[0];
        for index in 0..body.places.len() {
            let ty = body.places[index].ty;
            let row = types().get(ty).unwrap().clone();
            if !matches!(row.kind, hirsel_types::TypeKind::Function(_))
                || body.locals[body.places[index].root.0 as usize].storage
                    != hirsel_tir::Storage::Local
            {
                continue;
            }
            let forged_ty = *unsendable.entry(ty).or_insert_with(|| {
                let mut row = row.clone();
                row.modes.sendability = hirsel_types::Sendability::NotSendable;
                types().intern(row)
            });
            let root = body.places[index].root;
            body.places[index].ty = forged_ty;
            body.locals[root.0 as usize].ty = forged_ty;
        }
        assert!(!unsendable.is_empty());
        let violations = kinds(&forged, &signatures);
        assert!(
            violations.contains(&ViolationKind::TypeMismatch {
                entity: "closure result"
            }),
            "{violations:?}"
        );
    });
}

/// Forges every capture slot of `definition` holding a callable with a local
/// result into one that promises a global result.
fn promise_global_captured_results(definition: &hirsel_tir::Definition) -> hirsel_tir::Definition {
    let mut forged = definition.clone();
    let mut promised = std::collections::HashMap::new();
    for body in &mut forged.bodies {
        for index in 0..body.places.len() {
            let place = &body.places[index];
            if !place.projection.is_empty()
                || body.locals[place.root.0 as usize].storage != hirsel_tir::Storage::Capture
            {
                continue;
            }
            let ty = place.ty;
            let row = types().get(ty).unwrap().clone();
            let hirsel_types::TypeKind::Function(f) = &row.kind else {
                continue;
            };
            let result = types().get(f.result).unwrap().clone();
            if !result.modes.relation.has_environment() {
                continue;
            }
            let forged_ty = *promised.entry(ty).or_insert_with(|| {
                let mut result = result.clone();
                result.modes.relation = hirsel_types::Relation::global();
                let mut f = f.clone();
                f.result = types().intern(result);
                let mut row = row.clone();
                row.kind = hirsel_types::TypeKind::Function(f);
                types().intern(row)
            });
            let root = body.places[index].root;
            body.places[index].ty = forged_ty;
            body.locals[root.0 as usize].ty = forged_ty;
        }
    }
    assert!(!promised.is_empty());
    forged
}

#[test]
fn a_closure_capturing_a_closure_local_to_its_environment_is_local_to_the_same_environment() {
    // CLO-1: a closure over consumed local loop content is local to its
    // environment, so another closure's capture of it is a slot local to that
    // environment, whose calls return local results. Hand-built control: the
    // same capture slot promising a global result fails capture typing.
    with_compiler_stack(|| {
        for inner in [
            "let g = fn() { size(f()) }; sum += g()",
            "let h = fn() { f() }; sum += size(h())",
            "let g = fn() { size((fn() { f() })()) }; sum += g()",
        ] {
            let source = format!(
                "fn view(x: Bytes) -> Bytes in x {{ x }}\nfn size(x: Bytes) -> u64 {{ 1 }}\nfn pairs(p: Bytes) -> u64 {{\n let xs: List[(u64, Bytes)] = [(1, view(p))]\n let mut sum: u64 = 0\n for (n, v) in take xs {{ let f = fn() {{ v }}; {inner} }}\n sum\n}}\nfn main() {{}}"
            );
            let (pairs, signatures) = lowered(&source, "pairs");
            assert_eq!(kinds(&pairs, &signatures), [], "{source}");
            let violations = kinds(&promise_global_captured_results(&pairs), &signatures);
            assert!(
                violations.contains(&ViolationKind::TypeMismatch {
                    entity: "closure capture"
                }),
                "{source}: {violations:?}"
            );
        }
    });
}

#[test]
fn a_counted_closure_result_ends_the_uniqueness_of_a_global_borrowed_capture() {
    // MODE-2 (#80): calling a closure for a counted result shares what its
    // environment borrows, whatever its result relation, as reading the part
    // directly does. Hand-built control: each body is lowered against a `bump`
    // that accepts any Rec, then validated against one that requires a unique
    // Rec, as the rejected program declares it.
    with_compiler_stack(|| {
        let unique = "type Rec { items: List[String] }\nfn bump(mut x: unique Rec) { x.items[0] = \"new\" }\nfn main() {}";
        let probe = |read: &str, after: bool| {
            let body = if after {
                format!("bump(mut r); let old = {read}")
            } else {
                format!("let old = {read}; bump(mut r)")
            };
            format!(
                "type Rec {{ items: List[String] }}\nfn bump(mut x: Rec) {{ x.items[0] = \"new\" }}\nfn probe() -> u64 {{ let mut r: unique Rec = Rec(items: [\"old\"]); {body}; 1u64 }}\nfn main() {{}}"
            )
        };
        for read in [
            "(fn() { r.items })()",
            "(fn() { copy r.items })()",
            "r.items",
        ] {
            let (shared, signatures) = lowered(&probe(read, false), "probe");
            let signatures = with(&signatures, unique, "bump");
            let violations = kinds(&shared, &signatures);
            assert!(
                violations
                    .iter()
                    .any(|kind| matches!(kind, ViolationKind::TypeMismatch { .. })),
                "{read}: {violations:?}"
            );
            let (unique_until_bump, signatures) = lowered(&probe(read, true), "probe");
            let signatures = with(&signatures, unique, "bump");
            assert_eq!(kinds(&unique_until_bump, &signatures), [], "{read}");
        }
    });
}

#[test]
fn a_binding_introduced_before_a_loop_continues_across_its_backedge() {
    // PROV-1 (#78): a name bound before a loop and rebound inside it keeps
    // its current incarnation across the backedge, so a brand made after the
    // loop is valid. Hand-built control: a fresh rebinding of that name
    // inserted into the loop body still ends a brand made before the loop.
    with_compiler_stack(|| {
        let header = "type R{k:u64}\nfn make(r:R)->Option[u8 of r]{None}\nfn inspect(r:R,p:Option[u8 of r]){}";
        for (introduced, body) in [
            ("take r0", "r = R(k:1u64)"),
            ("take r0", "r.k = 1u64"),
            ("take r0", "r = r; k = false"),
            ("R(k:0u64)", "r = R(k:1u64)"),
        ] {
            let source = format!(
                "{header}\nfn probe(take r0:R, c:bool) {{ let mut k = c; let mut r = {introduced}; while k {{ {body} }}; inspect(r, make(r)) }}\nfn main() {{}}"
            );
            let (probe, signatures) = lowered(&source, "probe");
            assert_eq!(kinds(&probe, &signatures), [], "{source}");
        }
        let source = format!(
            "{header}\nfn probe(take r0:R, c:bool) {{ let mut r = take r0; let p = make(r); let mut k = c; while k {{ k = false }}; inspect(r, p) }}\nfn main() {{}}"
        );
        let (mut forged, signatures) = lowered(&source, "probe");
        assert_eq!(kinds(&forged, &signatures), []);
        let body = &mut forged.bodies[0];
        let rebind = body.blocks[0]
            .instructions
            .iter()
            .find(|i| {
                matches!(&i.kind, hirsel_tir::InstructionKind::BindOrigin { place, .. }
                    if matches!(body.places[place.0 as usize].binding.as_ref().map(|o| &o.root),
                        Some(hirsel_types::OriginRoot::Local(..))))
            })
            .unwrap()
            .clone();
        let latch = body
            .blocks
            .iter()
            .enumerate()
            .find(|(index, block)| {
                matches!(block.terminator, hirsel_tir::Terminator::Goto(target) if (target.0 as usize) < *index)
            })
            .map(|(index, _)| index)
            .unwrap();
        let hirsel_tir::InstructionKind::Charge(event) = body.blocks[latch].instructions[0].kind
        else {
            panic!("lowering must charge before entering the loop body");
        };
        assert_eq!(event.kind, hirsel_tir::SourceEventKind::Iteration);
        // Moving the charge behind a rebinding is independently invalid (CT-4).
        body.blocks[latch].instructions.insert(0, rebind);
        assert_eq!(
            kinds(&forged, &signatures),
            [ViolationKind::WrongKind {
                entity: "charge placement",
                index: event.source,
            }]
        );
        // Keep the charge first so the forged rebinding reaches PROV-1.
        forged.bodies[0].blocks[latch].instructions.swap(0, 1);
        let violations = kinds(&forged, &signatures);
        assert!(
            violations
                .iter()
                .any(|kind| matches!(kind, ViolationKind::InvalidOrigin(_))),
            "{violations:?}"
        );
    });
}

#[test]
fn a_closure_call_over_arena_only_captures_bounds_a_result_by_its_arena() {
    // #80 with MODE-1: a closure call's result may share what its
    // environment captures. When each capture is arena-only storage of this
    // body, the result holds only its arena bound and read loans on the
    // captures, as the direct argument does, and both checkers accept it.
    with_compiler_stack(|| {
        let header = "fn mk(a: Arena) -> Bytes in a { b\"x\" }\nfn choose(a: Arena, x: Bytes in a, y: Bytes) -> Bytes in {a, x} { x }";
        for body in [
            "let t = mk(a); choose(a, (fn() { t })(), y)",
            "let t = mk(a); let g = fn() { t }; choose(a, g(), y)",
            "choose(a, (fn() { mk(a) })(), y)",
            "let mut t = mk(a); let r = choose(a, (fn() { t })(), y); t = mk(a); mk(a)",
        ] {
            let source = format!(
                "{header}\nfn f(a: Arena, y: Bytes) -> Bytes in a {{ {body} }}\nfn main() {{}}"
            );
            let (f, signatures) = lowered(&source, "f");
            assert_eq!(kinds(&f, &signatures), [], "{source}");
        }
    });
}

#[test]
fn an_arena_only_argument_bounds_a_result_by_its_arena_unless_it_is_taken_storage() {
    // MODE-1: an arena-only argument, computed, named or copied, contributes
    // its arena bound and a read loan on its place, which ends at return.
    // Hand-built control: a taken parameter's storage is not one, so the same
    // body with the parameter dropped from its result relation is invalid.
    with_compiler_stack(|| {
        let header = "fn mk(a: Arena) -> Bytes in a { b\"x\" }\nfn choose(a: Arena, x: Bytes in a, y: Bytes) -> Bytes in {a, x} { x }";
        for body in [
            "choose(a, mk(a), y)",
            "let t = mk(a); choose(a, t, y)",
            "let m = mk(a); choose(a, copy m, y)",
        ] {
            let source = format!(
                "{header}\nfn f(a: Arena, y: Bytes) -> Bytes in a {{ {body} }}\nfn main() {{}}"
            );
            let (f, signatures) = lowered(&source, "f");
            assert_eq!(kinds(&f, &signatures), [], "{source}");
        }
        let source = format!(
            "{header}\nfn f(a: Arena, y: Bytes, take z: Bytes in a) -> Bytes in {{a, z}} {{ choose(a, copy z, y) }}\nfn main() {{}}"
        );
        let (f, signatures) = lowered(&source, "f");
        assert_eq!(kinds(&f, &signatures), []);
        let mut forged = f.clone();
        let mut result = types().get(forged.bodies[0].result).unwrap().clone();
        result.modes.relation = result.modes.relation.map(|r| match r {
            hirsel_types::Reach::Place(hirsel_types::Origin {
                root:
                    hirsel_types::OriginRoot::Parameter(hirsel_types::ParameterRef::Body {
                        parameter: hirsel_types::ParamId(2),
                        ..
                    }),
                ..
            }) => vec![],
            r => vec![r.clone()],
        });
        forged.bodies[0].result = types().intern(result);
        // Every row claims the copy holds only its arena bound, as a checker
        // that treated taken storage as arena-only would lower it.
        let mut strip = |ty: hirsel_types::TypeId| {
            let mut row = types().get(ty).unwrap().clone();
            if !row.modes.relation.has_environment() {
                return ty;
            }
            row.modes.relation = row.modes.relation.map(|r| match r {
                hirsel_types::Reach::Environment => vec![],
                r => vec![r.clone()],
            });
            types().intern(row)
        };
        let body = forged.bodies[0].clone();
        let own = |root: hirsel_tir::LocalId| {
            body.locals[root.0 as usize].storage != hirsel_tir::Storage::Parameter
        };
        let locals: Vec<_> = body
            .locals
            .iter()
            .enumerate()
            .map(|(i, l)| {
                if own(hirsel_tir::LocalId(i as u32)) {
                    strip(l.ty)
                } else {
                    l.ty
                }
            })
            .collect();
        let places: Vec<_> = body
            .places
            .iter()
            .map(|p| if own(p.root) { strip(p.ty) } else { p.ty })
            .collect();
        let body = &mut forged.bodies[0];
        for (local, ty) in body.locals.iter_mut().zip(locals) {
            local.ty = ty;
        }
        for (place, ty) in body.places.iter_mut().zip(places) {
            place.ty = ty;
        }
        let mut signatures = with(
            &signatures,
            &format!(
                "{header}\nfn f(a: Arena, y: Bytes, take z: Bytes in a) -> Bytes in a {{ mk(a) }}\nfn main() {{}}"
            ),
            "f",
        );
        // Only the declared result relation changes. Private uniqueness facts
        // still describe the original body, not the replacement header's stub.
        signatures.get_mut(f.path).unwrap().metadata.inferred_unique = f.inferred_unique.clone();
        let violations = kinds(&forged, &signatures);
        assert!(
            violations.iter().any(|v| matches!(
                v,
                ViolationKind::TypeMismatch {
                    entity: "call result"
                }
            )),
            "{violations:?}"
        );
    });
}

#[test]
fn a_closure_call_over_a_taken_capture_is_taken_storage_for_the_ir_checker() {
    // #80 with P2: a closure call's result may share a taken parameter's
    // storage the closure captures, so the IR checker follows unnamed storage
    // (here a let-kept temporary) through the call into the captures and
    // bounds a dependent result by the frame, as the source checker does and
    // as for `copy z`. A unique conversion keeps its operand's storage. Hand-built control: the same body lowered as a checker
    // that took the call for arena-only storage would lower it is invalid.
    with_compiler_stack(|| {
        let id = "fn id(a: Arena, x: Bytes in a) -> Bytes in {a, x} { x }";
        for argument in ["(fn() { z })()", "unique copy z"] {
            let source = format!(
                "{id}\nfn f(a: Arena, take z: Bytes in a) -> u64 {{ let r = id(a, {argument}); 1u64 }}\nfn main() {{}}"
            );
            let (f, signatures) = lowered(&source, "f");
            assert_eq!(kinds(&f, &signatures), [], "{source}");
            let mut forged = f.clone();
            let mut strip = |ty: hirsel_types::TypeId| {
                let mut row = types().get(ty).unwrap().clone();
                if !row.modes.relation.has_environment()
                    || !matches!(
                        row.kind,
                        hirsel_types::TypeKind::Primitive(hirsel_types::Primitive::Bytes)
                    )
                {
                    return ty;
                }
                row.modes.relation = row.modes.relation.map(|r| match r {
                    hirsel_types::Reach::Environment => vec![],
                    r => vec![r.clone()],
                });
                types().intern(row)
            };
            let body = forged.bodies[0].clone();
            let own = |root: hirsel_tir::LocalId| {
                body.locals[root.0 as usize].storage != hirsel_tir::Storage::Parameter
            };
            let locals: Vec<_> = body
                .locals
                .iter()
                .enumerate()
                .map(|(i, l)| {
                    if own(hirsel_tir::LocalId(i as u32)) {
                        strip(l.ty)
                    } else {
                        l.ty
                    }
                })
                .collect();
            let places: Vec<_> = body
                .places
                .iter()
                .map(|p| if own(p.root) { strip(p.ty) } else { p.ty })
                .collect();
            let body = &mut forged.bodies[0];
            for (local, ty) in body.locals.iter_mut().zip(locals) {
                local.ty = ty;
            }
            for (place, ty) in body.places.iter_mut().zip(places) {
                place.ty = ty;
            }
            let violations = kinds(&forged, &signatures);
            assert!(
                violations.iter().any(|v| matches!(
                    v,
                    ViolationKind::TypeMismatch {
                        entity: "call result"
                    }
                )),
                "{violations:?}"
            );
        }
    });
}

#[test]
fn a_written_take_keeps_a_records_sibling_relation_but_ends_copies_outside_it() {
    with_compiler_stack(|| {
        // PROV-5: reading the branded field through `c` before the written
        // take does not tie the moved record's relation to the name `c`.
        let source =
            include_str!("../../../../conformance/PROV/PROV-5-field-read-record-take-run.hir");
        for name in ["offset", "main"] {
            let (definition, signatures) = lowered(source, name);
            assert_eq!(kinds(&definition, &signatures), []);
        }
        let source = "resource type Cursor { text: String, pos: StrPos of .text }\nfn probe(take c: Cursor) -> bool { let p = c.pos; let same = p == p; let Cursor(text: text, pos: pos) = take c; same }\nfn main() {}";
        let (mut forged, signatures) = lowered(source, "probe");
        assert_eq!(kinds(&forged, &signatures), []);
        // Hand-built control: end `c` before its copied field is compared.
        // The copy lives outside the taken storage, so its brand ends (PROV-1).
        let body = &mut forged.bodies[0];
        let copied = body.blocks[0]
            .instructions
            .iter()
            .position(|i| {
                matches!(&i.kind, hirsel_tir::InstructionKind::BindOrigin { place, .. }
                    if matches!(body.places[place.0 as usize].binding.as_ref().map(|o| &o.root),
                        Some(hirsel_types::OriginRoot::Local(..))))
            })
            .unwrap();
        let hirsel_tir::InstructionKind::BindOrigin { place: copy, .. } =
            body.blocks[0].instructions[copied].kind
        else {
            unreachable!()
        };
        let instructions = &mut body.blocks[0].instructions;
        let end = instructions
            .iter()
            .position(|i| matches!(i.kind, hirsel_tir::InstructionKind::EndOrigin(_)))
            .unwrap();
        let end = instructions.remove(end);
        instructions.insert(copied + 1, end);
        let reads = forged.bodies[0]
            .rvalues
            .iter()
            .find_map(|value| match value {
                hirsel_tir::Rvalue::Binary { left, right, .. } => Some([left.place, right.place]),
                _ => None,
            })
            .unwrap();
        let violations = kinds(&forged, &signatures);
        assert!(!violations.contains(&ViolationKind::InvalidOrigin(copy)));
        for read in reads {
            assert!(
                violations.contains(&ViolationKind::InvalidOrigin(read)),
                "{violations:?}"
            );
        }
        // Hand-built control: end `c` as soon as it is bound, before `pos`,
        // whose owner moved into `c`, is compared. `pos` lives outside the
        // taken storage, so the owner's identity ends for it (PROV-1).
        let source = "resource type Cursor { text: String, pos: StrPos of .text }\nfn probe(t: Test) { let text: String = \"ab\"; match string.position(text, 0u64) { Some(pos) => { let c = Cursor(text: text, pos: pos); let same = pos == pos; let Cursor(text: x, pos: y) = take c; test.check(t, same) }, None => () } }\nfn main() {}";
        let (mut forged, signatures) = lowered(source, "probe");
        assert_eq!(kinds(&forged, &signatures), []);
        let block = forged.bodies[0]
            .blocks
            .iter_mut()
            .find(|b| {
                b.instructions
                    .iter()
                    .any(|i| matches!(i.kind, hirsel_tir::InstructionKind::EndOrigin(_)))
            })
            .unwrap();
        let end = block
            .instructions
            .iter()
            .position(|i| matches!(i.kind, hirsel_tir::InstructionKind::EndOrigin(_)))
            .unwrap();
        let end = block.instructions.remove(end);
        let hirsel_tir::InstructionKind::EndOrigin(ended) = end.kind else {
            unreachable!()
        };
        let bound = block
            .instructions
            .iter()
            .position(|i| {
                matches!(i.kind, hirsel_tir::InstructionKind::BindOrigin { place, .. } if place == ended)
            })
            .unwrap();
        block.instructions.insert(bound + 1, end);
        let reads = forged.bodies[0]
            .rvalues
            .iter()
            .find_map(|value| match value {
                hirsel_tir::Rvalue::Binary { left, right, .. } => Some([left.place, right.place]),
                _ => None,
            })
            .unwrap();
        let violations = kinds(&forged, &signatures);
        for read in reads {
            assert!(
                violations.contains(&ViolationKind::InvalidOrigin(read)),
                "{violations:?}"
            );
        }
    });
}

#[test]
fn nested_joined_global_views_keep_their_access_loans_across_block_extraction() {
    with_compiler_stack(|| {
        let source = r#"
fn touch(s: String) {}
fn probe() {
  let a = "a"
  let b = "b"
  let choices: List[bool] = [true, false]
  for select in choices {
    let chosen = match (if select { if select { a } else { b } } else { b }) { value => value }
    let again = match chosen { value => value }
    touch(again)
    touch(chosen)
  }
}
pub fn main() -> Result[(), ()] { probe(); Ok(()) }
"#;
        let (probe, signatures) = lowered(source, "probe");
        assert_eq!(kinds(&probe, &signatures), []);
        let mut engine = new_engine();
        let file = engine.sources.add("joined.hir", source.into()).unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
        native::build(&validated, &options).unwrap();

        // A whole forwarding chain supplies no permission for an ordinary
        // counted-storage read whose original loan evidence is missing.
        let mut forged = native::program(&validated);
        for definition in forged.definitions.values_mut() {
            for body in &mut std::sync::Arc::make_mut(definition).bodies {
                for place in &mut body.places {
                    place.loan = None;
                }
            }
        }
        let error = hirsel_rc::import::lower(&forged, &options.target)
            .err()
            .expect("forwarding cannot manufacture a loan");
        assert!(error.to_string().contains("outside any loan"), "{error}");
    });
}

#[test]
fn pattern_payload_places_keep_declaration_locality_beside_local_mut_content() {
    with_compiler_stack(|| {
        let source = r#"
type Item { name: String }
type Err = Archive(Reason) | Other
type Reason = Truncated | Header(String)
type State { text: String }
fn next(mut s: State) -> Result[Option[Item], Err] { Ok(None) }
fn error_kind(e: Err) -> u64 {
  match e { Archive(Truncated) => 1, Archive(Header(_)) => 2, _ => 3 }
}
fn scan(data: String) {
  let mut s = State(text: data)
  while true {
    match next(mut s) {
      Ok(None) => return,
      Error(e) => { error_kind(e); return },
      Ok(Some(_)) => {},
    }
  }
}
fn main() {}
"#;
        let (scan, signatures) = lowered(source, "scan");
        assert_eq!(kinds(&scan, &signatures), []);
    });
}

#[test]
fn nested_loop_origins_remain_live_until_their_own_iteration_or_explicit_end() {
    with_compiler_stack(|| {
        let prelude = "type R { x:u64 }\nfn owner()->R{R(x:0)}\nfn brand(r:R)->Option[u8 of r]{None}\nfn read(r:R,mut token:Option[u8 of r])->Result[Option[String],()]{Ok(None)}";
        for middle in [
            "let r=owner();let mut token=brand(r);while let Some(line)=read(r,mut token)?{}",
            "while true {let r=owner();let mut token=brand(r);while let Some(line)=read(r,mut token)?{};break}",
            "let r=owner();let mut token=brand(r);while let Some(line)=read(r,mut token)?{continue 'outer}",
            "let r=owner();let mut token=brand(r);while let Some(line)=read(r,mut token)?{break 'outer}",
        ] {
            let source = format!(
                "{prelude}\nfn probe()->Result[(),()]{{let ns:List[u64]=[0,1];'outer:for n in ns{{{middle}}};Ok(())}}\nfn main(){{}}"
            );
            let (probe, signatures) = lowered(&source, "probe");
            assert_eq!(kinds(&probe, &signatures), [], "{source}");
            let mut forged = probe;
            let body = &mut forged.bodies[0];
            let mut ended = 0;
            for block in &mut body.blocks {
                if let hirsel_tir::Terminator::Call {
                    callee: hirsel_tir::Callee::Definition { path, .. },
                    arguments,
                    ..
                } = &block.terminator
                {
                    if path.0.name.as_str() == "read" {
                        let token = body.places[arguments[1].value.place.0 as usize].ty;
                        let mut origins = Vec::new();
                        hirsel_types::visit_type_origins(types(), token, |origin, _| {
                            origins.push(origin.clone());
                            Ok(())
                        })
                        .unwrap();
                        let owner = body
                            .places
                            .iter()
                            .position(|p| {
                                p.loan.is_none()
                                    && p.binding
                                        .as_ref()
                                        .is_some_and(|name| origins.contains(name))
                            })
                            .expect("branded token names its original owner");
                        block.instructions.push(hirsel_tir::Instruction {
                            kind: hirsel_tir::InstructionKind::EndOrigin(hirsel_tir::PlaceId(
                                owner as u32,
                            )),
                            span: block.span,
                            on_failure: hirsel_tir::CleanupId(0),
                        });
                        ended += 1;
                    }
                }
            }
            assert_eq!(ended, 1);
            let violations = kinds(&forged, &signatures);
            assert!(
                violations
                    .iter()
                    .any(|v| matches!(v, ViolationKind::InvalidOrigin(_))),
                "{source}: {violations:?}"
            );
        }
    });
}

#[test]
fn nested_copied_prefix_brands_reject_forged_roots_and_siblings() {
    with_compiler_stack(|| {
        for argument in ["r.a", "s"] {
            let source = format!(
                r#"
type Inner {{ n: u64, other: u64 }}
type R {{ a: Inner, b: Inner }}
fn make_i(r: Inner) -> Option[u8 of r.other] {{ None }}
fn inspect_i(r: Inner, p: Option[u8 of r.other]) {{}}
fn probe(take x: R, y: R) {{
  let mut r = copy x
  r.a.n = 3
  let s = copy r.a
  let q = make_i({argument})
  inspect_i({argument}, q)
}}
fn main() {{}}
"#
            );
            let (probe, signatures) = lowered(&source, "probe");
            assert_eq!(kinds(&probe, &signatures), []);
            let body = &probe.bodies[0];
            let token = body
                .blocks
                .iter()
                .find_map(|b| match &b.terminator {
                    hirsel_tir::Terminator::Call {
                        callee: hirsel_tir::Callee::Definition { path, .. },
                        returns: Some(ret),
                        ..
                    } if path.0.name.as_str() == "make_i" => {
                        Some(body.places[ret.destination.0 as usize].ty)
                    }
                    _ => None,
                })
                .unwrap();
            let mut origins = vec![];
            hirsel_types::visit_type_origins(types(), token, |o, _| {
                origins.push(o.clone());
                Ok(())
            })
            .unwrap();
            assert_eq!(origins.len(), 1);
            let original = &origins[0];
            assert_eq!(
                original.fields.iter().copied().collect::<Vec<_>>(),
                [hirsel_types::FieldId(0), hirsel_types::FieldId(1)]
            );
            let owner = body
                .places
                .iter()
                .find(|p| {
                    p.root == body.parameters[0] && p.projection.is_empty() && p.binding.is_some()
                })
                .unwrap()
                .binding
                .as_ref()
                .unwrap();
            assert_eq!(original.root, owner.root);
            let foreign = body
                .places
                .iter()
                .find(|p| {
                    p.root == body.parameters[1] && p.projection.is_empty() && p.binding.is_some()
                })
                .unwrap()
                .binding
                .as_ref()
                .unwrap();
            for wrong_root in [true, false] {
                let mut replacement = original.clone();
                if wrong_root {
                    replacement.root = foreign.root.clone();
                } else {
                    replacement.fields[0] = hirsel_types::FieldId(1);
                }
                assert_ne!(&replacement, original);
                let rewrite = |ty| {
                    hirsel_types::map_brand_origins(types(), ty, |o, _| {
                        Ok(if o == original {
                            replacement.clone()
                        } else {
                            o.clone()
                        })
                    })
                    .unwrap()
                };
                let mut forged = probe.clone();
                for body in &mut forged.bodies {
                    for local in &mut body.locals {
                        local.ty = rewrite(local.ty);
                    }
                    for place in &mut body.places {
                        place.ty = rewrite(place.ty);
                    }
                }
                let violations = kinds(&forged, &signatures);
                assert_eq!(
                    violations,
                    [
                        ViolationKind::TypeMismatch {
                            entity: "call result"
                        },
                        ViolationKind::TypeMismatch {
                            entity: "call argument"
                        }
                    ],
                    "{argument}, wrong_root={wrong_root}"
                );
            }
        }
    });
}

#[test]
fn nested_copied_prefix_brands_deep_copies_agree() {
    with_compiler_stack(|| {
        let source =
            include_str!("../../../../conformance/PROV/PROV-1-copied-prefix-nested-deep-ok.hir");
        let (probe, signatures) = lowered(source, "probe");
        assert_eq!(kinds(&probe, &signatures), []);
    });
}
