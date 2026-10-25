//! Rejected projected-loop loans cannot reach an execution backend.
use std::process::Command;

#[test]
fn loop_place_52_negative_engine_entrypoints() {
    let dir = tempfile::tempdir().unwrap();
    for (name, rule) in [("write-bad", "OWN-3"), ("copy-bad", "MODE-2")] {
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance/MODE")
            .join(format!("MODE-2-loop-place-52-{name}.hir"));
        let path = dir.path().join(format!("{name}.hir"));
        std::fs::copy(source, &path).unwrap();
        // The checked heap executor uses the same source admission asserted in
        // the driver test. Exercise the other three engine entrypoints here.
        for flags in [
            vec!["run"],
            vec!["build", "--backend", "llvm"],
            vec!["build", "--backend", "cranelift"],
        ] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
            command.env("HIRSEL_DAEMON", "off").args(&flags).arg(&path);
            let executable = dir.path().join("refused");
            if flags[0] == "build" {
                command.arg("-o").arg(&executable);
            }
            let output = command.output().unwrap();
            assert_eq!(
                output.status.code(),
                Some(1),
                "{name} {flags:?}: {output:?}"
            );
            assert!(
                String::from_utf8_lossy(&output.stderr).contains(rule),
                "{name} {flags:?}: {output:?}"
            );
            assert!(!executable.exists());
        }
    }
}
