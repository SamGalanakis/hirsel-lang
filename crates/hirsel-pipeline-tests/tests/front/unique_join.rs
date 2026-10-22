//! MODE-2: unique consumers of expression joins keep a consistent destination type.
use hirsel_engine::{
    DefinitionQueries, QueryEngine, check_snapshot, default_context, default_environment,
};

const DIRECT: &str = r#"fn pick(a:String,b:String,select:bool)->unique String { if string.byte_len(a)==0u64 { return unique b }; unique (if select { a } else { b }) }
fn main()->unique String { pick("a","b",false) }"#;
const BOUND: &str = r#"fn pick(a:String,b:String,select:bool)->unique String { if string.byte_len(a)==0u64 { return unique b }; let selected = if select { a } else { b }; unique selected }
fn main()->unique String { pick("a","b",false) }"#;

fn accepts(source: &str) {
    let mut engine = QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
    let file = engine
        .sources
        .add("unique-join.hir", source.into())
        .unwrap();
    let result = check_snapshot(&mut engine, file, &default_context()).unwrap();
    assert!(result.is_empty(), "{result:?}");
    let mut pick = hirsel_engine::entry_path(&default_context());
    pick.0.name = "pick".into();
    let definition = engine.lower_to_tir(&pick).unwrap().value;
    eprintln!("{}", hirsel_lower::print::definition(&definition));
}

#[test]
fn unique_if_join_direct() {
    accepts(DIRECT);
}
#[test]
fn unique_if_join_bound() {
    accepts(BOUND);
}

#[test]
fn unique_if_join_match() {
    accepts(&DIRECT.replace(
        "if select { a } else { b }",
        "match select { true => a, false => b }",
    ));
}
#[test]
fn unique_if_join_nested() {
    accepts(&DIRECT.replace(
        "if select { a } else { b }",
        "if select { if select { a } else { b } } else { b }",
    ));
}
#[test]
fn unique_if_join_without_early_return() {
    accepts(&DIRECT.replace("if string.byte_len(a)==0u64 { return unique b }; ", ""));
}

#[test]
fn unique_if_join_borrowed_resource_is_rejected() {
    let source = include_str!("../../../../conformance/OWN/OWN-5-unique-if-join-resource-bad.hir");
    let mut engine = QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
    let file = engine
        .sources
        .add("unique-resource.hir", source.into())
        .unwrap();
    let diagnostics = match check_snapshot(&mut engine, file, &default_context()) {
        Ok(diagnostics) | Err(hirsel_types::QueryError::Diagnostics(diagnostics)) => diagnostics,
        Err(error) => panic!("unexpected {error:?}"),
    };
    assert!(
        diagnostics
            .iter()
            .any(|d| d.rule == hirsel_diag::RuleId::of("OWN-5")),
        "{diagnostics:?}"
    );
}

#[test]
fn unique_if_join_matches_reference_heap_llvm_and_cranelift() {
    use hirsel_driver::native::*;
    use hirsel_engine::{
        RunConfiguration, check_progress, entry_path, run_observed_snapshot, with_compiler_stack,
    };
    with_compiler_stack(|| {
        let mut options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        options.validation = Validation::EveryStage;
        let matrix = vec![
            Configuration {
                options: options.clone(),
                engine: Engine::Heap { campaign: true },
            },
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
        let directory =
            std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("unique-if-join");
        let toolchain = Toolchain::for_matrix(&directory.join("runtime"), &matrix).unwrap();
        for (name, source, expected) in [
            (
                "if",
                include_str!("../../../../conformance/MODE/MODE-2-unique-if-join-run.hir"),
                b"a\nb\nb\n".as_slice(),
            ),
            (
                "match",
                include_str!("../../../../conformance/MODE/MODE-2-unique-match-join-run.hir"),
                b"a\nb\nb\n".as_slice(),
            ),
            (
                "nested",
                include_str!("../../../../conformance/MODE/MODE-2-unique-nested-if-join-run.hir"),
                b"a\nb\nb\nb\n".as_slice(),
            ),
            (
                "bound",
                include_str!("../../../../conformance/MODE/MODE-2-unique-bound-join-run.hir"),
                b"a\nb\nb\n".as_slice(),
            ),
        ] {
            let mut engine =
                QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
            let file = engine
                .sources
                .add(format!("unique-{name}.hir"), source.into())
                .unwrap();
            let checked = check_progress(&mut engine, file, &default_context()).unwrap();
            assert!(
                checked.diagnostics.is_empty(),
                "{name}: {:?}",
                checked.diagnostics
            );
            let entry = entry_path(&default_context());
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
            assert_eq!(reference.output, expected, "{name}");
            let input = engine.export_native_input().unwrap();
            let input = engine.import_native_input(input).unwrap();
            std::fs::create_dir_all(directory.join(name)).unwrap();
            let outcomes = differential(
                &input,
                &entry,
                &reference,
                &matrix,
                &toolchain,
                &directory.join(name),
            );
            assert_eq!(outcomes.len(), 3);
            for (configuration, outcome) in outcomes {
                assert!(
                    matches!(outcome, Outcome::Agree),
                    "{name} {configuration}: {outcome:?}"
                );
            }
        }
    });
}
#[test]
fn unique_if_join_detached_destination_is_still_rejected_by_v() {
    use hirsel_tir::{InstructionKind, Rvalue, Storage};
    use hirsel_types::UseKind;
    let mut engine = QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
    let file = engine
        .sources
        .add("unique-join.hir", DIRECT.into())
        .unwrap();
    let checked = check_snapshot(&mut engine, file, &default_context()).unwrap();
    assert!(checked.is_empty(), "{checked:?}");
    let input = engine.export_native_input().unwrap();
    let mut pick = hirsel_engine::entry_path(&default_context());
    pick.0.name = "pick".into();
    let mut definition = (**input.definitions.get(&pick).unwrap()).clone();
    let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
    for signature in input.signatures.values() {
        signatures.insert(signature.definition, (**signature).clone());
    }
    hirsel_validate::assert_valid(&definition, engine.types(), &signatures);
    let body = &mut definition.bodies[definition.entry.0 as usize];
    let join = body
        .blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .find_map(|i| {
            let InstructionKind::Assign { destination, value } = i.kind else {
                return None;
            };
            let Rvalue::Use(ref source) = body.rvalues[value.0 as usize] else {
                return None;
            };
            let root = body.places[destination.0 as usize].root;
            (source.usage == UseKind::Borrow
                && body.locals[root.0 as usize].storage == Storage::Temporary)
                .then_some(root)
        })
        .expect("the conditional's borrowed destination");
    // Reintroduce the original bug: a detached result row on the input view.
    body.locals[join.0 as usize].ty = body.result;
    for place in &mut body.places {
        if place.root == join && place.projection.is_empty() {
            place.ty = body.result;
        }
    }
    let report = hirsel_validate::validate(&definition, engine.types(), &signatures, &mut vec![]);
    assert!(
        report.violations.iter().any(|v| matches!(
            v.kind,
            hirsel_validate::ViolationKind::TypeMismatch {
                entity: "assignment"
            }
        )),
        "{:?}",
        report.violations
    );
}
