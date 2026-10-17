//! PROG-4 package metadata: `unsafe_ffi` manifests, the root-only selection
//! rule, wrapper declarations, the check report and the interpreter refusal.
use hirsel_driver::*;
use hirsel_engine::*;
use serde_json::json;
use std::path::{Path, PathBuf};

const LIBRARY: &str = "/// Reads its arguments during the call and retains nothing.
extern \"c\" fn demo_add(u: Unsafe, a: i32, b: i32) -> i32

pub fn add(u: Unsafe, a: i32, b: i32) -> i32 {
  demo_add(u, a, b)
}
";
const MAIN: &str = "pub fn main(u: Unsafe) -> Result[(), ()] {
  let three = lib.add(u, 1, 2)
  Ok(())
}
";

/// A fresh directory holding `lib@1`, marked `unsafe ffi` with `foreign`.
fn fixture(name: &str, foreign: serde_json::Value) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hirsel-foreign-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("lib")).unwrap();
    std::fs::write(dir.join("lib/lib.hir"), LIBRARY).unwrap();
    let lib = json!({"schema":1,"package":{"name":"lib","version":"1"},
        "sources":[{"path":"lib.hir","module":[]}],"dependencies":[],"unsafe_ffi":foreign});
    std::fs::write(dir.join("lib/hirsel.json"), lib.to_string()).unwrap();
    std::fs::write(dir.join("main.hir"), MAIN).unwrap();
    dir
}
fn select_lib() -> serde_json::Value {
    json!({"manifest":"lib/hirsel.json","package":{"name":"lib","version":"1"},
        "expose":[{"head":"lib","module":[]}]})
}
fn launch(dir: &Path, dependencies: serde_json::Value) -> PathBuf {
    let manifest = json!({"schema":1,"package":{"name":"app","version":"1"},
        "entry":{"module":[],"name":"main"},"sources":[{"path":"main.hir","module":[]}],
        "dependencies":dependencies});
    let path = dir.join("hirsel.json");
    std::fs::write(&path, manifest.to_string()).unwrap();
    path
}
fn refusal(path: &Path) -> String {
    match load_program(path) {
        Err(ToolError::Runtime(message)) => message,
        Err(other) => panic!("expected a tool-input error, got {other}"),
        Ok(_) => panic!("expected a tool-input error"),
    }
}
fn deterministic_add() -> serde_json::Value {
    json!({"link":["demo"],"headers_checked":true,
        "wrappers":[{"module":[],"name":"add","kind":"pure","model":"deterministic"}]})
}

#[test]
fn the_root_selects_unsafe_ffi_and_check_reports_it() {
    with_compiler_stack(|| {
        let dir = fixture("root", deterministic_add());
        let path = launch(&dir, json!([select_lib()]));
        let output = check_path(&path).unwrap();
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let [package] = output.unsafe_ffi.as_slice() else {
            panic!("expected one unsafe ffi package: {:?}", output.unsafe_ffi);
        };
        assert_eq!(package.package.name, "lib");
        assert_eq!(package.link, [LinkInput::Library("demo".into())]);
        assert!(package.headers_checked);
        let (wrapper, declared) = package.wrappers.first_key_value().unwrap();
        assert_eq!(wrapper.0.name, "add");
        assert_eq!(declared.kind, hirsel_types::WrapperKind::Pure);
        assert!(declared.deterministic);
        // The interpreter calls no C: it refuses before entry, naming the extern.
        let Err(ToolError::Runtime(message)) = run_path(&path, &mut Vec::new()) else {
            panic!("expected the interpreter to refuse");
        };
        assert!(message.contains("lib@1/demo_add"), "{message}");
        assert!(message.contains("does not call C"), "{message}");
        std::fs::remove_dir_all(dir).unwrap();
    });
}

#[test]
fn only_the_root_manifest_selects_an_unsafe_ffi_package() {
    with_compiler_stack(|| {
        let dir = fixture("nested", deterministic_add());
        std::fs::write(dir.join("mid.hir"), "pub fn m() {}\n").unwrap();
        let mid = json!({"schema":1,"package":{"name":"mid","version":"1"},
            "sources":[{"path":"mid.hir","module":[]}],"dependencies":[select_lib()]});
        std::fs::write(dir.join("mid.json"), mid.to_string()).unwrap();
        let select_mid = json!({"manifest":"mid.json","package":{"name":"mid","version":"1"},
            "expose":[]});
        let message = refusal(&launch(&dir, json!([select_mid.clone()])));
        assert!(
            message
                .contains("only the root launch manifest may select the unsafe ffi package lib@1"),
            "{message}"
        );
        // The root selecting it as well does not excuse the dependency's selection.
        let path = launch(&dir, json!([select_mid, select_lib()]));
        assert!(refusal(&path).contains("only the root launch manifest"));
        std::fs::remove_dir_all(dir).unwrap();
    });
}

#[test]
fn unsafe_ffi_metadata_errors_are_tool_errors() {
    with_compiler_stack(|| {
        for (case, foreign, expected) in [
            (
                "extern-wrapper",
                json!({"link":[],"headers_checked":false,
                    "wrappers":[{"module":[],"name":"demo_add","kind":"pure","model":"none"}]}),
                "is not a non-extern function",
            ),
            (
                "absent-wrapper",
                json!({"link":[],"headers_checked":false,
                    "wrappers":[{"module":[],"name":"absent","kind":"pure","model":"none"}]}),
                "is not a non-extern function",
            ),
            (
                "kind",
                json!({"link":[],"headers_checked":false,
                    "wrappers":[{"module":[],"name":"add","kind":"pure-ish","model":"none"}]}),
                "unknown wrapper kind",
            ),
            (
                "model",
                json!({"link":[],"headers_checked":false,
                    "wrappers":[{"module":[],"name":"add","kind":"pure","model":"replayable"}]}),
                "unknown wrapper model",
            ),
            (
                "link",
                json!({"link":["-lz"],"headers_checked":false,"wrappers":[]}),
                "invalid link library",
            ),
            (
                "headers",
                json!({"link":[],"headers_checked":"no","wrappers":[]}),
                "headers_checked must be a boolean",
            ),
            (
                "field",
                json!({"link":[],"headers_checked":false,"wrappers":[],"libraries":[]}),
                "unknown field libraries",
            ),
        ] {
            let dir = fixture(case, foreign);
            let message = refusal(&launch(&dir, json!([select_lib()])));
            assert!(message.contains(expected), "{case}: {message}");
            std::fs::remove_dir_all(dir).unwrap();
        }
    });
}
