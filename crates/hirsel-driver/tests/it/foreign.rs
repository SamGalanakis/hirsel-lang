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
                "threads",
                json!({"link":[],"headers_checked":false,"threads":"many","wrappers":[]}),
                "unknown unsafe_ffi threads \"many\"",
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

/// A tiny C library: scalars, a byte view and a counter handle.
const DEMO_C: &str = r#"#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>

int32_t demo_add(int32_t a, int32_t b) { return a + b; }
uint8_t demo_low(uint16_t x) { return (uint8_t)x; }
int8_t demo_negate(int8_t x) { return (int8_t)-x; }
double demo_scale(int64_t x, double factor) { return (double)x * factor; }
uint64_t demo_sum(const uint8_t *data, size_t length) {
    uint64_t sum = 0;
    for (size_t i = 0; i < length; i++) sum += data[i];
    return sum;
}

typedef struct { int32_t value; } counter;
static int32_t freed;
void *counter_new(int32_t start) {
    counter *c = malloc(sizeof *c);
    c->value = start;
    return c;
}
int32_t counter_bump(void *c) { return ++((counter *)c)->value; }
void counter_free(void *c) { freed++; free(c); }
int32_t counter_freed(void) { return freed; }


/* Exercise the ABI itself: AAPCS64 permits arbitrary high bits in narrow
   callback arguments. The shim passes the declared low bytes with dirty highs. */
#if defined(__aarch64__)
__asm__(".text\n"
        ".global demo_narrow\n"
        ".type demo_narrow, %function\n"
        "demo_narrow:\n"
        "stp x29, x30, [sp, #-16]!\n"
        "mov x29, sp\n"
        "mov x3, x1\n"
        "movz x1, #0x7f81\n"
        "movk x1, #0xdead, lsl #16\n"
        "movz x2, #0xbeef\n"
        "movk x2, #0xabcd, lsl #16\n"
        "blr x3\n"
        "ldp x29, x30, [sp], #16\n"
        "ret\n"
        ".size demo_narrow, .-demo_narrow\n");
#else
int8_t demo_narrow(void *ctx, int8_t (*f)(void *, int8_t, uint16_t)) {
    return f(ctx, -127, 0xbeef);
}
#endif

int32_t demo_each(void *ctx, int32_t (*f)(void *, int32_t), int32_t n) {
    int32_t sum = 0;
    for (int32_t i = 0; i < n; i++) sum += f(ctx, i);
    return sum;
}
"#;

/// Its safe wrapper: externs, a handle resource, a callback and wrappers.
const DEMO_HIR: &str = "pub type Tally {
  calls: i32,
}

/// Calls f(ctx, i) for each i below n during the call and retains neither.
extern \"c\" fn demo_each(
  u: Unsafe,
  mut ctx: unique Tally,
  f: FnPtr[fn(mut state: unique Tally, i: i32) -> i32],
  n: i32
) -> i32


/// Calls f(ctx, -127, 0xbeef) during this call and retains neither argument.
extern \"c\" fn demo_narrow(
  u: Unsafe,
  mut ctx: unique Tally,
  f: FnPtr[fn(mut state: unique Tally, a: i8, b: u16) -> i8]
) -> i8

fn narrow(mut ctx: Tally, a: i8, b: u16) -> i8 {
  ctx.calls += 1
  if a == -127i8 && b == 0xbeefu16 { -5i8 } else { 7i8 }
}

pub fn narrow_callback(u: Unsafe) -> bool {
  let mut tally = Tally(calls: 0)
  let value = demo_narrow(u, mut tally, narrow)
  value == -5i8 && tally.calls == 1
}

fn double(mut ctx: Tally, i: i32) -> i32 {
  ctx.calls += 1
  i * 2
}

fn explode(mut ctx: Tally, i: i32) -> i32 {
  ctx.calls += 1
  2147483647 + i
}

/// 100 times the sum of the doubled numbers below n, plus how many C visited.
pub fn each_doubled(u: Unsafe, n: i32) -> i32 {
  let mut tally = Tally(calls: 0)
  let total = demo_each(u, mut tally, double, n)
  total * 100 + tally.calls
}

pub fn each_exploding(u: Unsafe, n: i32) -> i32 {
  let mut tally = Tally(calls: 0)
  demo_each(u, mut tally, explode, n)
}

/// Reads its arguments only.
extern \"c\" fn demo_add(u: Unsafe, a: i32, b: i32) -> i32

/// Reads its argument only.
extern \"c\" fn demo_low(u: Unsafe, x: u16) -> u8

/// Reads its argument only.
extern \"c\" fn demo_negate(u: Unsafe, x: i8) -> i8

/// Reads its arguments only.
extern \"c\" fn demo_scale(u: Unsafe, x: i64, factor: f64) -> f64

/// Reads the bytes during the call and retains nothing.
extern \"c\" fn demo_sum(u: Unsafe, data: Bytes) -> u64

/// Returns a counter C owns until counter_free.
extern \"c\" fn counter_new(u: Unsafe, start: i32) -> Ptr[u8]

/// Updates the counter at h, which stays C's.
extern \"c\" fn counter_bump(u: Unsafe, h: Ptr[u8]) -> i32

/// Frees the counter at h; nothing holds it afterwards.
extern \"c\" fn counter_free(u: Unsafe, take h: Ptr[u8]) -> ()

/// Reads how many counters were freed.
extern \"c\" fn counter_freed(u: Unsafe) -> i32

pub resource type Counter {
  u: Unsafe,
  h: Ptr[u8],
}
drop(c) {
  counter_free(c.u, c.h)
}

pub fn add(u: Unsafe, a: i32, b: i32) -> i32 {
  demo_add(u, a, b)
}

pub fn sum(u: Unsafe, data: Bytes) -> u64 {
  demo_sum(u, data)
}

pub fn counter(u: Unsafe, start: i32) -> Counter {
  Counter(u: u, h: counter_new(u, start))
}

pub fn bump(c: Counter) -> i32 {
  counter_bump(c.u, c.h)
}

pub fn freed(u: Unsafe) -> i32 {
  counter_freed(u)
}

pub fn low(u: Unsafe, x: u16) -> u8 {
  demo_low(u, x)
}

pub fn negate(u: Unsafe, x: i8) -> i8 {
  demo_negate(u, x)
}

pub fn scale(u: Unsafe, x: i64, factor: f64) -> f64 {
  demo_scale(u, x, factor)
}
";

const NATIVE_MAIN: &str = "pub fn main(u: Unsafe) -> Result[(), ()] {
  if lib.add(u, 40, 2) != 42 {
    return Error(())
  }
  if lib.low(u, 0x1234u16) != 0x34u8 {
    return Error(())
  }
  if lib.negate(u, -128i8 + 1i8) != 127i8 {
    return Error(())
  }
  if lib.scale(u, 3i64, 0.5) != 1.5 {
    return Error(())
  }
  if lib.sum(u, b\"\\x01\\x02\\xff\") != 258u64 {
    return Error(())
  }
  if counted(u) != 12 {
    return Error(())
  }
  if lib.freed(u) != 1 {
    return Error(())
  }
  if lib.each_doubled(u, 4) != 1204 {
    return Error(())
  }
  if !lib.narrow_callback(u) {
    return Error(())
  }
  Ok(())
}

fn counted(u: Unsafe) -> i32 {
  let c = lib.counter(u, 10)
  let first = lib.bump(c)
  first + lib.bump(c) - 11
}
";

/// Builds `main` against the demo library, compiled with the pinned clang;
/// returns the directory, its launch manifest and the executable.
fn native_demo(name: &str, main: &str) -> (PathBuf, PathBuf, PathBuf) {
    native_demo_backend(name, main, native::Backend::Llvm)
}
fn native_demo_backend(
    name: &str,
    main: &str,
    backend: native::Backend,
) -> (PathBuf, PathBuf, PathBuf) {
    native_demo_target(
        name,
        main,
        backend,
        native::TargetSpec::x86_64_unknown_linux_gnu(),
    )
}
fn native_demo_target(
    name: &str,
    main: &str,
    backend: native::Backend,
    target: native::TargetSpec,
) -> (PathBuf, PathBuf, PathBuf) {
    let wrappers: Vec<_> = [
        "add",
        "sum",
        "low",
        "negate",
        "scale",
        "each_doubled",
        "each_exploding",
        "narrow_callback",
    ]
    .into_iter()
    .map(|name| json!({"module":[],"name":name,"kind":"pure","model":"deterministic"}))
    .chain(
        ["counter", "bump", "freed"]
            .into_iter()
            .map(|name| json!({"module":[],"name":name,"kind":"handle","model":"none"})),
    )
    .collect();
    native_c_fixture(name, main, backend, target, DEMO_C, DEMO_HIR, wrappers)
}

fn native_c_fixture(
    name: &str,
    main: &str,
    backend: native::Backend,
    target: native::TargetSpec,
    c: &str,
    library: &str,
    wrappers: Vec<serde_json::Value>,
) -> (PathBuf, PathBuf, PathBuf) {
    use std::process::Command;
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("foreign-{name}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("lib")).unwrap();
    let tools = native::Toolchain::in_tree(&root.join("runtime"), &[&target]).unwrap();
    std::fs::write(root.join("lib/demo.c"), c).unwrap();
    let cc = if target.triple.starts_with("aarch64") {
        tools.aarch64_cc.as_ref().unwrap()
    } else {
        &tools.clang
    };
    let compiled = Command::new(cc)
        .args(["-c", "-O1", "-fPIC"])
        .arg(root.join("lib/demo.c"))
        .arg("-o")
        .arg(root.join("lib/demo.o"))
        .output()
        .unwrap();
    assert!(compiled.status.success(), "{compiled:?}");
    std::fs::write(root.join("lib/lib.hir"), library).unwrap();
    let lib = json!({"schema":1,"package":{"name":"lib","version":"1"},
        "sources":[{"path":"lib.hir","module":[]}],"dependencies":[],
        "unsafe_ffi":{"link":["demo.o"],"headers_checked":false,"wrappers":wrappers}});
    std::fs::write(root.join("lib/hirsel.json"), lib.to_string()).unwrap();
    std::fs::write(root.join("main.hir"), main).unwrap();
    std::fs::write(root.join("grants.json"), r#"{"unsafe":true}"#).unwrap();
    let path = launch(&root, json!([select_lib()]));
    let executable = root.join("app");
    let built = native::build_path_native_backend(
        &path,
        &executable,
        target,
        native::BuildProfile::Default,
        backend,
        None,
        false,
        hirsel_engine::Verify::On,
    )
    .unwrap();
    assert!(built.is_ok(), "{built:?}");
    (root, path, executable)
}
fn execute(root: &Path, executable: &Path, record: bool) -> std::process::Output {
    let mut command = std::process::Command::new(executable);
    if record {
        command.arg("--hirsel-record").arg(root.join("tape"));
    }
    command
        .arg("--hirsel-grants")
        .arg(root.join("grants.json"))
        .output()
        .unwrap()
}

#[test]
fn native_calls_c_with_scalars_a_byte_view_and_a_released_handle() {
    with_compiler_stack(|| {
        let (root, path, executable) = native_demo("native", NATIVE_MAIN);
        let ran = execute(&root, &executable, false);
        assert_eq!(ran.status.code(), Some(0), "{ran:?}");
        // The handle wrappers have no deterministic model: no recording, from the
        // executable or from the tool, which names the use.
        let recorded = execute(&root, &executable, true);
        assert_eq!(recorded.status.code(), Some(3), "{recorded:?}");
        let stderr = String::from_utf8_lossy(&recorded.stderr);
        assert!(
            stderr.contains("cannot be recorded or replayed"),
            "{stderr}"
        );
        assert!(!root.join("tape").exists());
        let grants = root.join("grants.json");
        let tape = root.join("tape");
        let launch = HostLaunch::Live {
            config: LaunchConfig::Grants {
                input: grants.as_os_str(),
                arguments: None,
            },
            record: Some(&tape),
            revocations: None,
        };
        let refused = native::run_path_native_with_config(
            &path,
            launch,
            None,
            native::Output::Streams {
                stdout: &mut Vec::new(),
                stderr: &mut Vec::new(),
            },
        );
        let Err(ToolError::Runtime(message)) = refused else {
            panic!(
                "expected a launch refusal, got {:?}",
                refused.map(|r| r.is_ok())
            );
        };
        assert!(
            message.contains("calls the extern lib@1/counter_"),
            "{message}"
        );
        assert!(
            message.contains("cannot be recorded or replayed"),
            "{message}"
        );
        // H executes no C either: a clear refusal, not a malformed program.
        let mut loaded = load_program(&path).unwrap();
        loaded.check_for_execution().unwrap();
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
        let ready = native::build(&validated, &options).unwrap();
        let sites = native::Sites::new(validated.snapshot());
        let Err(error) = native::run_heap(&ready, &loaded.entry, &sites) else {
            panic!("expected H to refuse the extern call");
        };
        assert!(error.to_string().contains("H does not run C"), "{error}");
    });
}

#[test]
fn native_runs_reaching_only_deterministic_wrappers_record() {
    with_compiler_stack(|| {
        let main = "pub fn main(u: Unsafe) -> Result[(), ()] {
  if lib.add(u, 40, 2) != 42 {
    return Error(())
  }
  Ok(())
}
";
        let (root, _, executable) = native_demo("replayable", main);
        let recorded = execute(&root, &executable, true);
        assert_eq!(recorded.status.code(), Some(0), "{recorded:?}");
        assert!(root.join("tape").exists());
    });
}

#[test]
fn native_callback_failure_ends_the_process() {
    with_compiler_stack(|| {
        let main = "pub fn main(u: Unsafe) -> Result[(), ()] {
  let _ = lib.each_exploding(u, 3)
  Ok(())
}
";
        let (root, _, executable) = native_demo("callback-failure", main);
        let ran = execute(&root, &executable, false);
        assert!(!ran.status.success(), "{ran:?}");
        let stderr = String::from_utf8_lossy(&ran.stderr);
        assert!(
            stderr.contains("a callback failed; the process ends"),
            "{stderr}"
        );
    });
}

/// Source syntax currently cannot form a FnPtr from a generic library operation.
/// Revalidate the existing callback body as a library unit, or mark it as an
/// inlined callable instance, to exercise both emission owners against real C.
#[test]
fn native_c_callbacks_follow_library_and_inline_definitions() {
    use hirsel_lir::ir::UnitOrigin;
    use hirsel_lir::validate::{Inputs, validate_ready};
    with_compiler_stack(|| {
        let (root, path, _) = native_demo("callback-owners", NATIVE_MAIN);
        let mut loaded = load_program(&path).unwrap();
        loaded.check_for_execution().unwrap();
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let target = native::TargetSpec::x86_64_unknown_linux_gnu();
        let tools = native::Toolchain::in_tree(&root.join("runtime"), &[&target]).unwrap();
        for library in [true, false] {
            let mut ready =
                native::build(&validated, &native::Options::new(target.clone())).unwrap();
            let index = ready
                .units
                .iter()
                .position(|u| u.unit().header.definition.0.name == "double")
                .unwrap();
            if library {
                let mut unit = ready.units[index].clone().into_raw();
                unit.header.origin = UnitOrigin::Library;
                ready.units[index] = validate_ready(
                    unit,
                    &Inputs {
                        target: &ready.target,
                        layouts: &ready.table,
                        manifest: &ready.manifest,
                        operations: &ready.operations,
                        discharge: None,
                    },
                )
                .unwrap();
            } else {
                ready
                    .inline_instances
                    .insert(ready.units[index].unit().header.instance);
            }
            for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
                for opt in [native::OptLevel::None, native::OptLevel::Speed] {
                    for partition in [native::Partition::Whole, native::Partition::Singleton] {
                        let dir = root.join(format!("{library}-{backend:?}-{opt:?}-{partition:?}"));
                        std::fs::create_dir_all(&dir).unwrap();
                        let registry = hirsel_lir::descriptor_registry::DescriptorRegistry::new(
                            &ready.table,
                            &ready.finalizers,
                        );
                        let objects = native::partition_backend_objects(
                            &ready, backend, opt, partition, &registry, false, None,
                        )
                        .unwrap();
                        let exe =
                            native::link_objects(&ready, &loaded.entry, &objects, &tools, &dir)
                                .unwrap();
                        let launched = native::launch(&exe, &target, &tools, &dir, false).unwrap();
                        let observation = native::observe(
                            &launched,
                            &ready,
                            &native::Sites::new(validated.snapshot()),
                        )
                        .unwrap();
                        assert_eq!(observation.exit_code, Some(0), "{observation:?}");
                        assert!(observation.failures.is_empty(), "{observation:?}");
                    }
                }
            }
        }
    });
}

#[test]
fn cranelift_calls_c_and_c_callbacks_with_the_same_observations_as_llvm() {
    with_compiler_stack(|| {
        for target in native::TargetSpec::executed() {
            let execute_target = |root: &Path, executable: &Path| {
                if target.triple.starts_with("aarch64") {
                    std::process::Command::new(std::env::var_os("HIRSEL_QEMU_AARCH64").unwrap())
                        .arg("-L")
                        .arg(std::env::var_os("HIRSEL_AARCH64_SYSROOT").unwrap())
                        .arg(executable)
                        .arg("--hirsel-grants")
                        .arg(root.join("grants.json"))
                        .output()
                        .unwrap()
                } else {
                    execute(root, executable, false)
                }
            };
            let mut checks = 0;
            let main = NATIVE_MAIN.replace("Result[(), ()]", "Result[(), i32]");
            let mut diagnostic = String::new();
            for part in main.split_inclusive("return Error(())") {
                if let Some(prefix) = part.strip_suffix("return Error(())") {
                    checks += 1;
                    diagnostic.push_str(&format!("{prefix}return Error({checks})"));
                } else {
                    diagnostic.push_str(part);
                }
            }
            for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
                let (root, _, executable) = native_demo_target(
                    &format!("parity-{backend:?}-{}", target.triple),
                    &diagnostic,
                    backend,
                    target.clone(),
                );
                let ran = execute_target(&root, &executable);
                assert_eq!(ran.status.code(), Some(0), "{backend:?}: {ran:?}");
                let main = "pub fn main(u: Unsafe) -> Result[(), ()] {
  let _ = lib.each_exploding(u, 3)
  Ok(())
}
";
                let (root, _, executable) = native_demo_target(
                    &format!("parity-failure-{backend:?}-{}", target.triple),
                    main,
                    backend,
                    target.clone(),
                );
                let ran = execute_target(&root, &executable);
                assert!(!ran.status.success(), "{backend:?}: {ran:?}");
                assert!(
                    String::from_utf8_lossy(&ran.stderr)
                        .contains("a callback failed; the process ends"),
                    "{backend:?}: {ran:?}"
                );
            }
        }
    });
}

/// Source conformance cannot execute C. This fixture tests the actual C ABI,
/// including zero bits at each inward pointer boundary and both target ABIs.
#[test]
fn native_null_pointer_boundaries_match_on_both_backends() {
    with_compiler_stack(|| {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let c = std::fs::read_to_string(repo.join("native-tests/foreign/null.c")).unwrap();
        let library =
            std::fs::read_to_string(repo.join("conformance/PROG/PROG-4-ffi-null-api-ok.hir"))
                .unwrap();
        let cases = [
            ("null_roundtrips", None),
            ("null_result", Some("required_pointer")),
            ("null_function_result", Some("required_function")),
            ("null_out", Some("required_out")),
            ("null_function_out", Some("required_function_out")),
            ("null_callback", Some("required_visit")),
            ("null_function_callback", Some("required_function_visit")),
            ("nested_null_callback", Some("nested_visit")),
        ];
        let mut main = String::from(
            "pub fn main(u: Unsafe, args: Args) -> Result[(), ()] {\n  match arg.len(args) {\n",
        );
        for (index, (function, violation)) in cases.iter().enumerate() {
            if violation.is_none() {
                main.push_str(&format!(
                    "    {index} => if lib.{function}(u) {{ Ok(()) }} else {{ Error(()) }},\n"
                ));
            } else {
                main.push_str(&format!(
                    "    {index} => {{ lib.{function}(u); Ok(()) }},\n"
                ));
            }
        }
        main.push_str("    _ => Error(()),\n  }\n}\n");
        for target in native::TargetSpec::executed() {
            for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
                let wrappers = cases
                    .iter()
                    .map(|(name, _)| {
                        json!({
                            "module":[], "name":name, "kind":"pure", "model":"none"
                        })
                    })
                    .collect();
                let (root, _, executable) = native_c_fixture(
                    &format!("ffi-null-{backend:?}-{}", target.triple),
                    &main,
                    backend,
                    target.clone(),
                    &c,
                    &library,
                    wrappers,
                );
                for (index, (function, violation)) in cases.iter().enumerate() {
                    // Standalone launch takes Args from trailing process arguments.
                    let mut command = if target.triple.starts_with("aarch64") {
                        let mut command = std::process::Command::new(
                            std::env::var_os("HIRSEL_QEMU_AARCH64").unwrap(),
                        );
                        command
                            .arg("-L")
                            .arg(std::env::var_os("HIRSEL_AARCH64_SYSROOT").unwrap())
                            .arg(&executable);
                        command
                    } else {
                        std::process::Command::new(&executable)
                    };
                    let ran = command
                        .arg("--hirsel-grants")
                        .arg(root.join("grants.json"))
                        .args(std::iter::repeat_n("case", index))
                        .output()
                        .unwrap();
                    let stderr = String::from_utf8_lossy(&ran.stderr);
                    if let Some(extern_name) = violation {
                        assert!(
                            !ran.status.success(),
                            "{function}/{backend:?}/{}: {ran:?}",
                            target.triple
                        );
                        assert!(
                            stderr.contains("PROG-4: null at a non-null foreign boundary"),
                            "{function}/{backend:?}: {stderr}"
                        );
                        // Attribution is the exact extern call expression, including callbacks.
                        let function_start = library
                            .lines()
                            .position(|line| line.starts_with(&format!("pub fn {function}(")))
                            .unwrap();
                        let line = library
                            .lines()
                            .enumerate()
                            .skip(function_start)
                            .find_map(|(index, line)| {
                                (line.trim_start().starts_with(&format!("{extern_name}(u,"))
                                    || line.contains(&format!("= {extern_name}(u, 0)")))
                                .then_some(index)
                            })
                            .unwrap()
                            + 1;
                        assert!(stderr.contains(&format!("lib@1/{function}")), "{stderr}");
                        assert!(
                            stderr.contains(&format!("lib.hir:{line}:")),
                            "expected line {line}: {stderr}"
                        );
                    } else {
                        assert_eq!(
                            ran.status.code(),
                            Some(0),
                            "{function}/{backend:?}/{}: {ran:?}",
                            target.triple
                        );
                    }
                }
                std::fs::remove_dir_all(root).unwrap();
            }
        }
    });
}

#[test]
fn dependency_cards_distinguish_modeled_and_unmodeled_foreign_reach() {
    with_compiler_stack(|| {
        for (name, foreign, expected) in [
            ("card-modeled", deterministic_add(), "no_unmodeled_ffi"),
            (
                "card-unmodeled",
                json!({"link":["demo"],"headers_checked":true,"wrappers":[]}),
                "unreplayable",
            ),
        ] {
            let dir = fixture(name, foreign);
            let path = launch(&dir, json!([select_lib()]));
            let report = COMPILER.dependency_cards(&path, Some("lib@1")).unwrap();
            let safety = &report["cards"][0]["determinism"]["reached_by_you"];
            assert_eq!(safety["externs"], json!(["lib@1/demo_add"]));
            assert_eq!(safety["replay"], expected);
            assert_eq!(
                report["cards"][0]["determinism"]["unsafe_ffi_package"],
                true
            );
            std::fs::remove_dir_all(dir).unwrap();
        }
    });
}
