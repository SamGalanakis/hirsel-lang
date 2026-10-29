//! The native edit loop runs a program image in a runner instead of linking an executable
//! (`native/runner.rs`). A run through the runner, forked from its server or as a fresh runner
//! process, and the linked executable agree byte for byte on stdout, stderr, exit status and
//! observation record, on both targets and backends, for consecutive runs of one server. A
//! failure before the program's code ran is a tooling outcome, and a program with a
//! foreign-call surface runs as an executable.
use hirsel_driver::compiler_adapters;
use hirsel_driver::native::{
    Backend, Execution, Launched, Mode, OptLevel, Options, Partition, ReadyProgram, Runnable,
    RuntimeBuild, Tape, TargetSpec, Toolchain, build, counts, emit_backend_objects,
    launch_runnable, link_objects, link_runnable, runner,
};
use hirsel_engine::{
    DefPath, QueryEngine, check_progress, default_context, default_environment, entry_path,
    with_compiler_stack,
};
use std::path::{Path, PathBuf};

/// Output without a final newline, then a failure with its exit report.
const PARTIAL: &str = "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let s = io.stdout(rt)?
  let _ = out.print(s, \"line\\n\")
  let _ = out.print(s, \"partial\")
  Ok(())
}
";
const RUNNERS: &str = "fn half(n: u64) -> Result[u64, String] {
  if n % 2u64 == 1u64 { return Error(\"odd\") }
  Ok(n / 2u64)
}
pub fn main(t: Test) -> Result[(), ()] {
  let worker = test.runner(t)
  failure.check(runner.compute(worker, half, 8u64) == Ok(4u64), Arithmetic())
  failure.check(runner.compute(worker, half, 3u64) == Error(Failed(\"odd\")), Arithmetic())
  Ok(())
}
";
const FAILS: &str = "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let _ = out.print(io.stdout(rt)?, \"before\\n\")
  let xs: List[u8] = [7u8]
  let i: u64 = arg.len(args) + 1
  let x = xs[i]
  Ok(())
}
";
/// Every 128-bit division, remainder and float conversion the backends call a builtin for,
/// on values the optimizer cannot fold; each result's low bit picks a word.
const WIDE: &str = "fn odd(x: u64) -> u64 {
  x % 2u64
}
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let s = io.stdout(rt)?
  let n: u128 = int.widen(arg.len(args) + 7u64)
  let a: u128 = n * 1000000000000000000000000007u128
  let b: u128 = n * 3u128 + 1u128
  let c: i128 = 0i128 - int.widen(arg.len(args) + 1234567890123u64) * 1000000000007i128
  let d: i128 = int.widen(arg.len(args) + 22u64)
  let q: f64 = float.from_int(a / b)
  let r: f32 = float.from_int(a % b)
  let p: f64 = float.from_int(c / d)
  let m: f32 = float.from_int(c % d)
  let u: u64 = int.truncate(a / b)
  let v: u64 = int.truncate(a % b)
  let w: i64 = int.truncate(c / d)
  let k: i64 = int.truncate(c % d)
  let bits = odd(u) + odd(v) * 2u64 + odd(f64.bits(q)) * 4u64 + odd(int.widen(f32.bits(r))) * 8u64
  let more = odd(f64.bits(p)) + odd(int.widen(f32.bits(m))) * 2u64
  let signs = if w < 0i64 && k < 0i64 { \"negative\\n\" } else { \"other\\n\" }
  let _ = out.print(s, if bits % 3u64 == 0u64 { \"three\\n\" } else { \"not three\\n\" })
  let _ = out.print(s, if more == 0u64 { \"even\\n\" } else { \"odd\\n\" })
  let _ = out.print(s, signs)
  Ok(())
}
";
/// Waits forever for a message no task sends: Blocked (CONC-8).
const BLOCKS: &str = "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let (sender, receiver): (Sender[u8], Receiver[u8]) = channel.open(tasks)
  let mut rx = take receiver
  let _ = out.print(io.stdout(rt)?, \"waiting\\n\")
  let _ = channel.receive(tasks, mut rx)
  Ok(())
}
";
const DEEP: &str = "fn deep(n: u64) -> u64 {
  if n == 0u64 { 0u64 } else { deep(n - 1u64) + 1u64 }
}
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let _ = out.print(io.stdout(rt)?, \"deep\\n\")
  let d = deep(arg.len(args) + 100000000u64)
  let _ = out.print(io.stdout(rt)?, if d == 0u64 { \"zero\\n\" } else { \"returned\\n\" })
  Ok(())
}
";
const SPINS: &str = "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let _ = out.print(io.stdout(rt)?, \"spinning\\n\")
  let mut i: u64 = arg.len(args)
  while i < 1u64 {
    i = i * 1u64
  }
  Ok(())
}
";

fn x86() -> TargetSpec {
    TargetSpec::x86_64_unknown_linux_gnu()
}
fn arm() -> TargetSpec {
    TargetSpec::aarch64_unknown_linux_gnu()
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn toolchain() -> Toolchain {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("runner-toolchain");
    Toolchain::in_tree_with_runtimes(
        &dir,
        &[&x86(), &arm()],
        &[
            RuntimeBuild::default(),
            RuntimeBuild {
                diagnostic: true,
                ..Default::default()
            },
        ],
    )
    .unwrap()
}

struct Compiled {
    program: ReadyProgram,
    entry: DefPath,
    objects: Vec<hirsel_lir::stage::ObjectArtifact>,
}

fn compiled(source: &str, target: &TargetSpec, backend: Backend) -> Compiled {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let context = default_context();
    let file = engine.sources.add("runner.hir", source.into()).unwrap();
    let output = check_progress(&mut engine, file, &context).unwrap();
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let input = engine.export_native_input().unwrap();
    let validated = engine.import_native_input(input).unwrap();
    let program = build(&validated, &Options::new(target.clone())).unwrap();
    let opt = match backend {
        Backend::Llvm => OptLevel::Speed,
        Backend::Cranelift => OptLevel::None,
    };
    let objects =
        emit_backend_objects(&program, backend, opt, Partition::Whole, false, None).unwrap();
    Compiled {
        program,
        entry: entry_path(&context),
        objects,
    }
}

fn run(runnable: &Runnable, target: &TargetSpec, toolchain: &Toolchain, dir: &Path) -> Launched {
    launch_runnable(
        runnable,
        target,
        toolchain,
        dir,
        false,
        Tape::Live {
            record: None,
            revocations: None,
        },
    )
    .unwrap()
}

/// What a launch observed, for byte comparison.
fn seen(launched: &Launched) -> (Option<i32>, String, String, String) {
    (
        launched.exit,
        String::from_utf8_lossy(&launched.stdout).into_owned(),
        String::from_utf8_lossy(&launched.stderr).into_owned(),
        launched.record.clone(),
    )
}

#[test]
fn images_run_in_runners_as_their_executables_do_on_both_targets_and_backends() {
    with_compiler_stack(|| {
        let toolchain = toolchain();
        for target in [x86(), arm()] {
            let runner = runner(&toolchain, &target).unwrap();
            assert_eq!(runner.mode, Mode::Fork, "{}", target.triple);
            for backend in [Backend::Llvm, Backend::Cranelift] {
                for (name, source) in [
                    ("partial", PARTIAL),
                    ("runners", RUNNERS),
                    ("fails", FAILS),
                    ("wide", WIDE),
                    ("blocks", BLOCKS),
                    ("deep", DEEP),
                ] {
                    let case = format!("{name}-{}-{backend:?}", target.triple);
                    let dir = scratch(&format!("runner-agree/{case}"));
                    let compiled = compiled(source, &target, backend);
                    let executable = link_objects(
                        &compiled.program,
                        &compiled.entry,
                        &compiled.objects,
                        &toolchain,
                        &dir,
                    )
                    .unwrap();
                    let expected = seen(&run(
                        &Runnable::Executable(executable),
                        &target,
                        &toolchain,
                        &dir,
                    ));
                    let image = link_runnable(
                        &compiled.program,
                        &compiled.entry,
                        &compiled.objects,
                        &toolchain,
                        &dir,
                        None,
                    )
                    .unwrap();
                    let Runnable::Image(_) = &image else {
                        panic!("{case}: linked as {image:?}: {:?}", counts().reasons);
                    };
                    // Consecutive runs of one fork server, then fresh runner processes.
                    for execution in [Execution::Runner, Execution::Runner, Execution::RunnerExec] {
                        let toolchain = Toolchain {
                            execution,
                            ..toolchain.clone()
                        };
                        assert_eq!(
                            seen(&run(&image, &target, &toolchain, &dir)),
                            expected,
                            "{case} under {execution:?}"
                        );
                    }
                }
            }
        }
    });
}

#[test]
fn a_standalone_scalar_preserves_events_with_and_without_dispatch_observers() {
    with_compiler_stack(|| {
        let tools = toolchain();
        for target in [x86(), arm()] {
            for backend in [Backend::Llvm, Backend::Cranelift] {
                let dir = scratch(&format!("scalar-roots/{}-{backend:?}", target.triple));
                let compiled = compiled(PARTIAL, &target, backend);
                let executable = hirsel_driver::native::link_standalone_objects(
                    &compiled.program,
                    &compiled.entry,
                    &compiled.objects,
                    &tools,
                    &dir,
                )
                .unwrap();
                for width in [1, 4] {
                    let grants = format!(
                        r#"{{"io":{{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","dirs":[]}},"placement":{{"cores":{width},"commit":"shared","reorder":"auto"}}}}"#
                    );
                    let mut events = None;
                    for observing in [false, true] {
                        let tape = dir.join(format!("w{width}-{observing}.tape"));
                        let mut command = if target.triple.starts_with("aarch64") {
                            let mut command =
                                std::process::Command::new(tools.qemu_aarch64.as_ref().unwrap());
                            command.arg(&executable);
                            command
                        } else {
                            std::process::Command::new(&executable)
                        };
                        command
                            .env_clear()
                            .env("HIRSEL_RT_WORKERS", width.to_string())
                            .arg("--hirsel-record")
                            .arg(&tape)
                            .args(["--hirsel-grants", &grants]);
                        if observing {
                            command
                                .arg("--hirsel-trace-events")
                                .arg(dir.join(format!("w{width}.jsonl")));
                        }
                        let output = command.output().unwrap();
                        assert!(output.status.success(), "{command:?}: {output:?}");
                        assert_eq!(output.stdout, b"line\npartial");
                        assert!(output.stderr.is_empty(), "{output:?}");
                        let recording = hirsel_rt::launch::parse_recording_bytes(
                            &std::fs::read(&tape).unwrap(),
                        )
                        .unwrap();
                        if let Some(expected) = &events {
                            assert_eq!(&recording.events, expected);
                        } else {
                            events = Some(recording.events);
                        }
                    }
                }
            }
        }
    });
}

fn without_runner_provider(tools: &Toolchain, target: &TargetSpec, dir: &Path) -> Toolchain {
    let archive = dir.join("missing-provider.a");
    let renamed = std::process::Command::new(tools.clang.parent().unwrap().join("llvm-objcopy"))
        .args([
            "--redefine-sym",
            "hirsel.runtime.runners.provider=missing.runner.provider",
        ])
        .arg(tools.archive(&target.triple).unwrap())
        .arg(&archive)
        .output()
        .unwrap();
    assert!(renamed.status.success(), "{renamed:?}");
    let mut tools = tools.clone();
    tools
        .runtime
        .insert((target.triple.clone(), RuntimeBuild::default()), archive);
    tools
}

#[test]
fn a_missing_runtime_feature_provider_refuses_the_link_on_both_targets_and_backends() {
    with_compiler_stack(|| {
        let tools = toolchain();
        for target in [x86(), arm()] {
            let dir = scratch(&format!("runner-missing-provider/{}", target.triple));
            let tools = without_runner_provider(&tools, &target, &dir);
            for backend in [Backend::Llvm, Backend::Cranelift] {
                let compiled = compiled(RUNNERS, &target, backend);
                let error = link_objects(
                    &compiled.program,
                    &compiled.entry,
                    &compiled.objects,
                    &tools,
                    &dir,
                )
                .unwrap_err();
                assert!(
                    error
                        .message
                        .contains("undefined symbol: hirsel.runtime.runners.provider"),
                    "{} {backend:?}: {error:?}",
                    target.triple,
                );
            }
        }
    });
}

/// A `debug_info` toolchain's runner links the debug-bearing archive, so it is a different
/// runner (images name it), and keeps its debug information in the executable.
#[test]
fn a_debug_runner_has_its_own_identity_and_debug_file() {
    with_compiler_stack(|| {
        let plain = toolchain();
        let toolchain = Toolchain {
            debug_info: true,
            ..plain.clone()
        };
        let target = x86();
        let debug = runner(&toolchain, &target).unwrap();
        assert_ne!(debug.build_id, runner(&plain, &target).unwrap().build_id);
        let file = debug.debug.clone().expect("a debug file");
        assert_eq!(file, debug.path);
        assert!(
            file.is_file() && file.parent() == debug.path.parent(),
            "{file:?}"
        );
        assert_eq!(debug.mode, Mode::Fork);
        let dir = scratch("runner-debug");
        let compiled = compiled(PARTIAL, &target, Backend::Cranelift);
        let image = link_runnable(
            &compiled.program,
            &compiled.entry,
            &compiled.objects,
            &toolchain,
            &dir,
            None,
        )
        .unwrap();
        let Runnable::Image(linked) = &image else {
            panic!("linked as {image:?}");
        };
        assert_eq!(linked.runner.build_id, debug.build_id);
        let launched = run(&image, &target, &toolchain, &dir);
        assert_eq!(launched.stdout, b"line\npartial");
    });
}

#[test]
fn a_hung_run_is_killed_with_its_group_and_its_server_serves_the_next_run() {
    with_compiler_stack(|| {
        let mut toolchain = toolchain();
        toolchain.launch_timeout = std::time::Duration::from_secs(3);
        let target = x86();
        let dir = scratch("runner-timeout");
        let spins = compiled(SPINS, &target, Backend::Llvm);
        let image = link_runnable(
            &spins.program,
            &spins.entry,
            &spins.objects,
            &toolchain,
            &dir,
            None,
        )
        .unwrap();
        let error = launch_runnable(
            &image,
            &target,
            &toolchain,
            &dir,
            false,
            Tape::Live {
                record: None,
                revocations: None,
            },
        )
        .unwrap_err();
        assert!(error.message.contains("launch timeout"), "{error}");
        let partial = compiled(PARTIAL, &target, Backend::Llvm);
        let image = link_runnable(
            &partial.program,
            &partial.entry,
            &partial.objects,
            &toolchain,
            &dir,
            None,
        )
        .unwrap();
        assert_eq!(
            run(&image, &target, &toolchain, &dir).stdout,
            b"line\npartial"
        );
    });
}

#[test]
fn failures_before_the_program_runs_are_tooling_outcomes() {
    with_compiler_stack(|| {
        let toolchain = toolchain();
        let target = x86();
        let dir = scratch("runner-tooling");
        let partial = compiled(PARTIAL, &target, Backend::Llvm);
        let Runnable::Image(image) = link_runnable(
            &partial.program,
            &partial.entry,
            &partial.objects,
            &toolchain,
            &dir,
            None,
        )
        .unwrap() else {
            panic!("expected an image");
        };
        let diagnostic = runner(
            &toolchain.with_runtime(RuntimeBuild {
                diagnostic: true,
                ..Default::default()
            }),
            &target,
        )
        .unwrap();
        assert_ne!(diagnostic.build_id, image.runner.build_id);
        // Cut inside the last loadable segment: what follows it (symbols, strings) is not
        // loaded, so a cut there would leave the image intact.
        let truncated = dir.join("truncated.so");
        let bytes = std::fs::read(&image.path).unwrap();
        let word = |at: usize, n: usize| {
            let mut b = [0u8; 8];
            b[..n].copy_from_slice(&bytes[at..at + n]);
            u64::from_le_bytes(b) as usize
        };
        let (phoff, phentsize, phnum) = (word(32, 8), word(54, 2), word(56, 2));
        let end = (0..phnum)
            .map(|i| phoff + i * phentsize)
            .filter(|&h| word(h, 4) == 1)
            .map(|h| word(h + 8, 8) + word(h + 32, 8))
            .max()
            .unwrap();
        std::fs::write(&truncated, &bytes[..end - 1]).unwrap();
        // Intact, but typed an executable, which the loader refuses to load.
        let executable = dir.join("executable.so");
        let mut typed = bytes.clone();
        typed[16] = 2;
        std::fs::write(&executable, typed).unwrap();
        for execution in [Execution::Runner, Execution::RunnerExec] {
            let toolchain = Toolchain {
                execution,
                ..toolchain.clone()
            };
            // Another runner's image is refused before it is loaded.
            let foreign = Runnable::Image(hirsel_driver::native::Image {
                runner: diagnostic.clone(),
                ..image.clone()
            });
            // A damaged image fails to load.
            let damaged = Runnable::Image(hirsel_driver::native::Image {
                path: truncated.clone(),
                ..image.clone()
            });
            let refused = Runnable::Image(hirsel_driver::native::Image {
                path: executable.clone(),
                ..image.clone()
            });
            for (runnable, reason) in [
                (foreign, "linked for another runner"),
                (damaged, "a segment lies past its end"),
                (refused, "dlopen"),
            ] {
                let error = launch_runnable(
                    &runnable,
                    &target,
                    &toolchain,
                    &dir,
                    false,
                    Tape::Live {
                        record: None,
                        revocations: None,
                    },
                )
                .unwrap_err();
                assert!(error.message.contains("Tooling(ImageLoad)"), "{error}");
                assert!(error.message.contains(reason), "{execution:?}: {error}");
            }
        }
    });
}

#[test]
fn programs_that_call_c_run_as_executables() {
    with_compiler_stack(|| {
        let dir = scratch("runner-foreign");
        // `unsafe_ffi` with no library: the externs bind to libc.
        std::fs::write(
            dir.join("main.hir"),
            "/// Reads nothing of Hirsel's and retains nothing.
extern \"c\" fn getpid(u: Unsafe) -> i32

pub fn main(rt: Io, u: Unsafe) -> Result[(), GrantError] {
  let _ = getpid(u)
  let _ = out.print(io.stdout(rt)?, \"called\\n\")
  Ok(())
}
",
        )
        .unwrap();
        let manifest = serde_json::json!({"schema":1,"package":{"name":"app","version":"1"},
            "entry":{"module":[],"name":"main"},"sources":[{"path":"main.hir","module":[]}],
            "dependencies":[],
            "unsafe_ffi":{"link":[],"headers_checked":false,"wrappers":[]}});
        std::fs::write(dir.join("hirsel.json"), manifest.to_string()).unwrap();
        let mut loaded = hirsel_driver::load_program(&dir.join("hirsel.json")).unwrap();
        loaded.check_for_execution().unwrap();
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let target = x86();
        let program = build(&validated, &Options::new(target.clone())).unwrap();
        let objects = emit_backend_objects(
            &program,
            Backend::Llvm,
            OptLevel::Speed,
            Partition::Whole,
            false,
            None,
        )
        .unwrap();
        let toolchain = toolchain();
        let before = counts().fallbacks.get("foreign").copied().unwrap_or(0);
        let runnable =
            link_runnable(&program, &loaded.entry, &objects, &toolchain, &dir, None).unwrap();
        assert!(matches!(runnable, Runnable::Executable(_)), "{runnable:?}");
        assert_eq!(counts().fallbacks.get("foreign"), Some(&(before + 1)));
        assert_eq!(
            run(&runnable, &target, &toolchain, &dir).stdout,
            b"called\n"
        );
        let missing = without_runner_provider(&toolchain, &target, &dir);
        let error = link_objects(&program, &loaded.entry, &objects, &missing, &dir).unwrap_err();
        assert!(
            error
                .message
                .contains("undefined symbol: hirsel.runtime.runners.provider"),
            "{error:?}",
        );
    });
}

#[test]
fn ordinary_products_restore_runners_and_images_without_path_or_json_authority() {
    with_compiler_stack(|| {
        let root = tempfile::tempdir().unwrap();
        let products = root.path().join("products");
        let mut tools = toolchain();
        tools.cache = root.path().join("materialized");
        tools.products = std::sync::Arc::new(hirsel_engine::store::DirStore::new(products.clone()));
        let target = x86();
        let first = runner(&tools, &target).unwrap();
        let expected_runner = std::fs::read(&first.path).unwrap();
        std::fs::write(&first.path, b"truncated runner").unwrap();
        tools.products = std::sync::Arc::new(hirsel_engine::store::DirStore::new(products.clone()));
        let restored = runner(&tools, &target).unwrap();
        assert_eq!(restored.build_id, first.build_id);
        assert_eq!(restored.mode, first.mode);
        assert_eq!(std::fs::read(&restored.path).unwrap(), expected_runner);
        assert!(!restored.path.parent().unwrap().join("runner.json").exists());
        let dir = root.path().join("launch");
        std::fs::create_dir_all(&dir).unwrap();
        let compiled = compiled(PARTIAL, &target, Backend::Cranelift);
        let image = link_runnable(
            &compiled.program,
            &compiled.entry,
            &compiled.objects,
            &tools,
            &dir,
            None,
        )
        .unwrap();
        let Runnable::Image(first_image) = &image else {
            panic!("expected the actual runner image");
        };
        assert!(!first_image.cached);
        let expected_image = std::fs::read(&first_image.path).unwrap();
        let expected = seen(&run(&image, &target, &tools, &dir));
        std::fs::remove_file(&first_image.path).unwrap();
        std::fs::remove_file(&restored.path).unwrap();
        tools.products = std::sync::Arc::new(hirsel_engine::store::DirStore::new(products));
        let next = link_runnable(
            &compiled.program,
            &compiled.entry,
            &compiled.objects,
            &tools,
            &dir,
            None,
        )
        .unwrap();
        let Runnable::Image(next_image) = &next else {
            panic!("restored product must remain an image");
        };
        assert!(
            next_image.cached,
            "the stored product restores a missing materialization"
        );
        assert_eq!(std::fs::read(&next_image.path).unwrap(), expected_image);
        assert_eq!(
            std::fs::read(&next_image.runner.path).unwrap(),
            expected_runner
        );
        assert_eq!(seen(&run(&next, &target, &tools, &dir)), expected);
        // Same materialized files under a fresh provider do not constitute hits.
        tools.products = std::sync::Arc::new(hirsel_engine::store::MemStore::default());
        let recomputed = link_runnable(
            &compiled.program,
            &compiled.entry,
            &compiled.objects,
            &tools,
            &dir,
            None,
        )
        .unwrap();
        let Runnable::Image(recomputed) = recomputed else {
            panic!("recomputation must remain an image");
        };
        assert!(!recomputed.cached);
    });
}
