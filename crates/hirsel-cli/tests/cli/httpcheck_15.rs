//! Sequential scenario assertions and full offline replay on H, LLVM, Cranelift.
use std::{path::Path, process::Command};

#[test]
fn httpcheck_15_h_body_cap() {
    run(Some("h-body-cap"));
}

// Each exact engine/scenario case owns its package, listener, grants and
// tapes. Cost-balanced shards execute the complete 57-by-three matrix.
macro_rules! engine_cases {
    ($engine:literal) => {
        #[test]
        fn ordered_methods() {
            super::run(Some(concat!($engine, ".ordered-methods")));
        }
        #[test]
        fn mismatch_continues() {
            super::run(Some(concat!($engine, ".mismatch-continues")));
        }
        #[test]
        fn status_404_exact() {
            super::run(Some(concat!($engine, ".404-exact")));
        }
        #[test]
        fn body_cap() {
            super::run(Some(concat!($engine, ".body-cap")));
        }
        #[test]
        fn steps_cap() {
            super::run(Some(concat!($engine, ".steps-cap")));
        }
        #[test]
        fn bad_json() {
            super::run(Some(concat!($engine, ".bad-json")));
        }
        #[test]
        fn utf8() {
            super::run(Some(concat!($engine, ".utf8")));
        }
        #[test]
        fn redirect() {
            super::run(Some(concat!($engine, ".redirect")));
        }
        #[test]
        fn body_over() {
            super::run(Some(concat!($engine, ".body-over")));
        }
        #[test]
        fn chunk_over() {
            super::run(Some(concat!($engine, ".chunk-over")));
        }
        #[test]
        fn eof_over() {
            super::run(Some(concat!($engine, ".eof-over")));
        }
        #[test]
        fn head_over() {
            super::run(Some(concat!($engine, ".head-over")));
        }
        #[test]
        fn headers_over() {
            super::run(Some(concat!($engine, ".headers-over")));
        }
        #[test]
        fn truncated() {
            super::run(Some(concat!($engine, ".truncated")));
        }
        #[test]
        fn deadline() {
            super::run(Some(concat!($engine, ".deadline")));
        }
        #[test]
        fn transport_stops() {
            super::run(Some(concat!($engine, ".transport-stops")));
        }
        #[test]
        fn request_body_cap() {
            super::run(Some(concat!($engine, ".request-body-cap")));
        }
        #[test]
        fn request_budget_under() {
            super::run(Some(concat!($engine, ".request-budget-under")));
        }
        #[test]
        fn scenario_byte_cap() {
            super::run(Some(concat!($engine, ".scenario-byte-cap")));
        }
        #[test]
        fn invalid_0() {
            super::run(Some(concat!($engine, ".invalid-0")));
        }
        #[test]
        fn invalid_1() {
            super::run(Some(concat!($engine, ".invalid-1")));
        }
        #[test]
        fn invalid_2() {
            super::run(Some(concat!($engine, ".invalid-2")));
        }
        #[test]
        fn invalid_3() {
            super::run(Some(concat!($engine, ".invalid-3")));
        }
        #[test]
        fn invalid_4() {
            super::run(Some(concat!($engine, ".invalid-4")));
        }
        #[test]
        fn invalid_5() {
            super::run(Some(concat!($engine, ".invalid-5")));
        }
        #[test]
        fn invalid_6() {
            super::run(Some(concat!($engine, ".invalid-6")));
        }
        #[test]
        fn invalid_7() {
            super::run(Some(concat!($engine, ".invalid-7")));
        }
        #[test]
        fn invalid_8() {
            super::run(Some(concat!($engine, ".invalid-8")));
        }
        #[test]
        fn invalid_9() {
            super::run(Some(concat!($engine, ".invalid-9")));
        }
        #[test]
        fn invalid_10() {
            super::run(Some(concat!($engine, ".invalid-10")));
        }
        #[test]
        fn invalid_11() {
            super::run(Some(concat!($engine, ".invalid-11")));
        }
        #[test]
        fn invalid_12() {
            super::run(Some(concat!($engine, ".invalid-12")));
        }
        #[test]
        fn invalid_13() {
            super::run(Some(concat!($engine, ".invalid-13")));
        }
        #[test]
        fn invalid_14() {
            super::run(Some(concat!($engine, ".invalid-14")));
        }
        #[test]
        fn invalid_15() {
            super::run(Some(concat!($engine, ".invalid-15")));
        }
        #[test]
        fn invalid_16() {
            super::run(Some(concat!($engine, ".invalid-16")));
        }
        #[test]
        fn invalid_17() {
            super::run(Some(concat!($engine, ".invalid-17")));
        }
        #[test]
        fn invalid_18() {
            super::run(Some(concat!($engine, ".invalid-18")));
        }
        #[test]
        fn invalid_19() {
            super::run(Some(concat!($engine, ".invalid-19")));
        }
        #[test]
        fn invalid_20() {
            super::run(Some(concat!($engine, ".invalid-20")));
        }
        #[test]
        fn invalid_21() {
            super::run(Some(concat!($engine, ".invalid-21")));
        }
        #[test]
        fn invalid_22() {
            super::run(Some(concat!($engine, ".invalid-22")));
        }
        #[test]
        fn invalid_23() {
            super::run(Some(concat!($engine, ".invalid-23")));
        }
        #[test]
        fn invalid_24() {
            super::run(Some(concat!($engine, ".invalid-24")));
        }
        #[test]
        fn invalid_25() {
            super::run(Some(concat!($engine, ".invalid-25")));
        }
        #[test]
        fn invalid_26() {
            super::run(Some(concat!($engine, ".invalid-26")));
        }
        #[test]
        fn invalid_27() {
            super::run(Some(concat!($engine, ".invalid-27")));
        }
        #[test]
        fn invalid_28() {
            super::run(Some(concat!($engine, ".invalid-28")));
        }
        #[test]
        fn denied() {
            super::run(Some(concat!($engine, ".denied")));
        }
        #[test]
        fn missing_net() {
            super::run(Some(concat!($engine, ".missing-net")));
        }
        #[test]
        fn missing_tasks() {
            super::run(Some(concat!($engine, ".missing-tasks")));
        }
        #[test]
        fn missing_monotonic_clock() {
            super::run(Some(concat!($engine, ".missing-monotonic_clock")));
        }
        #[test]
        fn missing_stdout() {
            super::run(Some(concat!($engine, ".missing-stdout")));
        }
        #[test]
        fn outside_grant() {
            super::run(Some(concat!($engine, ".outside-grant")));
        }
        #[test]
        fn record_pass() {
            super::run(Some(concat!($engine, ".record-pass")));
        }
        #[test]
        fn record_assertion() {
            super::run(Some(concat!($engine, ".record-assertion")));
        }
        #[test]
        fn record_transport() {
            super::run(Some(concat!($engine, ".record-transport")));
        }
    };
}

mod selected_h {
    engine_cases!("h");
}
mod selected_llvm {
    engine_cases!("llvm");
}
mod selected_cranelift {
    engine_cases!("cranelift");
}

#[test]
fn exact_selection_and_nonexistent_method() {
    // CPython test_loader.py:518-553: exact method selects one body;
    // nonexistent method refuses instead of running a containing suite.
    let helper = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(
        helper.path(),
        include_bytes!("../support/httpcheck_15/verify.py"),
    )
    .unwrap();
    let output = Command::new("python3")
        .arg("-c")
        .arg(
            r#"
import runpy, sys
v = runpy.run_path(sys.argv[1])
assert len(v['SCENARIOS']) == 57
assert len(v['SELECTIONS']) == 172
for engine in v['ENGINES']:
    for label in v['SCENARIOS']:
        assert v['resolve_case'](engine + '.' + label) == (engine, label)
for name in ('h.ordered-methodsfoo', 'h.ordered', 'h', 'llvm.record', 'wat.body-cap', ''):
    try:
        v['resolve_case'](name)
    except ValueError as error:
        assert name in str(error)
    else:
        raise AssertionError('nonexistent method accepted: ' + name)
# The public CLI must reject before touching the package, server or output.
import pathlib, subprocess, tempfile
with tempfile.TemporaryDirectory() as scratch:
    output = pathlib.Path(scratch) / 'must-not-exist'
    done = subprocess.run([sys.executable, sys.argv[1], '--hirsel', '/nonexistent',
                           '--root', '/nonexistent', '--output', str(output),
                           '--case', 'h.ordered-methodsfoo'], capture_output=True)
    assert done.returncode == 2, done
    assert b'invalid choice' in done.stderr, done
    assert not output.exists()
print('exact selections=171 invalid-name refusals=6 cli-refusals=1')
"#,
        )
        .arg(helper.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    println!("{}", String::from_utf8_lossy(&output.stdout));
}

fn run(case: Option<&str>) {
    // Retain failures, including setup/timeout cells, outside the source tree.
    let scratch = tempfile::Builder::new()
        .prefix("hirsel-httpcheck-15-")
        .tempdir()
        .unwrap()
        .keep();
    let package = scratch.join("package");
    std::fs::create_dir(&package).unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../../examples/httpcheck/hirsel.json")).unwrap();
    manifest["dependencies"][0]["manifest"] = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .as_ref()
        .into();
    std::fs::write(package.join("hirsel.json"), manifest.to_string()).unwrap();
    std::fs::write(
        package.join("main.hir"),
        include_bytes!("../../../../examples/httpcheck/main.hir"),
    )
    .unwrap();
    let helper = scratch.join("verify.py");
    std::fs::write(&helper, include_bytes!("../support/httpcheck_15/verify.py")).unwrap();
    let mut command = Command::new("python3");
    command
        .arg(&helper)
        .arg("--hirsel")
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .arg("--root")
        .arg(&package)
        .arg("--output")
        .arg(scratch.join("evidence"));
    if let Some(case) = case {
        command.arg("--case").arg(case);
    }
    let output = command.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "evidence: {}\n{stdout}\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    if case == Some("h-body-cap") {
        assert!(
            stdout.contains("case=h-body-cap engines={'h': 1} observations=1"),
            "{stdout}"
        );
        println!("{stdout}evidence: {}", scratch.display());
        return;
    }
    if let Some(case) = case {
        let engine = case.split('.').next().unwrap();
        let expected = if case.contains(".record-") {
            "observations=1 records=1 replays=3"
        } else {
            "observations=1 records=0 replays=0"
        };
        assert!(
            stdout.contains(&format!("case={case} engines={{'{engine}': 1}}")),
            "{stdout}"
        );
        assert!(stdout.contains(expected), "{stdout}");
        let result: serde_json::Value =
            serde_json::from_slice(&std::fs::read(scratch.join("evidence/results.json")).unwrap())
                .unwrap();
        for candidate in ["h", "llvm", "cranelift"] {
            assert_eq!(
                result["executed"][candidate],
                u64::from(candidate == engine)
            );
        }
    } else {
        assert!(
            stdout.contains("engines={'h': 1, 'llvm': 1, 'cranelift': 1}"),
            "{stdout}"
        );
        assert!(stdout.contains("records=9 replays=27"), "{stdout}");
    }
    println!("{stdout}evidence: {}", scratch.display());
}
