//! Comparisons borrow both operands, including nested aggregate fields.
use hirsel_engine::{
    DefinitionQueries, QueryEngine, check_snapshot, default_context, default_environment,
};

fn accepts(source: &str) {
    let mut engine = QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
    let file = engine
        .sources
        .add("tuple-compare.hir", source.into())
        .unwrap();
    let diagnostics = check_snapshot(&mut engine, file, &default_context()).unwrap();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let mut path = hirsel_engine::entry_path(&default_context());
    path.0.name = "visible".into();
    let definition = engine.lower_to_tir(&path).unwrap().value;
    eprintln!("{}", hirsel_lower::print::definition(&definition));
}

#[test]
fn tuple_compare_incompatible_nested_rows_are_still_rejected_by_v() {
    use hirsel_tir::Rvalue;
    use hirsel_types::{Relation, TypeKind};
    let mut engine = QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
    let file = engine.sources.add("tuple-compare.hir", "fn visible(p:(u64,List[u64]),take expected:List[(u64,List[u64])])->bool { let(k,v)=p; (k,v)==expected[0] }".into()).unwrap();
    assert!(
        check_snapshot(&mut engine, file, &default_context())
            .unwrap()
            .is_empty()
    );
    let input = engine.export_native_input().unwrap();
    let mut path = hirsel_engine::entry_path(&default_context());
    path.0.name = "visible".into();
    let mut definition = (**input.definitions.get(&path).unwrap()).clone();
    let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
    for signature in input.signatures.values() {
        signatures.insert(signature.definition, (**signature).clone());
    }
    hirsel_validate::assert_valid(&definition, engine.types(), &signatures);
    let body = &mut definition.bodies[definition.entry.0 as usize];
    let right = body
        .rvalues
        .iter()
        .find_map(|rvalue| match rvalue {
            Rvalue::Binary { right, .. } => Some(right.place),
            _ => None,
        })
        .unwrap();
    let mut row = engine
        .types()
        .get(body.places[right.0 as usize].ty)
        .unwrap()
        .clone();
    let TypeKind::Tuple(parts) = &mut row.kind else {
        panic!("tuple comparison")
    };
    let mut part = engine.types().get(parts[1]).unwrap().clone();
    assert!(!part.modes.relation.is_global());
    part.modes.relation = Relation::global();
    parts[1] = engine.types().intern(part);
    body.places[right.0 as usize].ty = engine.types().intern(row);
    definition.types = hirsel_tir::definition_types(engine.types(), &definition);
    let report = hirsel_validate::validate(&definition, engine.types(), &signatures, &mut vec![]);
    assert!(
        report.violations.iter().any(|v| matches!(
            v.kind,
            hirsel_validate::ViolationKind::TypeMismatch {
                entity: "binary operands"
            }
        )),
        "{:?}",
        report.violations
    );
}

#[test]
fn tuple_compare_matches_reference_heap_llvm_and_cranelift() {
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
        let directory = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("tuple-compare");
        let toolchain = Toolchain::for_matrix(&directory.join("runtime"), &matrix).unwrap();
        let root = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
        for name in ["loop", "equal", "nested", "field"] {
            let path = format!("conformance/MODE/MODE-2-tuple-compare-{name}-run.hir");
            let source = std::fs::read_to_string(root.join(path)).unwrap();
            let mut engine =
                QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
            let file = engine
                .sources
                .add(format!("tuple-{name}.hir"), source)
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
            assert_eq!(reference.output, b"ok\n", "{name}");
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
fn tuple_compare_ordering_stays_rejected() {
    // SEM-5 permits ordering operators only for scalar and byte operands.
    for op in ["<", "<=", ">", ">="] {
        for (left, right) in [("(k,v)", "expected[0]"), ("expected[0]", "(k,v)")] {
            let source = format!(
                "fn visible(p:(u64,List[u64]),take expected:List[(u64,List[u64])])->bool {{ let(k,v)=p; {left}{op}{right} }}"
            );
            let mut engine =
                QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
            let file = engine.sources.add("tuple-order.hir", source).unwrap();
            let diagnostics = match check_snapshot(&mut engine, file, &default_context()) {
                Ok(diagnostics) | Err(hirsel_types::QueryError::Diagnostics(diagnostics)) => {
                    diagnostics
                }
                Err(error) => panic!("unexpected {error:?}"),
            };
            assert!(
                diagnostics
                    .iter()
                    .any(|d| d.rule == hirsel_diag::RuleId::of("LANG-7")
                        && d.message == "ordering requires scalars or byte strings"),
                "{diagnostics:?}"
            );
        }
    }
}
