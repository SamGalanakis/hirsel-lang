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
        assert_eq!(std.value.path.0.package.name, "std");
        assert!(
            !registry::BuiltinRegistry::new()
                .signatures()
                .any(|s| s.definition.0.package.name == "std")
        );
        assert!(matches!(
            run_path(&manifest, &mut Vec::new()).unwrap(),
            RunOutcome::Exit(0)
        ));
    });
}

#[test]
fn launch_metadata_errors_are_tool_errors() {
    let dir = std::env::temp_dir().join(format!("hirsel-packages-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("main.hir"), "pub fn main(t: Test) {}\n").unwrap();
    let base = serde_json::json!({"schema":1,"package":{"name":"app","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}});
    let path = dir.join("hirsel.json");
    for mutate in [0, 1, 2, 3, 4, 5, 6, 7] {
        let mut manifest = base.clone();
        match mutate {
            0 => manifest["schema"] = 2.into(),
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
            _ => manifest["package"]["name"] = "fn".into(),
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

#[test]
fn packaged_replay_moves_without_original_manifest_paths() {
    with_compiler_stack(|| {
        let source = root().join("conformance/PROG/PROG-1-source-dependencies-run.hir");
        let dir =
            std::env::temp_dir().join(format!("hirsel-package-bundle-{}", std::process::id()));
        let moved = dir.with_extension("moved");
        let text = std::fs::read_to_string(&source).unwrap();
        assert!(bundle_program(&source, &text, &dir).unwrap());
        std::fs::rename(&dir, &moved).unwrap();
        assert!(matches!(
            run_path(&moved.join("source.hir"), &mut Vec::new()).unwrap(),
            RunOutcome::Exit(0)
        ));
        std::fs::remove_dir_all(moved).unwrap();
    });
}
