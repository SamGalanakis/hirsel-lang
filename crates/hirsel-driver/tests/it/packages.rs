use hirsel_driver::*;
use hirsel_engine::*;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn explicit_launch_selects_source_std_without_registry_status() {
    with_compiler_stack(|| {
        let source = root().join("conformance/PROG/PROG-1-source-dependencies-run.hir");
        let manifest = source.with_extension("hir.json");
        let mut p = load_program(&manifest).unwrap();
        p.check_for_execution().unwrap();
        let std = p
            .engine
            .lower_to_tir(&DefPath(hirsel_syntax::ItemPath {
                package: hirsel_syntax::PackageId {
                    name: "std".into(),
                    version: "1".into(),
                },
                modules: vec!["probe".into()],
                name: "identity".into(),
            }))
            .unwrap();
        assert_eq!(std.value.path.path().0.package.name, "std");
        assert!(
            !registry::BuiltinRegistry::new().signatures().any(|s| s
                .definition
                .path()
                .0
                .package
                .name_is("std"))
        );
        assert!(matches!(
            run_path(&manifest, &mut Vec::new()).unwrap(),
            RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
        ));
    });
}

#[test]
fn launch_metadata_errors_are_tool_errors() {
    let dir_scratch = tempfile::Builder::new()
        .prefix("hirsel-packages-")
        .tempdir()
        .unwrap();
    let dir = dir_scratch.path().to_path_buf();
    std::fs::write(dir.join("main.hir"), "pub fn main(t: Test) {}\n").unwrap();
    let base = serde_json::json!({"schema":1,"package":{"name":"app","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}});
    let path = dir.join("hirsel.json");
    for mutate in [0, 1, 2, 3, 4, 5, 6, 7, 8] {
        let mut manifest = base.clone();
        match mutate {
            0 => manifest["schema"] = 3.into(),
            1 => manifest["unknown"] = true.into(),
            2 => manifest["package"]["name"] = "not/a/name".into(),
            3 => manifest["package"] = serde_json::json!({"name":"core","version":"1"}),
            4 => {
                let source = manifest["sources"][0].clone();
                manifest["sources"].as_array_mut().unwrap().push(source);
            }
            5 => {
                manifest["dependencies"] = serde_json::json!([{"manifest":"hirsel.json","package":{"name":"app","version":"1"},"expose":[]}])
            }
            6 => manifest["package"]["version"] = "1-rc".into(),
            7 => manifest["package"]["name"] = "fn".into(),
            // Schema 2 selects through its catalog, not schema 1's edge form.
            _ => {
                manifest["schema"] = 2.into();
                manifest["dependencies"] = serde_json::json!([{"manifest":"hirsel.json","package":{"name":"app","version":"1"},"expose":[]}])
            }
        }
        std::fs::write(&path, manifest.to_string()).unwrap();
        assert!(
            matches!(load_program(&path), Err(ToolError::Runtime(_))),
            "mutation {mutate}"
        );
    }
    let duplicate = base.to_string().replacen("{", "{\"schema\":1,", 1);
    std::fs::write(&path, duplicate).unwrap();
    assert!(matches!(load_program(&path), Err(ToolError::Runtime(_))));
    std::fs::write(&path, base.to_string()).unwrap();
    assert!(load_program(&path).is_ok());
    std::fs::remove_dir_all(dir).unwrap();
}

/// PROG-1's source-dependency program, bundled and moved: a source outside the
/// program's directory, two exposed packages and a nested dependency on another
/// version of one of them. Without its std dependency: a bundle renames std's
/// sources, so the toolchain's precheck cannot serve it and the run would check
/// all of std from source for a manifest rewrite that treats std like any package.
#[test]
fn packaged_replay_moves_without_original_manifest_paths() {
    with_compiler_stack(|| {
        let fixtures = std::fs::canonicalize(root().join("packages/fixtures")).unwrap();
        let program = root().join("conformance/PROG/PROG-1-source-dependencies-run.hir");
        let text = std::fs::read_to_string(&program)
            .unwrap()
            .replace("std.identity(42)", "42");
        assert!(!text.contains("std."));
        let input_scratch = tempfile::Builder::new()
            .prefix("hirsel-package-input-")
            .tempdir()
            .unwrap();
        let input = input_scratch.path().to_path_buf();
        let source = input.join("source.hir");
        std::fs::write(&source, &text).unwrap();
        let dependency = |name: &str, head: &str, module: &[&str]| {
            serde_json::json!({
                "manifest": fixtures.join(format!("{name}@1/hirsel.json")),
                "package": {"name": name, "version": "1"},
                "expose": [{"head": head, "module": module}],
            })
        };
        let manifest = serde_json::json!({
            "schema": 1,
            "package": {"name": "application", "version": "1"},
            "entry": {"module": [], "name": "main"},
            "sources": [
                {"path": "source.hir", "module": []},
                {"path": fixtures.join("helper.hir"), "module": ["helper"]},
            ],
            "dependencies": [
                dependency("probe", "vendor", &["api"]),
                dependency("wrapper", "other", &[]),
            ],
        });
        std::fs::write(source.with_extension("hir.json"), manifest.to_string()).unwrap();
        let dir_scratch = tempfile::Builder::new()
            .prefix("hirsel-package-bundle-")
            .tempdir()
            .unwrap();
        let dir = dir_scratch.path().join("bundle");
        let moved = dir.with_extension("moved");
        assert!(bundle_program(&source, &text, &dir).unwrap());
        std::fs::remove_dir_all(&input).unwrap();
        std::fs::rename(&dir, &moved).unwrap();
        assert!(matches!(
            run_path(&moved.join("source.hir"), &mut Vec::new()).unwrap(),
            RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
        ));
        std::fs::remove_dir_all(moved).unwrap();
    });
}

#[test]
fn short_and_qualified_std_calls_keep_the_same_definition_dependencies() {
    with_compiler_stack(|| {
        let path = root().join("conformance/LANG/LANG-4-std-short-run.hir");
        let source = std::fs::read_to_string(&path).unwrap();
        let qualified = source
            .replace("string.find(", "std@1/string.find(")
            .replace("string.starts_with(", "std@1/string.starts_with(")
            .replace("string.split(", "std@1/string.split(")
            .replace("bytes.replace(", "std@1/bytes.replace(");
        let mut dependencies = Vec::new();
        for text in [&source, &qualified] {
            let mut program = load_program_source(&path, Some(text)).unwrap();
            let resolution = program.engine.resolve_definition(&program.entry).unwrap();
            assert!(resolution.diagnostics.is_empty());
            let paths: std::collections::BTreeSet<_> = resolution
                .value
                .dependencies
                .iter()
                .map(ToString::to_string)
                .collect();
            assert!(paths.contains("std@1/string/find"));
            assert!(paths.contains("core@1/string/byte_len"));
            assert!(paths.contains("std@1/bytes/replace"));
            dependencies.push(paths);
        }
        assert_eq!(dependencies[0], dependencies[1]);
    });
}
