//! Copying an owner ends uniqueness of its nested counted components (MODE-2).
use crate::rc_support::target;
use hirsel_driver::{compiler_adapters, native::*};
use hirsel_engine::*;

const WITNESS: &str = r#"
fn alter(mut row: unique List[u64]) { row[0] = 9 }
fn value() -> u64 {
    let mut first: unique List[u64] = []
    list.push(mut first, 7)
    let source: unique List[unique List[u64]] = [take first]
    let snapshot = copy source
    for row in take source { let mut owned = take row; alter(mut owned) }
    snapshot[0][0]
}
pub fn main() -> Result[(), u64] {
    if value() == 7 { Ok(()) } else { Error(1) }
}
"#;

fn parity(source: &str, native: bool) {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let context = default_context();
        let file = engine
            .sources
            .add("nested-copy.hir", source.into())
            .unwrap();
        let checked = check_progress(&mut engine, file, &context).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let entry = entry_path(&context);
        let run = run_observed_snapshot(
            &mut engine,
            &RunConfiguration {
                entry: entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            Default::default(),
        )
        .unwrap();
        let reference = Observation::of_reference(run.observation.as_ref().unwrap());
        assert_eq!(reference.exit_code, Some(0), "{reference:?}");
        assert!(reference.failures.is_empty(), "{reference:?}");
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let engines = if native {
            vec![
                Engine::Heap { campaign: true },
                Engine::Cranelift {
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
                Engine::Native {
                    opt: OptLevel::None,
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
                Engine::Native {
                    opt: OptLevel::Speed,
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            ]
        } else {
            vec![Engine::Heap { campaign: true }]
        };
        let matrix: Vec<_> = engines
            .into_iter()
            .map(|engine| Configuration {
                options: Options::new(target()),
                engine,
            })
            .collect();
        let directory = tempfile::tempdir().unwrap();
        let toolchain = Toolchain::for_matrix(&directory.path().join("runtime"), &matrix).unwrap();
        let outcomes = differential(
            &validated,
            &entry,
            &reference,
            &matrix,
            &toolchain,
            directory.path(),
        );
        for (configuration, outcome) in &outcomes {
            eprintln!("{configuration}: {outcome:?}");
        }
        assert!(
            outcomes
                .iter()
                .all(|(_, outcome)| matches!(outcome, Outcome::Agree)),
            "{outcomes:?}"
        );
    });
}

#[test]
fn copied_nested_unique_witness() {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine
        .sources
        .add("nested-copy.hir", WITNESS.into())
        .unwrap();
    let checked = check_progress(&mut engine, file, &default_context()).unwrap();
    let diagnostic = checked
        .diagnostics
        .iter()
        .find(|d| d.rule == hirsel_diag::RuleId::of("MODE-2"))
        .unwrap();
    assert!(
        diagnostic
            .secondary
            .iter()
            .any(|s| s.message.contains("copy ended")),
        "{diagnostic:?}"
    );
    assert!(
        diagnostic
            .secondary
            .iter()
            .any(|s| s.message.contains("parameter mode requirement")),
        "{diagnostic:?}"
    );
    assert!(
        diagnostic
            .notes
            .iter()
            .any(|s| s.contains("unique element") && s.contains("snapshot")),
        "{diagnostic:?}"
    );
}

#[test]
#[ignore = "native: run by tools/check-native.sh"]
fn copied_nested_unique_native() {
    parity(
        &WITNESS.replace("mut row: unique List", "mut row: List"),
        true,
    );
    parity(
        &WITNESS.replace("let mut owned = take row", "let mut owned = unique row"),
        true,
    );
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Variant {
    Bad,
    Ordinary,
    Repair,
    NoCopy,
    Dropped,
}

fn cases(depth: usize, access: &str, variant: Variant) -> Vec<(String, String)> {
    let leaf = if depth == 2 {
        "unique List[u64]"
    } else {
        "unique List[unique List[u64]]"
    };
    let index = "[0]".repeat(depth - 1);
    let init = format!(
        "let mut seed: unique List[u64] = []; list.push(mut seed, 7); let first: {leaf} = {};",
        if depth == 2 {
            "take seed"
        } else {
            "[take seed]"
        }
    );
    let mut out = Vec::new();
    for kind in [
        "list", "array", "map", "option", "result", "record", "tuple",
    ] {
        let mut declarations = String::new();
        let (setup, snapshot, moved, borrowed) = match kind {
            "list" | "array" => {
                let container = if kind == "list" { "List" } else { "Array" };
                (format!("let mut source: unique {container}[{leaf}] = [take first];"),
                 format!("snapshot[0]{index}"),
                 "for row in take source {let mut owned = TAKE row; alter(mut owned); seen = ownedINDEX}".into(),
                 "for row in mut source {CURSOR; seen = rowINDEX}".into())
            }
            "map" => (format!("let mut source: unique Map[u64,{leaf}] = map.empty(); map.insert(mut source,0,take first);"),
                format!("match map.get(snapshot,0) {{Some(row) => row{index}, None => 0}}"),
                "for (key,row) in take source {let mut owned = TAKE row; alter(mut owned); seen = ownedINDEX}".into(),
                "for (key,row) in mut source {CURSOR; seen = rowINDEX}".into()),
            "option" | "result" => {
                let (ty, tag, other) = if kind == "option" { (format!("Option[{leaf}]"), "Some", "None") } else { (format!("Result[{leaf},u64]"), "Ok", "Error(_)") };
                (format!("let mut source: unique {ty} = {tag}(take first);"),
                 format!("match snapshot {{{tag}(row) => row{index}, {other} => 0}}"),
                 format!("match take source {{{tag}(row) => {{let mut owned = TAKE row; alter(mut owned); seen = ownedINDEX}}, {other} => ()}}"),
                 format!("let mut cursor = source; match cursor {{{tag}(row) => {{let mut owned = COPY row; alter(mut owned); seen = ownedINDEX}}, {other} => ()}}"))
            }
            "record" => {
                declarations = format!("type Holder {{row: {leaf}}}\n");
                ("let mut source: unique Holder = Holder(row: take first);".into(), format!("snapshot.row{index}"),
                 "let Holder(row: row) = take source; let mut owned = TAKE row; alter(mut owned); seen = ownedINDEX".into(),
                 "let mut cursor = source; let mut owned = COPY cursor.row; alter(mut owned); seen = ownedINDEX".into())
            }
            "tuple" => (format!("let mut source: unique ({leaf},u64) = (take first,0);"),
                format!("{{let (saved,_) = snapshot; saved{index}}}"),
                "let (row,n) = take source; let mut owned = TAKE row; alter(mut owned); seen = ownedINDEX".into(),
                "let mut cursor = source; let (row,n) = cursor; let mut owned = COPY row; alter(mut owned); seen = ownedINDEX".into()),
            _ => unreachable!(),
        };
        if access == "cursor"
            && variant == Variant::NoCopy
            && !matches!(kind, "list" | "array" | "map")
        {
            continue;
        }
        let ordinary = matches!(variant, Variant::Ordinary | Variant::Dropped)
            || variant == Variant::NoCopy && access == "cursor" && kind == "map";
        let alter_ty = if ordinary {
            leaf.strip_prefix("unique ").unwrap()
        } else {
            leaf
        };
        let mut mutation: String = if access == "move" { moved } else { borrowed };
        mutation = mutation
            .replace(
                "TAKE",
                if variant == Variant::Repair {
                    "unique"
                } else {
                    "take"
                },
            )
            .replace(
                "COPY",
                if variant == Variant::Repair {
                    "unique"
                } else {
                    "copy"
                },
            )
            .replace("INDEX", &index)
            .replace(
                "CURSOR",
                if variant == Variant::Repair {
                    "{let mut owned = unique row; alter(mut owned)}"
                } else {
                    "alter(mut row)"
                },
            );
        let (copy, check) = match variant {
            Variant::NoCopy => (String::new(), String::new()),
            Variant::Dropped => (
                format!(
                    "{{let snapshot = copy source; if ({snapshot}) != 7 {{return Error(2)}}}};"
                ),
                String::new(),
            ),
            _ => (
                "let snapshot = copy source;".into(),
                format!(" && ({snapshot}) == 7"),
            ),
        };
        let expected = if variant == Variant::Repair
            && access == "cursor"
            && matches!(kind, "list" | "array" | "map")
        {
            7
        } else {
            9
        };
        out.push((format!("{kind}-{depth}-{access}-{variant:?}"), format!("{declarations}fn alter(mut row: {alter_ty}) {{row{index} = 9}}\npub fn main() -> Result[(),u64] {{{init}{setup}{copy}let mut seen: u64 = 0;{mutation};if seen == {expected}{check} {{Ok(())}} else {{Error(1)}}}}")));
    }
    out
}

#[test]
fn sibling_sources() {
    let mut failures = Vec::new();
    for depth in [2, 3] {
        for access in ["move", "cursor"] {
            for variant in [
                Variant::Bad,
                Variant::Ordinary,
                Variant::Repair,
                Variant::NoCopy,
                Variant::Dropped,
            ] {
                for (name, source) in cases(depth, access, variant) {
                    let result = crate::rc_support::try_compile(&source).map(|_| ());
                    eprintln!("{name}: {result:?}");
                    let valid = if variant == Variant::Bad {
                        result.as_ref().is_err_and(|e| e.contains("MODE-2"))
                    } else {
                        result.is_ok()
                    };
                    if !valid {
                        failures.push(format!("{name}: {result:?}\n{source}"));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
#[test]
#[ignore = "native: run by tools/check-native.sh"]
fn sibling_engines_preserve_snapshots() {
    let mut failures = Vec::new();
    let mut count = 0;
    for depth in [2, 3] {
        for access in ["move", "cursor"] {
            for variant in [
                Variant::Ordinary,
                Variant::Repair,
                Variant::NoCopy,
                Variant::Dropped,
            ] {
                for (name, source) in cases(depth, access, variant) {
                    eprintln!("running {name}");
                    if std::panic::catch_unwind(|| parity(&source, true)).is_err() {
                        failures.push(name);
                    }
                    count += 1;
                }
            }
        }
    }
    assert_eq!(count, 104);
    assert!(failures.is_empty(), "{failures:?}");
}

#[test]
fn independent_v_checks_current_nested_receipts_at_unique_receivers() {
    use hirsel_types::{SignatureKind, Uniqueness};
    for depth in [2, 3] {
        for variant in [
            Variant::Ordinary,
            Variant::Repair,
            Variant::NoCopy,
            Variant::Dropped,
        ] {
            for (name, source) in cases(depth, "move", variant) {
                // Check a legal ordinary receiver first, then ask V alone
                // whether that same executable transfer can satisfy unique.
                let source = source.replace("fn alter(mut row: unique ", "fn alter(mut row: ");
                with_compiler_stack(|| {
                    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
                    let file = engine.sources.add("unique-receiver.hir", source).unwrap();
                    let checked = check_progress(&mut engine, file, &default_context()).unwrap();
                    assert!(
                        checked.diagnostics.is_empty(),
                        "{name}: {:?}",
                        checked.diagnostics
                    );
                    let input = engine.export_native_input().unwrap();
                    let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
                    for signature in input.signatures.values() {
                        let mut signature = (**signature).clone();
                        if signature.definition.path().0.name == "alter" {
                            let SignatureKind::Function(function) = &mut signature.kind else {
                                panic!("alter is a function")
                            };
                            let mut parameters = function.parameters.to_vec();
                            let mut row = input.types.row(parameters[0].ty).clone();
                            row.modes.uniqueness = Uniqueness::Unique;
                            parameters[0].ty = input.types.intern(row);
                            function.parameters = parameters.into();
                        }
                        signatures.insert(signature.definition, signature);
                    }
                    let entry = entry_path(&default_context());
                    let report = hirsel_validate::validate(
                        &input.definitions[&entry],
                        &input.types,
                        &signatures,
                        &mut vec![],
                    );
                    let expected = matches!(variant, Variant::Repair | Variant::NoCopy);
                    assert_eq!(
                        report.violations.is_empty(),
                        expected,
                        "{name}: {:?}",
                        report.violations
                    );
                    if !expected {
                        assert!(
                            report.violations.iter().any(|v| matches!(
                                v.kind,
                                hirsel_validate::ViolationKind::TypeMismatch {
                                    entity: "call argument"
                                }
                            )),
                            "{name}: {:?}",
                            report.violations
                        );
                    }
                });
            }
        }
    }
}

#[test]
#[ignore = "native: run by tools/check-native.sh"]
fn borrowed_cursor_extractions_preserve_snapshots() {
    for depth in [2, 3] {
        for variant in [Variant::Ordinary, Variant::Repair] {
            for (name, source) in cases(depth, "cursor", variant) {
                if !["option", "result", "record", "tuple"]
                    .iter()
                    .any(|kind| name.starts_with(kind))
                {
                    continue;
                }
                let leaf = if depth == 2 {
                    "unique List[u64]"
                } else {
                    "unique List[unique List[u64]]"
                };
                let ty = if name.starts_with("option") {
                    format!("Option[{leaf}]")
                } else if name.starts_with("result") {
                    format!("Result[{leaf},u64]")
                } else if name.starts_with("record") {
                    "Holder".into()
                } else {
                    format!("({leaf},u64)")
                };
                let begin = source.find("let mut cursor = source;").unwrap();
                let end = source[begin..].find(";if seen ==").unwrap() + begin;
                let cursor = &source[begin..end];
                let helper =
                    format!("fn edit(source: {ty}) -> u64 {{let mut seen:u64=0;{cursor};seen}}\n");
                let source = format!(
                    "{helper}{}seen = edit(source){}",
                    &source[..begin],
                    &source[end..]
                );
                eprintln!("borrowed {name}");
                parity(&source, true);
            }
        }
    }
}
