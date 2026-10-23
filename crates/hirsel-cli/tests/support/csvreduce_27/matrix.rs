use super::cases;
use hirsel_driver::native::{
    self, Configuration, Engine, Observation, OptLevel, Options, Partition, Runtime, TargetSpec,
    Toolchain,
};
use sha2::{Digest, Sha256};
use std::path::Path;

const APP: &str = include_str!("../../../../../examples/csvreduce/main.hir");
fn literal(text: &str) -> String {
    serde_json::to_string(text).unwrap()
}

pub fn program(part: usize) -> (String, Vec<u8>) {
    // Existing public adapters supply empty Args and a writable virtual root.
    // Only entry plumbing changes: use the shipped execute (including bounded
    // file read and real task orchestration), never a substitute pure model.
    let mut source = APP.replace("pub fn main(", "fn cli_main(");
    source.push_str("\npub fn main(rt: Io) -> Result[(), AppError] {\nlet d = io.dir(rt, path.of(\"\"))?\nfile.mkdir(d, path.of(\"data\"))?\nlet output = io.stdout(rt)?\n");
    let mut expected = Vec::new();
    for case in cases::cases(part) {
        source.push_str(&format!(
            "file.write_bytes(d, path.of(\"data/input.csv\"), string.as_bytes({}))?\n",
            literal(&case.csv)
        ));
        for grain in case.grains {
            let label = format!("{}/{grain}\n", case.id);
            eprintln!(
                "csvreduce selected {label:?} input_sha256={:x} expected_sha256={:x}",
                Sha256::digest(case.csv.as_bytes()),
                Sha256::digest(case.expected.as_bytes())
            );
            source.push_str("{\n");
            if case.error {
                source.push_str(&format!("match execute(rt, \"input.csv\", 1, {grain}) {{ Error({}()) => {{}}, _ => return Error(Usage()), }}\n", case.expected));
            } else {
                source.push_str(&format!("let summary = execute(rt, \"input.csv\", 1, {grain})?\nif render(summary) != {} {{ return Error(Usage()) }}\n", literal(case.expected)));
            }
            source.push_str(&format!("out.print(output, {})?\n}}\n", literal(&label)));
            expected.extend_from_slice(label.as_bytes());
        }
    }
    source.push_str("Ok(())\n}\n");
    (source, expected)
}

pub fn partition(part: usize) {
    hirsel_engine::with_compiler_stack(|| {
        let scratch = tempfile::tempdir().unwrap();
        let (source, output) = program(part);
        let path = scratch.path().join("main.hir");
        std::fs::write(&path, &source).unwrap();
        let stdlib = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        let manifest = scratch.path().join("hirsel.json");
        std::fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"csvreduce_test","version":"1"},"entry":{"module":[],"name":"main"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}]}).to_string()).unwrap();
        let mut program = hirsel_driver::load_program(&manifest).unwrap();
        program.check_for_execution().unwrap();
        // Bounded Rust harness partitions, not corpus fixtures: the record-cap
        // and 1024-vector lanes have explicit initial 20M-step limits.
        let steps = if part == 3 || part == 5 {
            20_000_000
        } else {
            1_000_000
        };
        program.engine.adapters.interpreter = Some(Box::new(hirsel_engine::InterpreterAdapter {
            limits: hirsel_interp::Limits {
                steps: Some(steps),
                ..Default::default()
            },
            ..Default::default()
        }));
        let config = hirsel_engine::RunConfiguration {
            entry: program.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let expected = Observation {
            exit_code: Some(0),
            blocked: false,
            budget: None,
            signaled: None,
            output,
            failures: vec![],
            roots: vec![],
        };
        // Exact recovery may select only native cells after both reference
        // observations passed on the identical application source. Normal runs
        // retain the complete matrix and every assertion below.
        let engines = std::env::var("HIRSEL_CSVREDUCE_ENGINES").unwrap_or_else(|_| "all".into());
        assert!(
            engines == "all" || engines == "native",
            "unknown engine selection {engines}"
        );
        eprintln!("csvreduce engine-selection={engines} part={part}");
        for checked in [false, true].into_iter().filter(|_| engines == "all") {
            eprintln!(
                "csvreduce started part={part} engine={} source_sha256={:x}",
                if checked { "checked-H" } else { "interpreter" },
                Sha256::digest(source.as_bytes())
            );
            let run = if checked {
                hirsel_engine::run_observed_checked(
                    &mut program.engine,
                    &config,
                    Default::default(),
                )
            } else {
                hirsel_engine::run_observed_snapshot(
                    &mut program.engine,
                    &config,
                    Default::default(),
                )
            }
            .unwrap();
            assert_eq!(
                Observation::of_reference(run.observation.as_ref().unwrap()),
                expected,
                "part={part} checked={checked}"
            );
            eprintln!("csvreduce completed part={part} checked={checked} PASS");
        }
        let input = program.engine.export_native_input().unwrap();
        let validated = program.engine.import_native_input(input).unwrap();
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
        let fixture = native::Fixture {
            steps: Some(steps),
            ..Default::default()
        };
        let results = native::differential_in(
            &validated,
            &program.entry,
            &expected,
            &matrix,
            &toolchain,
            scratch.path(),
            &fixture,
        );
        assert_eq!(results.len(), 3);
        for (engine, outcome) in results {
            eprintln!("csvreduce completed part={part} engine={engine} {outcome:?}");
            assert_eq!(
                outcome,
                native::Outcome::Agree,
                "part={part} engine={engine}"
            );
        }
    });
}
