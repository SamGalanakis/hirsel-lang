//! The heap executor's acceptance tests:
//! each deliberately broken fixture fails while printing exactly what its
//! corrected twin prints, and each twin passes the terminal audit.
use crate::fixtures;

use fixtures::*;
use hirsel_codec::{self as snapshot, Snapshot};
use hirsel_heapcheck::build::path;
use hirsel_heapcheck::{Config, Run};
use hirsel_lir::stage::Stage;
use std::path::PathBuf;

fn execute_program<S: Stage>(p: &hirsel_heapcheck::Program<S>) -> Run {
    run(p, &path("main"), &[], &mut Printer, &Config::default())
}

fn execute(p: &AnyProgram) -> Run {
    match p {
        AnyProgram::Rc(p) => execute_program(p),
        AnyProgram::Reuse(p) => execute_program(p),
        AnyProgram::Ready(p) => execute_program(p),
    }
}

#[test]
fn every_broken_fixture_fails_although_its_output_matches() {
    for fx in FIXTURES {
        let broken = execute(&(fx.build)(false));
        assert_eq!(
            String::from_utf8_lossy(&broken.output),
            fx.output,
            "{}: output",
            fx.name
        );
        assert_eq!(
            broken.violation(),
            Some(fx.expect),
            "{}: {:?}",
            fx.name,
            broken.result
        );
    }
}

#[test]
fn every_corrected_twin_passes_the_terminal_audit() {
    for fx in FIXTURES {
        let fixed = execute(&(fx.build)(true));
        let audit = fixed
            .result
            .as_ref()
            .unwrap_or_else(|v| panic!("{}: {v}", fx.name));
        assert_eq!(
            String::from_utf8_lossy(&audit.observation.output),
            fx.output,
            "{}",
            fx.name
        );
        assert!(audit.observation.failures.is_empty(), "{}", fx.name);
    }
}

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../native-tests/heap")
}

/// Compare `text` with the golden file, or rewrite it under HIRSEL_BLESS=1.
fn golden(name: &str, text: &str) -> String {
    let file = golden_dir().join(name);
    if std::env::var_os("HIRSEL_BLESS").is_some() {
        std::fs::create_dir_all(golden_dir()).unwrap();
        std::fs::write(&file, text).unwrap();
    }
    let stored = std::fs::read_to_string(&file)
        .unwrap_or_else(|e| panic!("{}: {e} (bless with HIRSEL_BLESS=1)", file.display()));
    if stored != text {
        let line = stored
            .lines()
            .zip(text.lines())
            .position(|(a, b)| a != b)
            .unwrap_or(stored.lines().count().min(text.lines().count()));
        panic!(
            "{} differs from line {} (bless with HIRSEL_BLESS=1)",
            file.display(),
            line + 1
        );
    }
    stored
}

fn round_trip<S: Stage>(name: &str, p: &hirsel_heapcheck::Program<S>) -> Run {
    let text = snapshot::to_text(p);
    let stored = golden(name, &text);
    let decoded: hirsel_heapcheck::Program<S> = snapshot::from_text(&stored).expect("decodes");
    assert_eq!(&decoded, p, "{name} round trip");
    assert_eq!(
        snapshot::to_text(&decoded),
        text,
        "{name} re-encodes identically"
    );
    execute_program(&decoded)
}

#[test]
fn fixtures_run_from_their_snapshots_in_native_tests() {
    for fx in FIXTURES {
        for fixed in [false, true] {
            let twin = if fixed { "fixed" } else { "broken" };
            let name = format!("{}.{twin}.lir", fx.name);
            let run = match (fx.build)(fixed) {
                AnyProgram::Rc(p) => round_trip(&name, &p),
                AnyProgram::Reuse(p) => round_trip(&name, &p),
                AnyProgram::Ready(p) => round_trip(&name, &p),
            };
            assert_eq!(run.violation(), (!fixed).then_some(fx.expect), "{name}");
            if fixed {
                let trace: String = run.trace.iter().map(|e| format!("{e:?}\n")).collect();
                golden(&format!("{}.fixed.trace", fx.name), &trace);
            }
        }
    }
}

// Keep the Snapshot trait in scope for `to_text`/`from_text` bounds.
#[allow(dead_code)]
fn _snapshot_bound<T: Snapshot>() {}
