use std::{path::Path, process::Command};

fn check(backend: &str) {
    let directory =
        std::env::temp_dir().join(format!("la-graph-rss-10-{}-{backend}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    for (name, source) in [
        ("verify.py", include_str!("verify.py")),
        ("vectors.json", include_str!("vectors.json")),
        (
            "graph.hir",
            include_str!("../../../../../bench/langarena/graph.hir"),
        ),
        (
            "harness.hir",
            include_str!("../../../../../bench/langarena/harness.hir"),
        ),
    ] {
        std::fs::write(directory.join(name), source).unwrap();
    }
    let stdlib = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap();
    let result = Command::new("python3")
        .arg(directory.join("verify.py"))
        .args(["--cli", env!("CARGO_BIN_EXE_hirsel"), "--stdlib"])
        .arg(stdlib)
        .args(["--backend", backend, "--directory"])
        .arg(&directory)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "fixtures retained at {}\n{}\n{}",
        directory.display(),
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    eprintln!("{}", String::from_utf8_lossy(&result.stdout));
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn h() {
    check("h");
}

#[test]
fn llvm() {
    check("llvm");
}

#[test]
fn cranelift() {
    check("cranelift");
}
