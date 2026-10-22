//! Related data calls through body-local borrowed views.
use crate::rc_support::*;

const IMMUTABLE: &str = r#"
fn view(a: List[u64]) -> List[u64] in a { a }
fn f(a: List[u64]) -> List[u64] in a { let c = a; view(c) }
pub fn main() -> Result[(),u64] {
    if list.len(f([1,2])) == 2 { Ok(()) } else { Error(1) }
}
"#;

#[test]
fn compose_13_immutable_related_result() {
    lower_valid(IMMUTABLE);
}

#[test]
fn compose_13_immutable_expired_source_is_rejected_by_v() {
    use hirsel_tir::*;
    let program = compile(IMMUTABLE);
    let mut signatures = hirsel_validate::SignatureMap::new(program.types.clone());
    for (path, signature) in &program.signatures {
        signatures.insert(path.clone(), (**signature).clone());
    }
    let mut definition = (**program
        .definitions
        .iter()
        .find(|(p, _)| p.0.name == "f")
        .unwrap()
        .1)
        .clone();
    let body = &mut definition.bodies[0];
    let (loan, span, cleanup) = body
        .blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .find_map(|i| {
            if let InstructionKind::BeginLoan { id, .. } = i.kind {
                Some((id, i.span, i.on_failure))
            } else {
                None
            }
        })
        .unwrap();
    let next = body
        .blocks
        .iter()
        .find_map(|b| {
            if let Terminator::Call {
                returns: Some(ret), ..
            } = &b.terminator
            {
                Some(ret.next)
            } else {
                None
            }
        })
        .unwrap();
    body.blocks[next.0 as usize].instructions.insert(
        0,
        Instruction {
            kind: InstructionKind::EndLoan {
                loan,
                if_active: false,
            },
            span,
            on_failure: cleanup,
        },
    );
    let verdict = hirsel_validate::validate(&definition, &program.types, &signatures, &mut vec![]);
    assert!(
        verdict.violations.iter().any(|v| matches!(v.kind,
        hirsel_validate::ViolationKind::ExpiredLoan { loan: ended, .. } if ended == loan)),
        "{verdict:?}"
    );
}

#[test]
#[ignore = "native: immutable related-result interpreter, heap, LLVM and Cranelift parity"]
fn compose_13_immutable_native_parity() {
    native_parity(&[("immutable", IMMUTABLE)]);
}

fn native_parity(cases: &[(&str, &str)]) {
    use hirsel_driver::native::*;
    use hirsel_driver::*;
    use hirsel_engine::*;
    let options = Options::new(target());
    let matrix = vec![
        Configuration {
            options: options.clone(),
            engine: Engine::Heap { campaign: false },
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
        std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("borrow-cursor-compose-13");
    std::fs::create_dir_all(&directory).unwrap();
    let toolchain = Toolchain::for_matrix(&directory.join("runtime"), &matrix).unwrap();
    for &(name, source) in cases {
        with_compiler_stack(|| {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let context = default_context();
            let file = engine.sources.add("compose.hir", source.into()).unwrap();
            let checked = check_progress(&mut engine, file, &context).unwrap();
            assert!(
                checked.diagnostics.is_empty(),
                "{name}: {:?}",
                checked.diagnostics
            );
            let entry = entry_path(&context);
            let reference = run_observed_snapshot(
                &mut engine,
                &RunConfiguration {
                    entry: entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            )
            .unwrap();
            let reference = Observation::of_reference(reference.observation.as_ref().unwrap());
            assert_eq!(reference.exit_code, Some(0), "{name}: {reference:?}");
            eprintln!("{name}: interpreter exit0");
            let snapshot = engine.export_native_input().unwrap();
            let validated = engine.import_native_input(snapshot).unwrap();
            let directory = directory.join(name);
            std::fs::create_dir_all(&directory).unwrap();
            let mut executions = 0;
            for (configuration, outcome) in differential(
                &validated, &entry, &reference, &matrix, &toolchain, &directory,
            ) {
                assert!(
                    matches!(outcome, Outcome::Agree),
                    "{name}: {configuration}: {outcome:?}"
                );
                eprintln!("{name}: {configuration}: agrees");
                executions += 1;
            }
            assert_eq!(executions, 3);
        });
    }
}
