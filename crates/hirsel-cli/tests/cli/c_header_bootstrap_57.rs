//! Declaration export before the manifest's foreign artifact exists.
use super::*;

#[test]
fn c_header_and_descriptor_export_before_foreign_compilation() {
    let source = file(
        "@layout(c)\npub type Point { x: i32, y: i32 }\npub fn main() -> Result[(), ()] { Ok(()) }\n",
    );
    let manifest = source.with_extension("hir.json");
    let object = source.with_extension("o");
    let output = source.with_extension("export");
    std::fs::write(
        &manifest,
        serde_json::json!({
            "schema":1,"package":{"name":"bootstrap","version":"1"},
            "entry":{"module":[],"name":"main"},
            "sources":[{"path":source.file_name().unwrap().to_str().unwrap(),"module":[]}],
            "dependencies":[],
            "unsafe_ffi":{"link":[object],"headers_checked":true,"wrappers":[]}
        })
        .to_string(),
    )
    .unwrap();
    assert!(!object.exists());
    // The source's sidecar and the explicit manifest select the same declarations.
    for entry in [&source, &manifest] {
        for kind in ["--c-header", "--native-abi"] {
            let result = Command::new(env!("CARGO_BIN_EXE_hirsel"))
                .args(["export", kind])
                .arg(entry)
                .arg("-o")
                .arg(&output)
                .output()
                .unwrap();
            assert!(result.status.success(), "{result:?}");
            let text = std::fs::read_to_string(&output).unwrap();
            if kind == "--c-header" {
                assert!(text.contains("struct Point {"), "{text}");
                assert!(text.contains("_Static_assert"), "{text}");
            } else {
                let abi: serde_json::Value = serde_json::from_str(&text).unwrap();
                assert_eq!(abi["types"][0]["name"], "Point");
                assert_eq!(abi["types"][0]["size"], 8);
            }
            assert!(!object.exists());
            std::fs::remove_file(&output).unwrap();
        }
    }
    std::fs::write(&source, "pub fn main() -> Result[(), ()] { true }\n").unwrap();
    for kind in ["--c-header", "--native-abi"] {
        let result = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(["export", kind])
            .arg(&manifest)
            .arg("-o")
            .arg(&output)
            .output()
            .unwrap();
        assert!(!result.status.success(), "{result:?}");
        assert!(!result.stderr.is_empty());
        assert!(!output.exists());
    }
    std::fs::remove_file(manifest).unwrap();
    std::fs::remove_file(source).unwrap();
}
