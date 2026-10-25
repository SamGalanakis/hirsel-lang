//! Finite pairwise histories, not an exhaustive ownership proof.
//! Shapes adapt history.rs and ownership_history_21.rs at 8530c5a0, and rustc
//! initialized.rs:255-303 / framework/tests.rs:1-68 at 19833af7 (distinct
//! before/after-call and normal/error edges). No upstream code is copied;
//! Hirsel's CLO/OWN/MODE rules, never Rust borrowing, define validity.
#[path = "../support/history.rs"]
mod history;
use hirsel_driver::{native::*, *};
use hirsel_engine::*;
use history::{Category, Cell, Report, Runner, minimize, persist, query_error};
use std::collections::BTreeSet;

// Seed 310031, greedy maximum uncovered-pair gain, lexically frozen tie order.
// capture: borrow/own/mixed; sink: statement/wildcard/binding;
// wrapper: direct/one/two forwarding layers; payload: scalar/record/resource;
// Result: Ok/Error; calls: 0/1/2; scope: frame/block/loop; use: before/after.
// Owning/mixed callbacks are once: calls=2 is infeasible (MODE-3).
const ROWS: [[u8; 8]; 24] = [
    [2, 0, 1, 2, 1, 0, 0, 0],
    [0, 2, 2, 1, 0, 2, 0, 1],
    [1, 1, 0, 0, 1, 1, 2, 1],
    [1, 2, 2, 2, 0, 1, 1, 0],
    [0, 0, 0, 0, 0, 0, 2, 0],
    [2, 1, 1, 1, 0, 0, 1, 1],
    [0, 1, 0, 2, 1, 2, 1, 0],
    [2, 2, 2, 0, 1, 0, 2, 0],
    [1, 0, 1, 1, 1, 1, 2, 0],
    [0, 0, 1, 0, 1, 2, 1, 1],
    [2, 2, 0, 2, 0, 1, 0, 1],
    [1, 1, 2, 0, 0, 0, 0, 0],
    [0, 2, 1, 2, 1, 1, 2, 0],
    [0, 0, 2, 1, 0, 2, 2, 1],
    [1, 2, 0, 1, 1, 1, 0, 1],
    [0, 0, 2, 0, 0, 2, 1, 0],
    [2, 0, 2, 2, 1, 0, 1, 0],
    [1, 0, 1, 2, 0, 0, 2, 1],
    [2, 2, 1, 2, 0, 1, 1, 0],
    [0, 2, 2, 0, 0, 1, 1, 0],
    [2, 2, 2, 2, 1, 1, 2, 1],
    [1, 2, 2, 1, 1, 0, 0, 1],
    [0, 0, 0, 1, 1, 2, 1, 1],
    [0, 2, 2, 1, 1, 0, 0, 0],
];
// The pair cover did not itself ensure exact before/after twins for each
// capture x lexical-scope interaction. Freeze these nine additional histories
// before consumption, for 33 accepted histories (below the ceiling of 48).
const SCOPE_TWINS: [usize; 9] = [1, 6, 4, 11, 3, 2, 0, 5, 7];
const HISTORY_COUNT: usize = ROWS.len() + SCOPE_TWINS.len();
const SEED: u64 = 310031;
const PRELUDE: &str = r#"type Payload { n: u64 }
resource type Trace { out: Stdout, text: String, n: u64 }
drop(x) { let _ = out.print(x.out, x.text) }
fn once_run[T](take f: (once fn() -> Result[T, ()]) in f, calls: u64) -> Option[Result[T, ()]] {
 if calls == 0 { None } else { Some(f()) }
}
fn many_run[T](f: fn() -> Result[T, ()], calls: u64) -> Option[Result[T, ()]] {
 if calls == 0 { None } else { if calls == 2 { let _ = f() }; Some(f()) }
}
fn once_wrap[T](take f: (once fn() -> Result[T, ()]) in f, calls: u64) -> Option[Result[T, ()]] { once_run(take f, calls) }
fn once_nested[T](take f: (once fn() -> Result[T, ()]) in f, calls: u64) -> Option[Result[T, ()]] { once_wrap(take f, calls) }
fn many_wrap[T](f: fn() -> Result[T, ()], calls: u64) -> Option[Result[T, ()]] { many_run(f, calls) }
fn many_nested[T](f: fn() -> Result[T, ()], calls: u64) -> Option[Result[T, ()]] { many_wrap(f, calls) }
"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Begin,
    Before,
    Call,
    After,
    End,
    Noise,
}
#[derive(Clone, Debug)]
struct History {
    row: [u8; 8],
    value: u64,
    ops: Vec<Op>,
}
impl History {
    fn fixed(index: usize) -> Self {
        let mut row = ROWS[if index < ROWS.len() {
            index
        } else {
            SCOPE_TWINS[index - ROWS.len()]
        }];
        if index >= ROWS.len() {
            row[7] = 1 - row[7];
        }
        Self {
            row,
            value: 7,
            ops: vec![
                Op::Begin,
                Op::Before,
                Op::Noise,
                Op::Call,
                Op::After,
                Op::End,
            ],
        }
    }
    fn iterations(&self) -> usize {
        if self.row[6] == 2 && self.row[5] <= 1 {
            2
        } else {
            1
        }
    }
    fn valid(&self) -> bool {
        self.ops.len() <= 12
            && self.value <= 7
            && self
                .row
                .iter()
                .zip([3, 3, 3, 3, 2, 3, 3, 2])
                .all(|(v, n)| *v < n)
            && !(self.row[0] != 0 && self.row[5] == 2)
            && self.iterations() * usize::from(self.row[5]) <= 2
            && self.ops.first() == Some(&Op::Begin)
            && self.ops.last() == Some(&Op::End)
            && self.ops.iter().filter(|op| **op == Op::Call).count() == 1
            && self.ops.iter().filter(|op| **op == Op::Begin).count() == 1
            && self.ops.iter().filter(|op| **op == Op::End).count() == 1
    }
    fn source(&self, inject: bool) -> String {
        assert!(self.valid());
        let [capture, sink, wrapper, payload, error, calls, scope, after] = self.row;
        let ty = ["u64", "Payload", "Trace"][payload as usize];
        let result = if error == 1 {
            "Error(())".to_owned()
        } else {
            let value = match payload {
                0 => format!("{}u64", self.value),
                1 => format!("Payload(n: {}u64)", self.value),
                _ => format!("Trace(out: out, text: \"r\", n: {}u64)", self.value),
            };
            format!("Ok({value})")
        };
        let borrowed = if capture != 1 {
            "let _ = list.len(base);"
        } else {
            ""
        };
        let owned = if capture != 0 {
            "let _ = take token;"
        } else {
            ""
        };
        let family = if capture == 0 { "many" } else { "once" };
        let suffix = ["run", "wrap", "nested"][wrapper as usize];
        let callback = format!(
            "fn() -> Result[{ty}, ()] {{ {borrowed} let _ = out.print(out, \"c\"); {owned} {result} }}"
        );
        let call = format!("{family}_{suffix}({callback}, {calls}u64)");
        let payload_value = if payload == 0 { "p" } else { "p.n" };
        let observed_value = if inject { self.value + 1 } else { self.value };
        let observe = format!(
            "match retained {{ Some(Ok(p)) => {{ let _ = out.print(out, if {payload_value} == {observed_value}u64 {{ \"7\" }} else {{ \"!\" }}) }}, Some(Error(e)) => {{ let _ = out.print(out, \"e\") }}, None => {{ let _ = out.print(out, \"0\") }} }}"
        );
        let use_base = "let _ = out.print(out, if list.len(base) == 1 { \"b\" } else { \"!\" });\n";
        let mut body = String::new();
        for op in &self.ops {
            match op {
                Op::Begin => {
                    if capture != 0 {
                        body.push_str("let token = Trace(out: out, text: \"d\", n: 1);\n");
                    }
                }
                Op::Before if after == 0 => body.push_str(use_base),
                Op::After if after == 1 => body.push_str(use_base),
                Op::Call => {
                    match sink {
                        0 => body.push_str(&format!("{call};\n")),
                        1 => body.push_str(&format!("let _ = {call};\n")),
                        _ => body.push_str(&format!(
                            "let retained: Option[Result[{ty}, ()]] = {call};\n{observe};\n"
                        )),
                    };
                    body.push_str("let _ = out.print(out, \"|\");\n");
                }
                Op::Noise => body.push_str("let noise = 0u64;\n"),
                _ => {}
            }
        }
        let body = match scope {
            0 => body,
            1 => format!("{{\n{body}}}\n"),
            _ => format!(
                "for iteration in 0u64..{}u64 {{\n{body}}}\n",
                self.iterations()
            ),
        };
        let text = format!(
            "{PRELUDE}fn probe(out: Stdout, base: List[u64]) {{\n{body}}}\npub fn main(rt: Io) -> Result[(), GrantError] {{ let out = io.stdout(rt)?; probe(out, [1u64]); Ok(()) }}\n"
        );
        assert!(text.lines().count() <= 128);
        text
    }
}
// An explicit operation/value model: no compiler queries or ownership helpers.
#[derive(Debug)]
struct Model {
    output: Vec<u8>,
    live: BTreeSet<&'static str>,
    transfers: usize,
    drops: Vec<&'static str>,
}
fn model(h: &History) -> Option<Model> {
    if !h.valid() {
        return None;
    }
    let [capture, sink, _, payload, error, calls, _, after] = h.row;
    let mut m = Model {
        output: vec![],
        live: BTreeSet::new(),
        transfers: 0,
        drops: vec![],
    };
    for _ in 0..h.iterations() {
        for op in &h.ops {
            match op {
                Op::Begin => {
                    m.live.insert("base");
                    if capture != 0 {
                        m.live.insert("token");
                    }
                }
                Op::Before if after == 0 => m.output.push(b'b'),
                Op::After if after == 1 => m.output.push(b'b'),
                Op::Call => {
                    for invocation in 0..calls {
                        if capture != 1 && !m.live.contains("base") {
                            return None;
                        }
                        m.output.push(b'c');
                        if capture != 0 {
                            if !m.live.remove("token") {
                                return None;
                            }
                            m.transfers += 1;
                            m.output.push(b'd');
                            m.drops.push("token");
                        }
                        if payload == 2 && error == 0 {
                            if sink == 2 && invocation + 1 == calls {
                                m.live.insert("result");
                            } else {
                                m.output.push(b'r');
                                m.drops.push("result");
                            }
                        }
                    }
                    if calls == 0 && m.live.remove("token") {
                        m.output.push(b'd');
                        m.drops.push("token");
                    }
                    if sink == 2 {
                        m.output.push(if calls == 0 {
                            b'0'
                        } else if error == 1 {
                            b'e'
                        } else {
                            b'7'
                        });
                    }
                    m.output.push(b'|');
                }
                Op::End => {
                    if m.live.remove("result") {
                        m.output.push(b'r');
                        m.drops.push("result");
                    }
                    m.live.remove("base");
                }
                _ => {}
            }
        }
    }
    m.live.is_empty().then_some(m)
}

fn check_source(source: &str) -> Result<QueryEngine, String> {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("main.hir", source.to_owned()).unwrap();
    let checked =
        check_progress(&mut engine, file, &default_context()).map_err(|e| format!("API:{e:?}"))?;
    let diagnostics: Vec<_> = checked
        .diagnostics
        .iter()
        .filter(|d| d.severity == hirsel_diag::Severity::Error)
        .collect();
    if !diagnostics.is_empty() {
        return Err(format!("source:{diagnostics:?}"));
    }
    Ok(engine)
}

fn configurations() -> Vec<Configuration> {
    [
        Engine::Heap { campaign: false },
        Engine::Native {
            opt: OptLevel::Speed,
            partition: Partition::Whole,
            runtime: Runtime::Default,
        },
        Engine::Cranelift {
            partition: Partition::Whole,
            runtime: Runtime::Default,
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
fn artifacts(label: &str) -> std::path::PathBuf {
    let path = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("scoped-loan-matrix-31");
    // Every invocation owns a fresh directory; persist never overwrites an attempt.
    std::fs::create_dir_all(&path).unwrap();
    tempfile::Builder::new()
        .prefix(label)
        .tempdir_in(path)
        .unwrap()
        .keep()
}
fn source_report(source: &str) -> Result<QueryEngine, Report> {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("main.hir", source.into()).unwrap();
    let checked = check_progress(&mut engine, file, &default_context()).map_err(|e| Report {
        validated: false,
        cells: vec![query_error("source", e)],
    })?;
    let diagnostics: Vec<_> = checked
        .diagnostics
        .iter()
        .filter(|d| d.severity == hirsel_diag::Severity::Error)
        .collect();
    eprintln!("SCOPED_SOURCE {:?}", checked.diagnostics);
    if diagnostics.is_empty() {
        Ok(engine)
    } else {
        Err(Report {
            validated: false,
            cells: vec![Cell {
                engine: "source".into(),
                category: Category::SourceRejection,
                observation: None,
                detail: format!("{diagnostics:?}"),
                disagreement: format!(
                    "{:?}",
                    diagnostics
                        .iter()
                        .map(|d| d.rule.as_str())
                        .collect::<Vec<_>>()
                ),
            }],
        })
    }
}
fn execute(runner: &Runner, name: &str, source: &str) -> Report {
    let mut engine = match source_report(source) {
        Ok(engine) => engine,
        Err(report) => return report,
    };
    let input = match engine.export_native_input() {
        Ok(input) => input,
        Err(e) => {
            return Report {
                validated: false,
                cells: vec![query_error("V-export", e)],
            };
        }
    };
    let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
    signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
    for definition in input.definitions.values() {
        let validation =
            hirsel_validate::validate(definition, &input.types, &signatures, &mut vec![]);
        if !validation.violations.is_empty() {
            return Report {
                validated: false,
                cells: vec![Cell {
                    engine: "V".into(),
                    category: Category::VRejection,
                    observation: None,
                    detail: format!("{validation:?}"),
                    disagreement: format!(
                        "{:?}",
                        validation
                            .violations
                            .iter()
                            .map(|v| &v.kind)
                            .collect::<Vec<_>>()
                    ),
                }],
            };
        }
    }
    let validated = match engine.import_native_input(input) {
        Ok(v) => v,
        Err(e) => {
            return Report {
                validated: false,
                cells: vec![query_error("V-import", e)],
            };
        }
    };
    let entry = entry_path(&default_context());
    let run = match run_observed_snapshot(
        &mut engine,
        &RunConfiguration {
            entry: entry.clone(),
            hash_seed: SEED,
            inputs: vec![],
        },
        SimulationConfig::default(),
    ) {
        Ok(run) => run,
        Err(e) => {
            return Report {
                validated: true,
                cells: vec![Cell {
                    engine: "reference".into(),
                    category: history::tooling(&format!("{e:?}")),
                    observation: None,
                    detail: format!("{e:?}"),
                    disagreement: format!("{e:?}"),
                }],
            };
        }
    };
    let Some(observation) = run.observation else {
        return Report {
            validated: true,
            cells: vec![Cell {
                engine: "reference".into(),
                category: if matches!(&run.outcome, Ok(RunOutcome::ToolingLimit(_))) {
                    Category::ToolingLimit
                } else {
                    Category::ToolError
                },
                observation: None,
                detail: format!("{run:?}"),
                disagreement: format!("no-completed-observation:{:?}", run.outcome),
            }],
        };
    };
    runner.compare(
        &validated,
        &entry,
        &Observation::of_reference(&observation),
        name,
    )
}
fn differences(report: &Report, expected: &Model) -> Vec<(String, String)> {
    let mut differences = report.disagreements(&expected.output, false);
    // No roots, signals, budgets or language failures belong to this synchronous
    // model. Compare all available fields independently of reference agreement.
    for cell in &report.cells {
        if let Some(o) = &cell.observation {
            if !o.roots.is_empty() {
                differences.push((cell.engine.clone(), "model-roots".into()));
            }
        }
    }
    if report.cells.is_empty() {
        differences.push(("setup".into(), "zero-cells".into()));
    }
    differences
}
// Deletion, scalar and wrapper/scope-depth reductions share a hard evaluation
// budget. The predicate carries the exact phase plus rule/engine/field signature.
fn reduce(
    original: &History,
    mut preserves: impl FnMut(&History) -> bool,
) -> (History, usize, Vec<String>) {
    let mut count = 0;
    let mut decisions = vec![];
    let mut best = original.clone();
    best.ops = minimize(&original.ops, |ops| {
        if count == 96 {
            return false;
        }
        let mut candidate = original.clone();
        candidate.ops = ops.to_vec();
        if model(&candidate).is_none() {
            return false;
        }
        count += 1;
        let keep = preserves(&candidate);
        decisions.push(format!("{count}:delete:{ops:?}:{keep}"));
        keep
    });
    for value in [0, 1] {
        if count == 96 {
            break;
        }
        if value >= best.value {
            continue;
        }
        let mut candidate = best.clone();
        candidate.value = value;
        count += 1;
        let keep = preserves(&candidate);
        decisions.push(format!("{count}:value:{value}:{keep}"));
        if keep {
            best = candidate;
        }
    }
    for axis in [2, 6] {
        while best.row[axis] > 0 && count < 96 {
            let mut candidate = best.clone();
            candidate.row[axis] -= 1;
            if model(&candidate).is_none() {
                break;
            }
            count += 1;
            let keep = preserves(&candidate);
            decisions.push(format!("{count}:depth:{axis}:{keep}"));
            if keep {
                best = candidate;
            } else {
                break;
            }
        }
    }
    (best, count, decisions)
}
fn partition(first: usize) {
    with_compiler_stack(|| {
        let directory = artifacts(&format!("partition-{first}-"));
        let runner = Runner::with_matrix(&directory.join("runs"), configurations())
            .expect("INCOMPLETE toolchain");
        let mut counts = std::collections::BTreeMap::new();
        let mut failures = vec![];
        let count = (HISTORY_COUNT - first).min(4);
        for index in first..first + count {
            let h = History::fixed(index);
            let expected = model(&h).unwrap();
            let source = h.source(false);
            persist(
                &directory,
                &format!("seed-{index}-original"),
                SEED + index as u64,
                &source,
                &format!("history={h:?}\nmodel={expected:?}"),
            );
            let report = execute(&runner, &format!("seed-{index}"), &source);
            for cell in &report.cells {
                *counts
                    .entry(format!("{}:{:?}", cell.engine, cell.category))
                    .or_insert(0usize) += 1;
            }
            let disagreement = differences(&report, &expected);
            eprintln!(
                "SCOPED_RESULT seed={} disagreements={disagreement:?} report={report:#?}",
                SEED + index as u64
            );
            if !disagreement.is_empty() {
                persist(
                    &directory,
                    &format!("seed-{index}-red"),
                    SEED + index as u64,
                    &source,
                    &format!("model={expected:?}\n{report:#?}"),
                );
                let signature = disagreement[0].clone();
                let mut evaluation = 0;
                let (best, count, decisions) = reduce(&h, |candidate| {
                    evaluation += 1;
                    let result = execute(
                        &runner,
                        &format!("reduce-{index}-{evaluation}"),
                        &candidate.source(false),
                    );
                    let keep = result.validated == report.validated
                        && differences(&result, &model(candidate).unwrap()).contains(&signature);
                    if keep {
                        persist(
                            &directory,
                            &format!("seed-{index}-retained-{evaluation}"),
                            SEED + index as u64,
                            &candidate.source(false),
                            &format!(
                                "signature={signature:?} model={:?}\n{result:#?}",
                                model(candidate).unwrap()
                            ),
                        );
                    }
                    keep
                });
                persist(
                    &directory,
                    &format!("seed-{index}-reduced"),
                    SEED + index as u64,
                    &best.source(false),
                    &format!(
                        "signature={signature:?}\noriginal={h:?}\nreduced={best:?}\nevaluations={count}\n{decisions:?}"
                    ),
                );
                failures.push((index, disagreement));
            }
        }
        eprintln!("SCOPED_COUNTS partition={first} {counts:?}");
        assert!(failures.is_empty(), "{failures:?}");
        assert_eq!(counts.len(), 4);
        assert!(counts.values().all(|n| *n == count));
    });
}

#[test]
fn twelve_source_negative_twins() {
    with_compiler_stack(|| {
        let directory = artifacts("negatives-");
        for index in 0..12 {
            let h = History::fixed(index);
            let prefix = h.source(false);
            let (positive, negative, rule, site) = match index % 3 {
                0 => (
                    "fn reject(take x: Trace) -> once fn() -> () uses {Stdout} { fn() { let _ = take x } }",
                    "fn reject(x: Trace) -> once fn() -> () uses {Stdout} { fn() { let _ = take x } }",
                    "CLO-4",
                    "fn() { let _ = take x }",
                ),
                1 => (
                    "fn reject(take x: List[u64]) -> fn() -> u64 uses {} { fn() { list.len(x) } }",
                    "fn reject(x: List[u64]) -> fn() -> u64 uses {} { fn() { list.len(x) } }",
                    "CLO-4",
                    "fn() { list.len(x) }",
                ),
                _ => (
                    "fn reject(x: List[u64]) -> List[u64] in x { x }",
                    "fn reject(x: List[u64]) -> List[u64] { x }",
                    "MODE-1",
                    "x",
                ),
            };
            let source = format!("{prefix}{positive}\n");
            let mut engine = check_source(&source).unwrap();
            let input = engine.export_native_input().unwrap();
            engine.import_native_input(input).unwrap();
            let source = format!("{prefix}{negative}\n");
            persist(
                &directory,
                &format!("negative-{index}"),
                SEED + index as u64,
                &source,
                &format!("expected={rule} site={site} positive={positive}"),
            );
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("main.hir", source).unwrap();
            let checked = check_progress(&mut engine, file, &default_context())
                .expect("API failure, not source rejection");
            let errors: Vec<_> = checked
                .diagnostics
                .iter()
                .filter(|d| d.severity == hirsel_diag::Severity::Error)
                .collect();
            assert_eq!(errors.len(), 1, "index={index}: {errors:?}");
            assert_eq!(errors[0].rule.as_str(), rule, "index={index}: {errors:?}");
            assert_eq!(
                checked.sources.slice(errors[0].primary),
                Some(site),
                "index={index}: {errors:?}"
            );
            eprintln!(
                "SCOPED_NEGATIVE index={index} rule={rule} site={:?}",
                errors[0].primary
            );
        }
    });
}

#[test]
fn eight_direct_v_premature_loan_ends() {
    use hirsel_tir::{InstructionKind, Rvalue, Terminator};
    use hirsel_validate::ViolationKind;
    with_compiler_stack(|| {
        let directory = artifacts("v-adversaries-");
        let mut mutations = 0;
        for seed_index in [0, 5, 7, 10] {
            let h = History::fixed(seed_index);
            let source = h.source(false);
            let mut engine = check_source(&source).unwrap();
            let input = engine.export_native_input().unwrap();
            let mut path = entry_path(&default_context());
            path.0.name = "probe".into();
            let original = &input.definitions[&path];
            let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
            signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
            hirsel_validate::assert_valid(original, &input.types, &signatures);
            let proof = engine.export_tir(&path).unwrap();
            for at_call in [false, true] {
                let mut forged = (**original).clone();
                let body = &mut forged.bodies[0];
                let (creation_block, creation_index, _holder, capture, loan) = body
                    .blocks
                    .iter()
                    .enumerate()
                    .find_map(|(b, block)| {
                        block.instructions.iter().enumerate().find_map(|(i, inst)| {
                            let InstructionKind::Assign { destination, value } = inst.kind else {
                                return None;
                            };
                            let Rvalue::Closure { captures, .. } = &body.rvalues[value.0 as usize]
                            else {
                                return None;
                            };
                            captures.iter().find_map(|capture| {
                                let place = &body.places[capture.place.0 as usize];
                                (place.root == body.parameters[1])
                                    .then_some(place.loan)
                                    .flatten()
                                    .map(|loan| (b, i, destination, capture.place, loan))
                            })
                        })
                    })
                    .expect("identified borrowed callback producer");
                let (block, index, expected_place) = if at_call {
                    let b = creation_block;
                    let Terminator::Call { arguments, .. } = &body.blocks[b].terminator else {
                        panic!("callback consumer")
                    };
                    let actual = arguments
                        .iter()
                        .find(|a| a.parameter.0 == 0)
                        .unwrap()
                        .value
                        .place;
                    (b, body.blocks[b].instructions.len(), actual)
                } else {
                    (creation_block, creation_index, capture)
                };
                let mut end = body.blocks[creation_block].instructions[creation_index].clone();
                end.kind = InstructionKind::EndLoan {
                    loan,
                    if_active: false,
                };
                body.blocks[block].instructions.insert(index, end);
                let validation =
                    hirsel_validate::validate(&forged, &input.types, &signatures, &mut vec![]);
                persist(
                    &directory,
                    &format!("seed-{seed_index}-at-call-{at_call}"),
                    SEED + seed_index as u64,
                    &source,
                    &format!(
                        "mutation=EndLoan block={block} instruction={index} loan={loan:?} expected_place={expected_place:?}\nportable_original={proof}\noriginal={original:#?}\nforged={forged:#?}\nvalidation={validation:?}"
                    ),
                );
                assert!(
                    validation.violations.iter().any(|v| v.kind
                        == ViolationKind::ExpiredLoan {
                            loan,
                            place: expected_place
                        }),
                    "{validation:?}"
                );
                mutations += 1;
            }
        }
        assert_eq!(mutations, 8);
        eprintln!("SCOPED_V mutations={mutations}");
    });
}

const TASK_CONTROL: &str = r#"resource type Trace { out: Stdout, text: String }
drop(x) { let _ = out.print(x.out,x.text) }
fn probe(tasks: Tasks, out: Stdout, base: List[u64], take token: Trace) {
 let retained: u64 = task.scope(tasks,fn(s: Scope) -> u64 {
  let _ = list.len(base)
  let _ = take token
  7u64
 })
 let _ = out.print(out,if retained == 7 { "7" } else { "!" })
 let _ = out.print(out,if list.len(base) == 1 { "b" } else { "!" })
}
pub fn main(rt: Io) -> Result[(), GrantError] {
 let out = io.stdout(rt)?
 let tasks = io.tasks(rt)?
 probe(tasks,out,[1u64],Trace(out: out,text: "d"))
 Ok(())
}
"#;
#[test]
fn real_task_scope_public_adapters() {
    with_compiler_stack(|| {
        let directory = artifacts("task-control-");
        let runner = Runner::with_matrix(&directory.join("runs"), configurations()).unwrap();
        persist(
            &directory,
            "task-scope",
            SEED,
            TASK_CONTROL,
            "expected=d7b: token destruction then scalar then base; no task spawning/E1 assertions",
        );
        let report = execute(&runner, "task-scope", TASK_CONTROL);
        assert!(report.agrees(), "{report:#?}");
        assert_eq!(report.cells.len(), 4);
        assert!(
            report.disagreements(b"d7b", false).is_empty(),
            "{report:#?}"
        );
        eprintln!("SCOPED_TASK engines=4");
    });
}

#[test]
fn permanent_manifest_source_twins() {
    with_compiler_stack(|| {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/scoped_loan_matrix_31");
        let mut owned = load_program_source(&root.join("owned.json"), None).unwrap();
        owned.check_for_execution().unwrap();
        let mut borrowed = load_program_source(&root.join("borrowed.json"), None).unwrap();
        assert!(borrowed.check_for_execution().is_err());
        let source = include_str!("../fixtures/scoped_loan_matrix_31/borrowed.hir");
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("borrowed.hir", source.into()).unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert_eq!(checked.diagnostics.len(), 1, "{:?}", checked.diagnostics);
        assert_eq!(checked.diagnostics[0].rule.as_str(), "OWN-1");
    });
}

#[test]
fn runtime_24_27_exact_scope_twins() {
    partition(24)
}
#[test]
fn runtime_28_31_exact_scope_twins() {
    partition(28)
}
#[test]
fn runtime_32_exact_scope_twin() {
    partition(32)
}

// Related results must freeze the actual source, including through the
// closure's environment, until the last use. These three frozen accepted
// histories bring the total to 36; the negative twin moves only the write.
#[test]
fn call_result_retention_has_exact_source_scope() {
    with_compiler_stack(|| {
        let directory = artifacts("result-loans-");
        let runner = Runner::with_matrix(&directory.join("runs"), configurations()).unwrap();
        for scope in 0..3 {
            let before = "let retained = (fn() { view(x) })();\nlet _ = out.print(out, if bytes.len(retained) == 1 { \"1\" } else { \"!\" });\nbump(mut x);\nlet _ = out.print(out, if bytes.len(x) == 2 { \"2\" } else { \"!\" });\n";
            let after = "let retained = (fn() { view(x) })();\nbump(mut x);\nlet _ = out.print(out, if bytes.len(retained) == 1 { \"1\" } else { \"!\" });\nlet _ = out.print(out, if bytes.len(x) == 2 { \"2\" } else { \"!\" });\n";
            let emit = |body: &str| {
                let body = match scope {
                    0 => body.to_owned(),
                    1 => format!("{{\n{body}}}\n"),
                    _ => format!("for iteration in 0u64..2u64 {{\nx = b\"a\";\n{body}}}\n"),
                };
                format!(
                    "fn view(x: Bytes) -> Bytes in x {{ copy x }}\nfn bump(mut x: Bytes) {{ x = b\"bc\" }}\nfn probe(out: Stdout, mut x: Bytes) {{\n{body}}}\npub fn main(rt: Io) -> Result[(), GrantError] {{ let out = io.stdout(rt)?; let mut x: Bytes = b\"a\"; probe(out, mut x); Ok(()) }}\n"
                )
            };
            let source = emit(before);
            persist(
                &directory,
                &format!("scope-{scope}-accepted"),
                SEED + 33 + scope,
                &source,
                "model: source x remains live; retained holds x until len; bump follows last use; output=12 per iteration",
            );
            let report = execute(&runner, &format!("scope-{scope}"), &source);
            let expected: &[u8] = if scope == 2 { b"1212" } else { b"12" };
            assert!(report.agrees(), "scope={scope}: {report:#?}");
            assert_eq!(report.cells.len(), 4);
            assert!(
                report.disagreements(expected, false).is_empty(),
                "scope={scope}: {report:#?}"
            );
            let source = emit(after);
            persist(
                &directory,
                &format!("scope-{scope}-negative"),
                SEED + 33 + scope,
                &source,
                "MODE-1 at bump(mut x): returned content still holds a loan on x",
            );
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("main.hir", source).unwrap();
            let checked = check_progress(&mut engine, file, &default_context()).unwrap();
            let diagnostics: Vec<_> = checked
                .diagnostics
                .iter()
                .filter(|d| d.severity == hirsel_diag::Severity::Error)
                .collect();
            assert_eq!(diagnostics.len(), 1, "scope={scope}: {diagnostics:?}");
            assert_eq!(
                diagnostics[0].rule.as_str(),
                "MODE-1",
                "scope={scope}: {diagnostics:?}"
            );
            assert_eq!(
                checked.sources.slice(diagnostics[0].primary),
                Some("bump(mut x)"),
                "scope={scope}: {diagnostics:?}"
            );
            eprintln!(
                "SCOPED_RESULT_LOAN scope={scope} positive-engines=4 negative=MODE-1 source=x"
            );
        }
    });
}
