//! SEM-4 loans projected loop places, preserving MODE-2 without snapshots.
use hirsel_driver::*;
use hirsel_engine::*;

fn vector(name: &str) -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance/MODE")
            .join(format!("MODE-2-loop-place-52-{name}.hir")),
    )
    .unwrap()
}

fn runtime(source: &str) {
    use hirsel_driver::native::*;
    let mut engine = crate::source_std::engine();
    let file = engine
        .sources
        .add("loop-place-52.hir", source.into())
        .unwrap();
    let checked = check_progress(&mut engine, file, &default_context()).unwrap();
    assert_eq!(checked.completion, CheckCompletion::Complete);
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let entry = entry_path(&default_context());
    let run = run_observed_snapshot(
        &mut engine,
        &RunConfiguration {
            entry: entry.clone(),
            hash_seed: 29,
            inputs: vec![],
        },
        Default::default(),
    )
    .unwrap();
    let expected = Observation::of_reference(run.observation.as_ref().unwrap());
    assert_eq!(expected.exit_code, Some(0), "{expected:?}");
    assert!(expected.failures.is_empty(), "{expected:?}");
    let checked = run_observed_checked(
        &mut engine,
        &RunConfiguration {
            entry: entry.clone(),
            hash_seed: 29,
            inputs: vec![],
        },
        Default::default(),
    )
    .unwrap();
    assert_eq!(
        Observation::of_reference(checked.observation.as_ref().unwrap()),
        expected
    );
    let input = engine.export_native_input().unwrap();
    let validated = engine.import_native_input(input).unwrap();
    let mut options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
    options.validation = Validation::EveryStage;
    let ready = build(&validated, &options).unwrap();
    let heap = run_heap(&ready, &entry, &Sites::new(validated.snapshot())).unwrap();
    assert_eq!(heap.observation, expected, "checked H");
    assert_eq!(heap.counters.live_heap_bytes, 0);
    let matrix = [
        Configuration {
            options: options.clone(),
            engine: Engine::Native {
                opt: OptLevel::Speed,
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        },
        Configuration {
            options,
            engine: Engine::Cranelift {
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        },
    ];
    let scratch = tempfile::Builder::new()
        .prefix("loop-place-52-")
        .tempdir()
        .unwrap();
    let toolchain = Toolchain::for_matrix(&scratch.path().join("runtime"), &matrix).unwrap();
    let outcomes = differential(
        &validated,
        &entry,
        &expected,
        &matrix,
        &toolchain,
        scratch.path(),
    );
    assert_eq!(outcomes.len(), 2);
    for (configuration, outcome) in outcomes {
        assert!(
            matches!(outcome, Outcome::Agree),
            "{configuration}: {outcome:?}"
        );
    }
}

#[test]
fn loop_place_52_field_four_engines() {
    with_compiler_stack(|| runtime(&vector("field-run")));
}

#[test]
fn loop_place_52_index_field_four_engines() {
    with_compiler_stack(|| runtime(&vector("index-field-run")));
}

#[test]
fn loop_place_52_negative_witnesses() {
    with_compiler_stack(|| {
        for (name, rule) in [("write-bad", "OWN-3"), ("copy-bad", "MODE-2")] {
            let mut engine = crate::source_std::engine();
            let source = vector(name);
            let file = engine
                .sources
                .add("loop-place-negative.hir", source)
                .unwrap();
            let output = check_progress(&mut engine, file, &default_context()).unwrap();
            assert_eq!(output.completion, CheckCompletion::Complete);
            assert_eq!(
                output.diagnostics.first().unwrap().rule.as_str(),
                rule,
                "{name}: {:?}",
                output.diagnostics
            );
        }
    });
}

#[test]
fn loop_place_52_paths_four_engines() {
    with_compiler_stack(|| runtime(&vector("paths-run")));
}

#[test]
fn loop_place_52_v_reproves_projected_cursor_loan() {
    use hirsel_tir::*;
    with_compiler_stack(|| {
        let mut engine = crate::source_std::engine();
        let file = engine
            .sources
            .add("loop-place-cursor.hir", vector("paths-run"))
            .unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let input = engine.export_native_input().unwrap();
        let mut path = entry_path(&default_context());
        path.0.name = "scan".into();
        let definition = &input.definitions[&path];
        let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
        signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
        assert!(
            hirsel_validate::validate(definition, engine.types(), &signatures, &mut vec![])
                .violations
                .is_empty()
        );
        let mut forged = (**definition).clone();
        let body = &mut forged.bodies[0];
        let source = body
            .blocks
            .iter()
            .find_map(|block| match &block.terminator {
                Terminator::Call {
                    callee: Callee::Definition { path, .. },
                    arguments,
                    ..
                } if path.0.name == "SetElements_open" => Some(arguments[0].value.clone()),
                _ => None,
            })
            .expect("projected Set cursor open");
        assert_eq!(source.usage, hirsel_types::UseKind::Borrow);
        let place = &body.places[source.place.0 as usize];
        assert_eq!(place.projection, vec![Projection::Field(1)]);
        let loan = place.loan.expect("cursor borrows the projected place");
        let sibling = PlaceId(
            body.places
                .iter()
                .position(|p| {
                    p.root == place.root
                        && p.projection == vec![Projection::Field(0)]
                        && p.loan.is_none()
                })
                .expect("unloaned sibling field") as u32,
        );
        let mut changed = false;
        for instruction in body.blocks.iter_mut().flat_map(|b| &mut b.instructions) {
            if let InstructionKind::BeginLoan { id, place, .. } = &mut instruction.kind
                && *id == loan
            {
                // A forged cursor claims a loan of the unrelated sibling.
                *place = sibling;
                changed = true;
            }
        }
        assert!(changed);
        let violations =
            hirsel_validate::validate(&forged, engine.types(), &signatures, &mut vec![]).violations;
        assert!(
            violations.iter().any(|v| matches!(
                v.kind,
                hirsel_validate::ViolationKind::MissingLoan(_)
                    | hirsel_validate::ViolationKind::InactiveLoan(_)
                    | hirsel_validate::ViolationKind::ExpiredLoan { .. }
                    | hirsel_validate::ViolationKind::LoanConflict { .. }
            )),
            "V accepted a cursor without its source loan: {violations:?}"
        );
    });
}

#[test]
fn loop_place_52_owned_sources_still_share() {
    with_compiler_stack(|| {
        for source in [
            vector("field-run").replace("in owned.items {", "in copy owned.items {"),
            format!(
                "fn snapshot(take xs: List[String]) -> List[String] {{ take xs }}\n{}",
                vector("field-run").replace("in owned.items {", "in snapshot(copy owned.items) {")
            ),
        ] {
            let mut engine = crate::source_std::engine();
            let file = engine.sources.add("owned-loop-source.hir", source).unwrap();
            let output = check_progress(&mut engine, file, &default_context()).unwrap();
            assert_eq!(output.completion, CheckCompletion::Complete);
            assert!(
                output
                    .diagnostics
                    .iter()
                    .any(|d| d.rule.as_str() == "MODE-2"),
                "{:?}",
                output.diagnostics
            );
        }
    });
}

#[test]
fn loop_place_52_copied_element_twin_four_engines() {
    with_compiler_stack(|| runtime(&vector("copy-ok")));
}
