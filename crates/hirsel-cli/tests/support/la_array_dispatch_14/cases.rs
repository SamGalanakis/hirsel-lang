use std::process::Command;

// Vectors: MIT LangArena brainfuck_array.rs:10-121 at 3c9f84d.
// The oracle only records output and returns Tape; the bounded H variant uses
// 8 initial cells. Full native controls retain all 30000 cells and growth.
// Copyright and permission notice: bench/langarena/LICENSE.
fn source(full: bool) -> (String, usize) {
    let production = include_str!("../../../../../bench/langarena/brainfuck.hir");
    let mut program = production
        .split("fn step(")
        .next()
        .unwrap()
        .replace("type Tape {", "type Tape {\n  output: List[u8],")
        .replace(
            "Tape(cells: [], pos: 0, value_out: 0)",
            "Tape(cells: [], pos: 0, value_out: 0, output: [])",
        )
        .replace("b'.' => {\n      t.value_out", "b'.' => {\n      list.push(mut t.output, t.cells[t.pos])\n      t.value_out")
        .replace("-> Option[u32]", "-> Option[Tape]")
        .replace("Some(tape.value_out)", "Some(take tape)")
        .replace("b'.' => {\n          tape.value_out", "b'.' => {\n          list.push(mut tape.output, tape.cells[tape.pos])\n          tape.value_out");
    // Fail if instrumentation ever stops matching either production print.
    assert!(program.contains("list.push(mut t.output, t.cells[t.pos])"));
    assert!(program.contains("list.push(mut tape.output, tape.cells[tape.pos])"));
    if !full {
        program = program.replace("30000u64", "8u64");
    }
    let vectors: serde_json::Value = serde_json::from_str(if full {
        include_str!("full.json")
    } else {
        include_str!("small.json")
    })
    .unwrap();
    let rows = vectors.as_array().unwrap();
    for row in rows {
        let name = row["name"].as_str().unwrap();
        let text = if name == "full_growth" {
            ">".repeat(30000) + "+.<."
        } else {
            row["program"].as_str().unwrap().to_string()
        };
        let text = serde_json::to_string(&text).unwrap();
        let expected = &row["expected"];
        program.push_str(&format!("\n@test\nfn array_{name}(t: Test) {{\n"));
        if expected.is_null() {
            program.push_str(&format!(
                "  test.check(t, run({text}, false) == None())\n}}\n"
            ));
            continue;
        }
        program.push_str(&format!(
            "  let Some(tape) = run({text}, false) else {{ test.check(t, false); return }}\n"
        ));
        program.push_str(&format!("  test.check(t, list.len(tape.cells) == {}u64)\n  test.check(t, tape.pos == {}u64)\n  test.check(t, tape.value_out == {}u32)\n", expected["length"], expected["pos"], expected["checksum"]));
        // Compare every initialized/grown cell, including zero tails.
        program.push_str("  for i in 0u64..list.len(tape.cells) {\n    let value = ");
        for (index, value) in expected["cells"].as_object().unwrap() {
            program.push_str(&format!("if i == {index}u64 {{ {value}u8 }} else {{ "));
        }
        program.push_str("0u8");
        for _ in expected["cells"].as_object().unwrap() {
            program.push_str(" }");
        }
        program.push_str("\n    test.check(t, tape.cells[i] == value)\n  }\n");
        let output = expected["output"].as_array().unwrap();
        program.push_str(&format!(
            "  test.check(t, list.len(tape.output) == {}u64)\n",
            output.len()
        ));
        for (i, value) in output.iter().enumerate() {
            program.push_str(&format!(
                "  test.check(t, tape.output[{i}u64] == {value}u8)\n"
            ));
        }
        program.push_str("}\n");
    }
    let configs: serde_json::Value =
        serde_json::from_str(include_str!("../../../../../bench/langarena/test.js")).unwrap();
    let config = configs
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == "Brainfuck::Array")
        .unwrap();
    program.push_str(&format!(
        r#"
@test
fn array_upstream_reduced(t: Test) {{
  let _ = run({}, false)
  let mut sum = 0u32
  for _ in 0u64..2u64 {{
    let Some(tape) = run({}, false) else {{ test.check(t, false); return }}
    sum = u32.wrapping_add(sum, tape.value_out)
  }}
  test.check(t, sum == 3562897308u32)
}}
"#,
        config["warmup_program"], config["program"]
    ));
    (program, rows.len() + 1)
}

fn check(flags: &[&str], full: bool) {
    let (source, count) = source(full);
    let directory = tempfile::Builder::new()
        .prefix("la-array-dispatch-14-")
        .tempdir()
        .unwrap();
    let path = directory.path().join("vectors.hir");
    std::fs::write(&path, source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("test")
        .args(flags)
        .arg(path)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{flags:?}\n{stdout}\n{stderr}");
    assert!(
        stdout.contains(&format!("{count} passed; 0 failed; {count} total")),
        "{stdout}\n{stderr}"
    );
}
#[test]
fn array_h() {
    check(&[], false);
}
#[test]
fn array_llvm() {
    check(&["--native", "--backend", "llvm"], true);
}
#[test]
fn array_cranelift() {
    check(&["--native", "--backend", "cranelift"], true);
}
