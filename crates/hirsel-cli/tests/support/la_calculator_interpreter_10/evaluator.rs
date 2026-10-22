//! Independent, hand-authored trees; the generated fixture calls the real walker.
//! Reference: LangArena 3c9f84d calculator_interpreter.rs:23-109 (MIT).
use std::{collections::BTreeMap, fmt::Write, path::PathBuf, process::Command};

#[derive(Clone)]
enum Node {
    Num(i64),
    Var(&'static str),
    Set(&'static str, Box<Node>),
    Bin(u8, Box<Node>, Box<Node>),
}
use Node::{Num, Var};
fn set(k: &'static str, n: Node) -> Node {
    Node::Set(k, Box::new(n))
}
fn bin(op: u8, a: Node, b: Node) -> Node {
    Node::Bin(op, Box::new(a), Box::new(b))
}
fn evaluate(n: &Node, vars: &mut BTreeMap<&'static str, i64>) -> i64 {
    match n {
        Node::Num(n) => *n,
        Node::Var(k) => *vars.get(k).unwrap_or(&0),
        Node::Set(k, n) => {
            let v = evaluate(n, vars);
            vars.insert(k, v);
            v
        }
        Node::Bin(op, a, b) => {
            let a = evaluate(a, vars);
            let b = evaluate(b, vars);
            match op {
                b'+' => a.wrapping_add(b),
                b'-' => a.wrapping_sub(b),
                b'*' => a.wrapping_mul(b),
                // MIN/-1 is a Hirsel adaptation, separately asserted below;
                // never execute Rust's unsupported overflowing division.
                b'/' if b == 0 => 0,
                b'/' if a == i64::MIN && b == -1 => a,
                b'/' => a / b,
                b'%' if b == 0 || (a == i64::MIN && b == -1) => 0,
                b'%' => a % b,
                _ => 0,
            }
        }
    }
}
fn hirsel(n: &Node) -> String {
    match n {
        Num(n) => format!("box Number(value: {n}i64)"),
        Var(k) => format!("box Variable(name: {k:?})"),
        Node::Set(k, n) => format!("box Assignment(name: {k:?}, value: {})", hirsel(n)),
        Node::Bin(op, a, b) => format!(
            "box Binary(op: {op}u8, left: {}, right: {})",
            hirsel(a),
            hirsel(b)
        ),
    }
}
struct Vector {
    nodes: Vec<Node>,
    result: i64,
    vars: Vec<(&'static str, i64)>,
}
fn vectors() -> Vec<Vector> {
    let cases = vec![
        (vec![], 0, vec![]),
        (vec![Var("missing")], 0, vec![]),
        (
            vec![bin(b'+', Num(2), bin(b'*', Num(3), Num(4)))],
            14,
            vec![],
        ),
        (
            vec![bin(b'*', bin(b'+', Num(2), Num(3)), Num(4))],
            20,
            vec![],
        ),
        (
            vec![
                set("x", Num(3)),
                set("x", bin(b'+', Var("x"), Num(4))),
                Var("x"),
            ],
            7,
            vec![("x", 7)],
        ),
        (
            vec![bin(
                b'+',
                set("x", Num(2)),
                set("x", bin(b'*', Var("x"), Num(3))),
            )],
            8,
            vec![("x", 6)],
        ),
        (
            vec![set("x", set("y", Num(9))), bin(b'-', Var("x"), Var("y"))],
            0,
            vec![("x", 9), ("y", 9)],
        ),
        (
            vec![bin(b'?', set("left", Num(4)), set("right", Var("left")))],
            0,
            vec![("left", 4), ("right", 4)],
        ),
        (vec![bin(b'+', Num(i64::MAX), Num(1))], i64::MIN, vec![]),
        (vec![bin(b'-', Num(i64::MIN), Num(1))], i64::MAX, vec![]),
        (vec![bin(b'*', Num(i64::MAX), Num(2))], -2, vec![]),
        (vec![bin(b'/', Num(-7), Num(3))], -2, vec![]),
        (vec![bin(b'%', Num(-7), Num(3))], -1, vec![]),
        (vec![bin(b'/', Num(7), Num(-3))], -2, vec![]),
        (vec![bin(b'%', Num(7), Num(-3))], 1, vec![]),
        (vec![bin(b'/', Num(-7), Num(-3))], 2, vec![]),
        (vec![bin(b'%', Num(-7), Num(-3))], -1, vec![]),
        (vec![bin(b'/', set("x", Num(9)), Num(0))], 0, vec![("x", 9)]),
        (
            vec![bin(b'%', set("x", Num(-9)), Num(0))],
            0,
            vec![("x", -9)],
        ),
        (vec![bin(b'/', Num(i64::MIN), Num(-1))], i64::MIN, vec![]),
        (vec![bin(b'%', Num(i64::MIN), Num(-1))], 0, vec![]),
    ];
    cases
        .into_iter()
        .map(|(nodes, result, vars)| Vector {
            nodes,
            result,
            vars,
        })
        .collect()
}
#[test]
fn independent_rust_oracle() {
    for v in vectors() {
        let mut vars = BTreeMap::new();
        let mut result = 0;
        for n in &v.nodes {
            result = evaluate(n, &mut vars);
        }
        assert_eq!(result, v.result);
        assert_eq!(vars, v.vars.into_iter().collect());
    }
}
fn fixture() -> String {
    let mut out = include_str!("../../../../../bench/langarena/calculator.hir").to_string();
    out.push_str(
        r#"
fn check_program(nodes: List[box Node], expected_value: i64, expected: Map[String, i64]) {
  // Reuse the same AST with two fresh maps; variable reads never create entries.
  for _ in 0u64..2u64 {
    let mut variables: Map[String, i64] = map.empty()
    let mut value_out = 0i64
    for node in nodes { value_out = evaluate(node, mut variables) }
    failure.check(value_out == expected_value, Bounds())
    failure.check(map.same_entries(variables, expected), Bounds())
  }
}
pub fn main(rt: Io) -> Result[(), String] {
"#,
    );
    out.push_str(
        r#"
  let mut fresh = State(
    source: b"",
    ast: [box Assignment(name: "counter", value: box Binary(
      op: b'+', left: box Variable(name: "counter"), right: box Number(value: 1)
    ))],
    interpret: true,
    value_out: 0
  )
  step(mut fresh, 0)
  step(mut fresh, 1)
  failure.check(fresh.value_out == 2u32, Bounds())
"#,
    );
    for v in vectors() {
        out.push_str("{ let mut expected: Map[String, i64] = map.empty()\n");
        for (k, n) in &v.vars {
            writeln!(out, "map.insert(mut expected, {k:?}, {n}i64)").unwrap();
        }
        let nodes = v.nodes.iter().map(hirsel).collect::<Vec<_>>().join(", ");
        writeln!(
            out,
            "check_program([{nodes}], {}i64, expected)\n}}",
            v.result
        )
        .unwrap();
    }
    for (source, result, x) in [
        ("x = 2 + 3 * 4\nx", 14, 14),
        ("x = (2 + 3) * 4\nx", 20, 20),
        ("x = y = 9\nx - y", 0, 9),
        ("x = 1\nx = x + 2\nx", 3, 3),
    ] {
        writeln!(out,"{{ let mut expected: Map[String, i64] = map.empty()\nmap.insert(mut expected, \"x\", {x}i64)").unwrap();
        if source.contains("y =") {
            out.push_str("map.insert(mut expected, \"y\", 9i64)\n");
        }
        writeln!(
            out,
            "check_program(parse(string.as_bytes({source:?})), {result}i64, expected)\n}}"
        )
        .unwrap();
    }
    for (name, ops, iters, expected) in [
        ("Ast", 3, 3, 774028720u32),
        ("Interpreter", 10, 100, 3208680u32),
    ] {
        writeln!(
            out,
            r#"{{
let mut config: harness.Config = map.empty()
map.insert(mut config, "operations", Number("{ops}"))
map.insert(mut config, "iterations", Number("{iters}"))
map.insert(mut config, "checksum", Number("{expected}"))
let report = execute(rt, "Calculator::{name}", config)?
failure.check(report.actual == {expected}u32, Bounds())
}}"#
        )
        .unwrap();
    }
    out.push_str("Ok(())\n}\n");
    out
}
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn run(engine: Option<&str>) {
    let dir = Scratch(std::env::temp_dir().join(format!(
        "la-calculator-interpreter-10-{}-{}",
        std::process::id(),
        engine.unwrap_or("h")
    )));
    std::fs::create_dir(&dir.0).unwrap();
    std::fs::write(dir.0.join("main.hir"), fixture()).unwrap();
    std::fs::write(
        dir.0.join("harness.hir"),
        include_str!("../../../../../bench/langarena/harness.hir"),
    )
    .unwrap();
    let manifest = serde_json::json!({"schema":1,"package":{"name":"calculator10","version":"1"},"entry":{"module":[],"name":"main"},"sources":[{"path":"main.hir","module":[]},{"path":"harness.hir","module":["harness"]}],"dependencies":[{"manifest":PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../stdlib/std@1/hirsel.json").canonicalize().unwrap(),"package":{"name":"std","version":"1"},"expose":[]}]});
    std::fs::write(dir.0.join("hirsel.json"), manifest.to_string()).unwrap();
    std::fs::write(
        dir.0.join("grants.json"),
        r#"{"io":{"monotonic_clock":"clock.monotonic.ns@1","dirs":[]},"args":[]}"#,
    )
    .unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hirsel"));
    cmd.args(["run"])
        .arg(dir.0.join("hirsel.json"))
        .arg("--grants")
        .arg(dir.0.join("grants.json"));
    if let Some(engine) = engine {
        cmd.args(["--native", "--backend", engine]);
    }
    let result = cmd.output().unwrap();
    assert!(
        result.status.success(),
        "{cmd:?}\n{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}
#[test]
fn h_vectors_and_exact_checksums() {
    run(None);
}
#[test]
fn llvm_vectors_and_exact_checksums() {
    run(Some("llvm"));
}
#[test]
fn cranelift_vectors_and_exact_checksums() {
    run(Some("cranelift"));
}
