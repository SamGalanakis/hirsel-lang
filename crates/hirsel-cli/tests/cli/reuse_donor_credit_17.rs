use std::{fs, process::Command};

const SOURCE: &str = include_str!("../../../../conformance/MEM/MEM-14-reuse-donor-credit-run.hir");

#[test]
fn reuse_donor_credit_17_engines() {
    hirsel_engine::with_compiler_stack(|| {
        use hirsel_driver::native::*;
        let mut engine = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            hirsel_driver::compiler_adapters(),
        );
        let file = engine.sources.add("donor.hir", SOURCE.into()).unwrap();
        let context = hirsel_engine::default_context();
        let checked = hirsel_engine::check_progress(&mut engine, file, &context).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let ready = build(
            &validated,
            &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let heap = run_heap(
            &ready,
            &hirsel_engine::entry_path(&context),
            &Sites::new(validated.snapshot()),
        )
        .unwrap();
        assert_eq!(heap.observation.exit_code, Some(0));
        assert!(heap.observation.output.is_empty() && heap.observation.failures.is_empty());
    });
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("main.hir"), SOURCE).unwrap();
    let manifest = root.path().join("hirsel.json");
    fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"donor","version":"0"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .env("HIRSEL_DAEMON", "off")
        .arg("run")
        .arg(&manifest)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        output.stdout.is_empty() && output.stderr.is_empty(),
        "{output:?}"
    );
    for backend in ["llvm", "cranelift"] {
        let exe = root.path().join(backend);
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .env("HIRSEL_DAEMON", "off")
            .args(["build", "--backend", backend])
            .arg(&manifest)
            .arg("-o")
            .arg(&exe)
            .output()
            .unwrap();
        assert!(output.status.success(), "{backend}: {output:?}");
        let output = Command::new(exe).output().unwrap();
        assert_eq!(output.status.code(), Some(0), "{backend}: {output:?}");
        assert!(
            output.stdout.is_empty() && output.stderr.is_empty(),
            "{backend}: {output:?}"
        );
    }
}

// Keep the two full original witnesses as well as the minimal four-engine case:
// moving their constructed inputs out of the conditions used to hide the bug.
#[test]
fn original_paired_programs_keep_owned_credit() {
    for source in [
        include_str!("../support/reuse_donor_credit_17/original-long-chain.hir"),
        include_str!("../support/reuse_donor_credit_17/sequential-temporaries.hir"),
    ] {
        hirsel_engine::with_compiler_stack(|| {
            use hirsel_driver::native::*;
            let mut engine = hirsel_engine::QueryEngine::new(
                hirsel_engine::default_environment(),
                hirsel_driver::compiler_adapters(),
            );
            let file = engine.sources.add("paired.hir", source.into()).unwrap();
            let context = hirsel_engine::default_context();
            let checked = hirsel_engine::check_progress(&mut engine, file, &context).unwrap();
            assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
            let input = engine.export_native_input().unwrap();
            let validated = engine.import_native_input(input).unwrap();
            let ready = build(
                &validated,
                &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
            )
            .unwrap();
            let heap = run_heap(
                &ready,
                &hirsel_engine::entry_path(&context),
                &Sites::new(validated.snapshot()),
            )
            .unwrap();
            assert_eq!(heap.observation.exit_code, Some(0));
            assert!(heap.observation.output.is_empty() && heap.observation.failures.is_empty());
        });
    }
}
