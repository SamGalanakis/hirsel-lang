//! Fail compiler worker creation without changing host resource limits.
#[cfg(target_os = "linux")]
#[test]
fn compiler_worker_spawn_failure_falls_back_with_one_operational_warning() {
    use std::{path::PathBuf, process::Command};
    let source = super::file(
        &((0..16)
            .map(|i| format!("fn f{i}(x: u64) -> u64 {{ x + {i}u64 }}\n"))
            .collect::<String>()
            + "pub fn main() -> Result[(), ()] { Ok(()) }\n"),
    );
    let injection = source.with_extension("c");
    let library = source.with_extension("so");
    std::fs::write(&injection, include_str!("compiler_workers.c")).unwrap();
    let clang =
        PathBuf::from(std::env::var_os("LLVM_SYS_211_PREFIX").unwrap()).join("bin/hirsel-clang");
    let built = Command::new(clang)
        .args(["-shared", "-fPIC", "-nostdlib", "-fuse-ld=lld"])
        .arg(&injection)
        .arg("-o")
        .arg(&library)
        .output()
        .unwrap();
    assert!(built.status.success(), "{built:?}");
    let run = |jobs: &str, fail_after: Option<&str>, source: &std::path::Path| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command
            .env("HIRSEL_DAEMON", "off")
            .env("HIRSEL_RESULTS", "none@1")
            .env("HIRSEL_JOBS", jobs)
            .env_remove("LD_PRELOAD")
            .env_remove("HIRSEL_TEST_SPAWN_FAIL_AFTER")
            .args(["check", "--json"])
            .arg(source);
        if let Some(after) = fail_after {
            command
                .env("LD_PRELOAD", &library)
                .env("HIRSEL_TEST_SPAWN_FAIL_AFTER", after);
        }
        command.output().unwrap()
    };
    let normal = run("4", None, &source);
    assert!(normal.status.success(), "{normal:?}");
    for after in ["0", "1"] {
        let fallback = run("4", Some(after), &source);
        assert!(fallback.status.success(), "{fallback:?}");
        assert_eq!(fallback.stdout, normal.stdout);
        let stderr = String::from_utf8(fallback.stderr).unwrap();
        assert_eq!(
            stderr.matches("compiler worker creation failed").count(),
            1,
            "{stderr}"
        );
        assert!(stderr.contains("HIRSEL_JOBS=1"), "{stderr}");
        assert!(stderr.contains("os error 11"), "{stderr}");
        assert!(!stderr.contains("panicked"), "{stderr}");
    }
    let one = run("1", Some("0"), &source);
    assert!(one.status.success(), "{one:?}");
    assert_eq!(one.stdout, normal.stdout);
    assert!(
        !String::from_utf8(one.stderr)
            .unwrap()
            .contains("compiler worker creation failed")
    );

    // Source-language errors keep their JSON diagnostics and exit status even
    // when a compiler frontier also encounters an operational failure.
    let invalid = super::file(
        &std::fs::read_to_string(&source)
            .unwrap()
            .replace("Ok(())", "true"),
    );
    let normal_error = run("4", None, &invalid);
    let fallback_error = run("4", Some("0"), &invalid);
    assert_eq!(normal_error.status.code(), Some(1), "{normal_error:?}");
    assert_eq!(fallback_error.status.code(), Some(1), "{fallback_error:?}");
    assert_eq!(fallback_error.stdout, normal_error.stdout);
    assert!(
        String::from_utf8(fallback_error.stderr)
            .unwrap()
            .contains("compiler worker creation failed")
    );

    let minimal = super::file("pub fn main() -> Result[(), ()] { Ok(()) }\n");
    for jobs in ["1", "4"] {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .env("HIRSEL_DAEMON", "off")
            .env("HIRSEL_RESULTS", "none@1")
            // Pool actions have no user cache directory; keep this control
            // independent of the installed CLI's default object store.
            .env("HIRSEL_OBJECTS", "none@1")
            .env("HIRSEL_JOBS", jobs)
            .env_remove("LD_PRELOAD")
            .args(["run", "--native"])
            .arg(&minimal)
            .output()
            .unwrap();
        assert!(output.status.success(), "jobs={jobs}: {output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
    }
    for path in [source, invalid, minimal, injection, library] {
        std::fs::remove_file(path).unwrap();
    }
}
