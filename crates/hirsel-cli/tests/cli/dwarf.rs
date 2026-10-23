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
    std::path::PathBuf::from(std::env::var_os("LLVM_SYS_231_PREFIX").unwrap())
        .join("bin")
        .join(name)
}

fn dump(exe: &Path, argument: &str) -> String {
    let output = output(Command::new(tool("llvm-dwarfdump")).arg(argument).arg(exe));
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap()
}

// Standalone link-time debug objects import their function symbols. Their zero
// addresses are placeholders, so dwarfdump cannot check range overlap until a
// link resolves them. Check the relocations first, then verify a minimal link
// without the runtime's discarded debug ranges or any code folding/collection.
fn object_details(path: &Path) -> serde_json::Value {
    let result = output(
        Command::new(tool("llvm-readobj"))
            .args([
                "--elf-output-style=JSON",
                "--sections",
                "--symbols",
                "--relocations",
            ])
            .arg(path),
    );
    assert!(result.status.success(), "{result:?}");
    let mut files: Vec<serde_json::Value> = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(files.len(), 1);
    files.remove(0)
}

fn subprograms(path: &Path) -> BTreeMap<String, (String, u64, u64)> {
    let info = dump(path, "--debug-info");
    info.split("DW_TAG_subprogram")
        .skip(1)
        .map(|entry| {
            let attribute = |name| {
                entry
                    .lines()
                    .find(|line| line.contains(name))
                    .unwrap_or_else(|| panic!("missing {name}: {entry}"))
            };
            let name = attribute("DW_AT_name")
                .split('"')
                .nth(1)
                .unwrap()
                .to_owned();
            let symbol = attribute("DW_AT_linkage_name")
                .split('"')
                .nth(1)
                .unwrap()
                .to_owned();
            let address = |name| {
                let value = attribute(name)
                    .split("(0x")
                    .nth(1)
                    .unwrap()
                    .split(')')
                    .next()
                    .unwrap();
                u64::from_str_radix(value, 16).unwrap()
            };
            (
                symbol,
                (name, address("DW_AT_low_pc"), address("DW_AT_high_pc")),
            )
        })
        .collect()
}

fn verify_debug_objects(paths: &[std::path::PathBuf], linked: &Path) {
    let details: Vec<_> = paths.iter().map(|path| object_details(path)).collect();
    let definitions: BTreeMap<_, _> = details
        .iter()
        .flat_map(|file| {
            file["Symbols"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| &s["Symbol"])
                .filter(|s| s["Type"]["Name"] == "Function" && s["Section"]["Name"] != "Undefined")
                .map(|s| {
                    (
                        s["Name"]["Name"].as_str().unwrap().to_owned(),
                        s["Size"].as_u64().unwrap(),
                    )
                })
        })
        .collect();
    let mut expected = BTreeMap::new();
    for (path, file) in paths.iter().zip(&details) {
        let functions = subprograms(path);
        for (symbol, (_, low, high)) in &functions {
            let imported = file["Symbols"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| &s["Symbol"])
                .find(|s| s["Name"]["Name"] == symbol.as_str())
                .unwrap();
            assert_eq!(imported["Section"]["Name"], "Undefined");
            // ELF permits STT_NOTYPE for an undefined function import. Its
            // definition above must still be a function of the exact size.
            assert!(matches!(
                imported["Type"]["Name"].as_str(),
                Some("Function" | "None")
            ));
            assert_eq!(*low, 0, "an imported function's unlinked address: {symbol}");
            assert_eq!(
                definitions.get(symbol),
                Some(high),
                "debug/code size: {symbol}"
            );
        }
        let mut relocations = BTreeMap::<_, usize>::new();
        for section in file["Relocations"].as_array().unwrap() {
            let index = section["SectionIndex"].as_u64().unwrap();
            let name = file["Sections"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| &s["Section"])
                .find(|s| s["Index"] == index)
                .unwrap()["Name"]["Name"]
                .as_str()
                .unwrap();
            if !name.starts_with(".rela.debug_") {
                continue;
            }
            for relocation in section["Relocs"].as_array().unwrap() {
                let relocation = &relocation["Relocation"];
                let symbol = relocation["Symbol"]["Name"].as_str().unwrap();
                if functions.contains_key(symbol) {
                    assert!(
                        matches!(
                            relocation["Type"]["Name"].as_str(),
                            Some("R_X86_64_64" | "R_AARCH64_ABS64")
                        ),
                        "{relocation}"
                    );
                    assert_eq!(relocation["Addend"], 0, "{relocation}");
                    *relocations.entry((name, symbol)).or_default() += 1;
                }
            }
        }
        for (symbol, (_, low, high)) in &functions {
            for section in [".rela.debug_info", ".rela.debug_rnglists"] {
                assert_eq!(
                    relocations.get(&(section, symbol.as_str())),
                    Some(&1),
                    "{path:?}: {symbol} {section}"
                );
            }
            if high > low {
                assert!(
                    relocations.contains_key(&(".rela.debug_line", symbol.as_str())),
                    "no line relocation for {symbol}"
                );
            }
        }
        expected.extend(functions);
    }
    assert!(!expected.is_empty(), "no debug functions in {paths:?}");
    // Only runtime calls may remain unresolved in this non-runnable artifact:
    // every debug function was required to have a sized code definition above.
    let result = output(
        Command::new(tool("ld.lld"))
            .args([
                "--entry=0",
                "--build-id=none",
                "--no-gc-sections",
                "--icf=none",
                "--unresolved-symbols=ignore-all",
            ])
            .args(paths)
            .arg("-o")
            .arg(linked),
    );
    assert!(result.status.success(), "{result:?}");
    let verified = dump(linked, "--verify");
    assert!(verified.contains("No errors."), "{verified}");
    let actual = subprograms(linked);
    assert_eq!(actual.len(), expected.len());
    let symbols = object_details(linked);
    for (symbol, (name, _, size)) in expected {
        let (actual_name, low, high) = actual.get(&symbol).unwrap();
        let code = symbols["Symbols"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| &s["Symbol"])
            .find(|s| s["Name"]["Name"] == symbol)
            .unwrap();
        assert_eq!(actual_name, &name);
        assert_eq!(
            *low,
            code["Value"].as_u64().unwrap(),
            "range owner: {symbol}"
        );
        assert_eq!(*high - *low, size, "range size: {symbol}");
        let lookup = dump(linked, &format!("--lookup={low:#x}"));
        assert!(lookup.contains(&symbol), "{lookup}");
    }
}

/// Debug builds keep DWARF in the executable and need no sidecar.
pub(crate) fn debug_file(exe: &Path) -> std::path::PathBuf {
    let sections = output(
        Command::new(tool("llvm-readelf"))
            .arg("--sections")
            .arg(exe),
    );
    assert!(sections.status.success(), "{sections:?}");
    let text = String::from_utf8(sections.stdout).unwrap();
    assert!(
        text.contains(".debug_info") && text.contains(".debug_line"),
        "{text}"
    );
    assert!(!text.contains(".gnu_debuglink"), "{text}");
    exe.to_owned()
}

fn function_lines(exe: &Path, source: &Path) -> BTreeMap<String, BTreeSet<u64>> {
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
                    let paths: Vec<_> = objects
                        .iter()
                        .enumerate()
                        .map(|(index, object)| {
                            let path = source.with_extension(format!("{index}.o"));
                            std::fs::write(&path, &object.bytes).unwrap();
                            path
                        })
                        .collect();
                    let linked = source.with_extension("debug-link");
                    verify_debug_objects(&paths, &linked);
                    for path in paths.into_iter().chain([linked]) {
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
                    std::fs::remove_file(exe).unwrap();
                }
            }
            std::fs::remove_file(source).unwrap();
        }
    });
}

/// Generated-body sites name their generated text. Keep the virtual filename,
/// valid line coordinates and debugger ownership in both backends, independently
/// of the declaration's location (the generator remains separate origin data).
#[test]
fn a_generated_body_is_reported_in_its_generated_source() {
    hirsel_engine::with_compiler_stack(|| {
        let program = "@serializable\npub type Pair {\n  a: u64,\n  b: String,\n}\n\n\
        @preserves_unique(out)\npub fn pair_json_into(value: Pair, mut out: List[u8], depth: u64) -> Result[\n  \
        (),\n  json.JsonError\n] uses {} = comptime derive.json_into(shape(Pair))\n\n\
        pub fn main() -> Result[(), ()] {\n  let mut out: List[u8] = []\n  \
        match pair_json_into(Pair(a: 1, b: \"x\"), mut out, 0) {\n    Ok(()) => Ok(()),\n    \
        Error(_) => Error(()),\n  }\n}\n";
        let source = super::std_file(program);
        let mut loaded = hirsel_driver::load_program(&source).unwrap();
        loaded.check_for_execution().unwrap();
        let generated = loaded
            .engine
            .generated_sources()
            .find(|(definition, _)| definition.0.name == "pair_json_into")
            .unwrap()
            .1;
        let generated = loaded.engine.sources.file(generated.file).unwrap();
        assert_eq!(
            generated.logical_path().to_str(),
            Some("<generated:standalone@0/pair_json_into>")
        );
        let generated_lines: Vec<_> = generated.text().lines().collect();
        for backend in ["llvm", "cranelift"] {
            let exe = source.with_extension(format!("{backend}-exe"));
            let built = output(
                Command::new(env!("CARGO_BIN_EXE_hirsel"))
                    .args(["build", "--debug", "--backend", backend])
                    .arg(&source)
                    .arg("-o")
                    .arg(&exe),
            );
            assert!(built.status.success(), "{backend}: {built:?}");
            let lines = function_lines(&exe, generated.logical_path());
            assert_eq!(lines.len(), 1, "{backend}: {lines:?}");
            let generated = lines
                .iter()
                .find(|(name, _)| name.ends_with("/pair_json_into"))
                .unwrap_or_else(|| panic!("{backend}: {lines:?}"));
            assert!(
                generated.1.iter().all(|line| generated_lines
                    .get(*line as usize - 1)
                    .is_some_and(|text| !text.trim().is_empty())),
                "{backend}: {lines:?}"
            );
            assert_eq!(output(&mut Command::new(&exe)).status.code(), Some(0));
            std::fs::remove_file(exe).unwrap();
        }
        std::fs::remove_file(source).unwrap();
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
                        assert_eq!(
                            lines.get("helper@2/value"),
                            Some(&BTreeSet::from([3])),
                            "{lines:?}"
                        );
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
                    assert!(failure.contains("helper@2/src/helper.hir:3:"), "{failure}");
                    artifacts.push(files);
                }
                assert!(
                    artifacts[0] == artifacts[1],
                    "{backend} debug={debug}: artifact bytes differ across roots"
                );
            }
        }
        // Measurement emits ProfileSite.file in rodata as well as DWARF.
        // Compare both backends' actual objects, including generated-body source ownership.
        use hirsel_driver::native::{self, Backend, OptLevel, Options, Partition, TargetSpec};
        for backend in [Backend::Llvm, Backend::Cranelift] {
            let mut measured = Vec::new();
            for tree in &roots {
                // Hosts have independent compiler sessions; neither build may
                // adopt source products from the other root's package cache.
                // Lane pkgcache-comptime owns generated token sources on cache adoption.
                let mut loaded =
                    hirsel_driver::load_program_source(&tree.join("app/hirsel.json"), None)
                        .unwrap();
                loaded.check_for_execution().unwrap();
                let generated = loaded
                    .engine
                    .generated_sources()
                    .find(|(definition, _)| definition.0.name == "generated")
                    .unwrap()
                    .1;
                let source = loaded.engine.sources.file(generated.file).unwrap();
                assert_eq!(
                    source.logical_path().to_str(),
                    Some("<generated:helper@2/generated>")
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
                    backend,
                    OptLevel::None,
                    Partition::Whole,
                    true,
                    Some(&sources),
                )
                .unwrap();
                let mut bytes: Vec<_> = objects.into_iter().map(|o| o.bytes).collect();
                let mut paths: Vec<_> = bytes
                    .iter()
                    .enumerate()
                    .map(|(index, object)| {
                        let path = tree.join(format!("measured-{index}.o"));
                        std::fs::write(&path, object).unwrap();
                        path
                    })
                    .collect();
                for reversed in [false, true] {
                    if reversed {
                        paths.reverse();
                    }
                    let linked = tree.join(format!("measured-{reversed}"));
                    verify_debug_objects(&paths, &linked);
                    let info = dump(&linked, "--debug-info");
                    assert!(info.contains("<generated:helper@2/generated>"), "{info}");
                    assert!(info.contains("helper@2/value"), "{info}");
                    bytes.push(std::fs::read(linked).unwrap());
                }
                assert!(bytes.iter().any(|b| {
                    b.windows(b"helper@2/src/helper.hir".len())
                        .any(|w| w == b"helper@2/src/helper.hir")
                }));
                measured.push(bytes);
            }
            assert!(
                measured[0] == measured[1],
                "{}: measurement object/link bytes differ across roots",
                backend.name()
            );
        }
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
