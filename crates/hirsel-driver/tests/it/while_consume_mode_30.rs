use hirsel_driver::*;
use hirsel_engine::*;

const BATCH: &str = include_str!("../fixtures/while_consume_mode_30/batch.hir");
const RUNTIME_MAIN: &str = include_str!("../fixtures/while_consume_mode_30/runtime-main.hir");

fn modules(batch: &str, main: &str) -> QueryEngine {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let mut file = None;
    for (name, source, module) in [
        ("main.hir", main, None),
        ("batch.hir", batch, Some("batch")),
        (
            "document.hir",
            include_str!("../fixtures/while_consume_mode_30/document.hir"),
            Some("document"),
        ),
        (
            "manifest.hir",
            include_str!("../fixtures/while_consume_mode_30/manifest.hir"),
            Some("manifest"),
        ),
    ] {
        let id = engine.sources.add(name, source.into()).unwrap();
        let mut context = default_context();
        if let Some(module) = module {
            context.modules.push(module.into());
        }
        engine.parse_file(id, &context).unwrap();
        if module.is_none() {
            file = Some(id);
        }
    }
    let checked = check_progress(&mut engine, file.unwrap(), &default_context()).unwrap();
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    engine
}

#[test]
fn while_consume_mode_30_original_exports() {
    with_compiler_stack(|| {
        // The reported main does not call load: this is a lowering regression,
        // separate from the executed variants below.
        modules(
            BATCH,
            include_str!("../fixtures/while_consume_mode_30/main.hir"),
        )
        .export_native_input()
        .unwrap();
    });
}

fn runtime(mut engine: QueryEngine) {
    use hirsel_driver::native::*;
    let expected = Observation {
        exit_code: Some(0),
        blocked: false,
        budget: None,
        signaled: None,
        output: vec![],
        failures: vec![],
        roots: vec![],
    };
    let entry = entry_path(&default_context());
    let run = run_observed_snapshot(
        &mut engine,
        &RunConfiguration {
            entry: entry.clone(),
            hash_seed: 30,
            inputs: vec![],
        },
        Default::default(),
    )
    .unwrap();
    assert_eq!(
        Observation::of_reference(run.observation.as_ref().unwrap()),
        expected,
        "interpreter"
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
        .prefix("while-consume-mode-30-")
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
fn while_consume_mode_30_runtime_batched() {
    with_compiler_stack(|| runtime(modules(BATCH, RUNTIME_MAIN)));
}

#[test]
fn while_consume_mode_30_runtime_receiving_binding() {
    with_compiler_stack(|| {
        let batch = BATCH.replace(
            "      for page in take pages",
            "      let received: List[document.Parsed] = take pages\n      for page in take received",
        );
        runtime(modules(&batch, RUNTIME_MAIN));
    });
}

#[test]
fn while_consume_mode_30_runtime_break_drops_remainder() {
    with_compiler_stack(|| {
        let batch = BATCH.replace(
            "list.push(mut values, take page) }",
            "list.push(mut values, take page); break }",
        );
        let main = RUNTIME_MAIN
            .replace("== 7u64", "== 2u64")
            .replace("== i + 1u64", "== i * 4u64 + 1u64");
        runtime(modules(&batch, &main));
    });
}

#[test]
fn while_consume_mode_30_runtime_drop_all() {
    with_compiler_stack(|| {
        let batch = BATCH.replace(
            "for page in take pages { list.push(mut values, take page) }",
            "for page in take pages {}",
        );
        let main = RUNTIME_MAIN.replace("== 7u64", "== 0u64");
        runtime(modules(&batch, &main));
    });
}

#[test]
fn while_consume_mode_30_runtime_return_drops_remainder() {
    with_compiler_stack(|| {
        let batch = BATCH.replace(
            "list.push(mut values, take page) }",
            "list.push(mut values, take page); return Ok(take values) }",
        );
        let main = RUNTIME_MAIN.replace("== 7u64", "== 1u64");
        runtime(modules(&batch, &main));
    });
}

#[test]
fn while_consume_mode_30_runtime_empty() {
    with_compiler_stack(|| {
        let start = RUNTIME_MAIN.find("  let inputs = [").unwrap();
        let end = start + RUNTIME_MAIN[start..].find("  match batch.load").unwrap();
        let main = format!(
            "{}  let inputs: List[manifest.Input] = []\n{}",
            &RUNTIME_MAIN[..start],
            &RUNTIME_MAIN[end..]
        )
        .replace("== 7u64", "== 0u64");
        runtime(modules(BATCH, &main));
    });
}

#[test]
fn while_consume_mode_30_minimal_exports() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add(
                "minimal.hir",
                include_str!("../fixtures/while_consume_mode_30/minimal.hir").into(),
            )
            .unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        runtime(engine);
    });
}

#[test]
fn while_consume_mode_30_v_rejects_forged_cursor_modes() {
    use hirsel_tir::{Callee, Terminator};
    use hirsel_types::{GenericArgument, Sendability, TypeKind, Uniqueness};
    use hirsel_validate::ViolationKind;
    with_compiler_stack(|| {
        let input = modules(BATCH, RUNTIME_MAIN).export_native_input().unwrap();
        let original = input
            .definitions
            .iter()
            .find(|(path, _)| path.0.modules == ["batch"] && path.0.name == "load")
            .unwrap()
            .1;
        let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
        signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
        hirsel_validate::assert_valid(original, signatures.types(), &signatures);
        let mut found = vec![];
        for (index, body) in original.bodies.iter().enumerate() {
            for block in &body.blocks {
                if let Terminator::Call {
                    callee: Callee::Definition { path, .. },
                    returns: Some(receipt),
                    ..
                } = &block.terminator
                    && path.0.name == "ListConsume_open"
                {
                    found.push((index, body.places[receipt.destination.0 as usize].root));
                }
            }
        }
        assert_eq!(found.len(), 1);
        let (index, root) = found[0];
        let mut cursor = input
            .types
            .row(original.bodies[index].locals[root.0 as usize].ty)
            .clone();
        let TypeKind::Named { arguments, .. } = &cursor.kind else {
            panic!("cursor must be nominal")
        };
        let [GenericArgument::Type(item)] = arguments.as_ref() else {
            panic!("one cursor item")
        };
        let item = input.types.row(*item).clone();
        assert_eq!(item.modes.uniqueness, Uniqueness::Unique);
        assert_eq!(item.modes.sendability, Sendability::NotSendable);
        for forge_uniqueness in [false, true] {
            let mut item = item.clone();
            if forge_uniqueness {
                item.modes.uniqueness = Uniqueness::Aliased;
            } else {
                item.modes.sendability = Sendability::Sendable;
            }
            let TypeKind::Named { arguments, .. } = &mut cursor.kind else {
                unreachable!()
            };
            *arguments = vec![GenericArgument::Type(input.types.intern(item))].into();
            let ty = input.types.intern(cursor.clone());
            let mut forged = (**original).clone();
            let body = &mut forged.bodies[index];
            body.locals[root.0 as usize].ty = ty;
            for place in &mut body.places {
                if place.root == root {
                    place.ty = ty;
                }
            }
            forged.types = hirsel_tir::definition_types(&input.types, &forged);
            let report =
                hirsel_validate::validate(&forged, signatures.types(), &signatures, &mut vec![]);
            assert!(
                report.violations.iter().any(|v| v.kind
                    == ViolationKind::TypeMismatch {
                        entity: "call result"
                    }),
                "{:?}",
                report.violations
            );
        }
    });
}
