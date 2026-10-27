//! MEM-7 and PROG-4 native data ABI: `hirsel export --c-header` declares a program's
//! `@layout(c)` records and variants for C, a borrowed variable-size flat value reaches C
//! as its bytes and extent, and foreign code compiled from a header of another ABI version
//! or layout refuses to link.
use hirsel_driver::*;
use hirsel_engine::*;
use serde_json::json;
use std::path::{Path, PathBuf};

const LIBRARY: &str = r#"@layout(c)
pub type Point {
  x: i32,
  y: i32,
}

@layout(c)
pub type Shape = Circle(Point, i32) | Square(Point) | Empty

pub type Opaque {
  hidden: i32,
}

pub flat type Label {
  id: u64,
  name: String,
}

/// Reads the point during the call and retains nothing.
extern "c" fn geo_point_sum(u: Unsafe, p: Point) -> i32

/// Reads the shape during the call and retains nothing.
extern "c" fn geo_shape_weight(u: Unsafe, s: Shape) -> i32

/// Reads the label's bytes during the call and retains nothing.
extern "c" fn geo_label_extent(u: Unsafe, label: Label) -> u64

pub fn point_sum(u: Unsafe, p: Point) -> i32 {
  geo_point_sum(u, p)
}

pub fn shape_weight(u: Unsafe, s: Shape) -> i32 {
  geo_shape_weight(u, s)
}

pub fn label_extent(u: Unsafe, label: Label) -> u64 {
  geo_label_extent(u, label)
}
"#;

const C: &str = r#"#include <string.h>
#include "geo.h"

int32_t geo_point_sum(const struct Point *p) { return p->x + p->y; }

int32_t geo_shape_weight(const struct Shape *s) {
    switch (s->tag) {
    case Shape_Circle: return s->payload.Circle._0.x * 100 + s->payload.Circle._1;
    case Shape_Square: return 1000 + s->payload.Square._0.y;
    default: return -1;
    }
}

uint64_t geo_label_extent(const uint8_t *data, size_t len) {
    return data != 0 && memmem(data, len, "tower", 5) != 0 ? len : 0;
}
"#;

const MAIN: &str = r#"pub type AppError = Grant(GrantError) | Output(OutputError)

pub fn main(rt: Io, u: Unsafe) -> Result[(), AppError] {
  let stdout = io.stdout(rt) ? Grant
  let corner = geo.Point(x: 2, y: 3)
  let sum = geo.point_sum(u, corner)
  let circle = geo.shape_weight(u, Circle(geo.Point(x: 4, y: 0), 7))
  let square = geo.shape_weight(u, Square(geo.Point(x: 0, y: 9)))
  let empty = geo.shape_weight(u, Empty)
  let extent = geo.label_extent(u, geo.Label(id: 7, name: "tower"))
  out.print(stdout, if sum == 5 { "1" } else { "0" }) ? Output
  out.print(stdout, if circle == 407 { "1" } else { "0" }) ? Output
  out.print(stdout, if square == 1009 { "1" } else { "0" }) ? Output
  out.print(stdout, if empty == -1 { "1" } else { "0" }) ? Output
  out.print(stdout, if extent >= 13 { "1\n" } else { "0\n" }) ? Output
  Ok(())
}
"#;

/// The package `geo` with `library`, its C file compiled against `header`, and an app.
struct Fixture {
    root: PathBuf,
    manifest: PathBuf,
}

fn fixture(name: &str, library: &str) -> Fixture {
    fixture_with_main(name, library, MAIN)
}

fn fixture_with_main(name: &str, library: &str, main: &str) -> Fixture {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("data-abi-{name}"));
    let _ = std::fs::remove_dir_all(&root);
    let dir = root.join("geo");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("lib.hir"), library).unwrap();
    let wrappers: Vec<_> = ["point_sum", "shape_weight", "label_extent"]
        .iter()
        .map(|name| json!({"module":[],"name":name,"kind":"pure","model":"deterministic"}))
        .collect();
    let manifest = json!({"schema":1,"package":{"name":"geo","version":"1"},
        "sources":[{"path":"lib.hir","module":[]}],"dependencies":[],
        "unsafe_ffi":{"link":["lib.o"],"headers_checked":true,"wrappers":wrappers}});
    std::fs::write(dir.join("hirsel.json"), manifest.to_string()).unwrap();
    std::fs::write(root.join("main.hir"), main).unwrap();
    std::fs::write(
        root.join("grants.json"),
        r#"{"io":{"stdout":"stdout.bytes@1","dirs":[]},"unsafe":true}"#,
    )
    .unwrap();
    let manifest = json!({"schema":1,"package":{"name":"app","version":"1"},
        "entry":{"module":[],"name":"main"},"sources":[{"path":"main.hir","module":[]}],
        "dependencies":[{"manifest":"geo/hirsel.json","package":{"name":"geo","version":"1"},
            "expose":[{"head":"geo","module":[]}]}]});
    let path = root.join("hirsel.json");
    std::fs::write(&path, manifest.to_string()).unwrap();
    Fixture {
        root,
        manifest: path,
    }
}

impl Fixture {
    fn export(&self, header: bool) -> String {
        native::export_data_abi(
            &self.manifest,
            native::TargetSpec::x86_64_unknown_linux_gnu(),
            header,
        )
        .unwrap_or_else(|e| panic!("{e:?}"))
        .unwrap_or_else(|e| panic!("{e:?}"))
    }
    /// Compile the C file against `header` into the package's link input.
    fn compile(&self, header: &str) {
        let dir = self.root.join("geo");
        std::fs::write(dir.join("geo.h"), header).unwrap();
        std::fs::write(dir.join("lib.c"), C).unwrap();
        let target = native::TargetSpec::x86_64_unknown_linux_gnu();
        let tools = native::Toolchain::in_tree(&self.root.join("runtime"), &[&target]).unwrap();
        let compiled = std::process::Command::new(&tools.clang)
            .args(["-c", "-O1", "-fPIC", "-D_GNU_SOURCE"])
            .arg(dir.join("lib.c"))
            .arg("-o")
            .arg(dir.join("lib.o"))
            .output()
            .unwrap();
        assert!(compiled.status.success(), "{compiled:?}");
    }
    fn build(&self, backend: native::Backend) -> Result<PathBuf, String> {
        let executable = self.root.join(format!("app-{backend:?}"));
        native::build_path_native_backend(
            &self.manifest,
            &executable,
            native::TargetSpec::x86_64_unknown_linux_gnu(),
            native::BuildProfile::Default,
            backend,
            None,
            false,
            Verify::On,
        )
        .map_err(|error| format!("{error}"))?
        .map_err(|error| error.message)?;
        Ok(executable)
    }
    fn run(&self, executable: &Path) -> String {
        let output = std::process::Command::new(executable)
            .arg("--hirsel-grants")
            .arg(self.root.join("grants.json"))
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap()
    }
}

/// The header declares each C type with its offsets asserted and its identity referenced;
/// C reads records and variants through it and a flat label as bytes and extent, on both
/// backends.
#[test]
fn c_code_reads_records_variants_and_flat_extents_through_the_generated_header() {
    with_compiler_stack(|| {
        let fixture = fixture("header", LIBRARY);
        assert!(!fixture.root.join("geo/lib.o").exists());
        // An unselected source is outside this manifest's program.
        std::fs::write(fixture.root.join("geo/unselected.hir"), "not valid Hirsel").unwrap();
        let header = fixture.export(true);
        let abi = format!("_v{}_", hirsel_rt::ABI_VERSION);
        for expected in [
            "struct Point {\n    int32_t x;\n    int32_t y;\n};",
            "union {",
            "_Static_assert(sizeof(struct Point) == 8",
            "_Static_assert(sizeof(struct Shape) == 16",
            "#define Shape_Circle 0u",
            "#define Shape_Empty 2u",
            "_Static_assert(offsetof(struct Shape, payload.Circle._1) == 12",
            "extern const char hirsel_data_abi",
        ] {
            assert!(header.contains(expected), "{expected}\n{header}");
        }
        assert!(
            header.contains(&format!("hirsel_data_abi{abi}Point_")),
            "{header}"
        );
        // An ordinary nominal has no promised C representation.
        assert!(!header.contains("struct Opaque"), "{header}");
        // Point precedes the Shape that holds it.
        assert!(header.find("struct Point {") < header.find("struct Shape {"));
        let descriptor_text = fixture.export(false);
        let descriptor: serde_json::Value = serde_json::from_str(&descriptor_text).unwrap();
        assert_eq!(descriptor["target"], "x86_64-unknown-linux-gnu");
        assert_eq!(descriptor["abi_version"], hirsel_rt::ABI_VERSION);
        let shape = &descriptor["types"].as_array().unwrap()[1];
        assert_eq!(
            (shape["name"].as_str(), shape["size"].as_u64()),
            (Some("Shape"), Some(16))
        );
        assert!(!fixture.root.join("geo/lib.o").exists());
        for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
            let refusal = fixture.build(backend).unwrap_err();
            assert!(refusal.contains("No such file"), "{backend:?}: {refusal}");
        }
        fixture.compile(&header);
        assert_eq!(fixture.export(true), header);
        assert_eq!(fixture.export(false), descriptor_text);
        for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
            let executable = fixture
                .build(backend)
                .unwrap_or_else(|e| panic!("{backend:?}: {e}"));
            assert_eq!(fixture.run(&executable), "11111\n", "{backend:?}");
        }
    });
}

/// C compiled from a header of another ABI version, or of another layout of the same
/// type, leaves its identity symbol undefined: the build refuses with PROG-4 naming it.
#[test]
fn foreign_code_from_another_abi_version_or_layout_refuses_to_link() {
    with_compiler_stack(|| {
        let stale = fixture("version", LIBRARY);
        let header = stale.export(true);
        let ours = format!("hirsel_data_abi_v{}_", hirsel_rt::ABI_VERSION);
        let older = format!("hirsel_data_abi_v{}_", hirsel_rt::ABI_VERSION - 1);
        stale.compile(&header.replace(&ours, &older));
        let refusal = stale.build(native::Backend::Llvm).unwrap_err();
        assert!(refusal.starts_with("PROG-4: "), "{refusal}");
        assert!(
            refusal.contains(&format!(
                "data ABI v{}, but this build publishes v{}",
                hirsel_rt::ABI_VERSION - 1,
                hirsel_rt::ABI_VERSION
            )),
            "{refusal}"
        );

        // The same header against a program whose Point gained a field, and one whose
        // Point.y widened.
        let gained = LIBRARY.replace("  y: i32,\n", "  y: i32,\n  z: i32,\n");
        let main = MAIN
            .replace("y: 3)", "y: 3, z: 0)")
            .replace("y: 0)", "y: 0, z: 0)");
        let main = main.replace("y: 9)", "y: 9, z: 0)");
        let widened = LIBRARY.replace("  y: i32,\n", "  y: i64,\n");
        for (name, changed) in [
            ("gained", fixture_with_main("gained", &gained, &main)),
            ("widened", fixture("widened", &widened)),
        ] {
            changed.compile(&header);
            let refusal = changed.build(native::Backend::Cranelift).unwrap_err();
            assert!(
                refusal.contains("another layout or target of `"),
                "{name}: {refusal}"
            );
        }
    });
}

/// Deferring foreign files does not defer selected source or manifest validation.
#[test]
fn declaration_export_refuses_source_and_manifest_errors_without_link_inputs() {
    with_compiler_stack(|| {
        let fixture = fixture("export-errors", LIBRARY);
        let export = |header| {
            native::export_data_abi(
                &fixture.manifest,
                native::TargetSpec::x86_64_unknown_linux_gnu(),
                header,
            )
        };
        std::fs::write(
            fixture.root.join("main.hir"),
            MAIN.replace("geo.point_sum(u, corner)", "geo.point_sum(u, true)"),
        )
        .unwrap();
        for header in [true, false] {
            assert!(matches!(export(header), Err(ToolError::Query { .. })));
        }
        std::fs::write(fixture.root.join("main.hir"), MAIN).unwrap();
        let manifest = fixture.root.join("geo/hirsel.json");
        let original = std::fs::read_to_string(&manifest).unwrap();
        for (changed, expected) in [
            (original.replace("lib.o", "-lz"), "invalid link library"),
            (
                original.replace("point_sum", "absent"),
                "is not a non-extern function",
            ),
            (
                original.replace("\"version\":\"1\"", "\"version\":\"2\""),
                "selected package version does not match dependency",
            ),
        ] {
            std::fs::write(&manifest, changed).unwrap();
            for header in [true, false] {
                let error = export(header).unwrap_err().to_string();
                assert!(error.contains(expected), "{error}");
            }
        }
        assert!(!fixture.root.join("geo/lib.o").exists());
    });
}

#[test]
fn declaration_export_graph_cannot_supply_a_check_without_foreign_link_inputs() {
    with_compiler_stack(|| {
        use hirsel_driver::requests::{
            CompileOperation, CompileRequest, CompileResult, CompilerRequests,
        };
        use hirsel_engine::requests::RequestEngine;
        let fixture = fixture("export-graph", LIBRARY);
        let compiler = COMPILER;
        let mut selector = CompilerRequests::for_host(compiler.clone(), compiler.configuration());
        let exported = selector
            .execute(&fixture.manifest, CompileOperation::DataAbi)
            .unwrap();
        assert!(matches!(&exported.result, CompileResult::NativeSources(_)));
        assert!(!fixture.root.join("geo/lib.o").exists());
        let checked = selector.select(&CompileRequest {
            path: fixture.manifest.clone(),
            inputs: exported.inputs.clone(),
            operation: CompileOperation::Check {
                lowering: LoweringPolicy::Reachable,
            },
        });
        assert!(
            checked.is_err(),
            "declaration-only discovery authorized a check with missing link inputs"
        );
    });
}
