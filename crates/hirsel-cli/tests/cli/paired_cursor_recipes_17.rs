//! Checked publication of the retained physical-match recipes, without changing rules.
use std::{fs, path::Path, process::Command};

const GUIDE: &str = include_str!("../../../../docs/borrow-cursor-recipes.md");
const SOURCE: &str = include_str!("../../../../examples/borrow-cursor-recipes/main.hir");
const MANIFEST: &str = include_str!("../../../../examples/borrow-cursor-recipes/hirsel.json");

fn cli() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
    command.env("HIRSEL_DAEMON", "off");
    command
}

fn scratch() -> std::path::PathBuf {
    let root = tempfile::Builder::new()
        .prefix("paired-cursor-recipes-17-")
        .tempdir()
        .unwrap()
        .keep();
    eprintln!("paired recipe evidence: {}", root.display());
    root
}

fn package(root: &Path, source: &str, entry: bool) -> std::path::PathBuf {
    fs::create_dir_all(root).unwrap();
    fs::write(root.join("main.hir"), source).unwrap();
    let mut manifest: serde_json::Value = serde_json::from_str(MANIFEST).unwrap();
    if !entry {
        manifest.as_object_mut().unwrap().remove("entry");
    }
    let path = root.join("hirsel.json");
    fs::write(&path, manifest.to_string()).unwrap();
    path
}

fn check(root: &Path, source: &str, rule: Option<&str>, operation: Option<&str>) {
    let manifest = package(root, source, false);
    let output = cli()
        .args(["check", "--json"])
        .arg(manifest)
        .output()
        .unwrap();
    fs::write(root.join("check.stdout"), &output.stdout).unwrap();
    fs::write(root.join("check.stderr"), &output.stderr).unwrap();
    if let Some(rule) = rule {
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let diagnostics = serde_json::Deserializer::from_slice(&output.stdout)
            .into_iter::<serde_json::Value>()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let diagnostic = &diagnostics[0];
        assert_eq!(diagnostic["rule"], rule, "{diagnostic}");
        let start = diagnostic["primary"]["start"].as_u64().unwrap() as usize;
        let end = diagnostic["primary"]["end"].as_u64().unwrap() as usize;
        assert_eq!(&source[start..end], operation.unwrap(), "{diagnostic}");
    } else {
        assert!(output.status.success(), "{output:?}");
        assert!(
            output.stdout.is_empty() && output.stderr.is_empty(),
            "{output:?}"
        );
    }
}

#[test]
fn paired_cursor_recipes_17_snippets_and_twins() {
    let root = scratch();
    let refusals = [
        ("MODE-1", "c = xs[0u64]"),
        ("LANG-7", "inner"),
        ("LANG-15", "c = unboxed(inner)"),
        ("MODE-1", "a = [9u64]"),
        ("MODE-1", "c"),
    ];
    let mut accepted = 0;
    let mut rejected = 0;
    for (index, block) in GUIDE.split("```hir paired-cursor\n").skip(1).enumerate() {
        let source = block.split_once("```").unwrap().0;
        if source.starts_with("// expect: accept\n") {
            check(&root.join(format!("snippet-{index}")), source, None, None);
            accepted += 1;
        } else {
            let (rule, operation) = refusals[rejected];
            assert!(source.starts_with(&format!("// expect: reject {rule}\n")));
            check(
                &root.join(format!("snippet-{index}")),
                source,
                Some(rule),
                Some(operation),
            );
            rejected += 1;
        }
    }
    assert_eq!((accepted, rejected), (3, 5));
    for name in ["same", "tag_depth", "read"] {
        let function = SOURCE
            .split_once(&format!("// recipe: {name}\n"))
            .unwrap()
            .1
            .split_once("// endrecipe")
            .unwrap()
            .0;
        assert!(
            GUIDE.contains(function),
            "published function differs: {name}"
        );
    }
    let cases = [
        (
            "computed-pair",
            include_str!(
                "../../../../conformance/LANG/LANG-15-paired-cursor-recipes-17-computed-pair-bad.hir"
            ),
            refusals[0],
        ),
        (
            "box-type",
            include_str!(
                "../../../../conformance/LANG/LANG-15-paired-cursor-recipes-17-box-type-bad.hir"
            ),
            refusals[1],
        ),
        (
            "owned-refill",
            include_str!(
                "../../../../conformance/LANG/LANG-15-paired-cursor-recipes-17-owned-refill-bad.hir"
            ),
            refusals[2],
        ),
        (
            "stale-root",
            include_str!(
                "../../../../conformance/LANG/LANG-15-paired-cursor-recipes-17-stale-root-bad.hir"
            ),
            refusals[3],
        ),
        (
            "escape",
            include_str!(
                "../../../../conformance/LANG/LANG-15-paired-cursor-recipes-17-escape-bad.hir"
            ),
            refusals[4],
        ),
    ];
    for (name, source, (rule, operation)) in cases {
        check(&root.join(name), source, Some(rule), Some(operation));
    }
    let run = include_str!("../../../../conformance/LANG/LANG-15-paired-cursor-recipes-17-run.hir");
    assert_eq!(
        run.split_once("// Adapted").unwrap().1,
        SOURCE.split_once("// Adapted").unwrap().1
    );
    check(&root.join("run"), run, None, None);
    eprintln!("paired recipes: 8 guide blocks + 6 corpus cases checked");
}

#[test]
fn paired_cursor_recipes_17_bounded_matrix_engines() {
    let root = scratch();
    let manifest = package(&root, SOURCE, true);
    // One package test executes both the original controls and the paired matrix.
    let tested = cli()
        .args(["test", "--seed", "1"])
        .arg(&manifest)
        .output()
        .unwrap();
    fs::write(root.join("test.stdout"), &tested.stdout).unwrap();
    fs::write(root.join("test.stderr"), &tested.stderr).unwrap();
    assert!(tested.status.success(), "{tested:?}");
    assert!(
        String::from_utf8_lossy(&tested.stdout).contains("1 passed; 0 failed; 1 total"),
        "{tested:?}"
    );
    let interpreted = cli().arg("run").arg(&manifest).output().unwrap();
    assert_eq!(interpreted.status.code(), Some(0), "{interpreted:?}");
    assert!(interpreted.stdout.is_empty() && interpreted.stderr.is_empty());
    hirsel_engine::with_compiler_stack(|| {
        use hirsel_driver::native::*;
        let mut engine = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            hirsel_driver::compiler_adapters(),
        );
        let file = engine
            .sources
            .add("paired-recipes.hir", SOURCE.into())
            .unwrap();
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
    for backend in ["llvm", "cranelift"] {
        let exe = root.join(format!("paired-{backend}"));
        let built = cli()
            .args(["build", "--backend", backend])
            .arg(&manifest)
            .arg("-o")
            .arg(&exe)
            .output()
            .unwrap();
        fs::write(root.join(format!("{backend}-build.stdout")), &built.stdout).unwrap();
        fs::write(root.join(format!("{backend}-build.stderr")), &built.stderr).unwrap();
        assert!(built.status.success(), "{backend}: {built:?}");
        let bytes = fs::read(&exe).unwrap();
        for _ in 0..2 {
            let output = Command::new(&exe).output().unwrap();
            assert_eq!(output.status.code(), Some(0), "{backend}: {output:?}");
            assert!(
                output.stdout.is_empty() && output.stderr.is_empty(),
                "{backend}: {output:?}"
            );
        }
        assert_eq!(bytes, fs::read(exe).unwrap());
    }
    eprintln!("paired matrix: H test + H entry + checked heap + 2 LLVM + 2 Cranelift executions");
}
