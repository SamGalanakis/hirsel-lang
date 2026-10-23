//! Dogfood of the existing strong element contract; no ownership rule changes.
use super::super::*;
use hirsel_driver::native::{
    Configuration, Engine, Observation, OptLevel, Options, Partition, Runtime, TargetSpec,
    Toolchain, differential,
};

const APP: &str = include_str!("../../../../../examples/wc2/main.hir");

fn weak_app() -> String {
    // The original app, reconstructed without storing a second application.
    APP.replace("List[unique String]", "List[String]")
        .replace("for line in take scan_lines", "for line in scan_lines")
        .replace(
            "list.push(mut lines, take line)",
            "list.push(mut lines, unique line)",
        )
}

fn driven(source: &str) -> String {
    // H's existing adapter supplies empty Args and an empty writable root. Feed
    // explicit arguments at that boundary and create files through the language
    // API. Keep parse, buffered scanning, Scan transfer, sort and errors intact.
    // Host CLI entry coverage remains in the two existing wc2 tests.
    let source = source.split_once("pub fn main(").unwrap().0;
    let source = source
        .replace(
            "fn execute(rt: Io, args: Args)",
            "fn execute(rt: Io, args: List[String])",
        )
        .replace("arg.len(args)", "list.len(args)")
        .replace("arg.text(args, i)?", "unique args[i]")
        .replace("path.of(\"data\")", "path.of(\"\")");
    format!("{source}\n{DRIVER}")
}

const DRIVER: &str = r#"
fn put(d: Dir, name: String, data: Bytes) -> Result[(), AppError] {
  let mut f = file.create_atomic(d, path.of(name))?
  file.write_all(d, mut f, data)?
  file.commit(d, take f)?
  Ok(())
}
pub fn main(rt: Io) -> Result[(), AppError] {
  let d = io.dir(rt, path.of(""))?
  put(d, "empty", b"")?
  put(d, "single", b"one two")?
  put(d, "left", b"z\na\na\n")?
  put(d, "right", b"\na\nb")?
  execute(rt, ["empty"])?
  execute(rt, ["--unique", "empty"])?
  execute(rt, ["single"])?
  execute(rt, ["--unique", "single"])?
  execute(rt, ["left", "right"])?
  execute(rt, ["--unique", "left", "right"])?
  execute(rt, ["--unique", "left", "left"])?
  let output = io.stdout(rt)?
  match execute(rt, ["--unknown"]) {
    Error(Flags(_)) => out.print(output, "flags\n")?,
    _ => return Error(Files(NotFound())),
  }
  match execute(rt, ["--unique", "left", "missing"]) {
    Error(Files(NotFound())) => out.print(output, "missing\n")?,
    _ => return Error(Files(NotFound())),
  }
  Ok(())
}
"#;

#[test]
fn wc2_contract_original_and_strong_all_engines() {
    hirsel_engine::with_compiler_stack(|| {
        let scratch = tempfile::tempdir().unwrap();
        let matrix: Vec<_> = [
            Engine::Heap { campaign: false },
            Engine::Native {
                opt: OptLevel::Speed,
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
            Engine::Cranelift {
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        ]
        .into_iter()
        .map(|engine| Configuration {
            options: Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
            engine,
        })
        .collect();
        let toolchain = Toolchain::for_matrix(&scratch.path().join("runtime"), &matrix).unwrap();
        let mut original = None;
        for (name, source) in [("weak", weak_app()), ("strong", APP.to_owned())] {
            let fixture = Fixture::new("wc2_contract", &driven(&source));
            let mut program = hirsel_driver::load_program(&fixture.0.join("hirsel.json")).unwrap();
            program.check_for_execution().unwrap();
            let mut simulation = hirsel_engine::SimulationConfig::default();
            simulation.grants = ["Io"].map(String::from).into();
            simulation.projections = ["stdout", "dir"].map(String::from).into();
            let run = hirsel_engine::run_observed_checked(
                &mut program.engine,
                &hirsel_engine::RunConfiguration {
                    entry: program.entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                },
                simulation,
            )
            .unwrap();
            let reference = Observation::of_reference(run.observation.as_ref().unwrap());
            assert_eq!(reference.exit_code, Some(0), "{name}: {reference:?}");
            assert!(reference.failures.is_empty(), "{name}: {reference:?}");
            assert_eq!(
                reference.output,
                b"0 0 0\n0 2 7\none two\n5 5 10\n\na\nb\nz\na\nz\nflags\nmissing\n"
            );
            if let Some(expected) = &original {
                assert_eq!(&reference, expected);
            } else {
                original = Some(reference.clone());
            }
            let input = program.engine.export_native_input().unwrap();
            let validated = program.engine.import_native_input(input).unwrap();
            let directory = scratch.path().join(name);
            std::fs::create_dir_all(&directory).unwrap();
            let results = differential(
                &validated,
                &program.entry,
                &reference,
                &matrix,
                &toolchain,
                &directory,
            );
            assert_eq!(results.len(), 3);
            for (configuration, outcome) in results {
                eprintln!("wc2 {name}/{configuration}: {outcome:?}");
                assert!(matches!(outcome, hirsel_driver::native::Outcome::Agree));
            }
        }
    });
}

#[test]
fn wc2_contract_recipes_and_live_alias_rejection() {
    let document = include_str!("../../../../../examples/wc2/FRICTION.md");
    for (index, name) in ["wc2_borrowed", "wc2_strong"].into_iter().enumerate() {
        let source = friction_recipe(
            document,
            "### Whole-value handback and row copies",
            index + 1,
        );
        export_friction_recipe(name, source);
        if index == 1 {
            // Strong syntax does not restore evidence lost to a live snapshot.
            // Reuse unique-copy-nested-20's disposition: reject explicit unique
            // receipt; ordinary mutation and explicit unique-copy repairs are
            // covered by that owner's registered all-engine controls.
            let aliased = source
                .replace("let mut out:", "let snapshot = copy source\n  let mut out:")
                .replace("need(take out)", "need(take out)\n  let _ = snapshot");
            let fixture = Fixture::new("wc2_aliased", &aliased);
            let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
                .env("HIRSEL_DAEMON", "off")
                .env("HIRSEL_RESULTS", "none@1")
                .env("HIRSEL_JOBS", "1")
                .args(["check", "--json"])
                .arg(fixture.0.join("hirsel.json"))
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(1), "{output:?}");
            let diagnostics = String::from_utf8(output.stdout).unwrap();
            assert!(diagnostics.contains("\"rule\":\"MODE-2\""), "{diagnostics}");
            assert!(diagnostics.contains("copy ended"), "{diagnostics}");
        }
    }
}

#[test]
fn wc2_contract_error_exits_all_engines() {
    hirsel_engine::with_compiler_stack(|| {
        let scratch = tempfile::tempdir().unwrap();
        let matrix: Vec<_> = [
            Engine::Heap { campaign: false },
            Engine::Native {
                opt: OptLevel::Speed,
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
            Engine::Cranelift {
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        ]
        .into_iter()
        .map(|engine| Configuration {
            options: Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
            engine,
        })
        .collect();
        let toolchain = Toolchain::for_matrix(&scratch.path().join("runtime"), &matrix).unwrap();
        for (case, args) in [
            ("flags", r#"["--unknown"]"#),
            ("missing", r#"["--unique", "left", "missing"]"#),
        ] {
            let mut original = None;
            for (name, source) in [("weak", weak_app()), ("strong", APP.to_owned())] {
                // Return the actual execute error through the launch boundary.
                // The missing-file case drops rows collected from `left`.
                let driver = format!(
                    r#"
fn put(d: Dir, name: String, data: Bytes) -> Result[(), AppError] {{
  let mut f = file.create_atomic(d, path.of(name))?
  file.write_all(d, mut f, data)?
  file.commit(d, take f)?
  Ok(())
}}
pub fn main(rt: Io) -> Result[(), AppError] {{
  let d = io.dir(rt, path.of(""))?
  put(d, "left", b"z\na\na\n")?
  execute(rt, {args})
}}
"#
                );
                let source = driven(&source).replace(DRIVER, &driver);
                let fixture = Fixture::new("wc2_error_exit", &source);
                let mut program =
                    hirsel_driver::load_program(&fixture.0.join("hirsel.json")).unwrap();
                program.check_for_execution().unwrap();
                let mut simulation = hirsel_engine::SimulationConfig::default();
                simulation.grants = ["Io"].map(String::from).into();
                simulation.projections = ["stdout", "dir"].map(String::from).into();
                let run = hirsel_engine::run_observed_checked(
                    &mut program.engine,
                    &hirsel_engine::RunConfiguration {
                        entry: program.entry.clone(),
                        hash_seed: 0,
                        inputs: vec![],
                    },
                    simulation,
                )
                .unwrap();
                let reference = Observation::of_reference(run.observation.as_ref().unwrap());
                assert_eq!(reference.exit_code, Some(1), "{case}/{name}: {reference:?}");
                assert!(
                    reference.failures.is_empty(),
                    "{case}/{name}: {reference:?}"
                );
                assert!(reference.output.is_empty(), "{case}/{name}: {reference:?}");
                if let Some(expected) = &original {
                    assert_eq!(&reference, expected);
                } else {
                    original = Some(reference.clone());
                }
                let input = program.engine.export_native_input().unwrap();
                let validated = program.engine.import_native_input(input).unwrap();
                let directory = scratch.path().join(format!("{case}-{name}"));
                std::fs::create_dir_all(&directory).unwrap();
                let results = differential(
                    &validated,
                    &program.entry,
                    &reference,
                    &matrix,
                    &toolchain,
                    &directory,
                );
                assert_eq!(results.len(), 3);
                for (configuration, outcome) in results {
                    eprintln!("wc2 {case}/{name}/{configuration}: {outcome:?}");
                    assert!(matches!(outcome, hirsel_driver::native::Outcome::Agree));
                }
            }
        }
    });
}
