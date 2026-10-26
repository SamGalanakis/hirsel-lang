//! The candidate seam is private production source, exercised with test-only
//! selectors alongside the unmodified ordered VM on all execution engines.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

#[test]
fn regex_prefilter_17_forced_candidates_preserve_vm_results_and_work_limits() {
    with_compiler_stack(|| {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let manifest_path = root.join("stdlib/std@1/hirsel.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        let sources = manifest["sources"].as_array_mut().unwrap();
        for source in sources.iter_mut() {
            source["path"] = manifest_path
                .parent()
                .unwrap()
                .join(source["path"].as_str().unwrap())
                .into_os_string()
                .into_string()
                .unwrap()
                .into();
        }
        sources.push(serde_json::json!({
            "path": root.join("crates/hirsel-driver/tests/fixtures/regex-prefilter-17/hooks.hir"),
            "module": ["regex"]
        }));
        let std_manifest = directory.path().join("std.json");
        std::fs::write(&std_manifest, manifest.to_string()).unwrap();
        let vectors = std::fs::read_to_string(
            root.join("crates/hirsel-driver/tests/fixtures/regex-prefilter-17/vectors.hir"),
        )
        .unwrap();
        let (helpers, body) = vectors.split_once("pub fn main()").unwrap();
        let calls: Vec<_> = body
            .lines()
            .filter(|line| {
                let line = line.trim_start();
                line.starts_with("check(") || line.starts_with("failure.check(")
            })
            .collect();
        assert_eq!(calls.len(), 79);
        let groups: Vec<_> = calls
            .chunks(8)
            .map(|chunk| {
                format!(
                    "{helpers}pub fn main() -> Result[(), ()] {{\n{}\n  Ok(())\n}}\n",
                    chunk.join("\n")
                )
            })
            .collect();
        let program = serde_json::json!({
            "schema": 1, "package": {"name": "regex_prefilter_tests", "version": "1"},
            "entry": {"module": [], "name": "main"},
            "sources": [{"path": "main.hir", "module": []}],
            "dependencies": [{"manifest": std_manifest, "package": {"name": "std", "version": "1"}, "expose": []}]
        });
        let path = directory.path().join("hirsel.json");
        std::fs::write(&path, program.to_string()).unwrap();
        for (index, source) in groups.iter().enumerate() {
            eprintln!("regex prefilter group {index}: interpreter, LLVM, Cranelift");
            std::fs::write(directory.path().join("main.hir"), source).unwrap();
            let outcome = run_path_with_step_budget(
                &path,
                SimulationConfig::default(),
                Some(100_000_000),
                Verify::On,
                &mut Vec::new(),
                &mut Vec::new(),
            )
            .unwrap();
            assert!(
                matches!(
                    outcome,
                    RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
                ),
                "{outcome:?}"
            );
            for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
                let outcome = native::run_path_native_backend(
                    &path,
                    HostLaunch::empty(),
                    None,
                    backend,
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
                    "{backend:?}: {outcome:?}"
                );
            }
        }
    });
}

#[test]
fn regex_prefilter_17_admitted_unicode_folds_after_ascii_skip() {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/regex-prefilter-17/folds.json");
        let outcome = run_path_with_step_budget(
            &path,
            SimulationConfig::default(),
            Some(1_000_000),
            Verify::On,
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();
        assert!(
            matches!(
                outcome,
                RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
            ),
            "{outcome:?}"
        );
        for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
            let outcome = native::run_path_native_backend(
                &path,
                HostLaunch::empty(),
                None,
                backend,
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
                "{backend:?}: {outcome:?}"
            );
        }
    });
}

fn byte_candidate(entry: &str) {
    with_compiler_stack(|| {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/regex-prefilter-17")
            .canonicalize()
            .unwrap();
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("folds.json")).unwrap()).unwrap();
        for source in manifest["sources"].as_array_mut().unwrap() {
            source["path"] = root
                .join(source["path"].as_str().unwrap())
                .into_os_string()
                .into_string()
                .unwrap()
                .into();
        }
        for dependency in manifest["dependencies"].as_array_mut().unwrap() {
            dependency["manifest"] = root
                .join(dependency["manifest"].as_str().unwrap())
                .into_os_string()
                .into_string()
                .unwrap()
                .into();
        }
        let scratch = tempfile::tempdir().unwrap();
        let path = scratch.path().join("hirsel.json");
        {
            eprintln!("{entry}: interpreter, LLVM, Cranelift");
            manifest["entry"]["name"] = entry.into();
            std::fs::write(&path, manifest.to_string()).unwrap();
            let outcome = run_path_with_step_budget(
                &path,
                SimulationConfig::default(),
                Some(1_000_000),
                Verify::On,
                &mut Vec::new(),
                &mut Vec::new(),
            )
            .unwrap();
            assert!(
                matches!(
                    outcome,
                    RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
                ),
                "{entry}: {outcome:?}"
            );
            for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
                let outcome = native::run_path_native_backend(
                    &path,
                    HostLaunch::empty(),
                    None,
                    backend,
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
                    "{entry} {backend:?}: {outcome:?}"
                );
            }
        }
    });
}

macro_rules! byte_candidates {
    ($($name:ident => $entry:literal),+ $(,)?) => {$(
        #[test]
        fn $name() { byte_candidate($entry); }
    )+};
}
byte_candidates! {
    byte_candidate_alternatives => "candidate_alternatives",
    byte_candidate_end => "candidate_end",
    byte_candidate_partial => "candidate_partial",
    byte_candidate_boundary => "candidate_boundary",
    byte_candidate_single => "candidate_single",
}
