use hirsel_driver::native::Observation;
use hirsel_driver::*;
use hirsel_engine::{
    LoweringPolicy, RunConfiguration, SimulationConfig, run_observed_snapshot, with_compiler_stack,
};

#[test]
fn related_result_loop_source_v_agreement() {
    with_compiler_stack(|| {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/related_result_loop_v_33");
        let mut failures = vec![];
        for name in [
            "minimal.hir",
            "loop.hir",
            "owned-input.hir",
            "explicit-match.hir",
        ] {
            let mut program = load_program(&root.join(name)).unwrap();
            let output = program.check(LoweringPolicy::Comptime).unwrap();
            assert!(
                output.diagnostics.is_empty(),
                "{name}: {:?}",
                output.diagnostics
            );
            match program.engine.export_native_input() {
                Err(error) => failures.push(format!("{name}: {error:?}")),
                Ok(input) if matches!(name, "minimal.hir" | "explicit-match.hir") => {
                    let (_, definition) = input
                        .definitions
                        .iter()
                        .find(|(path, _)| path.0.name == "test")
                        .unwrap();
                    let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
                    signatures
                        .extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
                    let control = hirsel_validate::validate(
                        definition,
                        &input.types,
                        &signatures,
                        &mut vec![],
                    );
                    assert!(
                        control.violations.is_empty(),
                        "{name}: original V control: {control:?}"
                    );
                    let mut forged = (**definition).clone();
                    let mut mutations = 0;
                    for body in &mut forged.bodies {
                        for block in &body.blocks {
                            let hirsel_tir::Terminator::Call {
                                callee: hirsel_tir::Callee::Definition { path, .. },
                                returns: Some(receipt),
                                ..
                            } = &block.terminator
                            else {
                                continue;
                            };
                            if path.0.name != "object" {
                                continue;
                            }
                            let destination = &body.places[receipt.destination.0 as usize];
                            let root = destination.root;
                            let original = input.types.row(destination.ty).modes.relation.clone();
                            assert!(!original.is_global(), "{name}: stale Global receipt");
                            // Rebuild its inherited projections too, so V reaches
                            // call-result typing instead of rejecting malformed IR.
                            for place in body.places.iter_mut().filter(|p| p.root == root) {
                                let mut row = input.types.row(place.ty).clone();
                                if row.modes.relation == original {
                                    row.modes.relation = hirsel_types::Relation::global();
                                    place.ty = input.types.intern(row);
                                }
                                if place.projection.is_empty() {
                                    body.locals[root.0 as usize].ty = place.ty;
                                }
                            }
                            mutations += 1;
                        }
                    }
                    assert_eq!(mutations, 1, "{name}: missing related helper call");
                    forged.types = hirsel_tir::definition_types(&input.types, &forged);
                    let report =
                        hirsel_validate::validate(&forged, &input.types, &signatures, &mut vec![]);
                    assert!(
                        report.violations.iter().any(|v| v.kind
                            == hirsel_validate::ViolationKind::TypeMismatch {
                                entity: "call result"
                            }),
                        "{name}: V accepted a forged Global receipt: {report:?}"
                    );
                }
                Ok(_) => {}
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    });
}

#[path = "../support/history.rs"]
#[allow(dead_code, reason = "shared runner supports other ownership histories")]
mod history;

#[test]
fn related_result_loop_four_engines() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let runner = history::Runner::new(directory.path()).unwrap();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/related_result_loop_v_33");
        for name in [
            "minimal.hir",
            "loop.hir",
            "owned-input.hir",
            "explicit-match.hir",
        ] {
            let original = std::fs::read_to_string(root.join(name)).unwrap();
            // Keep the frozen empty-input witness verbatim; also enter the loop
            // twice, and leave it through an error after one successful element.
            let (sample, observe) = if name == "loop.hir" {
                ("Text(\"payload\")", "")
            } else {
                (
                    "Object(map.empty())",
                    "failure.check(map.len(row) == 0, Bounds())",
                )
            };
            let main = original.find("pub fn main()").unwrap();
            let source = format!(
                "{}\npub fn main() -> Result[(), Problem] {{\nlet _ = test(Array([{sample}, {sample}]))?\nmatch test(Array([{sample}, Null(), {sample}])) {{ Error(Shape()) => {{}}, _ => failure.check(false, Bounds()), }}\nOk(())\n}}",
                original[..main]
                    .replace(
                        "let row = object(item)?",
                        &format!("let row = object(item)?; {observe}")
                    )
                    .replace(
                        "let row = object(unique item)?",
                        &format!("let row = object(unique item)?; {observe}")
                    )
                    .replace(
                        "Error(e) => return Error(e), } }",
                        &format!("Error(e) => return Error(e), }}; {observe} }}")
                    )
            );
            let mut program = load_program_source(&root.join(name), Some(&source)).unwrap();
            let checked = program.check(LoweringPolicy::Comptime).unwrap();
            assert!(
                checked.diagnostics.is_empty(),
                "{name}: {:?}",
                checked.diagnostics
            );
            let input = program
                .engine
                .export_native_input()
                .expect("original positive source/V control");
            let validated = program.engine.import_native_input(input).unwrap();
            let entry = program.entry;
            let reference = run_observed_snapshot(
                &mut program.engine,
                &RunConfiguration {
                    entry: entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            )
            .unwrap();
            let report = runner.compare(
                &validated,
                &entry,
                &Observation::of_reference(reference.observation.as_ref().unwrap()),
                name,
            );
            assert!(report.agrees(), "{name}: {report:#?}\n{source}");
            assert_eq!(report.cells.len(), 4, "{name}: missing engine");
            assert!(
                report.disagreements(b"", false).is_empty(),
                "{name}: {report:#?}"
            );
        }
    });
}

#[test]
fn related_result_loop_cannot_escape_as_global() {
    with_compiler_stack(|| {
        let original = include_str!("../fixtures/related_result_loop_v_33/minimal.hir");
        let helpers = &original[..original.find("fn test(").unwrap()];
        let source = format!(
            "{helpers}\nfn escape(value: json.Value) -> Result[Map[String, json.Value], Problem] uses {{}} {{\nlet items = array(value)?\nfor item in items {{ return object(item) }}\nError(Shape())\n}}\npub fn main() {{}}"
        );
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/related_result_loop_v_33/minimal.hir");
        let mut program = load_program_source(&root, Some(&source)).unwrap();
        let output = program.check(LoweringPolicy::Comptime).unwrap();
        assert!(
            output
                .diagnostics
                .iter()
                .any(|d| d.rule.as_str() == "MODE-1"),
            "{:?}",
            output.diagnostics
        );
    });
}
