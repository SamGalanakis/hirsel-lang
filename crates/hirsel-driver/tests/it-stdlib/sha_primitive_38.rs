//! CAVP one-shot vectors exercise V, checked H, heap, LLVM and Cranelift.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::Path;
#[path = "../support/history.rs"]
#[allow(dead_code, reason = "shared runner supports other histories")]
mod history;

#[test]
fn sha_primitive_38_cavp_four_engines() {
    with_compiler_stack(|| {
        let mut source = String::from("pub fn main() -> Result[(), ()] {\n");
        let mut count = 0;
        for row in include_str!("../../../../native-tests/library/vectors/sha-cavp.tsv").lines() {
            let f: Vec<_> = row.split('\t').collect();
            let (algorithm, length) = (f[0], f[1]);
            let message = super::reference_vectors::bytes(f[2]);
            let backing = super::reference_vectors::bytes(&format!("abcd{}efff", f[2]));
            let expected = super::reference_vectors::bytes(f[3]);
            source.push_str(&format!(
                "// {algorithm} {length} {}\nfailure.check(std@1/bytes.from_inline(std@1/{algorithm}.digest({message})) == {expected}, Bounds())\nlet data_{count} = {backing}\nfailure.check(std@1/bytes.from_inline(std@1/{algorithm}.digest(bytes.slice(data_{count}, 2, 2 + {length}u64))) == {expected}, Bounds())\n", f[4]));
            if length == "55" {
                let elements = f[2]
                    .as_bytes()
                    .chunks_exact(2)
                    .map(|b| {
                        format!(
                            "{}u8",
                            u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16).unwrap()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                source.push_str(&format!("let raw_{count}: List[u8] = [{elements}]\nlet owned_{count}=bytes.from_list(take raw_{count})\nlet frozen_{count}=shared.freeze(take owned_{count})\nlet digest_{count}=shared.with(frozen_{count},core@1/bytes.{algorithm})\nfailure.check(std@1/bytes.from_inline(digest_{count}) == {expected},Bounds())\n"));
            }
            count += 1;
        }
        assert_eq!(count, 54);
        source.push_str("Ok(())\n}\n");
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/std_numeric.hir");
        let mut program = load_program_source(&path, Some(&source)).unwrap();
        program
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let entry = program.entry.clone();
        let configuration = RunConfiguration {
            entry: entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let reference = run_observed_snapshot(
            &mut program.engine,
            &configuration,
            SimulationConfig::default(),
        )
        .unwrap();
        assert!(
            matches!(
                reference.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            "{reference:?}"
        );
        let checked = run_observed_checked(
            &mut program.engine,
            &configuration,
            SimulationConfig::default(),
        )
        .unwrap();
        assert_eq!(reference.observation, checked.observation);
        assert_eq!(reference.step_usage, checked.step_usage);
        let input = program.engine.export_native_input().unwrap();
        let validated = program.engine.import_native_input(input).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let matrix = [
            hirsel_driver::native::Engine::Heap { campaign: false },
            hirsel_driver::native::Engine::Native {
                opt: hirsel_driver::native::OptLevel::None,
                partition: hirsel_driver::native::Partition::Whole,
                runtime: hirsel_driver::native::Runtime::Diagnostic,
            },
            hirsel_driver::native::Engine::Cranelift {
                partition: hirsel_driver::native::Partition::Whole,
                runtime: hirsel_driver::native::Runtime::Diagnostic,
            },
        ]
        .into_iter()
        .map(|engine| hirsel_driver::native::Configuration {
            options: hirsel_driver::native::Options::new(
                hirsel_driver::native::TargetSpec::x86_64_unknown_linux_gnu(),
            ),
            engine,
        })
        .collect();
        let runner = history::Runner::with_matrix(temp.path(), matrix).unwrap();
        let report = runner.compare(
            &validated,
            &entry,
            &hirsel_driver::native::Observation::of_reference(
                reference.observation.as_ref().unwrap(),
            ),
            "sha-cavp",
        );
        assert!(report.agrees(), "{report:#?}");
        assert_eq!(report.cells.len(), 4);
        eprintln!(
            "SHA_CAVP {count} vectors, full+subextent; reference/checked-H/heap/LLVM/Cranelift passed"
        );
    });
}

#[test]
fn sha_primitive_38_reference_and_checked_charges_are_exact() {
    with_compiler_stack(|| {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/std_numeric.hir");
        let measure = |source: &str| {
            let mut program = load_program_source(&path, Some(source)).unwrap();
            let output = program.check(LoweringPolicy::Reachable).unwrap();
            for diagnostic in &output.diagnostics {
                eprintln!("{}", diagnostic.render_human(&output.sources));
            }
            program
                .check_for_execution()
                .unwrap_or_else(|e| panic!("{e}"));
            let config = RunConfiguration {
                entry: program.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            };
            let reference =
                run_observed_snapshot(&mut program.engine, &config, SimulationConfig::default())
                    .unwrap();
            let checked =
                run_observed_checked(&mut program.engine, &config, SimulationConfig::default())
                    .unwrap();
            assert!(
                matches!(
                    reference.outcome,
                    Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
                ),
                "{reference:?}"
            );
            assert_eq!(reference.observation, checked.observation);
            assert_eq!(reference.step_usage, checked.step_usage);
            reference.step_usage.unwrap().consumed
        };
        for algorithm in ["sha1", "sha224", "sha256", "sha384", "sha512", "sha512_256"] {
            for callback in [false, true] {
                for length in [0, 55, 56, 63, 64, 119, 128] {
                    let input = super::reference_vectors::bytes(&"ab".repeat(length));
                    let elements = vec!["171u8"; length].join(",");
                    let source = |operation| {
                        if callback {
                            format!(
                                "pub fn main() -> Result[(), ()] {{ let raw: List[u8]=[{elements}]\nlet owned=bytes.from_list(take raw)\nlet frozen=shared.freeze(take owned)\nlet _=shared.with(frozen,core@1/bytes.{operation})\nOk(()) }}"
                            )
                        } else {
                            format!(
                                "pub fn main() -> Result[(), ()] {{ let _ = core@1/bytes.{operation}({input})\nOk(()) }}"
                            )
                        }
                    };
                    // The identical construction and ordinary call step cancel. The
                    // remaining delta is exactly the normative primitive charge.
                    let control = measure(&source("len"));
                    let charged = measure(&source(algorithm));
                    let blocks = if ["sha1", "sha224", "sha256"].contains(&algorithm) {
                        length / 64 + 1 + usize::from(length % 64 >= 56)
                    } else {
                        length / 128 + 1 + usize::from(length % 128 >= 112)
                    };
                    assert_eq!(
                        charged - control,
                        1 + blocks as u64,
                        "{algorithm} {length} callback={callback}"
                    );
                    eprintln!("SHA_CHARGE_PASS {algorithm} length={length} callback={callback}");
                }
            }
            let input = super::reference_vectors::bytes(&"ab".repeat(8192));
            let source = format!(
                "pub fn main() -> Result[(), ()] {{ let _ = core@1/bytes.{algorithm}({input})\nOk(()) }}"
            );
            let mut program = load_program_source(&path, Some(&source)).unwrap();
            program
                .check_for_execution()
                .unwrap_or_else(|e| panic!("{e}"));
            program.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
                limits: hirsel_interp::Limits {
                    steps: Some(32),
                    call_depth: 256,
                    source: None,
                },
                ..Default::default()
            }));
            let config = RunConfiguration {
                entry: program.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            };
            for checked in [false, true] {
                let result = if checked {
                    run_observed_checked(&mut program.engine, &config, SimulationConfig::default())
                } else {
                    run_observed_snapshot(&mut program.engine, &config, SimulationConfig::default())
                }
                .unwrap();
                assert!(
                    matches!(result.outcome, Ok(RunOutcome::ToolingLimit(_))),
                    "{algorithm}: {result:?}"
                );
                assert_eq!(result.step_usage.unwrap().consumed, 32);
            }
        }
    });
}
