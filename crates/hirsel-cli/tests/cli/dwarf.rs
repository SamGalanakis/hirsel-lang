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

fn function_lines(exe: &Path, source: &Path) -> BTreeMap<String, BTreeSet<u64>> {
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
        .flat_map(str::lines)
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
                    std::fs::remove_file(exe).unwrap();
                }
            }
            std::fs::remove_file(source).unwrap();
        }
    });
}
