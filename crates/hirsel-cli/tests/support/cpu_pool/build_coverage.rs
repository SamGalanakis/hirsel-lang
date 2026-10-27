#[test]
fn matmul_and_wide_cleanup_cli_builds_validate_continuation_coverage() {
    let _compiler = QUAL_COMPILER.lock().unwrap_or_else(|e| e.into_inner());
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    let mut sources = vec![serde_json::json!({"path":"main.hir","module":[]})];
    for (name, source) in [
        (
            "matmul_parallel",
            include_str!("../../../../../bench/langarena/matmul_parallel.hir"),
        ),
        (
            "harness",
            include_str!("../../../../../bench/langarena/harness.hir"),
        ),
        (
            "cpu_harness",
            include_str!("../../../../../bench/langarena/cpu_harness.hir"),
        ),
    ] {
        fs::write(root.join(format!("{name}.hir")), source).unwrap();
        sources.push(serde_json::json!({"path":format!("{name}.hir"),"module":[name]}));
    }
    let std = std::path::Path::new("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap();
    let manifest = root.join("hirsel.json");
    fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"cpu_build_coverage","version":"1"},
        "sources":sources,"dependencies":[{"manifest":std,"package":{"name":"std","version":"1"},"expose":[]}],
        "entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    let matmul = include_str!("cells.hir")
        .replace("WIDTH", "4u32")
        .replace("SIZES", "[1u64,4u64]");
    let wide = wide_cleanup_source(4, "success");
    for (name, source) in [("matmul", matmul), ("wide-cleanup", wide)] {
        fs::write(root.join("main.hir"), source).unwrap();
        for backend in ["llvm", "cranelift"] {
            // The production CLI always plans ahead and continuation execution.
            // Stage validation must accept exactly the bodies those plans cover.
            let executable = qual_build(root, &manifest, backend);
            assert!(executable.is_file(), "{name} {backend}");
            eprintln!("continuation coverage: {name} {backend} CLI build passed");
        }
    }
}
