//! Borrowed UTF-8 subjects agree with the frozen materializing implementation.
use hirsel_driver::*;
use hirsel_engine::*;
use serde_json::{Value, json};
use std::path::PathBuf;

const VECTORS: &str = include_str!("../fixtures/regex-subject-borrow-25.json");
const HELPERS: &str = include_str!("../fixtures/regex-subject-borrow-25.hir");

fn literal(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn run(engine: &str, partition: usize) {
    with_compiler_stack(|| {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let std_root = root.join("stdlib/std@1").canonicalize().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut manifest: Value =
            serde_json::from_slice(&std::fs::read(std_root.join("hirsel.json")).unwrap()).unwrap();
        let sources = manifest["sources"].as_array_mut().unwrap();
        sources.retain(|s| {
            s["module"] == json!(["regex"])
                || s["module"] == json!(["ascii"])
                || s["module"] == json!(["internal", "substring"])
        });
        for s in sources.iter_mut() {
            s["path"] = std_root
                .join(s["path"].as_str().unwrap())
                .to_str()
                .unwrap()
                .into();
        }
        let builders = std::fs::read_to_string(std_root.join("text.hir")).unwrap();
        std::fs::write(
            temp.path().join("text.hir"),
            builders.split("// Pads the text").next().unwrap(),
        )
        .unwrap();
        sources.push(json!({"path":temp.path().join("text.hir"), "module":["text"]}));
        let vectors: Value = serde_json::from_str(VECTORS).unwrap();
        let rows = vectors["pairs"].as_array().unwrap();
        assert_eq!(rows.len(), 96);
        let selection = if engine == "Cranelift" {
            0..rows.len()
        } else if engine == "LLVM" {
            partition * 24..(partition + 1) * 24
        } else {
            partition * 8..(partition + 1) * 8
        };
        let mut source = String::from(HELPERS);
        source.push_str("\npub fn borrow_check() -> Result[(), u64] uses {} {\n");
        for n in selection.clone() {
            let row = &rows[n];
            source.push_str(&format!(
                " if borrow_results({}, {}) != {} {{ return Error({}u64) }}\n",
                literal(row["pattern"].as_str().unwrap()),
                literal(row["subject"].as_str().unwrap()),
                row["expected"],
                n
            ));
        }
        source.push_str(" borrow_view_edges()?\n Ok(())\n}\n");
        std::fs::write(temp.path().join("vectors.hir"), source).unwrap();
        sources.push(json!({"path":temp.path().join("vectors.hir"), "module":["regex"]}));
        manifest["entry"] = json!({"module":["regex"],"name":"borrow_check"});
        let path = temp.path().join("hirsel.json");
        std::fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
        eprintln!("BORROW_START engine={engine} pairs={selection:?}");
        if engine == "LLVM" || engine == "Cranelift" {
            let outcome = native::run_path_native_backend(
                &path,
                HostLaunch::empty(),
                None,
                if engine == "LLVM" {
                    native::Backend::Llvm
                } else {
                    native::Backend::Cranelift
                },
                native::Output::Streams {
                    stdout: &mut Vec::new(),
                    stderr: &mut Vec::new(),
                },
                Verify::On,
            )
            .unwrap()
            .unwrap();
            assert!(
                matches!(
                    outcome,
                    RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
                ),
                "{engine}: {outcome:?}"
            );
        } else {
            let mut loaded = load_program(&path).unwrap();
            loaded.check_for_execution().unwrap();
            loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
                limits: hirsel_interp::Limits {
                    steps: Some(100_000_000),
                    ..Default::default()
                },
                ..Default::default()
            }));
            let config = RunConfiguration {
                entry: loaded.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            };
            let result = if engine == "interpreter" {
                run_observed_snapshot(&mut loaded.engine, &config, SimulationConfig::default())
            } else {
                run_observed_checked(&mut loaded.engine, &config, SimulationConfig::default())
            }
            .unwrap();
            assert!(
                matches!(
                    result.outcome,
                    Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
                ),
                "{engine}: {result:?}"
            );
        }
        eprintln!("BORROW_PASS engine={engine} pairs={selection:?}");
    });
}
macro_rules! partition {
    ($i:ident,$h:ident,$n:literal) => {
        #[test]
        fn $i() {
            run("interpreter", $n);
        }
        #[test]
        fn $h() {
            run("checked-H", $n);
        }
    };
}
partition!(interpreter_00, checked_h_00, 0);
partition!(interpreter_01, checked_h_01, 1);
partition!(interpreter_02, checked_h_02, 2);
partition!(interpreter_03, checked_h_03, 3);
partition!(interpreter_04, checked_h_04, 4);
partition!(interpreter_05, checked_h_05, 5);
partition!(interpreter_06, checked_h_06, 6);
partition!(interpreter_07, checked_h_07, 7);
partition!(interpreter_08, checked_h_08, 8);
partition!(interpreter_09, checked_h_09, 9);
partition!(interpreter_10, checked_h_10, 10);
partition!(interpreter_11, checked_h_11, 11);
#[test]
fn llvm_00() {
    run("LLVM", 0);
}
#[test]
fn llvm_01() {
    run("LLVM", 1);
}
#[test]
fn llvm_02() {
    run("LLVM", 2);
}
#[test]
fn llvm_03() {
    run("LLVM", 3);
}

#[test]
fn cranelift() {
    run("Cranelift", 0);
}
