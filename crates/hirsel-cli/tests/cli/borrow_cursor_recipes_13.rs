//! Executable controls for docs/borrow-cursor-recipes.md. Local Hirsel witnesses,
//! not a port of an external algorithm; preserve the diagnostic's real span.
use std::{fs, path::Path, process::Command};

const GUIDE: &str = include_str!("../../../../docs/borrow-cursor-recipes.md");
const SOURCE: &str = include_str!("../../../../examples/borrow-cursor-recipes/main.hir");
const MANIFEST: &str = include_str!("../../../../examples/borrow-cursor-recipes/hirsel.json");

fn scratch() -> std::path::PathBuf {
    // Retain failures and native artifacts outside the source tree.
    let path = tempfile::Builder::new()
        .prefix("borrow-cursor-recipes-13-")
        .tempdir()
        .unwrap()
        .keep();
    eprintln!("cursor recipe scratch: {}", path.display());
    path
}

fn cli() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
    command.env("HIRSEL_DAEMON", "off");
    command
}

fn package(root: &Path, source: &str) -> std::path::PathBuf {
    fs::create_dir_all(root).unwrap();
    fs::write(root.join("main.hir"), source).unwrap();
    let manifest = root.join("hirsel.json");
    fs::write(&manifest, MANIFEST).unwrap();
    manifest
}

fn checked(root: &Path, source: &str) -> Option<serde_json::Value> {
    let manifest = package(root, source);
    let mut library: serde_json::Value = serde_json::from_str(MANIFEST).unwrap();
    library.as_object_mut().unwrap().remove("entry");
    fs::write(&manifest, library.to_string()).unwrap();
    let output = cli()
        .args(["check", "--json"])
        .arg(manifest)
        .output()
        .unwrap();
    fs::write(root.join("check.stdout"), &output.stdout).unwrap();
    fs::write(root.join("check.stderr"), &output.stderr).unwrap();
    if output.status.success() {
        assert!(output.stdout.is_empty(), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        None
    } else {
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        // `check --json` emits a JSON-lines stream. Some shipped twins have
        // two erroneous assignments; the contract names the first primary span.
        let diagnostics = serde_json::Deserializer::from_slice(&output.stdout)
            .into_iter::<serde_json::Value>()
            .collect::<Result<Vec<_>, _>>()
            .expect("diagnostic JSON-lines stream");
        diagnostics
            .into_iter()
            .next()
            .or_else(|| panic!("no rejection diagnostic: {output:?}"))
    }
}

#[test]
fn borrow_cursor_recipes_13_guide_snippets() {
    // Observed primary byte spans, in reject order. Changing a diagnostic span
    // requires reviewing the actual source operation, not blessing rendered text.
    let spans = [
        (80, 93, 4, 3),
        (96, 105, 5, 3),
        (410, 428, 14, 3),
        (96, 104, 4, 3),
        (79, 90, 4, 3),
        (135, 153, 6, 3),
        (154, 173, 6, 3),
        (126, 134, 5, 3),
        (245, 260, 9, 32),
        (100, 108, 4, 3),
        (109, 117, 6, 3),
    ];
    let root = scratch();
    let mut accepted = 0;
    let mut rejected = 0;
    for (index, block) in GUIDE.split("```hir\n").skip(1).enumerate() {
        let source = block.split_once("```").unwrap().0;
        let diagnostic = checked(&root.join(index.to_string()), source);
        let expectation = source.lines().next().unwrap();
        if expectation == "// expect: accept" {
            assert!(diagnostic.is_none(), "block {index}: {diagnostic:?}");
            accepted += 1;
        } else {
            let rule = expectation.strip_prefix("// expect: reject ").unwrap();
            let diagnostic = diagnostic.expect("rejected guide block was accepted");
            assert_eq!(diagnostic["rule"], rule, "block {index}: {diagnostic}");
            let primary = &diagnostic["primary"];
            let observed = (
                primary["start"].as_u64().unwrap(),
                primary["end"].as_u64().unwrap(),
                primary["line"].as_u64().unwrap(),
                primary["column"].as_u64().unwrap(),
            );
            assert_eq!(observed, spans[rejected], "block {index}: {diagnostic}");
            rejected += 1;
        }
    }
    assert_eq!((accepted, rejected), (6, 11));
    // Accepted documentation functions are also the functions exercised at run time.
    for name in [
        "borrowed_tree",
        "owned_tree",
        "array_result",
        "sibling_reset",
        "saved_snapshot",
        "transaction",
    ] {
        let function = SOURCE
            .split_once(&format!("// recipe: {name}\n"))
            .unwrap()
            .1
            .split_once("// endrecipe")
            .unwrap()
            .0;
        assert!(
            GUIDE.contains(function),
            "guide and executable differ: {name}"
        );
    }
}

#[test]
fn borrow_cursor_recipes_13_corpus_twins() {
    macro_rules! case {
        ($path:literal) => {
            (
                $path,
                include_str!(concat!("../../../../conformance/", $path)),
            )
        };
    }
    let cases = [
        case!("LANG/LANG-15-borrow-cursor-tree-ok.hir"),
        case!("LANG/LANG-15-borrow-cursor-tree-bad.hir"),
        case!("LANG/LANG-15-borrow-cursor-json-pointer-ok.hir"),
        case!("LANG/LANG-15-borrow-cursor-json-pointer-bad.hir"),
        case!("LANG/LANG-15-borrow-cursor-reset-ok.hir"),
        case!("LANG/LANG-15-borrow-cursor-reset-bad.hir"),
        case!("LANG/LANG-15-borrow-cursor-project-ok.hir"),
        case!("LANG/LANG-15-borrow-cursor-project-bad.hir"),
        case!("LANG/LANG-15-borrow-cursor-generic-ok.hir"),
        case!("LANG/LANG-15-borrow-cursor-generic-bad.hir"),
        case!("MODE/MODE-1-borrow-cursor-field-ok.hir"),
        case!("MODE/MODE-1-borrow-cursor-field-bad.hir"),
        case!("MODE/MODE-1-borrow-cursor-owned-getter-bad.hir"),
        case!("OWN/OWN-1-borrow-cursor-take-ok.hir"),
        case!("OWN/OWN-1-borrow-cursor-take-bad.hir"),
        case!("OWN/OWN-3-borrow-cursor-snapshot-ok.hir"),
        case!("OWN/OWN-3-borrow-cursor-snapshot-bad.hir"),
    ];
    let root = scratch();
    for (name, source) in cases {
        let diagnostic = checked(&root.join(name.trim_end_matches(".hir")), source);
        let expectation = source
            .lines()
            .find_map(|line| line.strip_prefix("// expect: "))
            .unwrap();
        if expectation == "accept" {
            assert!(diagnostic.is_none(), "{name}: {diagnostic:?}");
        } else {
            let parts: Vec<_> = expectation.split_whitespace().collect();
            let diagnostic = diagnostic.expect("rejected corpus case was accepted");
            assert_eq!(diagnostic["rule"], parts[1], "{name}: {diagnostic}");
            assert_eq!(
                diagnostic["primary"]["line"],
                parts[4].parse::<u64>().unwrap(),
                "{name}: {diagnostic}"
            );
            let start = diagnostic["primary"]["start"].as_u64().unwrap() as usize;
            let end = diagnostic["primary"]["end"].as_u64().unwrap() as usize;
            assert!(
                start < end && source.get(start..end).is_some(),
                "{name}: {diagnostic}"
            );
        }
    }
}

#[test]
fn borrow_cursor_recipes_13_package_engines() {
    let root = scratch();
    let manifest = package(&root, SOURCE);
    for backend in [None, Some("llvm"), Some("cranelift")] {
        let mut command = cli();
        command.args(["test", "--seed", "1"]);
        if let Some(backend) = backend {
            command.args(["--native", "--backend", backend]);
        }
        let output = command.arg(&manifest).output().unwrap();
        let name = backend.unwrap_or("interpreted");
        fs::write(root.join(format!("{name}-test.stdout")), &output.stdout).unwrap();
        fs::write(root.join(format!("{name}-test.stderr")), &output.stderr).unwrap();
        assert!(output.status.success(), "{backend:?}: {output:?}");
        assert!(output.stderr.is_empty(), "{backend:?}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed; 1 total"),
            "{backend:?}: {output:?}"
        );
    }
    let interpreted = cli().arg("run").arg(&manifest).output().unwrap();
    assert!(
        interpreted.status.success()
            && interpreted.stdout.is_empty()
            && interpreted.stderr.is_empty(),
        "{interpreted:?}"
    );
    hirsel_engine::with_compiler_stack(|| {
        use hirsel_driver::native::*;
        let mut engine = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            hirsel_driver::compiler_adapters(),
        );
        let file = engine
            .sources
            .add("cursor-recipes.hir", SOURCE.into())
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
    count_free_transport(&root);
    for backend in ["llvm", "cranelift"] {
        let exe = root.join(format!("recipe-{backend}"));
        let built = cli()
            .args(["build", "--backend", backend])
            .arg(&manifest)
            .arg("-o")
            .arg(&exe)
            .output()
            .unwrap();
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
}

// A bounded use of the existing native RC instrumentation, with transport
// sites kept separate from the explicit owned result. No timing comparison.
fn count_free_transport(root: &Path) {
    hirsel_engine::with_compiler_stack(|| {
        use hirsel_driver::native::*;
        let source = r#"type Tree = Leaf(n: u64) | Branch(left: box Tree, right: box Tree)
fn scan(root: box Tree) -> u64 {
  let mut node = root
  let mut sum = 0u64
  for i in 0u64..4u64 {
    node = root
    while true {
      match node {
        Leaf(n: n) => { sum += n; break },
        Branch(left: child, right: _) => { node = child },
      }
    }
  }
  sum
}
fn owned_result(root: box Tree) -> (box Tree) in root {
  let mut node = root
  node = root
  copy node
}
pub fn main() -> Result[(), ()] {
  let root = box Branch(left: box Leaf(n: 7), right: box Leaf(n: 9))
  let selected = owned_result(root)
  if scan(root) == 28 && scan(selected) == 28 { Ok(()) } else { Error(()) }
}
"#;
        let artifact = root.join("transport");
        fs::create_dir(&artifact).unwrap();
        fs::write(artifact.join("input.hir"), source).unwrap();
        let mut engine = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            hirsel_driver::compiler_adapters(),
        );
        let file = engine.sources.add("transport.hir", source.into()).unwrap();
        let context = hirsel_engine::default_context();
        let checked = hirsel_engine::check_progress(&mut engine, file, &context).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let input = engine.export_native_input().unwrap();
        let profiles = profile_sources(&input);
        let validated = engine.import_native_input(input).unwrap();
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        let mut options = Options::new(target.clone());
        options.elide = false;
        options.reuse = false;
        options.regions = false;
        let ready = build(&validated, &options).unwrap();
        let entry = hirsel_engine::entry_path(&context);
        let heap = run_heap(&ready, &entry, &Sites::new(validated.snapshot())).unwrap();
        assert_eq!(heap.observation.exit_code, Some(0));
        eprintln!(
            "transport H counters (includes construction/result): {:?}",
            heap.counters
        );
        fs::write(
            artifact.join("heap-counts.txt"),
            format!("{:?}", heap.counters),
        )
        .unwrap();
        let tools = Toolchain::in_tree_with_runtime(
            &artifact.join("runtime"),
            &[&target],
            RuntimeBuild {
                measurement: true,
                ..Default::default()
            },
        )
        .unwrap();
        let objects = emit_backend_objects(
            &ready,
            Backend::Llvm,
            OptLevel::Speed,
            Partition::Whole,
            true,
            Some(&profiles),
        )
        .unwrap();
        let exe = link_objects(&ready, &entry, &objects, &tools, &artifact).unwrap();
        let profile = artifact.join("profile.json");
        let runtime_root = artifact.join("root");
        fs::create_dir(&runtime_root).unwrap();
        let output = Command::new(&exe)
            .env_clear()
            .env("HIRSEL_RT_ROOT", runtime_root)
            .env("HIRSEL_RT_PROFILE", &profile)
            .env("HIRSEL_RT_MEASUREMENT", artifact.join("measurement"))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        assert!(
            output.stdout.is_empty() && output.stderr.is_empty(),
            "{output:?}"
        );
        let report: serde_json::Value =
            serde_json::from_slice(&fs::read(profile).unwrap()).unwrap();
        let mut transport = [0u64; 2];
        let mut result = 0;
        for site in report["sites"].as_array().unwrap() {
            let function = site["function"].as_str().unwrap();
            let increments = site["counts"]["count_increments"].as_u64().unwrap_or(0);
            let decrements = site["counts"]["count_decrements"].as_u64().unwrap_or(0);
            if function.ends_with("/scan") {
                transport[0] += increments;
                transport[1] += decrements;
            }
            if function.ends_with("/owned_result") {
                result += increments;
            }
        }
        assert_eq!(transport, [0, 0], "{report}");
        assert!(
            result > 0,
            "explicit owned result did not record sharing: {report}"
        );
        eprintln!(
            "transport native increments/decrements={transport:?}; result increments={result}; totals={}",
            report["totals"]
        );
    });
}
