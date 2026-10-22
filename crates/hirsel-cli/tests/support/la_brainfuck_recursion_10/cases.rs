use std::{path::PathBuf, process::Command};

// Rust-produced vectors come from brainfuck_recursion.rs:4-104 at 3c9f84d.
// The adapter adds output recording and exposes Tape after Program::execute;
// no parser, tape operation or execution-order rule is changed. MIT: the
// original notice and license are retained in bench/langarena/LICENSE.
fn source() -> String {
    let production = include_str!("../../../../../bench/langarena/brainfuck.hir");
    let kernel = production.split("fn step(").next().unwrap();
    // Record emitted bytes at the production Print operations. Preserve their
    // checksum statements and every other parser/runner operation verbatim.
    let mut kernel = kernel
        .replace("type Tape {", "type Tape {\n  output: List[u8],")
        .replace(
            "Tape(cells: [], pos: 0, value_out: 0)",
            "Tape(cells: [], pos: 0, value_out: 0, output: [])",
        )
        .replace("-> Option[u32]", "-> Option[Tape]")
        .replace("Some(tape.value_out)", "Some(take tape)")
        // Reduced Array fixture: 8 initialized cells instead of 30000. This
        // keeps H within its unchanged step budget; tests also grow beyond 8.
        .replace("30000u64", "8u64");
    kernel = kernel
        .replace(
            "b'.' => {\n      t.value_out",
            "b'.' => {\n      list.push(mut t.output, t.cells[t.pos])\n      t.value_out",
        )
        .replace(
            "b'.' => {\n          tape.value_out",
            "b'.' => {\n          list.push(mut tape.output, tape.cells[tape.pos])\n          tape.value_out",
        )
        .replace(
            "Print() => {",
            "Print() => {\n      list.push(mut tape.output, tape.cells[tape.pos])",
        );
    let vectors: serde_json::Value = serde_json::from_str(include_str!("vectors.json")).unwrap();
    let mut program = kernel;
    for (i, row) in vectors["rows"].as_array().unwrap().iter().enumerate() {
        for recursive in [true, false] {
            let text = row["program"].as_str().unwrap();
            let name = if recursive { "recursive" } else { "array" };
            program.push_str(&format!("\n@test\nfn {name}_{i}(t: Test) {{\n"));
            if !recursive && (text.is_empty() || text == "comment only") {
                program.push_str(&format!(
                    "  test.check(t, run({}, false) == None())\n}}\n",
                    serde_json::to_string(text).unwrap()
                ));
                continue;
            }
            program.push_str(&format!(
                "  let Some(tape) = run({}, {recursive}) else {{ test.check(t, false); return }}\n",
                serde_json::to_string(text).unwrap()
            ));
            let mut cells: Vec<u64> = row["cells"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_u64().unwrap())
                .collect();
            if !recursive {
                cells.resize(cells.len().max(8), 0);
            }
            let output: Vec<u64> = row["output"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_u64().unwrap())
                .collect();
            for (field, values) in [("cells", cells), ("output", output)] {
                program.push_str(&format!(
                    "  test.check(t, list.len(tape.{field}) == {}u64)\n",
                    values.len()
                ));
                for (index, value) in values.iter().enumerate() {
                    program.push_str(&format!(
                        "  test.check(t, tape.{field}[{index}u64] == {value}u8)\n"
                    ));
                }
            }
            program.push_str(&format!("  test.check(t, tape.pos == {}u64)\n  test.check(t, tape.value_out == {}u32)\n}}\n", row["pos"], row["checksum"]));
        }
    }
    // Exact test.js program/checksum; parse/run is repeated inside the loop.
    let configs: serde_json::Value =
        serde_json::from_str(include_str!("../../../../../bench/langarena/test.js")).unwrap();
    let config = configs
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == "Brainfuck::Recursion")
        .unwrap();
    program.push_str(&format!(
        r#"
@test
fn upstream_test_js(t: Test) {{
  let _ = run({}, true)
  let mut sum = 0u32
  for _ in 0u64..2u64 {{
    let Some(tape) = run({}, true) else {{ test.check(t, false); return }}
    sum = u32.wrapping_add(sum, tape.value_out)
  }}
  test.check(t, sum == 3562897308u32)
}}
"#,
        config["warmup_program"], config["program"]
    ));
    program
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "la-brainfuck-recursion-10-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("vectors.hir"), source()).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn check(flags: &[&str]) {
    let fixture = Fixture::new();
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("test")
        .args(flags)
        .arg(fixture.0.join("vectors.hir"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{flags:?}\n{stdout}\n{stderr}");
    assert!(
        stdout.contains("49 passed; 0 failed; 49 total"),
        "{stdout}\n{stderr}"
    );
}

#[test]
fn recursive_and_array_h() {
    check(&[]);
}
#[test]
fn recursive_and_array_llvm() {
    check(&["--native", "--backend", "llvm"]);
}
#[test]
fn recursive_and_array_cranelift() {
    check(&["--native", "--backend", "cranelift"]);
}
