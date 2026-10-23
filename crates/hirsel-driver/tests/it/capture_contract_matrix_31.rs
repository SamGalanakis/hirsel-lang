//! Finite capture histories, independently modelled from CLO-1/CLO-4 and MODE-3.
//! The table is fixed before compiler consumers. It covers feasible axis pairs,
//! not all programs. History/reduction structure follows ownership_history_21;
//! bounded shrinking follows proptest traits.rs:580-621/num.rs:995-1069 at
//! 70cf37d46b7d (no upstream implementation copied). No Rust borrow oracle.
use hirsel_driver::{native::*, *};
use hirsel_engine::*;
use hirsel_tir::{Rvalue, Storage};
use hirsel_types::UseKind;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[path = "../support/history.rs"]
#[allow(
    dead_code,
    reason = "shared runner includes other lanes' configurations"
)]
mod history;

// capture: borrow/copy/take; multiplicity: many/once; callback: direct/scoped/take;
// payload: scalar/String/Holder[String]; returned: direct/nested; empty; calls.
const TABLE: [[u8; 7]; 24] = [
    [0, 0, 0, 0, 0, 0, 0],
    [1, 0, 1, 1, 1, 1, 1],
    [2, 1, 2, 2, 0, 0, 1],
    [1, 1, 0, 2, 1, 1, 0],
    [1, 0, 2, 0, 0, 1, 2],
    [2, 1, 1, 0, 1, 0, 0],
    [0, 0, 1, 1, 0, 0, 2],
    [2, 1, 2, 1, 1, 1, 0],
    [0, 0, 0, 2, 0, 1, 1],
    [1, 0, 0, 2, 1, 0, 2],
    [2, 1, 0, 0, 0, 1, 1],
    [1, 1, 0, 1, 0, 0, 1],
    [1, 0, 1, 2, 0, 1, 0],
    [1, 0, 2, 0, 1, 0, 1],
    [1, 0, 2, 1, 0, 0, 0],
    [0, 0, 0, 1, 0, 1, 2],
    [0, 0, 1, 0, 0, 1, 1],
    [2, 1, 0, 1, 1, 0, 1],
    [2, 1, 1, 1, 0, 1, 0],
    [1, 0, 1, 0, 1, 1, 2],
    [1, 0, 1, 2, 1, 0, 1],
    [1, 0, 2, 2, 1, 1, 2],
    [1, 1, 2, 0, 0, 1, 0],
    [0, 0, 1, 2, 0, 0, 0],
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Row([u8; 7]);
impl Row {
    fn valid(self) -> bool {
        let [capture, once, callback, payload, returned, empty, calls] = self.0;
        capture < 3
            && once < 2
            && callback < 3
            && payload < 3
            && returned < 2
            && empty < 2
            && calls < 3
            && !(capture == 0 && (once != 0 || callback == 2 || returned != 0))
            && !(capture == 2 && once == 0)
            && !(once == 1 && calls == 2)
            && !(once == 1 && callback == 1 && calls != 0)
    }
    fn once(self) -> &'static str {
        if self.0[1] == 1 { "once " } else { "" }
    }
    fn ty(self) -> &'static str {
        ["u64", "String", "Holder[String]"][self.0[3] as usize]
    }
    fn value(self) -> &'static str {
        match (self.0[3], self.0[5]) {
            (0, 0) => "2u64",
            (0, _) => "0u64",
            (1, 0) => "\"ab\"",
            (1, _) => "\"\"",
            (2, 0) => "Holder(value:\"ab\")",
            _ => "Holder(value:\"\")",
        }
    }
    fn score(self, name: &str) -> String {
        match self.0[3] {
            0 => name.into(),
            1 => format!("string.byte_len({name})"),
            _ => format!("string.byte_len({name}.value)"),
        }
    }
    fn show(self, name: &str) -> String {
        match self.0[3] {
            0 => format!("digit({name})"),
            1 => name.into(),
            _ => format!("{name}.value"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Construct,
    Save,
    Create,
    Invoke,
    ObserveSaved,
    Noise,
}
#[derive(Clone, Debug, Default)]
struct Model {
    live: bool,
    saved: Option<String>,
    environment: Option<String>,
    invocations: u8,
    output: String,
    destructors: Vec<&'static str>,
}
fn history(row: Row) -> Vec<Op> {
    let mut ops = vec![Op::Construct];
    if row.0[0] != 2 {
        ops.push(Op::Save);
    }
    ops.push(Op::Create);
    for _ in 0..row.0[6] {
        ops.push(Op::Invoke);
    }
    if row.0[0] != 2 {
        ops.push(Op::ObserveSaved);
    }
    ops.push(Op::Noise);
    ops
}
fn model(row: Row, ops: &[Op]) -> Option<Model> {
    if !row.valid() || ops.len() > 12 {
        return None;
    }
    let value = if row.0[3] == 0 {
        if row.0[5] == 0 { "2" } else { "0" }
    } else if row.0[5] == 0 {
        "ab"
    } else {
        ""
    };
    let mut m = Model::default();
    let mut constructed = false;
    for op in ops {
        match op {
            Op::Construct if !constructed => {
                constructed = true;
                m.live = true;
            }
            Op::Save if m.live && m.saved.is_none() => m.saved = Some(value.into()),
            Op::Create if m.live && m.environment.is_none() => {
                m.environment = Some(value.into());
                if row.0[0] == 2 {
                    m.live = false;
                }
            }
            Op::Invoke if m.environment.is_some() => {
                // Once is consumed, irrespective of whether construction copied or took.
                if row.0[1] == 1 && m.invocations != 0 {
                    return None;
                }
                m.invocations += 1;
                if m.invocations > 2 {
                    return None;
                }
                m.output.push_str(if row.0[5] == 0 { "2;" } else { "0;" });
            }
            Op::ObserveSaved => {
                m.output.push_str(m.saved.as_ref()?);
                m.output.push(';');
            }
            Op::Noise => (),
            _ => return None,
        }
    }
    if !constructed || m.environment.is_none() {
        return None;
    }
    m.destructors = vec!["<2>", "<1>"];
    for event in &m.destructors {
        m.output.push_str(event);
    }
    Some(m)
}
const PRELUDE: &str = r#"type Holder[T] { value:T }
resource type Trace { out:Stdout, text:String }
 drop(x) { let _=out.print(x.out,x.text) }
fn digit(n:u64)->String { if n==0 {"0"} else if n==2 {"2"} else {"wrong"} }
fn relay[T](take value:T)->T { take value }
"#;
fn source(row: Row, ops: &[Op]) -> String {
    assert!(model(row, ops).is_some(), "invalid generator history");
    let mut s = PRELUDE.to_owned();
    let calls = ops.iter().filter(|op| **op == Op::Invoke).count();
    let once = row.once();
    let callback_ty = format!("{once}fn()->u64");
    let invocation = |index| {
        let name = if index == 0 {
            "n".to_owned()
        } else {
            format!("n{index}")
        };
        format!(
            "let {name}=f();let _=out.print(stdout,digit({name}));let _=out.print(stdout,\";\");\n"
        )
    };
    if row.0[2] != 0 {
        let passing = if row.0[2] == 2 { "take " } else { "" };
        s.push_str(&format!(
            "fn invoke({passing}f:{callback_ty},stdout:Stdout) {{\n{} }}\n",
            (0..calls).map(invocation).collect::<String>()
        ));
    }
    s.push_str("pub fn main(rt:Io)->Result[(),GrantError] {\nlet stdout=io.stdout(rt)?;\nlet first=Trace(out:stdout,text:\"<1>\");\nlet second=Trace(out:stdout,text:\"<2>\");\n");
    let mut called = false;
    let mut direct_calls = 0;
    for op in ops {
        match op {
            Op::Construct => s.push_str(&format!(
                "let payload:{} in heap={};\n",
                row.ty(),
                row.value()
            )),
            Op::Save => s.push_str("let saved=copy payload;\n"),
            Op::Create => {
                let body = match row.0[0] {
                    0 => row.score("payload"),
                    1 => format!("let snapshot=copy payload;{}", row.score("snapshot")),
                    _ => format!("let snapshot=take payload;{}", row.score("snapshot")),
                };
                let literal = format!("fn()->u64 {{{body}}}");
                if row.0[4] == 1 {
                    s.push_str(&format!("let outer:{once}fn()->{callback_ty}=fn()->{callback_ty} {{let inner:{callback_ty}={literal};relay(take inner)}};\nlet f:{callback_ty}=outer();\n"));
                } else if row.0[0] == 0 {
                    s.push_str(&format!("let f={literal};\n"));
                } else {
                    s.push_str(&format!("let f:{callback_ty}={literal};\n"));
                }
                // A zero-call transport still passes/drops its argument.
                if calls == 0 && row.0[2] != 0 {
                    s.push_str(if row.0[2] == 2 {
                        "invoke(take f,stdout);\n"
                    } else {
                        "invoke(f,stdout);\n"
                    });
                }
            }
            Op::Invoke if row.0[2] == 0 => {
                s.push_str(&invocation(direct_calls));
                direct_calls += 1;
            }
            Op::Invoke if !called => {
                s.push_str(if row.0[2] == 2 {
                    "invoke(take f,stdout);\n"
                } else {
                    "invoke(f,stdout);\n"
                });
                called = true;
            }
            Op::Invoke => (),
            Op::ObserveSaved => s.push_str(&format!(
                "let _=out.print(stdout,{});let _=out.print(stdout,\";\");\n",
                row.show("saved")
            )),
            Op::Noise => s.push_str("let unused=7u64;\n"),
        }
    }
    s.push_str("Ok(())\n}\n");
    assert!(s.lines().count() <= 128);
    s
}

// Unique per action/process: persist() never overwrites another attempt. Its
// captured HISTORY_ARTIFACT JSON survives ephemeral remote action directories.
fn artifacts(label: &str) -> PathBuf {
    let _ = provenance(); // Seal actual tools before this process starts consumers.
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("capture-contract-matrix-31")
        .join(format!("{label}-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
fn retain(
    dir: &std::path::Path,
    name: &str,
    seed: usize,
    row: Row,
    ops: &[Op],
    text: &str,
    detail: &str,
) {
    history::persist(
        dir,
        name,
        seed as u64,
        text,
        &format!(
            "row={row:?}\nops={ops:?}\nmodel={:?}\nprovenance={}\n{detail}",
            model(row, ops),
            provenance()
        ),
    );
}
fn check(text: &str) -> (QueryEngine, CheckOutput) {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("main.hir", text.into()).unwrap();
    let output = check_progress(&mut engine, file, &default_context())
        .expect("source API error, not a source diagnostic");
    (engine, output)
}
fn validate(input: &NativeInput) -> Result<(), String> {
    let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
    signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
    for (path, definition) in &input.definitions {
        let report = hirsel_validate::validate(definition, &input.types, &signatures, &mut vec![]);
        if !report.violations.is_empty() {
            return Err(format!("{path:?}: {:?}", report.violations));
        }
    }
    Ok(())
}
fn source_partition(part: usize) {
    with_compiler_stack(|| {
        let dir = artifacts(&format!("source-{part}"));
        let mut errors = vec![];
        for seed in part * 4..part * 4 + 4 {
            let row = Row(TABLE[seed]);
            let ops = history(row);
            let text = source(row, &ops);
            let result = evaluate(row, &ops, None, "source");
            retain(
                &dir,
                &format!("seed-{seed:02}"),
                seed,
                row,
                &ops,
                &text,
                &result.detail,
            );
            if let Some(failure) = result.failure {
                retain_reduction(&dir, seed, row, &ops, &failure, None);
                errors.push((seed, failure));
            }
        }
        assert!(errors.is_empty(), "{errors:#?}");
    });
}
fn configurations() -> Vec<Configuration> {
    [
        Engine::Heap { campaign: false },
        Engine::Native {
            opt: OptLevel::Default,
            partition: Partition::Whole,
            runtime: Runtime::Default,
        },
        Engine::Cranelift {
            partition: Partition::Whole,
            runtime: Runtime::Diagnostic,
        },
    ]
    .into_iter()
    .map(|engine| Configuration {
        engine,
        options: Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
    })
    .collect()
}
fn runtime_partition(part: usize) {
    with_compiler_stack(|| {
        let dir = artifacts(&format!("runtime-{part}"));
        let runner = history::Runner::with_matrix(&dir.join("runs"), configurations())
            .expect("INCOMPLETE toolchain setup");
        let mut errors = vec![];
        let mut counts = BTreeMap::new();
        for seed in part * 4..part * 4 + 4 {
            let row = Row(TABLE[seed]);
            let ops = history(row);
            let text = source(row, &ops);
            let result = evaluate(row, &ops, Some(&runner), &format!("seed-{seed:02}"));
            if let Some(report) = &result.report {
                for cell in &report.cells {
                    *counts.entry(cell.engine.clone()).or_insert(0usize) +=
                        usize::from(cell.category == history::Category::Observation);
                }
            }
            retain(
                &dir,
                &format!("seed-{seed:02}"),
                seed,
                row,
                &ops,
                &text,
                &result.detail,
            );
            if let Some(failure) = result.failure {
                retain_reduction(&dir, seed, row, &ops, &failure, Some(&runner));
                errors.push((seed, failure));
            }
        }
        eprintln!("CAPTURE_COUNTS partition={part} {counts:?}");
        assert!(errors.is_empty(), "{errors:#?}");
        assert_eq!(counts.len(), 4);
        assert!(counts.values().all(|n| *n == 4));
    });
}

struct Attempt {
    failure: Option<Failure>,
    report: Option<history::Report>,
    detail: String,
}
fn failed(
    source_accepted: bool,
    v_accepted: bool,
    fingerprint: Fingerprint,
    detail: String,
) -> Attempt {
    Attempt {
        failure: Some(Failure {
            source_accepted,
            v_accepted,
            fingerprint,
        }),
        report: None,
        detail,
    }
}
fn evaluate(row: Row, ops: &[Op], runner: Option<&history::Runner>, name: &str) -> Attempt {
    let text = source(row, ops);
    let source_dir = tempfile::tempdir().expect("source staging directory");
    let path = source_dir.path().join("main.hir");
    std::fs::write(&path, &text).expect("source staging write");
    let mut program = match load_program(&path) {
        Ok(program) => program,
        Err(e) => {
            return failed(
                false,
                false,
                Fingerprint::Setup {
                    phase: "source load".into(),
                    category: format!("{e:?}"),
                },
                format!("INCOMPLETE source load API={e:?}"),
            );
        }
    };
    // Reachable checking can already invoke V and return a V Operational error
    // before yielding diagnostics. Keep the source verdict independently first.
    let output = match program.check(LoweringPolicy::Comptime) {
        Ok(output) => output,
        Err(e) => {
            return failed(
                false,
                false,
                Fingerprint::Setup {
                    phase: "source API".into(),
                    category: format!("{e:?}"),
                },
                format!("INCOMPLETE source API={e:?}"),
            );
        }
    };
    let mut engine = program.engine;
    let diagnostics: Vec<_> = output
        .diagnostics
        .iter()
        .map(|d| d.to_json(&output.sources))
        .collect();
    let mut detail = format!(
        "completion={:?} source_diagnostics={diagnostics:?}\n",
        output.completion
    );
    if let Some(d) = output
        .diagnostics
        .iter()
        .find(|d| d.severity == hirsel_diag::Severity::Error)
    {
        let fingerprint = Fingerprint::Source {
            rule: d.rule.as_str().into(),
            contract: "independent capture history expects ACCEPT".into(),
        };
        return failed(false, false, fingerprint, detail);
    }
    if output.completion != CheckCompletion::Complete {
        return failed(
            false,
            false,
            Fingerprint::Setup {
                phase: "source completion".into(),
                category: format!("{:?}", output.completion),
            },
            detail,
        );
    }
    let input = match engine.export_native_input() {
        Ok(input) => input,
        Err(e) => {
            let cell = history::query_error("V-export", e);
            detail.push_str(&format!("export={cell:?}"));
            return failed(
                true,
                false,
                Fingerprint::Validator {
                    rule: format!("{:?}:{}:{}", cell.category, cell.disagreement, cell.detail),
                },
                detail,
            );
        }
    };
    if let Err(e) = validate(&input) {
        detail.push_str(&format!("V={e}"));
        return failed(true, false, Fingerprint::Validator { rule: e }, detail);
    }
    let validated = match engine.import_native_input(input) {
        Ok(input) => input,
        Err(e) => {
            let cell = history::query_error("V-import", e);
            detail.push_str(&format!("import={cell:?}"));
            return failed(
                true,
                false,
                Fingerprint::Validator {
                    rule: format!("{:?}:{}:{}", cell.category, cell.disagreement, cell.detail),
                },
                detail,
            );
        }
    };
    detail.push_str("SOURCE+V ACCEPT\nADAPTER: visible destructor tokens are output; separate native effects/return-value graph unavailable (INCOMPLETE fields); entry returns unit; roots disabled/empty; no file stages so Pending/AfterWrite not applicable.\n");
    let Some(runner) = runner else {
        return Attempt {
            failure: None,
            report: None,
            detail,
        };
    };
    let entry = entry_path(&default_context());
    let observed = match run_observed_snapshot(
        &mut engine,
        &RunConfiguration {
            entry: entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        },
        SimulationConfig::default(),
    ) {
        Ok(run) => run,
        Err(e) => {
            return failed(
                true,
                true,
                Fingerprint::Setup {
                    phase: "reference API".into(),
                    category: format!("{e:?}"),
                },
                format!("{detail}reference API={e:?}"),
            );
        }
    };
    detail.push_str(&format!("reference_full={observed:?}\n"));
    let Some(observation) = observed.observation.as_ref() else {
        return failed(
            true,
            true,
            Fingerprint::Setup {
                phase: "reference tooling".into(),
                category: format!("{observed:?}"),
            },
            detail,
        );
    };
    let reference = Observation::of_reference(observation);
    let report = runner.compare(&validated, &entry, &reference, name);
    let expected = model(row, ops).unwrap();
    let differences = report.disagreements(expected.output.as_bytes(), false);
    detail.push_str(&format!("report={report:#?}\ndifferences={differences:?}"));
    let failure = if let Some((engine, field)) = differences.first() {
        Some(Failure {
            source_accepted: true,
            v_accepted: true,
            fingerprint: Fingerprint::Runtime {
                engine: engine.clone(),
                field: field.clone(),
            },
        })
    } else if report.cells.len() != 4 || !report.agrees() {
        Some(Failure {
            source_accepted: true,
            v_accepted: true,
            fingerprint: Fingerprint::Setup {
                phase: "matrix".into(),
                category: "missing observation".into(),
            },
        })
    } else {
        None
    };
    Attempt {
        failure,
        report: Some(report),
        detail,
    }
}
fn retain_reduction(
    dir: &std::path::Path,
    seed: usize,
    row: Row,
    ops: &[Op],
    failure: &Failure,
    runner: Option<&history::Runner>,
) {
    let mut evaluation = 0;
    let reduced = reduce(row, ops, failure, |r, operations| {
        evaluation += 1;
        let result = evaluate(
            r,
            operations,
            runner,
            &format!("reduce-{seed}-{evaluation}"),
        );
        if result
            .failure
            .as_ref()
            .is_some_and(|f| same_failure(failure, f))
        {
            retain(
                dir,
                &format!("seed-{seed:02}-retained-{evaluation}"),
                seed,
                r,
                operations,
                &source(r, operations),
                &result.detail,
            );
        }
        result.failure
    });
    retain(
        dir,
        &format!("seed-{seed:02}-reduced"),
        seed,
        reduced.row,
        &reduced.ops,
        &source(reduced.row, &reduced.ops),
        &format!(
            "same_failure={failure:?} evaluations={} decisions={:?}",
            reduced.evaluations, reduced.decisions
        ),
    );
}

fn pairs(row: [u8; 7]) -> BTreeSet<(usize, u8, usize, u8)> {
    (0..7)
        .flat_map(|a| ((a + 1)..7).map(move |b| (a, row[a], b, row[b])))
        .collect()
}
#[test]
fn coverage_ledger_is_frozen_and_pairwise_complete() {
    let mut feasible = BTreeSet::new();
    let mut all = BTreeSet::new();
    for a in 0..3 {
        for b in 0..2 {
            for c in 0..3 {
                for d in 0..3 {
                    for e in 0..2 {
                        for f in 0..2 {
                            for g in 0..3 {
                                let r = [a, b, c, d, e, f, g];
                                all.extend(pairs(r));
                                if Row(r).valid() {
                                    feasible.extend(pairs(r));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let mut covered = BTreeMap::new();
    for (seed, row) in TABLE.iter().enumerate() {
        assert!(Row(*row).valid());
        assert!(model(Row(*row), &history(Row(*row))).is_some());
        for pair in pairs(*row) {
            covered.entry(pair).or_insert(seed);
        }
    }
    assert_eq!(feasible, covered.keys().copied().collect());
    assert_eq!(covered.len(), 132);
    eprintln!(
        "CAPTURE_LEDGER {}",
        serde_json::json!({"rows":TABLE,"pairs":covered.iter().map(|(pair,seed)|format!("{pair:?}: seed-{seed:02} ACCEPT")).collect::<Vec<_>>(),"excluded_pairs":all.difference(&feasible).map(|p|format!("{p:?}")).collect::<Vec<_>>(),"constraints":"CLO-4: no borrowed escaping/taken environment; MODE-3: take implies once, at most one call, no borrowed once invocation; envelope: borrowed literal is scoped Many; depth<=3 ops<=12 calls<=2 lines<=128 loops=0"})
    );
}

macro_rules! partition_tests {($($source:ident,$runtime:ident,$part:expr;)*)=>{$(
    #[test] fn $source(){source_partition($part);}
    #[test] fn $runtime(){runtime_partition($part);}
)*};}
partition_tests! {
 source_00,runtime_00,0;
 source_01,runtime_01,1;
 source_02,runtime_02,2;
 source_03,runtime_03,3;
 source_04,runtime_04,4;
 source_05,runtime_05,5;
}

fn negative_partition(part: usize) {
    with_compiler_stack(|| {
        let dir = artifacts(&format!("negative-{part}"));
        for local in 0..6 {
            let row = Row([
                if part == 0 { 1 } else { 2 },
                part as u8,
                0,
                (local % 3) as u8,
                (local / 3) as u8,
                0,
                1,
            ]);
            let ops = history(row);
            let positive = source(row, &ops);
            let (mut engine, checked) = check(&positive);
            assert!(
                checked.diagnostics.is_empty(),
                "negative's positive twin: {:?}",
                checked.diagnostics
            );
            validate(&engine.export_native_input().unwrap()).unwrap();
            // One edit, one declared rule: taking a Many capture or invoking the
            // same consumed Once value. The independent model refuses the latter.
            let negative = if part == 0 {
                positive.replacen("snapshot=copy payload", "snapshot=take payload", 1)
            } else {
                let mut invalid = ops.clone();
                invalid.insert(invalid.len() - 1, Op::Invoke);
                assert!(model(row, &invalid).is_none());
                positive.replace("let unused=7u64;", "let again=f();")
            };
            assert_ne!(positive, negative);
            let (_, output) = check(&negative);
            let diagnostics: Vec<_> = output
                .diagnostics
                .iter()
                .map(|d| d.to_json(&output.sources))
                .collect();
            retain(
                &dir,
                &format!("twin-{local}-positive"),
                local,
                row,
                &ops,
                &positive,
                "expected ACCEPT; source+V ACCEPT",
            );
            retain(
                &dir,
                &format!("twin-{local}-negative"),
                local,
                row,
                &ops,
                &negative,
                &format!(
                    "expected MODE-3 at {}; diagnostics={diagnostics:?}; V/runtime NOT APPLICABLE",
                    if part == 0 {
                        "closure construction"
                    } else {
                        "second f invocation"
                    }
                ),
            );
            let errors: Vec<_> = output
                .diagnostics
                .iter()
                .filter(|d| d.severity == hirsel_diag::Severity::Error)
                .collect();
            assert!(!errors.is_empty(), "{negative}\n{diagnostics:#?}");
            // A nested take also makes the outer closure Once: both receiving
            // sites can diagnose the same single MODE-3 rule violation.
            assert!(
                errors.iter().all(|d| d.rule.as_str() == "MODE-3"),
                "{diagnostics:#?}"
            );
            let site = output.sources.slice(errors[0].primary).unwrap();
            assert!(
                if part == 0 {
                    site.contains("fn()") || site == "take payload"
                } else {
                    site == "f" || site == "f()"
                },
                "unexpected rule site {site:?}: {diagnostics:#?}"
            );
        }
    });
}
#[test]
fn negative_many_take() {
    negative_partition(0);
}
#[test]
fn negative_once_twice() {
    negative_partition(1);
}

fn mutation_partition(part: usize) {
    with_compiler_stack(|| {
        let dir = artifacts(&format!("mutation-{part}"));
        let mut actual_sites = 0;
        for local in 0..4 {
            let seed = part * 4 + local;
            let row = Row([
                1,
                0,
                0,
                1 + (seed % 2) as u8,
                (seed / 4) as u8,
                ((seed / 2) % 2) as u8,
                2,
            ]);
            let ops = history(row);
            let text = source(row, &ops);
            let (mut engine, output) = check(&text);
            assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
            let input = engine.export_native_input().unwrap();
            validate(&input).unwrap();
            let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
            signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
            let main = &input.definitions[&entry_path(&default_context())];
            let mut selected = None;
            'bodies: for (bi, body) in main.bodies.iter().enumerate() {
                for (ri, rvalue) in body.rvalues.iter().enumerate() {
                    if let Rvalue::Use(operand) = rvalue {
                        let place = &body.places[operand.place.0 as usize];
                        if operand.usage == UseKind::Copy
                            && place.projection.is_empty()
                            && body.locals[place.root.0 as usize].storage == Storage::Capture
                        {
                            selected = Some((bi, ri, operand.place));
                            break 'bodies;
                        }
                    }
                }
            }
            let (bi, ri, place) = selected.expect("SETUP: zero identified capture Copy sites");
            let mut forged = (**main).clone();
            let Rvalue::Use(operand) = &mut forged.bodies[bi].rvalues[ri] else {
                unreachable!()
            };
            operand.usage = UseKind::Take;
            let report = hirsel_validate::validate(&forged, &input.types, &signatures, &mut vec![]);
            retain(
                &dir,
                &format!("mutation-{seed}"),
                seed,
                row,
                &ops,
                &text,
                &format!(
                    "original validates; mutation Body{bi} Rvalue{ri} Place{place:?} Capture Copy->Take; report={report:?}"
                ),
            );
            assert!(
                report.violations.iter().any(
                    |v| matches!(v.kind,hirsel_validate::ViolationKind::BorrowedMove(p) if p==place)
                ),
                "{report:?}"
            );
            actual_sites += 1;
        }
        assert_eq!(actual_sites, 4);
        eprintln!("CAPTURE_MUTATIONS partition={part} actual_sites={actual_sites}");
    });
}
#[test]
fn mutations_00() {
    mutation_partition(0);
}
#[test]
fn mutations_01() {
    mutation_partition(1);
}

/// The phase and rule/field/engine are the reduction identity. Coordinates can
/// move under deletion; a source red must never replace a checker-positive V red.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Fingerprint {
    Setup { phase: String, category: String },
    Source { rule: String, contract: String },
    Validator { rule: String },
    Runtime { engine: String, field: String },
    Emitter { field: String },
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Failure {
    source_accepted: bool,
    v_accepted: bool,
    fingerprint: Fingerprint,
}
fn same_failure(original: &Failure, candidate: &Failure) -> bool {
    original == candidate
        && match &original.fingerprint {
            Fingerprint::Validator { .. } => candidate.source_accepted && !candidate.v_accepted,
            Fingerprint::Runtime { .. } => candidate.source_accepted && candidate.v_accepted,
            Fingerprint::Source { .. } => !candidate.source_accepted,
            Fingerprint::Emitter { .. } | Fingerprint::Setup { .. } => true,
        }
}
struct Reduced {
    row: Row,
    ops: Vec<Op>,
    evaluations: usize,
    decisions: Vec<String>,
}
fn reduce(
    row: Row,
    ops: &[Op],
    failure: &Failure,
    mut evaluate: impl FnMut(Row, &[Op]) -> Option<Failure>,
) -> Reduced {
    let mut evaluations = 0;
    let mut decisions = vec![];
    let mut attempt = |r: Row, candidate: &[Op]| {
        if evaluations >= 96 || model(r, candidate).is_none() {
            return false;
        }
        evaluations += 1;
        let result = evaluate(r, candidate);
        let keep = result.as_ref().is_some_and(|f| same_failure(failure, f));
        decisions.push(format!(
            "evaluation={evaluations} row={r:?} ops={candidate:?} result={result:?} retained={keep}"
        ));
        keep
    };
    let mut kept = history::minimize(ops, |candidate| attempt(row, candidate));
    let mut reduced = row;
    // Value and depth steps obey the same independent validity and phase tests.
    for axis in [5, 4, 3] {
        let mut candidate = reduced;
        candidate.0[axis] = if axis == 5 { 1 } else { 0 };
        if candidate != reduced && attempt(candidate, &kept) {
            reduced = candidate;
        }
    }
    kept = history::minimize(&kept, |candidate| attempt(reduced, candidate));
    Reduced {
        row: reduced,
        ops: kept,
        evaluations,
        decisions,
    }
}
#[test]
fn reducer_injected_emitter_error_preserves_phase_and_converges() {
    // Deliberately corrupt only scalar digit emission. This tests the reducer;
    // it is never classified as a compiler defect or sent to a runtime engine.
    let row = Row([1, 0, 0, 2, 1, 0, 2]);
    let ops = history(row);
    let failure = Failure {
        source_accepted: true,
        v_accepted: true,
        fingerprint: Fingerprint::Emitter {
            field: "saved-output".into(),
        },
    };
    let injected = |r: Row, operations: &[Op]| {
        let expected = model(r, operations)?;
        let good = source(r, operations);
        let statement = format!("out.print(stdout,{})", r.show("saved"));
        let faulty = good.replace(&statement, "out.print(stdout,\"injected-emitter-error\")");
        if good == faulty {
            return None;
        }
        with_compiler_stack(|| {
            let (mut engine, output) = check(&faulty);
            assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
            validate(&engine.export_native_input().unwrap()).unwrap();
            let observed = run_observed_snapshot(
                &mut engine,
                &RunConfiguration {
                    entry: entry_path(&default_context()),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            )
            .unwrap();
            let observation = observed.observation.unwrap();
            (observation.output != expected.output.as_bytes()).then_some(failure.clone())
        })
    };
    let reduced = reduce(row, &ops, &failure, injected);
    assert!(reduced.evaluations <= 96);
    assert_eq!(
        reduced.ops,
        vec![Op::Construct, Op::Save, Op::Create, Op::ObserveSaved]
    );
    assert_eq!(reduced.row.0[3..6], [0, 0, 1]);
    let again = reduce(reduced.row, &reduced.ops, &failure, injected);
    assert_eq!(again.ops, reduced.ops);
    assert_eq!(again.row, reduced.row);
    let dir = artifacts("reducer-self-test");
    for (name, r, operations) in [
        ("original", row, ops.as_slice()),
        ("reduced", reduced.row, reduced.ops.as_slice()),
    ] {
        retain(
            &dir,
            name,
            0,
            r,
            operations,
            &source(r, operations).replace(
                &format!("out.print(stdout,{})", r.show("saved")),
                "out.print(stdout,\"injected-emitter-error\")",
            ),
            &format!(
                "INJECTED EMITTER ERROR; NOT A COMPILER BUG; evaluations={} decisions={:?}",
                reduced.evaluations, reduced.decisions
            ),
        );
    }
    for original in [
        Failure {
            source_accepted: false,
            v_accepted: false,
            fingerprint: Fingerprint::Source {
                rule: "MODE-3".into(),
                contract: "Many capture Take".into(),
            },
        },
        Failure {
            source_accepted: true,
            v_accepted: false,
            fingerprint: Fingerprint::Validator {
                rule: "BorrowedMove".into(),
            },
        },
        Failure {
            source_accepted: true,
            v_accepted: true,
            fingerprint: Fingerprint::Runtime {
                engine: "llvm".into(),
                field: "output".into(),
            },
        },
    ] {
        assert!(same_failure(&original, &original));
        let mut wrong_phase = original.clone();
        wrong_phase.source_accepted = !original.source_accepted;
        assert!(!same_failure(&original, &wrong_phase));
        assert!(!same_failure(&original, &failure));
    }
}

fn provenance() -> &'static serde_json::Value {
    static VALUE: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
    VALUE.get_or_init(|| {
        use sha2::{Digest,Sha256};
        use std::io::Read;
        fn collect(path:PathBuf,seen:&mut BTreeSet<PathBuf>,files:&mut BTreeMap<String,String>) {
            let Ok(real)=path.canonicalize() else {return;};
            if !seen.insert(real.clone()){return;}
            if real.is_dir() {
                for entry in std::fs::read_dir(&path).unwrap() {collect(entry.unwrap().path(),seen,files);}
            } else if real.is_file() {
                let mut file=std::fs::File::open(&real).unwrap();let mut hash=Sha256::new();let mut buffer=[0u8;65536];
                loop {let n=file.read(&mut buffer).unwrap();if n==0{break;}hash.update(&buffer[..n]);}
                files.insert(path.display().to_string(),format!("{:x}",hash.finalize()));
            }
        }
        let mut files=BTreeMap::new();let mut seen=BTreeSet::new();
        collect(std::env::current_exe().unwrap(),&mut seen,&mut files);
        let mut environment=BTreeMap::new();
        for key in ["HIRSEL_IMAGE","HIRSEL_RUNTIME_ARCHIVES","HIRSEL_LINK_KIT","HIRSEL_GLIBC_FLOOR","LLVM_SYS_231_PREFIX"] {
            if let Some(value)=std::env::var_os(key) {
                environment.insert(key,value.to_string_lossy().into_owned());
                if key=="LLVM_SYS_231_PREFIX" {
                    collect(PathBuf::from(value).join("bin/ld.lld"),&mut seen,&mut files);
                } else {collect(PathBuf::from(value),&mut seen,&mut files);}
            }
        }
        serde_json::json!({"compiler_identity":format!("{COMPILER_IDENTITY:?}"),"sha256":files,"environment":environment,"observation_schema":OBSERVATION_SCHEMA,"matrix":MATRIX_VERSION})
    })
}

#[test]
fn permanent_manifest_twins_and_prefix_annotations() {
    with_compiler_stack(|| {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/capture_contract_matrix_31");
        let dir = artifacts("permanent-twins");
        for (name, rule, site) in [
            ("many-ok", None, ""),
            ("many-bad", Some("MODE-3"), "fn()->String {take payload}"),
            ("once-ok", None, ""),
            ("once-bad", Some("MODE-3"), "f"),
            ("prefix-ok", None, ""),
            ("prefix-bad", Some("MODE-2"), "take xs"),
        ] {
            let text = std::fs::read_to_string(root.join(name).join("main.hir")).unwrap();
            let manifest = std::fs::read_to_string(root.join(name).join("hirsel.json")).unwrap();
            let recipe: serde_json::Value = serde_json::from_str(&manifest).unwrap();
            assert_eq!(
                recipe["sources"],
                serde_json::json!([{"path":"main.hir","module":[]}])
            );
            let (mut engine, output) = check(&text);
            let diagnostics: Vec<_> = output
                .diagnostics
                .iter()
                .map(|d| d.to_json(&output.sources))
                .collect();
            history::persist(
                &dir,
                name,
                0,
                &text,
                &format!(
                    "expected={rule:?} at {site:?}; exact_manifest={manifest}; diagnostics={diagnostics:?}; provenance={}",
                    provenance()
                ),
            );
            match rule {
                Some(rule) => {
                    assert_eq!(output.diagnostics.len(), 1, "{name}: {diagnostics:#?}");
                    assert_eq!(output.diagnostics[0].rule.as_str(), rule);
                    assert_eq!(
                        output.sources.slice(output.diagnostics[0].primary),
                        Some(site),
                        "{name}"
                    );
                }
                None => {
                    assert!(output.diagnostics.is_empty(), "{name}: {diagnostics:#?}");
                    let input = engine.export_native_input().unwrap();
                    validate(&input).unwrap();
                }
            }
        }
    });
}

// The 12 source-negative twins and eight V originals are fixed additional
// histories. Exact duplicates of an already executed table row are skipped.
fn control_row(seed: usize) -> Row {
    if seed < 12 {
        let part = seed / 6;
        let local = seed % 6;
        Row([
            if part == 0 { 1 } else { 2 },
            part as u8,
            0,
            (local % 3) as u8,
            (local / 3) as u8,
            0,
            1,
        ])
    } else {
        let site = seed - 12;
        Row([
            1,
            0,
            0,
            1 + (site % 2) as u8,
            (site / 4) as u8,
            ((site / 2) % 2) as u8,
            2,
        ])
    }
}
fn control_runtime_partition(part: usize) {
    with_compiler_stack(|| {
        let dir = artifacts(&format!("control-runtime-{part}"));
        let runner = history::Runner::with_matrix(&dir.join("runs"), configurations()).unwrap();
        let mut completed = 0;
        for seed in part * 4..part * 4 + 4 {
            let row = control_row(seed);
            let ops = history(row);
            let text = source(row, &ops);
            if TABLE
                .iter()
                .any(|r| source(Row(*r), &history(Row(*r))) == text)
            {
                eprintln!("CAPTURE_CONTROL_ALIAS control={seed} already observed as table row");
                continue;
            }
            let result = evaluate(row, &ops, Some(&runner), &format!("control-{seed}"));
            retain(
                &dir,
                &format!("control-{seed}"),
                seed,
                row,
                &ops,
                &text,
                &result.detail,
            );
            if let Some(failure) = result.failure {
                retain_reduction(&dir, seed, row, &ops, &failure, Some(&runner));
                panic!("control {seed}: {failure:?}");
            }
            assert_eq!(result.report.unwrap().cells.len(), 4);
            completed += 1;
        }
        eprintln!("CAPTURE_CONTROL_COUNTS partition={part} observations_per_engine={completed}");
        assert!(completed > 0);
    });
}
#[test]
fn control_runtime_00() {
    control_runtime_partition(0);
}
#[test]
fn control_runtime_01() {
    control_runtime_partition(1);
}
#[test]
fn control_runtime_02() {
    control_runtime_partition(2);
}
#[test]
fn control_runtime_03() {
    control_runtime_partition(3);
}
#[test]
fn control_runtime_04() {
    control_runtime_partition(4);
}

#[test]
fn permanent_positive_twins_runtime() {
    let dir = artifacts("permanent-runtime");
    let runner = history::Runner::with_matrix(&dir.join("runs"), configurations()).unwrap();
    for name in ["many-ok", "once-ok", "prefix-ok"] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/capture_contract_matrix_31")
            .join(name)
            .join("main.hir");
        let text = std::fs::read_to_string(path).unwrap();
        let report = runner.run(name, &text);
        history::persist(
            &dir,
            name,
            0,
            &text,
            &format!(
                "expected unit exit0 empty output; model-visible destructor events=[]; report={report:#?}; provenance={}",
                provenance()
            ),
        );
        assert!(report.agrees(), "{report:#?}");
        assert_eq!(report.cells.len(), 4);
        assert!(report.disagreements(b"", false).is_empty());
    }
    eprintln!("CAPTURE_PERMANENT_COUNTS observations_per_engine=3");
}

#[test]
fn nested_generic_result_twins_runtime() {
    with_compiler_stack(|| {
        let dir = artifacts("nested-generic-results");
        let runner = history::Runner::with_matrix(&dir.join("runs"), configurations()).unwrap();
        for once in [false, true] {
            let mode = if once { "once " } else { "" };
            let transfer = if once { "take" } else { "copy" };
            let body = format!(
                "fn make()->{mode}fn()->{mode}fn()->Holder[String] {{let payload:Holder[String] in heap=Holder(value:\"ab\");fn()->{mode}fn()->Holder[String] {{let inner:{mode}fn()->Holder[String]=fn()->Holder[String]{{{transfer} payload}};relay(take inner)}}}}\n"
            );
            let mut text = format!(
                "{PRELUDE}{body}pub fn main(rt:Io)->Result[(),GrantError]{{let stdout=io.stdout(rt)?;let first=Trace(out:stdout,text:\"<1>\");let second=Trace(out:stdout,text:\"<2>\");let outer=make();let f=outer();let a=f();let _=out.print(stdout,a.value);\n"
            );
            if !once {
                text.push_str("let b=f();let _=out.print(stdout,b.value);\n");
            }
            text.push_str("Ok(())}\n");
            let expected = if once {
                b"ab<2><1>".as_slice()
            } else {
                b"abab<2><1>".as_slice()
            };
            let name = if once {
                "taken-once-result"
            } else {
                "copied-many-result"
            };
            let (mut engine, output) = check(&text);
            history::persist(
                &dir,
                &format!("{name}-source"),
                0,
                &text,
                &format!(
                    "expected={expected:?}; source diagnostics={:?}; provenance={}",
                    output.diagnostics,
                    provenance()
                ),
            );
            assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
            let input = engine.export_native_input().unwrap();
            validate(&input).unwrap();
            // The relay's T specializes to a callable while its source parameter
            // remains generic and retaining, with no invented scoped selector.
            assert!(
                input
                    .definitions
                    .iter()
                    .filter(|(p, _)| p.0.name == "relay")
                    .all(|(_, d)| d.bodies.iter().all(|b| b.scoped_parameters.is_empty()))
            );
            let validated = engine.import_native_input(input).unwrap();
            let entry = entry_path(&default_context());
            let reference = run_observed_snapshot(
                &mut engine,
                &RunConfiguration {
                    entry: entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            )
            .unwrap();
            let report = runner.compare(
                &validated,
                &entry,
                &Observation::of_reference(reference.observation.as_ref().unwrap()),
                name,
            );
            history::persist(
                &dir,
                name,
                0,
                &text,
                &format!(
                    "expected={expected:?}; visible destructors=<2>,<1>; reference_full={reference:?}; report={report:#?}; provenance={}",
                    provenance()
                ),
            );
            assert!(report.agrees(), "{report:#?}");
            assert_eq!(report.cells.len(), 4);
            assert!(
                report.disagreements(expected, false).is_empty(),
                "{report:#?}"
            );
        }
        eprintln!("CAPTURE_GENERIC_RESULT_COUNTS observations_per_engine=2");
    });
}

#[test]
fn generic_actual_does_not_invent_scoped_contract_runtime() {
    with_compiler_stack(|| {
        let dir = artifacts("generic-scoped-receipt");
        let text = r#"fn relay[T](take value:T)->T {take value}
fn invoke(f:fn()->u64)->u64 {f()}
pub fn main(rt:Io)->Result[(),GrantError] {
 let stdout=io.stdout(rt)?;
 let n=2u64;
 let f:fn()->u64=fn()->u64 {n};
 let g=relay(take f);
 let _=out.print(stdout,if invoke(g)==2 {"ok"}else{"wrong"});
 Ok(())
}
"#;
        let (mut engine, output) = check(text);
        history::persist(
            &dir,
            "generic-receipt-source",
            0,
            text,
            &format!(
                "expected ACCEPT; diagnostics={:?}; provenance={}",
                output.diagnostics,
                provenance()
            ),
        );
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let input = engine.export_native_input().unwrap();
        validate(&input).unwrap();
        let relay = input
            .definitions
            .iter()
            .find(|(p, _)| p.0.name == "relay")
            .expect("zero generic producer probes")
            .1;
        assert!(!relay.bodies.is_empty());
        assert!(relay.bodies.iter().all(|b| b.scoped_parameters.is_empty()));
        let invoke = input
            .definitions
            .iter()
            .find(|(p, _)| p.0.name == "invoke")
            .expect("zero explicitly scoped control probes")
            .1;
        assert!(invoke.bodies.iter().any(|b| {
            b.scoped_parameters
                .iter()
                .any(|p| p.parameter == 0 && p.fields.is_empty())
        }));
        let validated = engine.import_native_input(input).unwrap();
        let entry = entry_path(&default_context());
        let reference = run_observed_snapshot(
            &mut engine,
            &RunConfiguration {
                entry: entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        let runner = history::Runner::with_matrix(&dir.join("runs"), configurations()).unwrap();
        let report = runner.compare(
            &validated,
            &entry,
            &Observation::of_reference(reference.observation.as_ref().unwrap()),
            "generic-receipt",
        );
        history::persist(
            &dir,
            "generic-receipt",
            0,
            text,
            &format!(
                "expected output=ok exit0 destructors=[]; relay scoped selectors=[]; invoke scoped selector=0; reference_full={reference:?}; report={report:#?}; provenance={}",
                provenance()
            ),
        );
        assert!(report.agrees(), "{report:#?}");
        assert_eq!(report.cells.len(), 4);
        assert!(report.disagreements(b"ok", false).is_empty());
        eprintln!(
            "CAPTURE_GENERIC_RECEIPT_COUNTS observations_per_engine=1 generic_producer_probes=1 scoped_control_probes=1"
        );
    });
}

#[test]
fn sheet_pattern_capture_source_v_agreement() {
    with_compiler_stack(|| {
        let dir = artifacts("sheet-pattern-capture");
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/capture_contract_matrix_31");
        for name in ["sheet-minimal.hir", "sheet-taken.hir"] {
            let text = std::fs::read_to_string(root.join(name)).unwrap();
            let mut program = load_program(&root.join(name)).unwrap();
            let output = program.check(LoweringPolicy::Comptime).unwrap();
            history::persist(
                &dir,
                &format!("{name}-source"),
                0,
                &text,
                &format!(
                    "transferred original sheet witness expected source/V ACCEPT; source_diagnostics={:?}; provenance={}",
                    output.diagnostics,
                    provenance()
                ),
            );
            assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
            let result = program.engine.export_native_input();
            history::persist(
                &dir,
                name,
                0,
                &text,
                &format!(
                    "source accepted; export={:?}",
                    result.as_ref().map(|_| "ACCEPT")
                ),
            );
            let input = result.expect("checker-positive/V closure capture TypeMismatch");
            validate(&input).unwrap();
        }
    });
}

#[test]
fn sheet_borrowed_capture_affected_rows_runtime() {
    with_compiler_stack(|| {
        let dir = artifacts("sheet-affected-borrowed");
        let runner = history::Runner::with_matrix(&dir.join("runs"), configurations()).unwrap();
        for seed in [6, 8, 10, 15, 23] {
            let row = Row(TABLE[seed]);
            let ops = history(row);
            let attempt = evaluate(row, &ops, Some(&runner), &format!("affected-{seed}"));
            retain(
                &dir,
                &format!("affected-{seed}"),
                seed,
                row,
                &ops,
                &source(row, &ops),
                &attempt.detail,
            );
            assert!(attempt.failure.is_none(), "{:?}", attempt.failure);
            assert_eq!(attempt.report.unwrap().cells.len(), 4);
        }
        eprintln!("CAPTURE_AFFECTED_COUNTS observations_per_engine=5");
    });
}

#[test]
fn sheet_originals_runtime_and_capture_transfer_adversaries() {
    with_compiler_stack(|| {
        let dir = artifacts("sheet-original-runtime");
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/capture_contract_matrix_31");
        let runner = history::Runner::with_matrix(&dir.join("runs"), configurations()).unwrap();
        let mut mutation_sites = 0;
        for name in ["sheet-minimal.hir", "sheet-taken.hir"] {
            let text = std::fs::read_to_string(root.join(name)).unwrap();
            let mut program = load_program(&root.join(name)).unwrap();
            let source = program.check(LoweringPolicy::Comptime).unwrap();
            assert!(source.diagnostics.is_empty(), "{:?}", source.diagnostics);
            let input = program.engine.export_native_input().unwrap();
            validate(&input).unwrap();
            let job = input
                .definitions
                .iter()
                .find(|(p, _)| p.0.name == "job")
                .unwrap()
                .1;
            let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
            signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
            let mut selected = None;
            'producer: for (bi, body) in job.bodies.iter().enumerate() {
                for (ri, rvalue) in body.rvalues.iter().enumerate() {
                    if let Rvalue::Closure { captures, .. } = rvalue {
                        for (ci, capture) in captures.iter().enumerate() {
                            let place = &body.places[capture.place.0 as usize];
                            if capture.usage == UseKind::Borrow
                                && matches!(
                                    input.types.row(place.ty).kind,
                                    hirsel_types::TypeKind::Box(_)
                                )
                            {
                                selected = Some((bi, ri, ci, capture.place));
                                break 'producer;
                            }
                        }
                    }
                }
            }
            let (bi, ri, ci, place) = selected.expect("zero captured borrowed view probes");
            let mut forged = (**job).clone();
            let Rvalue::Closure { captures, .. } = &mut forged.bodies[bi].rvalues[ri] else {
                unreachable!()
            };
            captures[ci].usage = UseKind::Take;
            let report = hirsel_validate::validate(&forged, &input.types, &signatures, &mut vec![]);
            history::persist(
                &dir,
                &format!("{name}-mutation"),
                0,
                &text,
                &format!(
                    "original validates; Body{bi} Rvalue{ri} capture{ci} Borrow->Take Place{place:?}; V={report:?}; provenance={}",
                    provenance()
                ),
            );
            // Construction cannot invent ownership in a scoped Borrow
            // capture or consume it while retaining a Many callable type.
            // These are producer classification rules; the earlier eight
            // Copy->Take invocation adversaries still require BorrowedMove.
            for expected in ["closure source classification", "closure multiplicity"] {
                assert!(report.violations.iter().any(|v|matches!(v.kind,hirsel_validate::ViolationKind::TypeMismatch{entity} if entity==expected)),"{report:?}");
            }
            mutation_sites += 1;
            let validated = program.engine.import_native_input(input).unwrap();
            let entry = program.entry;
            let reference = run_observed_snapshot(
                &mut program.engine,
                &RunConfiguration {
                    entry: entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            )
            .unwrap();
            let report = runner.compare(
                &validated,
                &entry,
                &Observation::of_reference(reference.observation.as_ref().unwrap()),
                name,
            );
            history::persist(
                &dir,
                name,
                0,
                &text,
                &format!(
                    "model callback result=7 checked by entry, output=[] visible destructors=[] exit0; reference_full={reference:?}; report={report:#?}; provenance={}",
                    provenance()
                ),
            );
            assert!(report.agrees(), "{report:#?}");
            assert_eq!(report.cells.len(), 4);
            assert!(report.disagreements(b"", false).is_empty());
        }
        assert_eq!(mutation_sites, 2);
        eprintln!("CAPTURE_SHEET_COUNTS observations_per_engine=2 mutations=2");
    });
}

#[test]
fn sheet_diagnostic_o0_runtime() {
    let dir = artifacts("sheet-o0");
    let matrix = vec![Configuration {
        engine: Engine::Native {
            opt: OptLevel::None,
            partition: Partition::Whole,
            runtime: Runtime::Diagnostic,
        },
        options: Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
    }];
    let runner = history::Runner::with_matrix(&dir.join("runs"), matrix).unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/capture_contract_matrix_31");
    for name in ["sheet-minimal.hir", "sheet-taken.hir"] {
        let text = std::fs::read_to_string(root.join(name)).unwrap();
        let report = runner.run(name, &text);
        history::persist(
            &dir,
            name,
            0,
            &text,
            &format!(
                "affected lowering witness LLVM O0 diagnostic; model exit0 output=[]; report={report:#?}; provenance={}",
                provenance()
            ),
        );
        assert!(report.agrees(), "{report:#?}");
        assert_eq!(report.cells.len(), 2);
        assert!(report.disagreements(b"", false).is_empty());
    }
    eprintln!("CAPTURE_O0_COUNTS reference=2 llvm_o0_diagnostic=2");
}
