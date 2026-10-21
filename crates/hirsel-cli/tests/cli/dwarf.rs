//! Line tables and debugger lookup are object-format behavior, outside the source corpus.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

// Keep a bad generated executable or a malformed-DWARF reader from holding the
// pool test for its whole harness timeout. Drain both pipes while it runs.
fn output(command: &mut Command) -> std::process::Output {
    use std::io::Read;
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let stdout = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).unwrap();
        bytes
    });
    let stderr = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).unwrap();
        bytes
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            child.kill().unwrap();
            break child.wait().unwrap();
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let output = std::process::Output {
        status,
        stdout: stdout.join().unwrap(),
        stderr: stderr.join().unwrap(),
    };
    assert!(!timed_out, "{command:?} exceeded 30 seconds: {output:?}");
    output
}

fn tool(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var_os("LLVM_SYS_211_PREFIX").unwrap())
        .join("bin")
        .join(name)
}

fn dump(exe: &Path, argument: &str) -> String {
    let output = output(Command::new(tool("llvm-dwarfdump")).arg(argument).arg(exe));
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap()
}

/// A `--debug` executable's split debug information: `<build-id>.debug` beside
/// it, which its `.gnu_debuglink` names.
pub(crate) fn debug_file(exe: &Path) -> std::path::PathBuf {
    let notes = output(Command::new(tool("llvm-readelf")).arg("--notes").arg(exe));
    let notes = String::from_utf8(notes.stdout).unwrap();
    let id = notes
        .split("Build ID: ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("no build ID: {notes}"));
    let debug = exe.with_file_name(format!("{id}.debug"));
    let link = output(
        Command::new(tool("llvm-readelf"))
            .arg("--string-dump=.gnu_debuglink")
            .arg(exe),
    );
    assert!(
        String::from_utf8_lossy(&link.stdout).contains(&format!("{id}.debug")),
        "{link:?}"
    );
    debug
}

fn function_lines(exe: &Path, source: &Path) -> BTreeMap<String, BTreeSet<u64>> {
    // The executable itself carries no line table; its debug file does.
    assert!(!dump(exe, "--debug-line").contains(source.file_name().unwrap().to_str().unwrap()));
    let exe = &debug_file(exe);
    let text = dump(exe, "--debug-line");
    assert!(
        text.contains(source.file_name().unwrap().to_str().unwrap()),
        "{text}"
    );
    let mut result: BTreeMap<String, BTreeSet<u64>> = BTreeMap::new();
    let mut functions: Vec<(u64, u64, String)> = Vec::new();
    let file = source.file_name().unwrap().to_str().unwrap();
    for row in text
        .split("debug_line[")
        .filter(|table| table.contains(file))
        .flat_map(|table| {
            // A Cranelift CU can contain several source files. Select rows by
            // their file index, rather than every row in a matching table.
            let files: BTreeSet<u64> = table
                .split("file_names[")
                .skip(1)
                .filter_map(|entry| {
                    let (index, entry) = entry.split_once("]:")?;
                    let name = entry.split("name:").nth(1)?.split('"').nth(1)?;
                    (Path::new(name).file_name()?.to_str() == Some(file))
                        .then(|| index.trim().parse().ok())?
                })
                .collect();
            table.lines().filter(move |row| {
                row.split_whitespace()
                    .nth(3)
                    .and_then(|index| index.parse().ok())
                    .is_some_and(|index| files.contains(&index))
            })
        })
        .filter(|row| row.starts_with("0x") && !row.contains("end_sequence") && !row.ends_with(']'))
    {
        let mut fields = row.split_whitespace();
        let address = fields.next().unwrap();
        let line: u64 = fields.next().unwrap().parse().unwrap();
        if line == 0 {
            continue;
        }
        let pc = u64::from_str_radix(address.trim_start_matches("0x"), 16).unwrap();
        let name = if let Some((_, _, name)) = functions
            .iter()
            .find(|(low, high, _)| (*low..*high).contains(&pc))
        {
            name.clone()
        } else {
            let lookup = dump(exe, &format!("--lookup={address}"));
            let subprogram = lookup
                .split("DW_TAG_subprogram")
                .nth(1)
                .unwrap_or_else(|| panic!("no subprogram at {address}: {lookup}"));
            let name = subprogram
                .lines()
                .find(|line| line.contains("DW_AT_name"))
                .and_then(|line| line.split('"').nth(1))
                .unwrap_or_else(|| panic!("{subprogram}"));
            assert!(subprogram.contains(file), "{subprogram}");
            assert!(
                subprogram.contains(&format!(", line {line},")),
                "{subprogram}"
            );
            let bound = |attribute| {
                let value = subprogram
                    .lines()
                    .find(|line| line.contains(attribute))
                    .and_then(|line| line.split("(0x").nth(1))
                    .and_then(|line| line.split(')').next())
                    .unwrap_or_else(|| panic!("{subprogram}"));
                u64::from_str_radix(value, 16).unwrap()
            };
            let (low, high) = (bound("DW_AT_low_pc"), bound("DW_AT_high_pc"));
            assert!((low..high).contains(&pc), "{subprogram}");
            functions.push((low, high, name.to_owned()));
            name.to_owned()
        };
        result.entry(name).or_default().insert(line);
    }
    assert!(
        !result.is_empty(),
        "debugger could not resolve lines: {text}"
    );
    result
}

#[test]
fn cranelift_debug_lines_match_llvm_per_function_on_both_targets() {
    hirsel_engine::with_compiler_stack(|| {
        for program in [
            "fn step(x: u64) -> u64 {\n  x + 1u64\n}\npub fn main() -> Result[(), ()] {\n  let n = step(3u64)\n  failure.check(n == 4u64, Arithmetic())\n  Ok(())\n}\n",
            "fn step(x: u64) -> u64 {\n  if x == 0u64 {\n    7u64\n  } else {\n    x + 1u64\n  }\n}\npub fn main() -> Result[(), ()] {\n  let n = step(0u64)\n  failure.check(n == 7u64, Arithmetic())\n  Ok(())\n}\n",
            "fn step(x: u64) -> u64 {\n  let mut n = x\n  for i in 0u64..3u64 {\n    n = n + i\n  }\n  n\n}\npub fn main() -> Result[(), ()] {\n  let n = step(0u64)\n  failure.check(n == 3u64, Arithmetic())\n  Ok(())\n}\n",
        ] {
            let source = super::file(program);
            let mut loaded = hirsel_driver::load_program(&source).unwrap();
            loaded.check_for_execution().unwrap();
            let input = loaded.engine.export_native_input().unwrap();
            let sources = hirsel_driver::native::profile_sources(&input);
            let validated = loaded.engine.import_native_input(input).unwrap();
            for target in ["x86_64-linux", "aarch64-linux"] {
                use hirsel_driver::native::{
                    self, Backend, OptLevel, Options, Partition, TargetSpec,
                };
                let target_spec = if target == "aarch64-linux" {
                    TargetSpec::aarch64_unknown_linux_gnu()
                } else {
                    TargetSpec::x86_64_unknown_linux_gnu()
                };
                let ready = native::build(&validated, &Options::new(target_spec)).unwrap();
                let mut expected = None;
                for backend in ["llvm", "cranelift"] {
                    let objects = native::emit_backend_objects(
                        &ready,
                        if backend == "llvm" {
                            Backend::Llvm
                        } else {
                            Backend::Cranelift
                        },
                        OptLevel::None,
                        Partition::Whole,
                        false,
                        Some(&sources),
                    )
                    .unwrap();
                    assert!(!objects.is_empty());
                    for object in objects {
                        let path = source.with_extension("o");
                        std::fs::write(&path, object.bytes).unwrap();
                        let verified = dump(&path, "--verify");
                        assert!(verified.contains("No errors."), "{verified}");
                        std::fs::remove_file(path).unwrap();
                    }
                    let exe = source.with_extension(format!("{backend}-{target}-exe"));
                    let built = output(
                        Command::new(env!("CARGO_BIN_EXE_hirsel"))
                            .args(["build", "--debug", "--backend", backend, "--target", target])
                            .arg(&source)
                            .arg("-o")
                            .arg(&exe),
                    );
                    assert!(
                        built.status.success(),
                        "{backend} {target}: {program}\n{built:?}"
                    );
                    let lines = function_lines(&exe, &source);
                    assert_eq!(lines.len(), 2, "{backend} {target}: {lines:?}");
                    assert!(
                        lines.keys().any(|name| name.ends_with("/main")),
                        "{lines:?}"
                    );
                    assert!(
                        lines.keys().any(|name| name.ends_with("/step")),
                        "{lines:?}"
                    );
                    if let Some(expected) = &expected {
                        assert_eq!(&lines, expected, "{target}: {program}");
                    } else {
                        expected = Some(lines);
                    }
                    let output = if target == "aarch64-linux" {
                        output(
                            Command::new(std::env::var_os("HIRSEL_QEMU_AARCH64").unwrap())
                                .arg("-L")
                                .arg(std::env::var_os("HIRSEL_AARCH64_SYSROOT").unwrap())
                                .arg(&exe),
                        )
                    } else {
                        output(&mut Command::new(&exe))
                    };
                    assert_eq!(output.status.code(), Some(0), "{output:?}");
                    std::fs::remove_file(debug_file(&exe)).unwrap();
                    std::fs::remove_file(exe).unwrap();
                }
            }
            std::fs::remove_file(source).unwrap();
        }
    });
}

#[test]
fn manifest_artifacts_are_identical_under_different_source_roots() {
    hirsel_engine::with_compiler_stack(|| {
        let root = super::file("");
        std::fs::remove_file(&root).unwrap();
        std::fs::create_dir(&root).unwrap();
        let roots = [root.join("one"), root.join("different-root")];
        for tree in &roots {
            std::fs::create_dir_all(tree.join("app/src")).unwrap();
            std::fs::create_dir_all(tree.join("dep/src")).unwrap();
            std::fs::write(tree.join("app/src/main.hir"), "pub fn main() -> Result[(), ()] { let _ = helper@2/generated(); let _ = helper@2/value(); Ok(()) }\n").unwrap();
            std::fs::write(tree.join("dep/src/helper.hir"), r#"fn generator() -> Result[Body[fn() -> u64 uses {}], String] { Ok(codegen.body("fn() -> u64 uses {} { 3u64 }")) }
pub fn generated() -> u64 uses {} = comptime generator()
pub fn value() -> u64 { failure.check(false, Arithmetic()); 0u64 }
"#).unwrap();
            std::fs::write(
                tree.join("dep/hirsel.json"),
                serde_json::json!({
                    "schema":1, "package":{"name":"helper","version":"2"},
                    "sources":[{"path":"src/helper.hir","module":[]}], "dependencies":[]
                })
                .to_string(),
            )
            .unwrap();
            std::fs::write(tree.join("app/hirsel.json"), serde_json::json!({
                "schema":1, "package":{"name":"relocated","version":"1"},
                "entry":{"module":[],"name":"main"},
                "sources":[{"path":"src/main.hir","module":[]}],
                "dependencies":[{"manifest":"../dep/hirsel.json", "package":{"name":"helper","version":"2"}, "expose":[]}]
            }).to_string()).unwrap();
        }
        for backend in ["llvm", "cranelift"] {
            for debug in [false, true] {
                let mut artifacts = Vec::new();
                for tree in &roots {
                    let manifest = tree.join("app/hirsel.json");
                    let flags = if debug {
                        vec!["--backend", backend, "--debug"]
                    } else {
                        vec!["--backend", backend]
                    };
                    let exe = super::build_executable(&manifest, &flags);
                    let mut files = vec![std::fs::read(&exe).unwrap()];
                    if debug {
                        let split = debug_file(&exe);
                        // Linking tombstones discarded Rust-runtime code at address
                        // zero. Check our linked address-to-source lookup here;
                        // compiler-owned objects are verified separately below.
                        let lines = function_lines(&exe, &tree.join("dep/src/helper.hir"));
                        assert!(lines.contains_key("helper@2/value"), "{lines:?}");
                        let info = dump(&split, "--debug-info");
                        assert!(info.contains("helper@2"), "{info}");
                        files.push(std::fs::read(split).unwrap());
                    }
                    // Inspect the artifacts themselves: dwarfdump prints the
                    // physical output filename in its header, outside DWARF.
                    for bytes in &files {
                        for tree in &roots {
                            assert!(
                                !bytes
                                    .windows(tree.as_os_str().len())
                                    .any(|w| w == tree.to_str().unwrap().as_bytes()),
                                "{backend}: source root in artifact"
                            );
                        }
                    }
                    let run = output(&mut Command::new(&exe));
                    assert_eq!(run.status.code(), Some(2), "{run:?}");
                    let failure = String::from_utf8(run.stderr).unwrap();
                    assert!(failure.contains("helper@2/src/helper.hir"), "{failure}");
                    artifacts.push(files);
                }
                assert!(
                    artifacts[0] == artifacts[1],
                    "{backend} debug={debug}: artifact bytes differ across roots"
                );
            }
        }
        // Measurement emits ProfileSite.file in rodata as well as DWARF.
        // Compare actual LLVM objects, including generated-body source ownership.
        let mut measured = Vec::new();
        for tree in &roots {
            use hirsel_driver::native::{self, Backend, OptLevel, Options, Partition, TargetSpec};
            // Hosts have independent compiler sessions; neither build may
            // adopt source products from the other root's package cache.
            // Lane pkgcache-comptime owns generated token sources on cache adoption.
            let mut loaded = hirsel_driver::load_program_source_with_images(
                &tree.join("app/hirsel.json"),
                None,
                &hirsel_engine::StdImages::none(),
            )
            .unwrap();
            loaded.check_for_execution().unwrap();
            let generated = loaded
                .engine
                .generated_sources()
                .find(|(definition, _)| definition.0.name == "generated")
                .unwrap()
                .1;
            let source = loaded.engine.sources.file(generated.file).unwrap();
            assert!(
                source
                    .logical_path()
                    .to_str()
                    .unwrap()
                    .starts_with("helper@2/src/helper.hir#")
            );
            let input = loaded.engine.export_native_input().unwrap();
            let sources = native::profile_sources(&input);
            let validated = loaded.engine.import_native_input(input).unwrap();
            let ready = native::build(
                &validated,
                &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
            )
            .unwrap();
            let objects = native::emit_backend_objects(
                &ready,
                Backend::Llvm,
                OptLevel::None,
                Partition::Whole,
                true,
                Some(&sources),
            )
            .unwrap();
            let bytes: Vec<_> = objects.into_iter().map(|o| o.bytes).collect();
            for (index, object) in bytes.iter().enumerate() {
                let path = tree.join(format!("measured-{index}.o"));
                std::fs::write(&path, object).unwrap();
                let verified = dump(&path, "--verify");
                assert!(verified.contains("No errors."), "{verified}");
            }
            assert!(bytes.iter().any(|b| {
                b.windows(b"helper@2/src/helper.hir".len())
                    .any(|w| w == b"helper@2/src/helper.hir")
            }));
            measured.push(bytes);
        }
        assert!(
            measured[0] == measured[1],
            "measurement object bytes differ across roots"
        );
        std::fs::remove_dir_all(root).unwrap();
    });
}

/// Cached code carries item-relative ranges. Each link must resolve those
/// ranges in the current file after line insertion and a cross-file move.
#[test]
fn cached_debug_lines_after_insertion_and_a_file_move_match_fresh_emission() {
    let root = super::file("");
    std::fs::remove_file(&root).unwrap();
    std::fs::create_dir(&root).unwrap();
    let manifest = root.join("hirsel.json");
    let helper = root.join("helper.hir");
    let moved = root.join("moved.hir");
    // Trivia belongs to the following item. Keep helper's owned text unchanged
    // by inserting a line before an unused constant that precedes it.
    let body = "const spacer:u64 = 1u64\npub fn helper(x:u64)->u64 { x + 1u64 }\n";
    std::fs::write(root.join("main.hir"), "pub fn main()->Result[(),()] { failure.check(helper(3u64)==4u64, Arithmetic()); Ok(()) }\n").unwrap();
    std::fs::write(&manifest, serde_json::json!({
        "schema":1, "package":{"name":"moving","version":"1"},
        "entry":{"module":[],"name":"main"},
        "sources":[{"path":"main.hir","module":[]},{"path":"helper.hir","module":[]},{"path":"moved.hir","module":[]}],
        "dependencies":[]
    }).to_string()).unwrap();
    for backend in ["llvm", "cranelift"] {
        std::fs::write(&helper, body).unwrap();
        std::fs::write(&moved, "").unwrap();
        let exe = root.join(format!("{backend}-program"));
        let build = |store| {
            let built = output(
                Command::new(env!("CARGO_BIN_EXE_hirsel"))
                    .env("HIRSEL_DAEMON", "off")
                    .env("HIRSEL_RESULTS", "none@1")
                    .env("HIRSEL_STD_OBJECTS", "none")
                    .env("HIRSEL_OBJECTS", store)
                    .env("HIRSEL_JOBS", "1")
                    .env("XDG_CACHE_HOME", root.join(backend))
                    .args(["build", "--debug", "--timings=json", "--backend", backend])
                    .arg(&manifest)
                    .arg("-o")
                    .arg(&exe),
            );
            assert!(built.status.success(), "{backend}: {built:?}");
            let stderr = std::str::from_utf8(&built.stderr).unwrap();
            serde_json::from_str::<serde_json::Value>(stderr.lines().last().unwrap()).unwrap()
        };
        let cold = build("dir@1");
        assert!(cold["counts"]["object misses"].as_u64().unwrap() > 0);
        for file_move in [false, true] {
            let current = if file_move { &moved } else { &helper };
            std::fs::write(&helper, "").unwrap();
            std::fs::write(current, format!("\n{body}")).unwrap();
            let warm = build("dir@1");
            assert!(
                warm["counts"]["object hits"].as_u64().unwrap() > 0,
                "{warm}"
            );
            assert_eq!(warm["counts"]["object misses"], 0, "{warm}");
            let cached_lines = function_lines(&exe, current);
            assert!(
                cached_lines["moving@1/helper"].contains(&3),
                "{cached_lines:?}"
            );
            let cached = std::fs::read(debug_file(&exe)).unwrap();
            build("none@1");
            assert_eq!(
                cached,
                std::fs::read(debug_file(&exe)).unwrap(),
                "{backend} file_move={file_move}"
            );
            assert_eq!(cached_lines, function_lines(&exe, current));
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
