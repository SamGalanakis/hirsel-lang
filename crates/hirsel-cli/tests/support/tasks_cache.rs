//! Compiler-only private storage for the Tasks native integration fixtures.
use std::process::Command;

pub fn compiler(scratch: &tempfile::TempDir) -> Command {
    let cache = scratch.path().join("cache");
    std::fs::create_dir_all(&cache).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
    command
        .env_remove("HOME")
        .env_remove("XDG_CACHE_HOME")
        .env("XDG_CACHE_HOME", cache)
        .env("HIRSEL_OBJECTS", "dir@1");
    command
}

#[test]
fn private_cache_emits_and_reopens_without_home() {
    let scratch = tempfile::Builder::new()
        .prefix("hirsel-tasks-cache-control-")
        .tempdir()
        .unwrap();
    let source = scratch.path().join("main.hir");
    std::fs::write(&source, "pub fn main()->Result[(),()] { Ok(()) }\n").unwrap();
    let manifest = scratch.path().join("hirsel.json");
    std::fs::write(
        &manifest,
        serde_json::json!({
            "schema":1,"package":{"name":"tasks_cache_control","version":"1"},
            "entry":{"module":[],"name":"main"},
            "sources":[{"path":"main.hir","module":[]}],"dependencies":[]
        })
        .to_string(),
    )
    .unwrap();
    let exe = scratch.path().join("control");
    let build = || {
        let mut command = compiler(&scratch);
        command
            .args(["build", "--backend", "llvm", "--timings=json"])
            .arg(&manifest)
            .arg("-o")
            .arg(&exe);
        command
    };
    let refused = build().env_remove("XDG_CACHE_HOME").output().unwrap();
    assert!(!refused.status.success(), "{refused:?}");
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("dir@1: no cache directory"),
        "{refused:?}"
    );
    assert!(!exe.exists(), "cache refusal must precede emission");
    eprintln!(
        "stripped dir@1 refusal: {}",
        String::from_utf8_lossy(&refused.stderr)
    );
    let mut bytes = None;
    for warm in [false, true] {
        let output = build().output().unwrap();
        assert!(output.status.success(), "{output:?}");
        let stderr = std::str::from_utf8(&output.stderr).unwrap();
        let report: serde_json::Value =
            serde_json::from_str(stderr.lines().last().unwrap()).unwrap();
        let counts = &report["counts"];
        if warm {
            assert!(counts["object hits"].as_u64().unwrap() > 0, "{report}");
            assert_eq!(counts["object misses"], 0, "{report}");
            assert_eq!(std::fs::read(&exe).unwrap(), bytes.take().unwrap());
        } else {
            assert!(counts["object misses"].as_u64().unwrap() > 0, "{report}");
            bytes = Some(std::fs::read(&exe).unwrap());
        }
        let run = Command::new(&exe).env_clear().output().unwrap();
        assert!(run.status.success(), "{run:?}");
        assert!(run.stdout.is_empty() && run.stderr.is_empty(), "{run:?}");
        eprintln!("private cache warm={warm}: emitted and ran; {counts}");
    }
    assert!(scratch.path().join("cache/hirsel").is_dir());
}
