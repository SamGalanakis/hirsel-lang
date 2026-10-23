//! Finite retirement histories; not an exhaustive ownership proof. Shapes follow
//! ownership_history_21.rs:303-364 and rustc initialized.rs:255-303 (19833af7):
//! successful initialization and the instruction's failure edge are distinct.
//! No upstream implementation or Rust borrowing/unwinding policy is copied.
//! Builtin borrowed cursor drops are silent. Source-triggered Trace failures
//! exercise resource cleanup beside retirement, not arbitrary native faults
//! inside the cursor drop; that unavailable injection cell remains incomplete.
use hirsel_driver::{compiler_adapters, native::*};
use hirsel_engine::*;
use hirsel_tir::*;
use std::{collections::BTreeSet, path::PathBuf};

#[allow(dead_code)]
#[path = "../support/history.rs"]
mod history;

// Frozen greedy pair cover (selection seed 0), padded to 24 before consumers.
// initialize: assign/replace/call; owner: generic unique/resource aggregate;
// cursor: borrowed/copy/no cursor; exit: normal/return/break/?/bug;
// retired drop: normal/failing; iterations: 1/2; scoped callback: absent/present.
const ROWS: [[u8; 7]; 24] = [
    [1, 1, 0, 4, 0, 1, 0],
    [2, 1, 2, 1, 1, 0, 1],
    [0, 0, 1, 3, 0, 0, 1],
    [0, 0, 2, 0, 1, 1, 0],
    [1, 0, 0, 2, 1, 0, 1],
    [2, 1, 1, 2, 0, 1, 0],
    [2, 0, 1, 4, 1, 0, 1],
    [1, 1, 1, 0, 0, 0, 1],
    [1, 1, 2, 3, 1, 0, 0],
    [0, 1, 0, 1, 0, 1, 0],
    [2, 0, 0, 3, 1, 1, 1],
    [1, 0, 1, 1, 0, 0, 0],
    [0, 0, 2, 4, 0, 0, 0],
    [2, 0, 0, 0, 0, 1, 0],
    [0, 0, 2, 2, 1, 0, 0],
    [0, 0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 1],
    [0, 0, 0, 0, 0, 1, 0],
    [0, 0, 0, 0, 0, 1, 1],
    [0, 0, 0, 0, 1, 0, 0],
    [0, 0, 0, 0, 1, 0, 1],
    [0, 0, 0, 0, 1, 1, 0],
    [0, 0, 0, 0, 1, 1, 1],
    [0, 0, 0, 1, 0, 0, 0],
];

#[derive(Clone, Debug, PartialEq, Eq)]
enum Op {
    Outer,
    Retired,
    Cursor,
    Old,
    Initialize,
    Observe,
    Exit,
    Noise(u8),
}

fn ops(row: [u8; 7]) -> Vec<Op> {
    let mut ops = vec![Op::Outer, Op::Retired];
    if row[0] == 1 {
        ops.push(Op::Old);
    }
    ops.extend([Op::Cursor, Op::Initialize, Op::Observe, Op::Exit]);
    ops
}

#[derive(Clone, Debug, Default)]
struct Model {
    // Membership and execution order deliberately have different representations.
    live: BTreeSet<&'static str>,
    declarations: Vec<(&'static str, &'static str)>,
    transitions: Vec<BTreeSet<&'static str>>,
    output: String,
    failures: Vec<&'static str>,
    events: Vec<&'static str>,
}

impl Model {
    fn own(&mut self, name: &'static str, event: &'static str) -> Option<()> {
        if !self.live.insert(name) {
            return None;
        }
        self.declarations.push((name, event));
        self.transitions.push(self.live.clone());
        Some(())
    }
    fn release(&mut self, name: &'static str, event: &'static str, fail: bool) -> Option<()> {
        if !self.live.remove(name) {
            return None;
        }
        if !event.is_empty() {
            self.output.push_str(event);
            self.events.push(event);
        }
        if fail {
            self.failures.push("drop_failure");
        }
        self.transitions.push(self.live.clone());
        Some(())
    }
}

// This model consumes only the history and declared contracts, never compiler
// inference, ownership helpers, V, native output, or Rust drop/borrow behavior.
fn model(row: [u8; 7], ops: &[Op]) -> Option<Model> {
    let mut m = Model::default();
    let mut cursor = false;
    let mut initialized = false;
    let mut exited = false;
    for op in ops {
        match op {
            Op::Outer => m.own("outer", "O")?,
            Op::Retired if m.live.contains("outer") => m.own("retired", "R")?,
            Op::Old if row[0] == 1 && m.live.contains("retired") => {
                m.own("new", if row[1] == 1 { "D" } else { "" })?
            }
            Op::Cursor if m.live.contains("retired") && !cursor => cursor = true,
            Op::Initialize if cursor && !initialized => {
                if row[0] == 1 {
                    m.release("new", "", false)?; // transfer old value to displaced
                    m.declarations.retain(|(name, _)| *name != "new");
                    m.own("new", if row[1] == 1 { "N" } else { "" })?;
                    m.own("displaced", if row[1] == 1 { "D" } else { "" })?;
                } else {
                    m.own("new", if row[1] == 1 { "N" } else { "" })?;
                }
                initialized = true;
            }
            Op::Observe if initialized && !exited => m.output.push('v'),
            Op::Exit if initialized && !exited => {
                exited = true;
                if row[3] == 4 {
                    m.failures.push("body_failure");
                }
            }
            Op::Noise(_) => (),
            _ => return None,
        }
        m.transitions.push(m.live.clone());
    }
    if !initialized || !exited {
        return None;
    }
    let inner: Vec<_> = m
        .declarations
        .iter()
        .copied()
        .filter(|(name, _)| *name != "outer")
        .collect();
    let repeats = if row[3] == 0 && row[4] == 0 {
        row[5] + 1
    } else {
        1
    };
    for iteration in 0..repeats {
        if iteration > 0 {
            for &(name, event) in &inner {
                m.own(name, event)?;
            }
            if ops.contains(&Op::Observe) {
                m.output.push('v');
            }
        }
        for &(name, event) in inner.iter().rev() {
            m.release(name, event, name == "retired" && row[4] == 1)?;
        }
        m.transitions.push(m.live.clone());
    }
    m.release("outer", "O", false)?;
    m.transitions.push(m.live.clone());
    Some(m)
}

const PRELUDE: &str = r#"type Holder[T] { value: T }
type Bundle { value: String, trace: Trace }
resource type Trace { output: Stdout, tag: String, fail: bool }
drop(x) {
 let _ = out.print(x.output, x.tag)
 if x.fail { drop_failure() }
}
fn drop_failure() uses {} { let zero:u64=0; let _=1u64/zero }
fn body_failure() uses {} { let zero:u64=0; let _=1u64/zero }
fn stop() -> Result[(),String] uses {} { Error("stop") }
fn make_unique(f:fn()->unique Holder[unique String])->unique Holder[unique String] uses {f} { f() }
fn make_bundle(f:fn()->Bundle)->Bundle uses {f} { f() }
fn relay[T](take x:T)->T { take x }
fn exercise(output:Stdout, input:String)->Result[(),String] {
"#;

fn source(row: [u8; 7], ops: &[Op]) -> String {
    let mut s = PRELUDE.to_owned();
    if ops.contains(&Op::Outer) {
        s.push_str("let outer=Trace(output:output,tag:\"O\",fail:false)\n");
    }
    s.push_str(if row[5] == 0 {
        "for turn in [0u64] {\n"
    } else {
        "for turn in [0u64,1u64] {\n"
    });
    s.push_str("let row=input\n");
    let ty = if row[1] == 0 {
        "unique Holder[unique String]"
    } else {
        "Bundle"
    };
    let constructor = |tag: &str| {
        if row[1] == 0 {
            "Holder(value:unique row)".into()
        } else {
            format!("Bundle(value:unique row,trace:Trace(output:output,tag:\"{tag}\",fail:false))")
        }
    };
    for op in ops {
        let line = match op {
            Op::Outer => continue,
            Op::Retired => format!(
                "let retired=Trace(output:output,tag:\"R\",fail:{})",
                row[4] == 1
            ),
            Op::Old => format!("let mut new:{ty}={}", constructor("D")),
            Op::Cursor => match row[2] {
                0 => "for c in string.scalars(row) {}".into(),
                1 => "let cursor_copy=copy row; for c in string.scalars(cursor_copy) {}".into(),
                _ => "let _=string.byte_len(row)".into(),
            },
            Op::Initialize => {
                let mut value = constructor("N");
                if row[6] == 1 {
                    value = format!(
                        "{}(fn()->{ty} {{ {value} }})",
                        if row[1] == 0 {
                            "make_unique"
                        } else {
                            "make_bundle"
                        }
                    );
                }
                if row[0] == 2 {
                    value = format!("relay({value})");
                }
                if row[0] == 1 {
                    format!("let displaced:{ty}=replace(mut new,{value})")
                } else {
                    format!("let new:{ty}={value}")
                }
            }
            Op::Observe => "let _=out.print(output,new.value)".into(),
            Op::Exit => match row[3] {
                0 => "let _=turn",
                1 => "return Ok(())",
                2 => "break",
                3 => "stop()?",
                _ => "body_failure()",
            }
            .into(),
            Op::Noise(value) => format!("let _={value}u64"),
        };
        s.push_str(&line);
        s.push('\n');
    }
    s.push_str("}\nOk(())\n}\npub fn main(rt:Io)->Result[(),GrantError] {\nlet output=io.stdout(rt)?\nlet _=exercise(output,\"v\")\nOk(())\n}\n");
    assert!(ops.len() <= 12 && s.lines().count() <= 128);
    s
}

fn artifacts() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("retirement-cleanup-matrix-31")
        .join(format!(
            "attempt-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
}

// Capture action-local tool identities before the engine consumers. Follow
// directory symlinks with inode cycle protection, including ephemeral trees.
fn tool_receipt() {
    use sha2::{Digest, Sha256};
    use std::{io::Read, os::unix::fs::MetadataExt, path::Path};
    fn walk(
        path: &Path,
        dirs: &mut BTreeSet<(u64, u64)>,
        hashes: &mut std::collections::BTreeMap<(u64, u64), String>,
        files: &mut std::collections::BTreeMap<String, String>,
    ) {
        let meta = std::fs::metadata(path).unwrap();
        let inode = (meta.dev(), meta.ino());
        if meta.is_dir() {
            if !dirs.insert(inode) {
                return;
            }
            let mut children: Vec<_> = std::fs::read_dir(path)
                .unwrap()
                .map(|e| e.unwrap().path())
                .collect();
            children.sort();
            for child in children {
                walk(&child, dirs, hashes, files);
            }
        } else if meta.is_file() {
            let hash = hashes
                .entry(inode)
                .or_insert_with(|| {
                    let mut f = std::fs::File::open(path).unwrap();
                    let mut sha = Sha256::new();
                    let mut buffer = [0u8; 65536];
                    loop {
                        let n = f.read(&mut buffer).unwrap();
                        if n == 0 {
                            break;
                        }
                        sha.update(&buffer[..n]);
                    }
                    format!("{:x}", sha.finalize())
                })
                .clone();
            files.insert(path.to_string_lossy().into_owned(), hash);
        }
    }
    let mut dirs = BTreeSet::new();
    let mut hashes = std::collections::BTreeMap::new();
    let mut files = std::collections::BTreeMap::new();
    for variable in [
        "LLVM_SYS_231_PREFIX",
        "HIRSEL_RUNTIME_ARCHIVES",
        "HIRSEL_IMAGE",
        "HIRSEL_LINK_KIT",
        "HIRSEL_GLIBC_FLOOR",
    ] {
        let path = std::env::var_os(variable)
            .unwrap_or_else(|| panic!("INCOMPLETE missing action tool {variable}"));
        walk(Path::new(&path), &mut dirs, &mut hashes, &mut files);
    }
    walk(
        &std::env::current_exe().unwrap(),
        &mut dirs,
        &mut hashes,
        &mut files,
    );
    eprintln!(
        "RETIREMENT_TOOLS {}",
        serde_json::json!({"compiler":hirsel_driver::COMPILER_IDENTITY,"files":files,"configurations":format!("{:?}",matrix()),"reference_step_limit":1000000,"native_timeout_seconds":60})
    );
}

fn checked(source: &str) -> Result<QueryEngine, String> {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("main.hir", source.into()).unwrap();
    let checked =
        check_progress(&mut engine, file, &default_context()).map_err(|e| format!("API:{e:?}"))?;
    let errors: Vec<_> = checked
        .diagnostics
        .iter()
        .filter(|d| d.severity == hirsel_diag::Severity::Error)
        .collect();
    if errors.is_empty() {
        Ok(engine)
    } else {
        Err(format!("SOURCE:{errors:?}"))
    }
}

#[test]
fn coverage_and_source_contracts() {
    with_compiler_stack(|| {
        let sizes = [3, 2, 3, 5, 2, 2, 2];
        let mut pairs = BTreeSet::new();
        let dir = artifacts();
        for (seed, row) in ROWS.into_iter().enumerate() {
            let ops = ops(row);
            let expected = model(row, &ops).unwrap();
            assert!(expected.live.is_empty());
            for a in 0..7 {
                for b in a + 1..7 {
                    pairs.insert((a, row[a], b, row[b]));
                }
            }
            let source = source(row, &ops);
            history::persist(
                &dir,
                &format!("seed-{seed}-frozen"),
                seed as u64,
                &source,
                &format!("axes={row:?}\nops={ops:?}\nmodel={expected:?}"),
            );
            let mut engine =
                checked(&source).unwrap_or_else(|e| panic!("SETUP seed {seed}: {e}\n{source}"));
            let input = engine
                .export_native_input()
                .unwrap_or_else(|e| panic!("seed {seed} export: {e:?}"));
            engine
                .import_native_input(input)
                .unwrap_or_else(|e| panic!("seed {seed} checker-positive/V-red: {e:?}"));
        }
        let mut count = 0;
        for a in 0..7 {
            for b in a + 1..7 {
                for x in 0..sizes[a] {
                    for y in 0..sizes[b] {
                        assert!(
                            pairs.contains(&(a, x, b, y)),
                            "missing pair {a}:{x}/{b}:{y}"
                        );
                        let seed = ROWS.iter().position(|r| r[a] == x && r[b] == y).unwrap();
                        eprintln!(
                            "RETIREMENT_PAIR {a}:{x}/{b}:{y} seed={seed} axes={:?} expected={:?}",
                            ROWS[seed],
                            model(ROWS[seed], &ops(ROWS[seed])).unwrap()
                        );
                        count += 1;
                    }
                }
            }
        }
        assert_eq!(count, 151);
        eprintln!(
            "RETIREMENT_SOURCE accepted=24 V=24 pairs={count}/151 infeasible_pairs=0; early exits/failing drops truncate requested iterations; no native arbitrary fault injection; no file stages/multi-module cells"
        );
    });
}

fn matrix() -> Vec<Configuration> {
    [
        Engine::Heap { campaign: false },
        Engine::Native {
            opt: OptLevel::Speed,
            partition: Partition::Whole,
            runtime: Runtime::Default,
        },
        Engine::Cranelift {
            partition: Partition::Whole,
            runtime: Runtime::Diagnostic,
        },
    ]
    .into_iter()
    .map(|engine| {
        let mut options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        options.validation = Validation::EveryStage;
        Configuration { options, engine }
    })
    .collect()
}

fn differences(report: &history::Report, expected: &Model, source: &str) -> Vec<(String, String)> {
    let mut result =
        report.disagreements(expected.output.as_bytes(), !expected.failures.is_empty());
    // Shared runner's simple failure oracle accepts one SEM-1 only. Replace
    // that model check with the complete independent primary+cleanup chain.
    result.retain(|(_, field)| field != "model-outcome");
    // Full diagnostic identity is conservative under reduction: a different V
    // rule, source rejection or tooling stop cannot replace the original red.
    for cell in &report.cells {
        if cell.category != history::Category::Observation {
            result.retain(|(engine, _)| engine != &cell.engine);
            result.push((
                cell.engine.clone(),
                format!("{:?}:{}:{}", cell.category, cell.disagreement, cell.detail),
            ));
        }
    }
    if let Some(o) = report
        .cells
        .iter()
        .find(|c| c.engine == "reference")
        .and_then(|c| c.observation.as_ref())
    {
        let names: Vec<_> = o
            .failures
            .iter()
            .map(|f| f.definition.0.name.as_str())
            .collect();
        let sites_ok = o.failures.iter().all(|f| {
            let line = source
                .lines()
                .position(|l| l.starts_with(&format!("fn {}()", f.definition.0.name)))
                .unwrap_or(usize::MAX);
            f.rule == "SEM-1"
                && f.line as usize == line + 1
                && f.column as usize
                    == source
                        .lines()
                        .nth(line)
                        .and_then(|l| l.find("1u64/zero"))
                        .unwrap_or(usize::MAX)
                        + 1
                && f.file == "main.hir"
                && f.operation.is_none()
        });
        if names != expected.failures
            || !sites_ok
            || o.exit_code
                != if expected.failures.is_empty() {
                    Some(0)
                } else {
                    None
                }
            || o.blocked
            || o.budget.is_some()
            || o.signaled.is_some()
            || !o.roots.is_empty()
        {
            result.push(("reference".into(), "model-outcome-chain-site".into()));
        }
    }
    result
}

// Reuse history's deletion reducer, bound its consumer, then shrink values and
// loop depth. Every candidate must preserve model validity and the exact phase,
// engine and observation/rule signature supplied by its caller.
fn reduce(
    row: [u8; 7],
    original: &[Op],
    mut preserves: impl FnMut([u8; 7], &[Op]) -> bool,
) -> ([u8; 7], Vec<Op>, Vec<String>) {
    let mut decisions = Vec::new();
    let mut evaluations = 0;
    let mut consider = |row, candidate: &[Op]| {
        if evaluations == 96 || model(row, candidate).is_none() {
            return false;
        }
        evaluations += 1;
        let kept = preserves(row, candidate);
        decisions.push(format!(
            "evaluation={evaluations} axes={row:?} ops={candidate:?} retained={kept}"
        ));
        kept
    };
    let mut kept = history::minimize(original, |candidate| consider(row, candidate));
    let mut best = row;
    let mut shallower = best;
    shallower[5] = 0;
    if shallower != best && consider(shallower, &kept) {
        best = shallower;
    }
    for index in 0..kept.len() {
        if matches!(kept[index],Op::Noise(n) if n>0) {
            let mut candidate = kept.clone();
            candidate[index] = Op::Noise(0);
            if consider(best, &candidate) {
                kept = candidate;
            }
        }
    }
    (best, kept, decisions)
}

fn runtime_partition(start: usize, end: usize) {
    with_compiler_stack(|| {
        tool_receipt();
        let dir = artifacts();
        let runner = history::Runner::with_matrix(&dir.join("runs"), matrix())
            .expect("INCOMPLETE toolchain");
        let mut counts = [0usize; 4];
        let mut failed = Vec::new();
        for (seed, row) in ROWS.into_iter().enumerate().take(end).skip(start) {
            let ops = ops(row);
            let expected = model(row, &ops).unwrap();
            let text = source(row, &ops);
            let report = runner.run(&format!("seed-{seed}"), &text);
            for (i, cell) in report.cells.iter().enumerate() {
                if cell.category == history::Category::Observation {
                    counts[i] += 1;
                }
            }
            let disagreements = differences(&report, &expected, &text);
            history::persist(
                &dir,
                &format!("seed-{seed}-original"),
                seed as u64,
                &text,
                &format!(
                    "axes={row:?}\nops={ops:?}\nmodel={expected:?}\nreport={report:#?}\nsignatures={disagreements:?}"
                ),
            );
            // Supplemental physical audit; no native physical heap claim.
            if report.validated && report.cells.len() == 4 {
                let mut engine = checked(&text).unwrap();
                let input = engine.export_native_input().unwrap();
                let validated = engine.import_native_input(input).unwrap();
                let ready = build(&validated, &matrix()[0].options).unwrap();
                let heap = run_heap(
                    &ready,
                    &entry_path(&default_context()),
                    &Sites::new(validated.snapshot()),
                )
                .unwrap();
                assert_eq!(
                    heap.counters.live_heap_bytes, 0,
                    "seed {seed}: {:?}",
                    heap.counters
                );
                assert_eq!(
                    heap.observation,
                    report.cells[0].observation.clone().unwrap()
                );
                eprintln!("RETIREMENT_H_ZERO seed={seed} counters={:?}", heap.counters);
            }
            eprintln!(
                "RETIREMENT_RUNTIME seed={seed} cells={} signatures={disagreements:?}",
                report.cells.len()
            );
            if let Some(signature) = disagreements.first() {
                let mut evaluation = 0;
                let (reduced_row, reduced, decisions) = reduce(row, &ops, |r, candidate| {
                    let text = source(r, candidate);
                    let expected = model(r, candidate).unwrap();
                    evaluation += 1;
                    let next = runner.run(&format!("reduce-{seed}-{evaluation}"), &text);
                    // Runtime red cannot shrink into a source/V rejection.
                    let retained = next.validated == report.validated
                        && differences(&next, &expected, &text).contains(signature);
                    if retained {
                        history::persist(
                            &dir,
                            &format!("seed-{seed}-retained-{evaluation}"),
                            seed as u64,
                            &text,
                            &format!("signature={signature:?}\nmodel={expected:?}\n{next:#?}"),
                        );
                    }
                    retained
                });
                history::persist(
                    &dir,
                    &format!("seed-{seed}-reduced"),
                    seed as u64,
                    &source(reduced_row, &reduced),
                    &format!(
                        "signature={signature:?}\n{decisions:#?}\nmodel={:?}",
                        model(reduced_row, &reduced)
                    ),
                );
                failed.push((seed, disagreements));
            }
        }
        eprintln!(
            "RETIREMENT_COUNTS seeds={start}..{end} reference/H/LLVM-release/Cranelift={counts:?}; native resource effect stream unavailable; visible destructor events encoded as output; no native physical claim"
        );
        assert!(failed.is_empty(), "{failed:?}");
        assert_eq!(counts, [end - start; 4]);
    });
}

#[test]
fn negative_twins_require_the_source_rule_and_site() {
    with_compiler_stack(|| {
        let dir = artifacts();
        for seed in 0..12 {
            let row = ROWS[seed];
            let text = source(row, &ops(row));
            // Each independent positive twin changes one parameter passing mode.
            // Generic nested uniqueness is a separate prefix-typing control.
            let (good, bad, rule, marker) = match seed % 3 {
                0 => (
                    "fn negative_probe(take x:Trace) { let moved=take x }",
                    "fn negative_probe(x:Trace) { let moved=take x }",
                    "OWN-1",
                    "take x",
                ),
                1 => (
                    "fn negative_probe(mut x:String) { x=\"changed\" }",
                    "fn negative_probe(x:String) { x=\"changed\" }",
                    "OWN-1",
                    "x=\"changed\"",
                ),
                _ => (
                    "fn need_rows[T](take xs:unique List[unique T]) {}\nfn negative_probe() { let xs:unique List[unique String]=[\"a\"]; need_rows(take xs) }",
                    "fn need_rows[T](take xs:unique List[unique T]) {}\nfn negative_probe() { let xs:unique List[String]=[\"a\"]; need_rows(take xs) }",
                    "MODE-2",
                    "need_rows(take xs)",
                ),
            };
            let positive = format!("{text}{good}\n");
            let negative = format!("{text}{bad}\n");
            history::persist(
                &dir,
                &format!("negative-{seed}-positive"),
                seed as u64,
                &positive,
                "expected=source accepted, V accepted",
            );
            history::persist(
                &dir,
                &format!("negative-{seed}-original"),
                seed as u64,
                &negative,
                &format!("expected={rule} at {marker}; V/runtime not applicable"),
            );
            let mut engine =
                checked(&positive).unwrap_or_else(|e| panic!("positive twin {seed}: {e}"));
            let input = engine.export_native_input().unwrap();
            engine.import_native_input(input).unwrap();
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("main.hir", negative.clone()).unwrap();
            let output = check_progress(&mut engine, file, &default_context())
                .unwrap_or_else(|e| panic!("API error, not diagnostic: {e:?}"));
            let errors: Vec<_> = output
                .diagnostics
                .iter()
                .filter(|d| d.severity == hirsel_diag::Severity::Error)
                .collect();
            assert!(!errors.is_empty(), "negative {seed} accepted");
            assert_eq!(errors[0].rule.as_str(), rule, "negative {seed}: {errors:?}");
            let span = engine.sources.resolve(errors[0].primary);
            let expected_line = negative[..negative.rfind(marker).unwrap()].lines().count();
            assert_eq!(
                negative[..span.start as usize].lines().count(),
                expected_line,
                "negative {seed} site: {errors:?}"
            );
            eprintln!(
                "RETIREMENT_NEGATIVE seed={seed} rule={rule} span={span:?} V=not-applicable engines=0"
            );
        }
    });
}

// Build a fresh cleanup chain that omits precisely the selected whole owner.
// Only the retirement instruction's on_failure field is changed in the clone.
fn without_owner(body: &mut Body, cleanup: CleanupId, root: LocalId) -> CleanupId {
    let actions:Vec<_>=body.cleanup_actions(cleanup).filter(|action| !matches!(action,CleanupAction::Drop{place,..} if body.places[place.0 as usize].root==root)).cloned().collect();
    let mut head = None;
    for action in actions.into_iter().rev() {
        let node = CleanupNodeId(body.cleanup_nodes.len() as u32);
        body.cleanup_nodes.push(CleanupNode {
            action,
            parent: head,
        });
        head = Some(node);
    }
    let id = CleanupId(body.cleanups.len() as u32);
    body.cleanups.push(Cleanup { head });
    id
}

fn mutation_partition(start: usize, end: usize) {
    with_compiler_stack(|| {
        let dir = artifacts();
        let mut mutations = 0;
        // Eight receiving-expression shapes. These mutations target the newly
        // initialized RHS staging owner (Assign), including the operand of
        // Replace; they do not claim a displaced-root mutation.
        for (seed, initialization) in [
            "let new=unique row",
            "let new=(unique row,0u64)",
            "let new:List[String]=[unique row]",
            "let new=Holder(value:unique row)",
            "let new:Option[String]=Some(unique row)",
            "let new:Result[String,u64]=Ok(unique row)",
            "let new=box (unique row)",
            "let new=replace(mut old,unique row)",
        ]
        .into_iter()
        .enumerate()
        .take(end)
        .skip(start)
        {
            let text = format!(
                "type Holder[T] {{ value:T }}\nfn mutation(input:String) {{\nlet row=input\nlet mut old=\"old\"\nfor c in string.scalars(row) {{}}\n{initialization}\nlet _=new\n}}\npub fn main()->Result[(),()] {{ mutation(\"v\"); Ok(()) }}\n"
            );
            let mut engine = checked(&text).unwrap();
            let input = engine.export_native_input().unwrap();
            engine.import_native_input(input.clone()).unwrap();
            let path = input
                .definitions
                .keys()
                .find(|p| p.0.name == "mutation")
                .unwrap();
            let original = &input.definitions[path];
            let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
            signatures.extend(
                input
                    .signatures
                    .iter()
                    .map(|(p, s)| (p.clone(), (**s).clone())),
            );
            assert!(
                hirsel_validate::validate(original, &input.types, &signatures, &mut vec![])
                    .violations
                    .is_empty()
            );
            let mut sites = Vec::new();
            for (bi, body) in original.bodies.iter().enumerate() {
                for (block_id, block) in body.blocks.iter().enumerate() {
                    for (i, pair) in block.instructions.windows(2).enumerate() {
                        let roots: Vec<_> = match pair[0].kind {
                            InstructionKind::Assign { destination, .. } => {
                                vec![body.places[destination.0 as usize].root]
                            }
                            InstructionKind::Replace {
                                destination,
                                displaced,
                                ..
                            } => vec![
                                body.places[destination.0 as usize].root,
                                body.places[displaced.0 as usize].root,
                            ],
                            _ => continue,
                        };
                        if !matches!(pair[1].kind, InstructionKind::Drop { if_live: true, .. }) {
                            continue;
                        }
                        for root in roots {
                            let has = |cleanup| {
                                body.cleanup_actions(cleanup).any(|a|matches!(a,CleanupAction::Drop{place,..} if body.places[place.0 as usize].root==root))
                            };
                            if has(pair[1].on_failure) && !has(pair[0].on_failure) {
                                sites.push((bi, block_id, i + 1, root));
                            }
                        }
                    }
                }
            }
            eprintln!("RETIREMENT_V_SITES seed={seed} {sites:?}");
            let &(bi, block_id, i, root) = sites.first().unwrap_or_else(|| {
                panic!("SETUP zero new-owner retirement sites seed {seed}\n{original:#?}")
            });
            let mut forged = (**original).clone();
            let body = &mut forged.bodies[bi];
            let old = body.blocks[block_id].instructions[i].on_failure;
            let new = without_owner(body, old, root);
            body.blocks[block_id].instructions[i].on_failure = new;
            let result = hirsel_validate::validate(&forged, &input.types, &signatures, &mut vec![]);
            assert!(result.violations.iter().any(|v|matches!(&v.kind,hirsel_validate::ViolationKind::CleanupMismatch{expected,actual,..} if expected.contains(&root) && !actual.contains(&root))),"seed {seed} omission of {root:?}: {result:?}");
            history::persist(
                &dir,
                &format!("v-{seed}-omission"),
                seed as u64,
                &text,
                &format!(
                    "site={bi}/{block_id}/{i} omitted={root:?}\noriginal={original:#?}\nforged={forged:#?}\nexpected=CleanupMismatch\nactual={result:#?}"
                ),
            );
            mutations += 1;
        }
        assert_eq!(mutations, end - start);
        eprintln!(
            "RETIREMENT_V actual_mutations={mutations} originals_validated={mutations} CleanupMismatch_rejections={mutations}"
        );
    });
}

#[test]
fn direct_v_00_06() {
    mutation_partition(0, 7)
}
#[test]
fn direct_v_07() {
    mutation_partition(7, 8)
}

#[test]
fn reducer_injected_emitter_error_is_not_a_compiler_bug() {
    with_compiler_stack(|| {
        let row = ROWS[18];
        let mut original = ops(row);
        original.insert(3, Op::Noise(9));
        original.insert(4, Op::Noise(7));
        let dir = artifacts();
        // Deliberately emit 'wrong' instead of the modeled value. Check source/V
        // and the exact reference model-output mismatch at every retained step.
        let mut evaluations = 0;
        let signature = ("reference".to_owned(), "model-output".to_owned());
        let mut preserves = |r, candidate: &[Op]| {
            let text = if candidate.iter().any(|op| matches!(op, Op::Noise(_))) {
                source(r, candidate)
                    .replace("out.print(output,new.value)", "out.print(output,\"wrong\")")
            } else {
                source(r, candidate)
            };
            let Ok(mut engine) = checked(&text) else {
                return false;
            };
            let Ok(input) = engine.export_native_input() else {
                return false;
            };
            if engine.import_native_input(input).is_err() {
                return false;
            }
            let run = run_observed_snapshot(
                &mut engine,
                &RunConfiguration {
                    entry: entry_path(&default_context()),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            )
            .unwrap();
            let Some(observed) = run.observation else {
                return false;
            };
            let observation = Observation::of_reference(&observed);
            let report = history::Report {
                validated: true,
                cells: vec![history::Cell {
                    engine: "reference".into(),
                    category: history::Category::Observation,
                    observation: Some(observation),
                    detail: String::new(),
                    disagreement: String::new(),
                }],
            };
            evaluations += 1;
            let retained =
                differences(&report, &model(r, candidate).unwrap(), &text) == [signature.clone()];
            if retained {
                history::persist(
                    &dir,
                    &format!("injected-retained-{evaluations}"),
                    18,
                    &text,
                    &format!(
                        "TEST OF REDUCER, NOT COMPILER BUG\nsignature={signature:?}\nops={candidate:?}\nmodel={:?}\n{report:#?}",
                        model(r, candidate)
                    ),
                );
            }
            retained
        };
        history::persist(
            &dir,
            "injected-emitter-original",
            18,
            &source(row, &original)
                .replace("out.print(output,new.value)", "out.print(output,\"wrong\")"),
            &format!(
                "TEST OF REDUCER, NOT COMPILER BUG\nmodel={:?}\nops={original:?}",
                model(row, &original)
            ),
        );
        assert!(preserves(row, &original));
        let (row, reduced, decisions) = reduce(row, &original, &mut preserves);
        assert!(preserves(row, &reduced));
        assert!(evaluations <= 98);
        assert!(reduced.len() < original.len());
        assert!(reduced.contains(&Op::Observe));
        assert_eq!(row[5], 0);
        assert!(reduced.contains(&Op::Noise(0)));
        history::persist(
            &dir,
            "injected-emitter-reducer",
            18,
            &source(row, &reduced)
                .replace("out.print(output,new.value)", "out.print(output,\"wrong\")"),
            &format!(
                "TEST OF REDUCER, NOT COMPILER BUG\nsignature={signature:?}\noriginal={original:?}\nreduced={reduced:?}\n{decisions:#?}"
            ),
        );
        eprintln!(
            "RETIREMENT_REDUCER injected-emitter-error operations={} -> {} evaluations={evaluations} signature={signature:?}",
            original.len(),
            reduced.len()
        );
    });
}

#[test]
fn permanent_manifest_twins() {
    with_compiler_stack(|| {
        let accepted = include_str!("../fixtures/retirement_cleanup_matrix_31/accepted/main.hir");
        let rejected = include_str!("../fixtures/retirement_cleanup_matrix_31/rejected/main.hir");
        for (manifest, source) in [
            (
                include_str!("../fixtures/retirement_cleanup_matrix_31/accepted/hirsel.json"),
                accepted,
            ),
            (
                include_str!("../fixtures/retirement_cleanup_matrix_31/rejected/hirsel.json"),
                rejected,
            ),
        ] {
            let dir = artifacts();
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("hirsel.json"), manifest).unwrap();
            std::fs::write(dir.join("main.hir"), source).unwrap();
            hirsel_driver::load_program_source(&dir.join("hirsel.json"), None).unwrap();
            history::persist(
                &dir,
                "manifest-control",
                0,
                source,
                "permanent complete module set, single source; manifest parsed",
            );
        }
        assert_eq!(accepted.replace("transfer(take x", "transfer(x"), rejected);
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("main.hir", rejected.into()).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        let errors: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|d| d.severity == hirsel_diag::Severity::Error)
            .collect();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].rule.as_str(), "OWN-1");
        let span = engine.sources.resolve(errors[0].primary);
        assert_eq!(rejected[..span.start as usize].lines().count(), 8);
        let dir = artifacts();
        let runner = history::Runner::with_matrix(&dir, matrix()).unwrap();
        let report = runner.run("permanent", accepted);
        assert!(report.agrees(), "{report:#?}");
        assert_eq!(report.cells.len(), 4);
        assert!(report.disagreements(b"", false).is_empty(), "{report:#?}");
        eprintln!(
            "RETIREMENT_FIXTURE manifests=2 accepted_source_V=1 rejected_OWN1=1 engine_observations=4"
        );
    });
}

#[test]
fn runtime_00_07() {
    runtime_partition(0, 8)
}
#[test]
fn runtime_08_15() {
    runtime_partition(8, 16)
}
#[test]
fn runtime_16_23() {
    runtime_partition(16, 24)
}
