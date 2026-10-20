//! Receiving storage and current content must agree through real lowering.
use crate::source_std;
use hirsel_driver::*;
use hirsel_engine::*;

fn check(source: &str) {
    let mut engine = if source.contains("std@1/") {
        source_std::engine()
    } else {
        QueryEngine::new(default_environment(), compiler_adapters())
    };
    let file = engine.sources.add("receiving.hir", source.into()).unwrap();
    let checked = check_progress(&mut engine, file, &default_context())
        .unwrap_or_else(|error| panic!("{source}\n{error:?}"));
    assert!(
        checked.diagnostics.is_empty(),
        "{source}\n{:?}",
        checked.diagnostics
    );
    assert_eq!(checked.completion, CheckCompletion::Complete);
}

#[test]
fn initialized_local_receivers_supply_global_owned_content() {
    with_compiler_stack(|| {
        for source in [
            "fn total() -> u64 { let mut xs: List[u64] in xs = [1, 2, 3]; let mut sum: u64 = 0; for x in take xs {}; sum }",
            "resource type Trace { text: String } drop(x) {}\nfn probe() { let mut m: Map[u8,Trace] in m = map.empty(); for (k,v) in take m { break } }",
            "fn probe() -> String { let s: String in s = \"fresh\"; copy s }",
        ] {
            check(source);
        }
    });
}

#[test]
fn initialized_local_receivers_cannot_export_borrowed_content_as_global() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let source = "fn probe(x: String) -> String in x { let s: String in s = copy x; copy s }";
        let file = engine.sources.add("receiving.hir", source.into()).unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let input = engine.export_native_input().unwrap();
        let mut path = entry_path(&default_context());
        path.0.name = "probe".into();
        let mut forged = (*input.definitions[&path]).clone();
        let mut signatures: hirsel_validate::SignatureMap = input
            .signatures
            .iter()
            .map(|(path, signature)| (path.clone(), (**signature).clone()))
            .collect();
        let signature = signatures.get_mut(&path).unwrap();
        let hirsel_types::SignatureKind::Function(function) = &mut signature.kind else {
            panic!()
        };
        let mut row = signature.types.get(function.result).unwrap().clone();
        row.modes.relation = hirsel_types::Relation::global();
        function.result = signature.types.push(row);
        let mut row = forged.types.get(forged.bodies[0].result).unwrap().clone();
        row.modes.relation = hirsel_types::Relation::global();
        forged.bodies[0].result = forged.types.push(row);
        let violations = hirsel_validate::validate(&forged, &signatures, &mut vec![]).violations;
        assert!(
            violations.iter().any(|v| matches!(
                v.kind,
                hirsel_validate::ViolationKind::TypeMismatch { entity: "return" }
                    | hirsel_validate::ViolationKind::InvalidResultSource { .. }
            )),
            "{violations:?}"
        );
    });
}

#[test]
fn local_receiving_patterns_use_their_actual_staging_storage() {
    with_compiler_stack(|| {
        for source in [
            "fn probe(tasks: Tasks) { let (sender,receiver): (Sender[u32],Receiver[u32]) in {sender, receiver} = channel.open(tasks) }",
            "fn probe(tasks: Tasks) { let status: Result[u8,Failure] in status = task.isolate(tasks, fn() -> u8 { 1 }); let caught: bool = match status { Ok(value) => false, Error(failure) => true } }",
        ] {
            check(source);
        }
    });
}
#[test]
fn borrowed_unicode_cursors_use_current_content_locality() {
    with_compiler_stack(|| {
        for source in [
            "fn probe() { let s: String in s = \"\"; for x in string.scalars(s) {} }",
            "fn probe() { let s: String in s = \"e\"; for x in string.positions(s) {} }",
            "fn probe() { let s: String in s = \"e\"; for x in std@1/unicode.graphemes(s) {} }",
            "fn probe(s: String) { for x in std@1/unicode.graphemes(s) {} }",
        ] {
            check(source);
        }
    });
}

#[test]
fn borrowed_cursor_items_keep_their_local_source_bound() {
    with_compiler_stack(|| {
        let mut engine = source_std::engine();
        let file = engine.sources.add("receiving.hir", "fn probe(s: String) -> String { for x in std@1/unicode.graphemes(s) { return x }; \"\" }".into()).unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(
            checked
                .diagnostics
                .iter()
                .any(|d| d.rule.as_str() == "MODE-1"),
            "{:?}",
            checked.diagnostics
        );
    });
}

#[test]
fn nested_box_receiving_content_has_its_complete_global_contract() {
    with_compiler_stack(|| {
        check(
            "fn inspect(x: box List[Bytes]) {}\nfn probe() { let xs: List[Bytes] in xs = [b\"x\"]; let built = box unique xs; let received: box List[Bytes] = take built; inspect(received) }",
        )
    });
}

#[test]
fn local_receiving_call_sources_use_current_content_locality() {
    with_compiler_stack(|| {
        for source in [
            "fn probe(x: Bytes) -> Bytes in x { let mut slot = copy x; slot }\npub fn main(rt: Io,args: Args) -> Result[(),()] { let mut data: Bytes in data = b\"old\"; let got = probe(data); Ok(()) }",
            "fn probe(mut x: Bytes) -> Bytes in x { let mut slot = copy x; slot }\npub fn main(rt: Io,args: Args) -> Result[(),()] { let mut data: Bytes in data = b\"old\"; let got = probe(mut data); Ok(()) }",
        ] {
            check(source);
        }
    });
}

#[test]
fn current_call_content_cannot_launder_a_borrowed_result_as_global() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let source = "fn probe(x: Bytes) -> Bytes in x { copy x }\nfn caller(x: Bytes) -> Bytes in x { probe(x) }";
        let file = engine.sources.add("receiving.hir", source.into()).unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let input = engine.export_native_input().unwrap();
        let mut path = entry_path(&default_context());
        path.0.name = "caller".into();
        let mut forged = (*input.definitions[&path]).clone();
        let root = forged.bodies[0]
            .blocks
            .iter()
            .find_map(|block| match &block.terminator {
                hirsel_tir::Terminator::Call {
                    returns: Some(hirsel_tir::CallReturn { destination, .. }),
                    ..
                } => Some(forged.bodies[0].places[destination.0 as usize].root),
                _ => None,
            })
            .unwrap();
        let mut row = forged
            .types
            .get(forged.bodies[0].locals[root.0 as usize].ty)
            .unwrap()
            .clone();
        row.modes.relation = hirsel_types::Relation::global();
        let global = forged.types.push(row);
        forged.bodies[0].locals[root.0 as usize].ty = global;
        for place in &mut forged.bodies[0].places {
            if place.root == root && place.projection.is_empty() {
                place.ty = global;
            }
        }
        let signatures: hirsel_validate::SignatureMap = input
            .signatures
            .iter()
            .map(|(p, s)| (p.clone(), (**s).clone()))
            .collect();
        let violations = hirsel_validate::validate(&forged, &signatures, &mut vec![]).violations;
        assert!(
            violations.iter().any(|v| matches!(
                v.kind,
                hirsel_validate::ViolationKind::TypeMismatch {
                    entity: "call result"
                }
            )),
            "{violations:?}"
        );
    });
}

#[test]
fn local_iterator_receivers_retain_current_sendable_content() {
    with_compiler_stack(|| {
        check(
            "@noalloc\nfn next(mut n: u8) -> Option[u8] { if n == 3 { None } else { let value = n; Some(value) } }\npub fn main(rt: Io,args: Args) -> Result[(),()] { let state: u8 = 0; let mut it: Iter[u8] in it = iter.from(take state,next); for x in take it {}; Ok(()) }",
        );
    });
}

#[test]
fn local_iterator_receivers_cannot_invent_sendable_content() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        for source in [
            "@noalloc\nfn next(mut state: String) -> Option[u8] { None }\nfn probe() { let text: String = \"x\"; let alias = copy text; let mut it: Iter[u8] in it = iter.from(take text,next); for x in take it {} }",
            "@noalloc\nfn next(mut n: u8) -> Option[u8] { None }\nfn change(mut it: Iter[u8]) {}\nfn probe() { let state: u8 = 0; let mut it: Iter[u8] in it = iter.from(take state,next); change(mut it); for x in take it {} }",
        ] {
            let file = engine.sources.add("receiving.hir", source.into()).unwrap();
            let checked = check_progress(&mut engine, file, &default_context()).unwrap();
            assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
            let input = engine.export_native_input().unwrap();
            let mut path = entry_path(&default_context());
            path.0.name = "probe".into();
            let mut forged = (*input.definitions[&path]).clone();
            let body = &mut forged.bodies[0];
            let root = body
                .locals
                .iter()
                .enumerate()
                .find_map(|(i, local)| {
                    let row = forged.types.get(local.ty).unwrap();
                    (matches!(
                        row.kind,
                        hirsel_types::TypeKind::Builtin {
                            kind: hirsel_types::BuiltinType::Iter,
                            ..
                        }
                    ) && row.modes.relation.is_global()
                        && row.modes.sendability == hirsel_types::Sendability::NotSendable
                        && local.storage != hirsel_tir::Storage::Parameter)
                        .then_some(i)
                })
                .unwrap();
            let mut row = forged.types.get(body.locals[root].ty).unwrap().clone();
            row.modes.sendability = hirsel_types::Sendability::Sendable;
            let bogus = forged.types.push(row);
            body.locals[root].ty = bogus;
            for place in &mut body.places {
                if place.root.0 as usize == root && place.projection.is_empty() {
                    place.ty = bogus
                }
            }
            let signatures: hirsel_validate::SignatureMap = input
                .signatures
                .iter()
                .map(|(p, s)| (p.clone(), (**s).clone()))
                .collect();
            let violations =
                hirsel_validate::validate(&forged, &signatures, &mut vec![]).violations;
            assert!(
                violations.iter().any(|v| matches!(
                    v.kind,
                    hirsel_validate::ViolationKind::TypeMismatch {
                        entity: "call result" | "value modes"
                    }
                )),
                "{violations:?}"
            );
        }
    });
}

#[test]
fn local_function_receivers_retain_current_sendable_content() {
    with_compiler_stack(|| {
        check(
            "fn double(x: u8) -> u8 { x * 2 }\nfn keep(take f: fn(u8) -> u8) -> u8 { f(1u8) }\nfn probe() { let mut f: (fn(u8) -> u8) in f = double; let n = keep(take f) }",
        );
    });
}

#[test]
fn local_function_receivers_cannot_invent_sendable_content() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let source = "fn double(x: u8) -> u8 { x * 2 }\nfn keep(take f: fn(u8) -> u8) -> u8 { f(1u8) }\nfn change(mut f: fn(u8) -> u8) {}\nfn probe() { let mut f: (fn(u8) -> u8) in f = double; change(mut f); let n = keep(take f) }";
        let file = engine.sources.add("receiving.hir", source.into()).unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let input = engine.export_native_input().unwrap();
        let mut path = entry_path(&default_context());
        path.0.name = "probe".into();
        let signatures: hirsel_validate::SignatureMap = input
            .signatures
            .iter()
            .map(|(p, s)| (p.clone(), (**s).clone()))
            .collect();
        let original = (*input.definitions[&path]).clone();
        assert!(
            hirsel_validate::validate(&original, &signatures, &mut vec![])
                .violations
                .is_empty()
        );
        // After the mutable call only the Local receiving contract's crossing
        // mode survives, so the taken value cannot claim a Sendable producer.
        let mut forged = original;
        let body = &mut forged.bodies[0];
        let root = body
            .locals
            .iter()
            .enumerate()
            .find_map(|(i, local)| {
                let row = forged.types.get(local.ty).unwrap();
                (matches!(row.kind, hirsel_types::TypeKind::Function(_))
                    && row.modes.relation.is_global()
                    && row.modes.sendability == hirsel_types::Sendability::NotSendable
                    && local.storage != hirsel_tir::Storage::Parameter)
                    .then_some(i)
            })
            .unwrap();
        let mut row = forged.types.get(body.locals[root].ty).unwrap().clone();
        row.modes.sendability = hirsel_types::Sendability::Sendable;
        let bogus = forged.types.push(row);
        body.locals[root].ty = bogus;
        for place in &mut body.places {
            if place.root.0 as usize == root && place.projection.is_empty() {
                place.ty = bogus
            }
        }
        let violations = hirsel_validate::validate(&forged, &signatures, &mut vec![]).violations;
        assert!(
            violations.iter().any(|v| matches!(
                v.kind,
                hirsel_validate::ViolationKind::TypeMismatch {
                    entity: "call result" | "value modes"
                }
            )),
            "{violations:?}"
        );
    });
}

#[test]
fn projected_box_payloads_keep_their_instantiated_generic_contract() {
    with_compiler_stack(|| {
        for source in [
            "type R { xs: unique List[Bytes] }\nfn probe(r: R) { let built = box unique r.xs; let frozen = shared.freeze(take built) }",
            "type R { xs: unique List[Bytes] }\nfn probe(take r: R in r) { let built = box unique ({r}).xs; let frozen = shared.freeze(take built) }",
            "type R { part: Shared[Bytes] }\nfn probe(take r: R in r) { let built = box ({r}).part; let frozen = shared.freeze(take built) }",
        ] {
            check(source);
        }
    });
}

#[test]
fn freezing_a_projected_box_requires_global_contents_before_uniqueness() {
    with_compiler_stack(|| {
        let source = "type R { xs: unique List[Bytes] }\nfn probe(take r: R in r) { let built = box ({r}).xs; let frozen = shared.freeze(take built) }";
        check(&source.replace("box ({r}).xs", "box unique ({r}).xs"));
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("freeze.hir", source.into()).unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert_eq!(checked.diagnostics[0].rule.as_str(), "MODE-1");
        // With global contents, the box's aliased contents fail MEM-11.
        let global = source
            .replace("take r: R in r", "take r: R")
            .replace("({r})", "r");
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("global.hir", global.clone()).unwrap();
        let unique = check_progress(&mut engine, file, &default_context()).unwrap();
        assert_eq!(unique.diagnostics[0].rule.as_str(), "MEM-11");
        for checked in [&checked, &unique] {
            let diagnostic = &checked.diagnostics[0];
            assert_eq!(
                checked.sources.slice(diagnostic.primary).unwrap(),
                "shared.freeze(take built)",
            );
        }
    });
}

/// An inferred binding's receiving locality is that of the value it received
/// (MODE-1): local content makes local storage, which accepts a local write.
/// A written or received Global contract still rejects it at the write.
#[test]
fn inferred_receivers_of_local_content_accept_local_writes() {
    with_compiler_stack(|| {
        let view = "fn view(x: Bytes) -> Bytes in x { x }\n";
        for body in [
            "let xs: List[Bytes] = [view(p)]\n for x in take xs { let mut y = x\n y = view(p) }",
            "let xs: List[Bytes] = [view(p)]\n for x in take xs { let mut y = take x\n y = view(p) }",
            "let mut (a, b): (Bytes, Bytes) = (b\"x\", view(p))\n a = view(p)",
        ] {
            check(&format!("{view}fn probe(p: Bytes) {{ {body} }}"));
        }
        for (body, line) in [
            (
                "let xs: List[Bytes] = [view(p)]\n for x in take xs { let mut y: Bytes in heap = take x\n y = view(p) }",
                None,
            ),
            (
                "let xs: List[Bytes] = [b\"a\"]\n for x in take xs { let mut y = take x\n y = view(p) }",
                Some(4),
            ),
            (
                "let mut (a, b): (Bytes, Bytes) = (b\"x\", b\"y\")\n a = view(p)",
                Some(3),
            ),
            ("let mut y: Bytes = b\"a\"\n y = view(p)", Some(3)),
        ] {
            let source = format!("{view}fn probe(p: Bytes) {{ {body} }}");
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("receiving.hir", source.clone()).unwrap();
            let checked = check_progress(&mut engine, file, &default_context()).unwrap();
            let first = checked.diagnostics.first();
            assert_eq!(first.map(|d| d.rule.as_str()), Some("MODE-1"), "{source}");
            if let Some(line) = line {
                let start = checked.sources.resolve(first.unwrap().primary).start as usize;
                assert_eq!(source[..start].matches('\n').count() + 1, line, "{source}");
            }
        }
    });
}

/// A `mut` binding written `once` holds a once function whatever it received
/// (MODE-3): a call consumes it, a write refills it, and a second call without
/// a refill uses a moved value.
#[test]
fn once_receiving_storage_consumes_its_function_on_call() {
    with_compiler_stack(|| {
        for body in [
            "let mut g: once fn() = fn() {}\n g()",
            "let mut g: once fn() = fn() {}\n g()\n g = fn() {}\n g()",
            "let mut g: once fn() = fn() {}\n g = fn() {}\n g()",
            "let f: once fn() = fn() {}\n let mut g: once fn() = take f\n g()",
            "let mut g: once fn() = fn() {}\n run(take g)",
            "let mut g = fn() {}\n g()\n g()",
        ] {
            check(&format!(
                "fn run(take f: once fn()) {{ f() }}\nfn probe() {{ {body} }}"
            ));
        }
        let source = "fn probe() {\n let mut g: once fn() = fn() {}\n g()\n g()\n}";
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("receiving.hir", source.into()).unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        let first = checked.diagnostics.first().expect("a second once call");
        assert_eq!(first.rule.as_str(), "MODE-3");
        assert_eq!(
            source[..checked.sources.resolve(first.primary).start as usize]
                .matches('\n')
                .count(),
            3
        );
    });
}
