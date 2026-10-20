//! Run budgets (`budgets.limits@2`): on the interpreter and both native backends, an
//! infinite loop and an unbounded allocation each end their run with a typed
//! BudgetExceeded outcome after the output they produced, and their twins, a bounded loop
//! and an allocation that frees what it allocates, run to completion under the same
//! budgets.
use std::process::Command;

const SPIN: &str = r#"pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let _ = out.print(io.stdout(rt)?, "start\n")
  let mut i: u64 = 0
  while true {
    i += 1
  }
  Ok(())
}
"#;
const COUNT: &str = r#"pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let mut i: u64 = 0
  while i < 1000u64 {
    i += 1
  }
  let _ = out.print(io.stdout(rt)?, "done\n")
  Ok(())
}
"#;
const GROW: &str = r#"pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let _ = out.print(io.stdout(rt)?, "start\n")
  let mut l: List[u64] = []
  while true {
    list.push(mut l, 7u64)
  }
  Ok(())
}
"#;
/// Allocates `count` elements in all, never holding more than `reset` of them: the
/// interpreter's twin allocates twice its budget, the native one many times its own.
fn churn(count: u64, reset: u64) -> String {
    format!(
        r#"pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {{
  let mut l: List[u64] = []
  let mut i: u64 = 0
  while i < {count}u64 {{
    list.push(mut l, i)
    if list.len(l) == {reset}u64 {{
      l = []
    }}
    i += 1
  }}
  let _ = out.print(io.stdout(rt)?, "done\n")
  Ok(())
}}
"#
    )
}

const MAIN_RECORDED: &str = r#"pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let _ = out.print(io.stdout(rt)?, "recorded on main\n")
  Ok(())
}
"#;
/// The tape `hirsel run --record` wrote for [`MAIN_RECORDED`] before budgets existed
/// (ed98fd54), without its `admitted_at` line.
const MAIN_TAPE: &str = "hirsel-io-tape 7\n\
run_identity 6233d5a9096aa82b49c79b447e3405b2b37bd0e032c565c9811596f8a7eca71b\n\
hash_seed 0\n\
random_seed 0\n\
test_seed 0\n\
run 7b22736368656d61223a312c2270726f6772616d223a2261643938393032626431646664343837373439623139633937626339356335393866333861333665396664646332316433643362373434386334313633393733222c22656e747279223a227374616e64616c6f6e6540302f6d61696e222c226c61756e6368223a226d61696e222c22696e70757473223a7b22686173685f73656564223a302c2272616e646f6d5f73656564223a302c22746573745f73656564223a302c227363686564756c6572223a6e756c6c7d2c22696f223a7b22737464696e223a6e756c6c2c22737464657272223a6e756c6c2c227374646f7574223a227374646f75742e62797465734031222c227461736b73223a6e756c6c2c226d6f6e6f746f6e69635f636c6f636b223a6e756c6c2c2277616c6c5f636c6f636b223a6e756c6c2c2272616e646f6d223a6e756c6c2c2264697273223a5b5d2c226e6574223a6e756c6c2c2270726f63657373223a6e756c6c7d2c2261726773223a5b5d2c22656e76223a6e756c6c2c22756e73616665223a66616c73652c2261646d697373696f6e223a6e756c6c7d\n\
directory \n\
events\n\
stdout 1 -> handle ok 2\n\
print 2 7265636f72646564206f6e206d61696e0a -> unit ok \n\
complete 2\n\
";

fn grants(budgets: &str) -> String {
    format!(
        r#"{{"io":{{"stdout":"stdout.bytes@1","dirs":[]}},"args":[],"budgets":{{"provider":"budgets.limits@2",{budgets}}}}}"#
    )
}
const FUEL: &str = r#""fuel":100000"#;
const MEMORY: &str = r#""memory_bytes":65536"#;
/// Natively memory is logical storage: size classes, large requests rounded up to 4 KiB and
/// canonical region segments.
const NATIVE_MEMORY: &str = r#""memory_bytes":8388608"#;

/// The interpreter, then each native backend.
const ENGINES: [&[&str]; 3] = [&[], &["--native"], &["--native", "--backend", "cranelift"]];

fn run(source: &str, budgets: &str, engine: &[&str], extra: &[&str]) -> std::process::Output {
    let source = super::file(source);
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .args(engine)
        .arg(&source)
        .arg("--grants")
        .arg(grants(budgets))
        .args(extra)
        .output()
        .unwrap();
    std::fs::remove_file(source).unwrap();
    output
}

fn assert_exceeded(output: &std::process::Output, kind: &str) {
    assert_eq!(output.status.code(), Some(5), "{output:?}");
    assert_eq!(output.stdout, b"start\n", "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        format!("budget exceeded: {kind}\n")
    );
}

fn assert_done(output: &std::process::Output) {
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(output.stdout, b"done\n", "{output:?}");
}

#[test]
fn an_infinite_loop_spends_its_fuel_and_a_bounded_loop_finishes_on_every_engine() {
    for engine in ENGINES {
        assert_exceeded(&run(SPIN, FUEL, engine, &[]), "fuel");
        assert_done(&run(COUNT, FUEL, engine, &[]));
    }
}

#[test]
fn unbounded_allocation_exceeds_memory_and_freed_allocation_does_not_on_every_engine() {
    for engine in ENGINES {
        let (memory, twin) = if engine.is_empty() {
            (MEMORY, churn(8000, 100))
        } else {
            (NATIVE_MEMORY, churn(4_000_000, 200_000))
        };
        assert_exceeded(&run(GROW, memory, engine, &[]), "memory");
        assert_done(&run(&twin, memory, engine, &[]));
    }
}

#[test]
fn a_recorded_budget_stop_replays_to_the_same_stop() {
    for (engine, source, budgets, kind) in [
        (ENGINES[0], SPIN, FUEL, "fuel"),
        (ENGINES[0], GROW, MEMORY, "memory"),
        (ENGINES[1], SPIN, FUEL, "fuel"),
        (ENGINES[1], GROW, NATIVE_MEMORY, "memory"),
    ] {
        let tape = std::env::temp_dir().join(format!(
            "hirsel-budget-tape-{}-{kind}-{}",
            std::process::id(),
            engine.len()
        ));
        let path = super::file(source);
        let recorded = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("run")
            .args(engine)
            .arg(&path)
            .arg("--grants")
            .arg(grants(budgets))
            .arg("--record")
            .arg(&tape)
            .output()
            .unwrap();
        assert_exceeded(&recorded, kind);
        let replayed = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("run")
            .args(engine)
            .arg(&path)
            .arg("--replay")
            .arg(&tape)
            .output()
            .unwrap();
        assert_exceeded(&replayed, kind);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(tape).unwrap();
    }
}

/// The tape's lines without `admitted_at`, with its run record decoded and its program
/// digest, and the run identity it determines, replaced by `PROGRAM`.
fn normalized(tape: &str) -> Vec<String> {
    let mut program = None;
    let lines: Vec<String> = tape
        .lines()
        .filter(|line| !line.starts_with("admitted_at "))
        .map(|line| match line.strip_prefix("run ") {
            Some(hex) => {
                let bytes: Vec<u8> = (0..hex.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                    .collect();
                let record = String::from_utf8(bytes).unwrap();
                let start = record.find(r#""program":""#).unwrap() + 11;
                program = Some(record[start..start + 64].to_owned());
                format!(
                    "run {}",
                    record.replacen(&record[start..start + 64], "PROGRAM", 1)
                )
            }
            None if line.starts_with("run_identity ") => "run_identity PROGRAM".into(),
            None => line.into(),
        })
        .collect();
    assert!(program.is_some(), "{tape}");
    lines
}

#[test]
fn a_run_without_budgets_keeps_historical_identity_and_events_with_placement_provenance() {
    // A tape recorded before this change cannot replay after it: the program digest binds
    // the runtime's sources (hirsel-engine's build script), and those changed. Everything
    // else, including canonical bytes without `budgets`, stays unchanged; placement now
    // has its own diagnostic metadata line outside those canonical bytes.
    let path = super::file(MAIN_RECORDED);
    let tape = std::env::temp_dir().join(format!("hirsel-budget-main-tape-{}", std::process::id()));
    let recorded = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&path)
        .arg("--grants")
        .arg(r#"{"io":{"stdout":"stdout.bytes@1","dirs":[]},"args":[]}"#)
        .arg("--record")
        .arg(&tape)
        .output()
        .unwrap();
    assert_eq!(recorded.status.code(), Some(0), "{recorded:?}");
    assert_eq!(recorded.stdout, b"recorded on main\n");
    let written = std::fs::read_to_string(&tape).unwrap();
    let mut expected = normalized(MAIN_TAPE);
    // Exact default placement provenance; all historical semantic bytes and events match.
    expected.insert(
        6,
        concat!(
            "placement ",
            "7b22636f726573223a312c2263707573223a6e756c6c2c2270696e223a226e6f6e65222c",
            "22636f6d6d6974223a6e756c6c2c2272756e5f6168656164223a6e756c6c2c2272656f72",
            "646572223a6e756c6c2c2273706563756c6174696f6e5f6279746573223a6e756c6c2c226d",
            "657472696373223a6e756c6c2c22636f6e74726f6c223a6e756c6c7d"
        )
        .into(),
    );
    assert_eq!(normalized(&written), expected);
    let replayed = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&path)
        .arg("--replay")
        .arg(&tape)
        .output()
        .unwrap();
    assert_eq!(replayed.status.code(), Some(0), "{replayed:?}");
    assert_eq!(replayed.stdout, b"recorded on main\n");
    std::fs::remove_file(path).unwrap();
    std::fs::remove_file(tape).unwrap();
}

#[test]
fn an_executable_built_without_fuel_checks_refuses_a_fuel_budget() {
    let source = super::file(SPIN);
    let exe = source.with_extension("exe");
    let built = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("build")
        .arg(&source)
        .arg("-o")
        .arg(&exe)
        .output()
        .unwrap();
    assert_eq!(built.status.code(), Some(0), "{built:?}");
    let refused = Command::new(&exe)
        .arg("--hirsel-grants")
        .arg(grants(FUEL))
        .output()
        .unwrap();
    assert_eq!(refused.status.code(), Some(3), "{refused:?}");
    assert!(refused.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("budgets.fuel"),
        "{refused:?}"
    );
    // A memory budget needs no compiled checks: one built from GROW enforces it.
    let grow = super::file(GROW);
    let grow_exe = grow.with_extension("exe");
    let built = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("build")
        .arg(&grow)
        .arg("-o")
        .arg(&grow_exe)
        .output()
        .unwrap();
    assert_eq!(built.status.code(), Some(0), "{built:?}");
    let stopped = Command::new(&grow_exe)
        .arg("--hirsel-grants")
        .arg(grants(NATIVE_MEMORY))
        .output()
        .unwrap();
    assert_eq!(stopped.status.code(), Some(5), "{stopped:?}");
    assert_eq!(stopped.stdout, b"start\n", "{stopped:?}");
    std::fs::remove_file(grow).unwrap();
    std::fs::remove_file(grow_exe).unwrap();
    // Built with fuel checks, the same program spends its fuel.
    let built = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("build")
        .arg("--fuel")
        .arg(&source)
        .arg("-o")
        .arg(&exe)
        .output()
        .unwrap();
    assert_eq!(built.status.code(), Some(0), "{built:?}");
    let spent = Command::new(&exe)
        .arg("--hirsel-grants")
        .arg(grants(FUEL))
        .output()
        .unwrap();
    assert_eq!(spent.status.code(), Some(5), "{spent:?}");
    assert_eq!(spent.stdout, b"start\n", "{spent:?}");
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(exe).unwrap();
}

/// A fuel build without a fuel budget takes a 64 Ki-point chunk at a time: a loop of 300 000
/// iterations refills its chunk several times (`hirsel_rt_v2_fuel_refill` returns into the
/// loop) and finishes on both backends, and a budget below its charges still stops it.
#[test]
fn a_fuel_build_without_a_budget_refills_its_chunk_and_finishes_on_both_backends() {
    const LONG: &str = r#"pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let mut i: u64 = 0
  while i < 300000u64 {
    i += 1
  }
  let _ = out.print(io.stdout(rt)?, "done\n")
  Ok(())
}
"#;
    let source = super::file(LONG);
    for backend in ["llvm", "cranelift"] {
        let exe = source.with_extension(format!("{backend}.exe"));
        let built = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("build")
            .arg("--fuel")
            .arg("--backend")
            .arg(backend)
            .arg(&source)
            .arg("-o")
            .arg(&exe)
            .output()
            .unwrap();
        assert_eq!(built.status.code(), Some(0), "{built:?}");
        let finished = Command::new(&exe)
            .arg("--hirsel-grants")
            .arg(r#"{"io":{"stdout":"stdout.bytes@1","dirs":[]},"args":[]}"#)
            .output()
            .unwrap();
        assert_done(&finished);
        let spent = Command::new(&exe)
            .arg("--hirsel-grants")
            .arg(grants(FUEL))
            .output()
            .unwrap();
        assert_eq!(spent.status.code(), Some(5), "{spent:?}");
        assert!(spent.stdout.is_empty(), "{spent:?}");
        std::fs::remove_file(exe).unwrap();
    }
    std::fs::remove_file(source).unwrap();
}
