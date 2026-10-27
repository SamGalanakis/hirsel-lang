//! Finite pairwise histories, not a soundness proof. The independent model
//! follows Hirsel GEN-1/MODE-2 prefix contracts and DROP-2, not Rust borrowing.
//! Reuses history21's runner/persistence/reduction interfaces. Call-edge shapes
//! follow rustc initialized.rs:255-303 and framework/tests.rs:1-68 at
//! 19833af7224424e8b53e288097d56b6af43d5bcf; no upstream code is copied.
#[path = "../support/history.rs"]
#[allow(dead_code)]
mod history;
use hirsel_driver::{compiler_adapters, native::*};
use hirsel_engine::*;
use std::{collections::BTreeSet, path::Path};

// Greedy lexicographic maximum-uncovered-pairs table, frozen before consumers.
// Axes: loop (for/while-pop/receiving), length, container (List/Option/Result),
// payload (Bytes/Row[Bytes]), producer (concrete/generic), module boundary,
// exit (normal/continue/break/return), callback input (literal/capture-free).
const TABLE: [[u8; 8]; 16] = [
    [0, 0, 0, 0, 0, 0, 0, 0],
    [0, 1, 1, 1, 1, 1, 1, 1],
    [1, 2, 2, 0, 0, 1, 2, 1],
    [2, 2, 2, 1, 1, 0, 3, 0],
    [1, 0, 0, 1, 1, 0, 2, 1],
    [2, 1, 1, 0, 0, 0, 1, 0],
    [2, 0, 0, 0, 0, 1, 3, 1],
    [1, 1, 1, 0, 1, 1, 0, 0],
    [0, 2, 2, 1, 0, 0, 0, 1],
    [0, 0, 1, 0, 0, 0, 2, 0],
    [1, 0, 2, 0, 0, 0, 1, 0],
    [0, 1, 0, 0, 0, 0, 3, 0],
    [0, 2, 0, 0, 0, 0, 1, 0],
    [1, 2, 1, 0, 0, 0, 3, 0],
    [2, 1, 2, 0, 0, 0, 2, 0],
    [2, 0, 0, 0, 0, 0, 0, 0],
];
const AXES: [&str; 8] = [
    "loop",
    "length",
    "container",
    "payload",
    "producer",
    "boundary",
    "exit",
    "callback",
];
const SIZES: [u8; 8] = [3, 3, 3, 2, 2, 2, 4, 2];

#[derive(Clone, Debug)]
struct Case {
    seed: usize,
    axes: [u8; 8],
    empty_payload: bool,
}
fn cases() -> Vec<Case> {
    let mut rows: Vec<_> = TABLE
        .into_iter()
        .enumerate()
        .map(|(seed, axes)| Case {
            seed,
            axes,
            empty_payload: seed % 3 == 0,
        })
        .collect();
    // Eight exact explicit/implicit twins. Even empty outer lists still call
    // the producer and enter the loop header, with zero successful steps.
    for (seed, parent) in [0, 1, 3, 5, 6, 8, 12, 14].into_iter().enumerate() {
        let mut twin = rows[parent].clone();
        twin.seed = 16 + seed;
        twin.axes[0] = if twin.axes[0] == 0 { 2 } else { 0 };
        rows.push(twin);
    }
    let mut empty_option = rows[1].clone();
    empty_option.seed = 24;
    empty_option.empty_payload = true;
    rows.push(empty_option.clone());
    empty_option.seed = 25;
    empty_option.axes[0] = 2;
    rows.push(empty_option);
    rows
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Op {
    Construct(Vec<u8>),
    Produce,
    Receive,
    Iterate,
    Observe,
}
#[derive(Clone, Debug, Default)]
struct Model {
    live: Option<Vec<u8>>,
    produced: bool,
    received: bool,
    iterated: bool,
    output: Vec<u8>,
    events: Vec<&'static str>,
}
fn history(case: &Case) -> Vec<Op> {
    vec![
        Op::Construct((0..case.axes[1]).map(|n| b'a' + n).collect()),
        Op::Produce,
        Op::Receive,
        Op::Iterate,
        Op::Observe,
    ]
}
fn model(case: &Case, ops: &[Op]) -> Option<Model> {
    let mut state = Model::default();
    for op in ops {
        match op {
            Op::Construct(values)
                if state.live.is_none()
                    && !state.produced
                    && values.len() <= 2
                    && values.len() == usize::from(case.axes[1]) =>
            {
                state.live = Some(values.clone())
            }
            Op::Produce if !state.produced && state.live.is_some() => state.produced = true,
            Op::Receive if state.produced && !state.received => state.received = true,
            Op::Iterate if state.received && !state.iterated => {
                let mut values = state.live.take()?;
                if case.axes[0] == 1 {
                    values.reverse();
                }
                let mut returned = false;
                for value in values {
                    state.output.push(b'i');
                    state
                        .output
                        .push(if case.empty_payload { b'e' } else { value });
                    if case.axes[6] == 0 {
                        state.output.push(b't');
                    }
                    state.output.push(b'd');
                    state.events.push("iteration");
                    if case.axes[6] >= 2 {
                        returned = case.axes[6] == 3;
                        break;
                    }
                }
                if !returned {
                    state.output.push(b'z');
                }
                state.output.extend_from_slice(b"21");
                state.events.extend(["second", "first"]);
                state.iterated = true;
            }
            Op::Observe if state.iterated => {}
            _ => return None,
        }
    }
    state.iterated.then_some(state)
}

#[derive(Clone, Debug)]
struct Modules {
    sources: Vec<(String, Vec<String>, String)>,
}
impl Modules {
    fn manifest(&self) -> serde_json::Value {
        serde_json::json!({"schema":1,"package":{"name":"standalone","version":"0"},
            "sources":self.sources.iter().map(|(path,module,_)| serde_json::json!({"path":path,"module":module})).collect::<Vec<_>>(),
            "entry":{"module":[],"name":"main"},"dependencies":[]})
    }
    fn persist(&self, directory: &Path, name: &str, case: &Case, detail: &str) {
        let root = directory.join(name);
        assert!(
            !root.exists(),
            "attempt must not overwrite {}",
            root.display()
        );
        history::persist(
            directory,
            name,
            case.seed as u64,
            &self.sources[0].2,
            detail,
        );
        for (path, _, text) in &self.sources {
            std::fs::write(root.join(path), text).unwrap();
        }
        let manifest = self.manifest();
        std::fs::write(root.join("hirsel.json"), manifest.to_string()).unwrap();
        eprintln!(
            "GENERIC_LOOP_ARTIFACT {}",
            serde_json::json!({"name":name,"seed":case.seed,"axes":case.axes,
            "empty_payload":case.empty_payload,"modules":self.sources,"manifest":manifest,"detail":detail,"provenance":provenance()})
        );
    }
}

fn emit(case: &Case, ops: &[Op]) -> Modules {
    assert!(model(case, ops).is_some(), "invalid generation");
    assert!(ops.len() <= 12);
    let prefix = if case.axes[5] == 1 { "producer." } else { "" };
    let payload = if case.axes[3] == 0 {
        "Bytes".into()
    } else {
        format!("{prefix}Row[unique Bytes]")
    };
    let inner = match case.axes[2] {
        0 => format!("List[unique {payload}]"),
        1 => format!("Option[unique {payload}]"),
        _ => format!("Result[unique {payload}, u64]"),
    };
    let list = format!("unique List[unique {inner}]");
    let own_inner = inner.replace("producer.", "");
    let producer = if case.axes[4] == 1 {
        "pub fn produce[T](take xs: unique List[unique T]) -> unique List[unique T] { take xs }\n"
            .into()
    } else {
        format!(
            "pub fn produce(take xs: unique List[unique {own_inner}]) -> unique List[unique {own_inner}] {{ take xs }}\n"
        )
    };
    let helper = if case.axes[3] == 1 && case.axes[5] == 1 {
        "pub fn unrow[T](take row: unique Row[unique T]) -> unique T { let Row(value: data) = take row; take data }\n"
    } else {
        ""
    };
    let declarations = format!(
        "pub type Row[T] {{ value: T }}\n{helper}pub fn relay[T](take xs: unique List[unique T]) -> unique List[unique T] {{ let mut gathered: unique List[unique T] = []; for value in take xs {{ list.push(mut gathered, take value) }}; take gathered }}\n{producer}"
    );
    let values = ops
        .iter()
        .find_map(|op| {
            if let Op::Construct(values) = op {
                Some(values)
            } else {
                None
            }
        })
        .unwrap();
    let rows: Vec<_> = values
        .iter()
        .map(|value| {
            let bytes = if case.axes[7] == 1 {
                format!("(fn() -> unique Bytes {{ b\"{}\" }})()", char::from(*value))
            } else {
                format!("b\"{}\"", char::from(*value))
            };
            let value = if case.axes[3] == 0 {
                bytes
            } else {
                format!("{prefix}Row(value: {bytes})")
            };
            match (case.axes[2], case.empty_payload) {
                (0, true) => "[]".into(),
                (0, false) => format!("[{value}]"),
                (1, true) => "None()".into(),
                (1, false) => format!("Some({value})"),
                (_, true) => "Error(7u64)".into(),
                (_, false) => format!("Ok({value})"),
            }
        })
        .collect();
    let unpack = if case.axes[3] == 0 {
        "emit(stdout, take value)".into()
    } else if case.axes[5] == 1 {
        format!("emit(stdout, take {prefix}unrow(take value))")
    } else {
        "let Row(value: data) = take value; emit(stdout, take data)".into()
    };
    let visit = match case.axes[2] {
        0 => format!(
            "if list.len(item) == 0 {{ let _ = out.print(stdout, \"e\") }}; for value in take item {{ {unpack} }}"
        ),
        1 => format!(
            "match take item {{ Some(value) => {{ {unpack} }}, None() => {{ let _ = out.print(stdout, \"e\") }} }}"
        ),
        _ => format!(
            "match take item {{ Ok(value) => {{ {unpack} }}, Error(_) => {{ let _ = out.print(stdout, \"e\") }} }}"
        ),
    };
    let exit = match case.axes[6] {
        0 => "let _ = out.print(stdout, \"t\")",
        1 => "continue",
        2 => "break",
        _ => "return",
    };
    let body = format!(
        "let iteration = Trace(out: stdout, text: \"d\"); let _ = out.print(stdout, \"i\"); {visit}; {exit}"
    );
    let call = format!("{prefix}produce(take {prefix}relay(take xs))");
    let loop_body = match case.axes[0] {
        0 => format!("for item in take {call} {{ {body} }}"),
        1 => format!(
            "let mut received: {list} = {call}\nwhile let Some(item) = list.pop(mut received) {{ {body} }}"
        ),
        _ => format!("let received: {list} = {call}\nfor item in take received {{ {body} }}"),
    };
    let unique_check = if ops.iter().any(|op| matches!(op, Op::Observe)) {
        "need(take value)"
    } else {
        ""
    };
    let main = format!(
        r#"resource type Trace {{ out: Stdout, text: String }}
 drop(x) {{ let _ = out.print(x.out, x.text) }}
fn need[T](take value: unique T) {{}}
fn emit(stdout: Stdout, take value: unique Bytes) {{
 let _ = out.print(stdout, if value[0] == 97u8 {{ "a" }} else {{ "b" }})
 {unique_check}
}}
fn work(stdout: Stdout, take xs: {list}) {{
 let first = Trace(out: stdout, text: "1")
 let second = Trace(out: stdout, text: "2")
 {loop_body}
 let _ = out.print(stdout, "z")
}}
pub fn main(rt: Io) -> Result[(), GrantError] {{
 let stdout = io.stdout(rt)?
 let xs: {list} = [{}]
 work(stdout, take {prefix}relay(take xs))
 Ok(())
}}
"#,
        rows.join(",")
    );
    let modules = if case.axes[5] == 1 {
        vec![
            ("main.hir".into(), vec![], main),
            ("producer.hir".into(), vec!["producer".into()], declarations),
        ]
    } else {
        vec![("main.hir".into(), vec![], format!("{declarations}{main}"))]
    };
    assert!(modules.iter().all(|(_, _, s)| s.lines().count() <= 128));
    Modules { sources: modules }
}

// Source checking is recorded before native export, including API errors.
fn check(modules: &Modules) -> Result<QueryEngine, (String, String)> {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let mut main = None;
    for (path, module, text) in &modules.sources {
        let file = engine.sources.add(path, text.clone()).unwrap();
        let mut context = default_context();
        context.modules = module.clone();
        engine.parse_file(file, &context).map_err(|e| {
            (
                match &e {
                    QueryError::Diagnostics(_) => "source",
                    QueryError::Operational(f) if f.operation == "TIR validation" => "V-lower",
                    _ => "source-api",
                }
                .into(),
                format!("{e:?}"),
            )
        })?;
        if module.is_empty() {
            main = Some(file);
        }
    }
    let checked = check_progress(&mut engine, main.unwrap(), &default_context()).map_err(|e| {
        (
            match &e {
                QueryError::Diagnostics(_) => "source",
                QueryError::Operational(f) if f.operation == "TIR validation" => "V-lower",
                _ => "source-api",
            }
            .into(),
            format!("{e:?}"),
        )
    })?;
    let errors: Vec<_> = checked
        .diagnostics
        .iter()
        .filter(|d| d.severity == hirsel_diag::Severity::Error)
        .collect();
    eprintln!("SOURCE_DIAGNOSTICS {errors:?}");
    if errors.is_empty() {
        Ok(engine)
    } else {
        Err((
            "source".into(),
            format!(
                "rules={:?}; sites={:?}; detail={errors:?}",
                errors.iter().map(|d| d.rule.as_str()).collect::<Vec<_>>(),
                errors
                    .iter()
                    .map(|d| checked.sources.slice(d.primary))
                    .collect::<Vec<_>>()
            ),
        ))
    }
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
fn run(
    runner: &history::Runner,
    name: &str,
    modules: &Modules,
) -> Result<history::Report, (String, String)> {
    let mut engine = check(modules)?;
    let input = engine
        .export_native_input()
        .map_err(|e| ("V-export".into(), format!("{e:?}")))?;
    let validated = engine
        .import_native_input(input)
        .map_err(|e| ("V-import".into(), format!("{e:?}")))?;
    let entry = entry_path(&default_context());
    let run = run_observed_snapshot(
        &mut engine,
        &RunConfiguration {
            entry: entry.clone(),
            hash_seed: 31,
            inputs: vec![],
        },
        SimulationConfig::default(),
    )
    .map_err(|e| ("reference-api".into(), format!("{e:?}")))?;
    let observation = run.observation.ok_or_else(|| {
        (
            "reference-incomplete".into(),
            format!(
                "outcome={:?}; step_usage={:?}; no completed semantic observation",
                run.outcome, run.step_usage
            ),
        )
    })?;
    Ok(runner.compare(
        &validated,
        &entry,
        &Observation::of_reference(&observation),
        name,
    ))
}
fn expected(model: &Model) -> Observation {
    Observation {
        exit_code: Some(0),
        blocked: false,
        budget: None,
        signaled: None,
        output: model.output.clone(),
        failures: vec![],
        roots: vec![],
    }
}
fn artifacts(label: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("generic-loop-31-{label}-"))
        .tempdir()
        .unwrap()
}
fn partition(start: usize, end: usize) {
    with_compiler_stack(|| {
        let artifacts = artifacts(&format!("{start}-{end}"));
        let runner =
            history::Runner::with_matrix(&artifacts.path().join("runs"), matrix()).unwrap();
        let mut counts = std::collections::BTreeMap::new();
        let mut reds = vec![];
        for case in &cases()[start..end] {
            let ops = history(case);
            let model = model(case, &ops).unwrap();
            let modules = emit(case, &ops);
            let name = format!("seed-{}", case.seed);
            modules.persist(
                artifacts.path(),
                &format!("{name}-original"),
                case,
                &format!("model={model:?}"),
            );
            let result = run(&runner, &name, &modules);
            let differences = signatures(&result, &model);
            if !differences.is_empty() {
                modules.persist(
                    artifacts.path(),
                    &format!("{name}-failure"),
                    case,
                    &format!("differences={differences:?}; result={result:?}"),
                );
                retain_failure(artifacts.path(), &runner, case, &ops, &differences[0]);
                reds.push(format!("seed={} {differences:?} {result:?}", case.seed));
            }
            if let Ok(report) = &result {
                for cell in &report.cells {
                    *counts.entry(cell.engine.clone()).or_insert(0) += 1;
                }
                if report.cells.len() != 4 {
                    reds.push(format!("seed={} missing engine cells", case.seed));
                }
                eprintln!(
                    "GENERIC_LOOP_RESULT seed={} engines={} report={report:?}",
                    case.seed,
                    report.cells.len()
                );
            }
        }
        eprintln!("GENERIC_LOOP_COUNTS partition={start}..{end} {counts:?}");
        assert!(reds.is_empty(), "{}", reds.join("\n"));
        assert_eq!(counts.len(), 4);
        assert!(counts.values().all(|&n| n == end - start));
    });
}
#[test]
fn generic_loop_matrix_31_runtime_00_08() {
    partition(0, 8);
}
#[test]
fn generic_loop_matrix_31_runtime_08_16() {
    partition(8, 16);
}
#[test]
fn generic_loop_matrix_31_runtime_16_24() {
    partition(16, 24);
}
#[test]
fn generic_loop_matrix_31_runtime_24_26() {
    partition(24, 26);
}

#[test]
fn generic_loop_matrix_31_negative_nested_annotations() {
    with_compiler_stack(|| {
        let directory = artifacts("negative");
        for case in &cases()[..12] {
            let mut modules = emit(case, &history(case));
            // Exactly one missing nested receiving qualifier on main's
            // source binding. GEN-1/MODE-2 under LANG-2 forbids inferring it
            // back from the later generic relay call.
            let main = &mut modules.sources[0].2;
            let start = main.find(" let xs: ").unwrap();
            let end = start + main[start..].find(" = ").unwrap();
            let before = main[start..end].to_owned();
            let after = before.replacen("List[unique ", "List[", 1);
            main.replace_range(start..end, &after);
            modules.persist(
                directory.path(),
                &format!("negative-{}", case.seed),
                case,
                "expected=MODE-2 at take xs in main; no V/runtime applicable after rejection",
            );
            let (_, detail) = check(&modules)
                .err()
                .expect("missing nested contract must reject");
            assert!(detail.contains("MODE-2"), "seed={} {detail}", case.seed);
            // Require the documented rule/site, not merely any source error.
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let mut main = None;
            for (path, module, text) in &modules.sources {
                let file = engine.sources.add(path, text.clone()).unwrap();
                let mut context = default_context();
                context.modules = module.clone();
                engine.parse_file(file, &context).unwrap();
                if module.is_empty() {
                    main = Some(file);
                }
            }
            let checked = check_progress(&mut engine, main.unwrap(), &default_context()).unwrap();
            let errors: Vec<_> = checked
                .diagnostics
                .iter()
                .filter(|d| d.severity == hirsel_diag::Severity::Error)
                .collect();
            assert_eq!(errors.len(), 1, "seed={} {errors:?}", case.seed);
            assert_eq!(errors[0].rule.as_str(), "MODE-2");
            assert_eq!(checked.sources.slice(errors[0].primary), Some("take xs"));
            eprintln!(
                "GENERIC_LOOP_NEGATIVE seed={} MODE-2 at main generic transfer; V/runtime=N/A",
                case.seed
            );
        }
    });
}

fn v_result_adversaries(seeds: &[usize], conservative_control: bool) {
    use hirsel_tir::{Callee, Terminator};
    use hirsel_types::{GenericArgument, Sendability, TypeKind, Uniqueness};
    use hirsel_validate::ViolationKind;
    with_compiler_stack(|| {
        let directory = artifacts("V");
        let mut mutations = 0;
        let mut sites = 0;
        for &seed in seeds {
            let function = if conservative_control {
                "work"
            } else {
                "relay"
            };
            let operation = "ListConsume_open";
            let case = &cases()[seed];
            let modules = emit(case, &history(case));
            let mut engine = check(&modules).unwrap();
            let input = engine.export_native_input().unwrap();
            let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
            signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
            let original = input
                .definitions
                .iter()
                .find(|(path, _)| path.0.name == function)
                .unwrap()
                .1;
            hirsel_validate::assert_valid(original, signatures.types(), &signatures);
            let mut found = vec![];
            for (index, body) in original.bodies.iter().enumerate() {
                for block in &body.blocks {
                    if let Terminator::Call {
                        callee: Callee::Definition { path, .. },
                        returns: Some(receipt),
                        ..
                    } = &block.terminator
                        && path.0.name == operation
                    {
                        found.push((index, body.places[receipt.destination.0 as usize].root));
                    }
                }
            }
            assert_eq!(found.len(), 1, "seed={seed}");
            if !conservative_control {
                sites += found.len();
            }
            let (index, root) = found[0];
            let cursor = input
                .types
                .row(original.bodies[index].locals[root.0 as usize].ty)
                .clone();
            let TypeKind::Named { arguments, .. } = &cursor.kind else {
                panic!("nominal cursor")
            };
            let [GenericArgument::Type(item)] = arguments.as_ref() else {
                panic!("one item")
            };
            let item = input.types.row(*item).clone();
            assert_eq!(item.modes.uniqueness, Uniqueness::Unique);
            modules.persist(directory.path(),&format!("seed-{seed}-{function}-{operation}-original"),case,&format!("original_item={item:?}; body={index}; root={root:?}; operation={operation}; original_tir={original:#?}"));
            if conservative_control {
                assert_eq!(item.modes.sendability, Sendability::Sendable);
            } else {
                let Sendability::DependsOn(dependencies) = &item.modes.sendability else {
                    panic!("generic result must retain its symbolic crossing contract: {item:?}")
                };
                assert_eq!(dependencies.len(), 1);
                assert_eq!(
                    item.kind,
                    hirsel_types::TypeKind::Parameter(dependencies[0].clone())
                );
            }
            let forged_sendability = if function == "work" {
                Sendability::NotSendable
            } else {
                Sendability::Sendable
            };
            for uniqueness in [false, true] {
                if conservative_control && uniqueness {
                    continue;
                }
                let mut forged_item = item.clone();
                if uniqueness {
                    forged_item.modes.uniqueness = Uniqueness::Aliased;
                } else {
                    forged_item.modes.sendability = forged_sendability.clone();
                }
                let mut only_mode = item.clone();
                only_mode.modes = forged_item.modes.clone();
                assert_eq!(only_mode, forged_item, "only one nested mode may differ");
                assert_eq!(item.modes.relation, forged_item.modes.relation);
                assert_eq!(item.modes.multiplicity, forged_item.modes.multiplicity);
                if uniqueness {
                    assert_eq!(item.modes.sendability, forged_item.modes.sendability);
                } else {
                    assert_eq!(item.modes.uniqueness, forged_item.modes.uniqueness);
                }
                let mut forged_cursor = cursor.clone();
                let TypeKind::Named { arguments, .. } = &mut forged_cursor.kind else {
                    unreachable!()
                };
                *arguments = vec![GenericArgument::Type(
                    input.types.intern(forged_item.clone()),
                )]
                .into();
                let ty = input.types.intern(forged_cursor);
                let mut forged = (**original).clone();
                let body = &mut forged.bodies[index];
                body.locals[root.0 as usize].ty = ty;
                for place in &mut body.places {
                    if place.root == root {
                        place.ty = ty;
                    }
                }
                forged.types = hirsel_tir::definition_types(&input.types, &forged);
                let report = hirsel_validate::validate(
                    &forged,
                    signatures.types(),
                    &signatures,
                    &mut vec![],
                );
                let types: Vec<_> = forged
                    .types
                    .iter()
                    .map(|id| (*id, input.types.row(*id).clone()))
                    .collect();
                modules.persist(directory.path(),&format!("seed-{seed}-{function}-{operation}-forge-{uniqueness}"),case,&format!("conservative_control={conservative_control}; original_item={item:?}; forged_item={forged_item:?}; body={index}; root={root:?}; operation={operation}; report={report:?}; original_tir={original:#?}; forged_tir={forged:#?}; type_universe={types:#?}; original_declared_signature={:?}",input.signatures.iter().find(|(path,_)| path.0.name == function).map(|(_,s)|s)));
                if conservative_control {
                    assert!(
                        report.violations.is_empty(),
                        "conservative sendability evidence may be omitted for this concrete payload: {report:?}"
                    );
                    eprintln!(
                        "GENERIC_LOOP_V_CONTROL reconstructed seed1 work ListConsume_open; Sendable->NotSendable; uniqueness unchanged; validates; original failed-adversary expectation retained externally"
                    );
                    continue;
                }
                assert!(
                    report.violations.iter().any(|v| v.kind
                        == ViolationKind::TypeMismatch {
                            entity: "call result"
                        }),
                    "seed={seed} operation={operation} {report:?}"
                );
                mutations += 1;
            }
        }
        assert_eq!(
            (sites, mutations),
            if conservative_control {
                (0, 0)
            } else {
                (seeds.len(), seeds.len() * 2)
            }
        );
        eprintln!("GENERIC_LOOP_V sites={sites} mutations={mutations}");
    });
}

// Separate identities retain independently completed sites when a later
// adversary needs setup repair. Four original generated inputs each provide
// one actual cursor-open receipt; the ledger names seed/body/root/operation.
#[test]
fn generic_loop_matrix_31_v_conservative_evidence_control() {
    v_result_adversaries(&[1], true);
}
#[test]
fn generic_loop_matrix_31_v_generic_open_seed_1() {
    v_result_adversaries(&[1], false);
}
#[test]
fn generic_loop_matrix_31_v_generic_open_seeds_3_5_14() {
    v_result_adversaries(&[3, 5, 14], false);
}

// A signature includes the phase, engine and rule/observation field. Source
// and V rejections are valid reducer targets; they never require V acceptance.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Signature {
    phase: String,
    engine: String,
    field: String,
}
fn signatures(result: &Result<history::Report, (String, String)>, model: &Model) -> Vec<Signature> {
    match result {
        Err((phase, detail)) => {
            let rules = [
                "MODE-1",
                "MODE-2",
                "MODE-4",
                "OWN-1",
                "OWN-2",
                "GEN-1",
                "LANG-2",
                "LANG-14",
                "TypeMismatch",
                "call result",
                "BorrowedMove",
            ];
            let field = if phase == "source" {
                detail.split("; detail=").next().unwrap().to_owned()
            } else {
                rules
                    .iter()
                    .filter(|rule| detail.contains(**rule))
                    .copied()
                    .collect::<Vec<_>>()
                    .join("/")
            };
            vec![Signature {
                phase: phase.clone(),
                engine: phase.clone(),
                field: if field.is_empty() {
                    detail.clone()
                } else {
                    field
                },
            }]
        }
        Ok(report) => {
            let target = expected(model);
            let mut out = vec![];
            for cell in &report.cells {
                if cell.category != history::Category::Observation {
                    out.push(Signature {
                        phase: "runtime".into(),
                        engine: cell.engine.clone(),
                        field: format!("{:?}:{}", cell.category, cell.disagreement),
                    });
                    continue;
                }
                let got = cell.observation.as_ref().unwrap();
                for (field, diff) in [
                    ("output", got.output != target.output),
                    ("exit", got.exit_code != target.exit_code),
                    ("failures", got.failures != target.failures),
                    ("roots", got.roots != target.roots),
                    ("blocked", got.blocked != target.blocked),
                    ("budget", got.budget != target.budget),
                    ("signal", got.signaled != target.signaled),
                ] {
                    if diff {
                        out.push(Signature {
                            phase: "runtime".into(),
                            engine: cell.engine.clone(),
                            field: format!("model-{field}"),
                        });
                    }
                }
            }
            out
        }
    }
}

// Bounded deletion, then value and aggregate-depth simplification. This adapts
// proptest's simplify/retain/restore control (traits.rs:580-621, num.rs:280-345)
// to our own finite histories, with no imported Rust ownership semantics.
fn reduce(
    original: &Case,
    ops: &[Op],
    mut preserves: impl FnMut(&Case, &[Op]) -> bool,
) -> (Case, Vec<Op>, usize, Vec<String>) {
    let mut case = original.clone();
    let mut attempts = 0;
    let mut decisions = vec![];
    let mut evaluate = |case: &Case, candidate: &[Op]| {
        if attempts >= 96 || model(case, candidate).is_none() {
            return false;
        }
        attempts += 1;
        let keep = preserves(case, candidate);
        decisions.push(format!(
            "candidate={attempts} case={case:?} ops={candidate:?} retain={keep}"
        ));
        keep
    };
    let mut kept = history::minimize(ops, |candidate| evaluate(&case, candidate));
    // Values first shrink their count, then their magnitude. A zero-item
    // history remains valid but cannot preserve an item-output disagreement.
    for count in [0, 1] {
        let mut candidate = kept.clone();
        if let Some(Op::Construct(values)) = candidate.first_mut() {
            if values.len() <= count {
                continue;
            }
            values.truncate(count);
        }
        let mut smaller = case.clone();
        smaller.axes[1] = count as u8;
        if evaluate(&smaller, &candidate) {
            kept = candidate;
            case = smaller;
        }
    }
    let mut candidate = kept.clone();
    if let Some(Op::Construct(values)) = candidate.first_mut() {
        values.fill(b'a');
    }
    if format!("{candidate:?}") != format!("{kept:?}") && evaluate(&case, &candidate) {
        kept = candidate;
    }
    for axis in [3, 2] {
        if case.axes[axis] == 0 {
            continue;
        }
        let mut smaller = case.clone();
        smaller.axes[axis] = 0;
        if evaluate(&smaller, &kept) {
            case = smaller;
        }
    }
    (case, kept, attempts, decisions)
}

fn retain_failure(
    directory: &Path,
    runner: &history::Runner,
    case: &Case,
    ops: &[Op],
    target: &Signature,
) {
    let mut n = 0;
    let (reduced, kept, attempts, decisions) = reduce(case, ops, |candidate, ops| {
        n += 1;
        let modules = emit(candidate, ops);
        let result = run(runner, &format!("reduce-{}-{n}", case.seed), &modules);
        let preserves = signatures(&result, &model(candidate, ops).unwrap()).contains(target);
        if preserves {
            modules.persist(
                directory,
                &format!("seed-{}-retained-{n}", case.seed),
                candidate,
                &format!("target={target:?}; result={result:?}"),
            );
        }
        preserves
    });
    emit(&reduced,&kept).persist(directory,&format!("seed-{}-reduced",case.seed),&reduced,
        &format!("target={target:?}; original={case:?} {ops:?}; reduced={kept:?}; attempts={attempts}; decisions={decisions:?}; original remains red"));
}

fn provenance() -> &'static serde_json::Value {
    static VALUE: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
    VALUE.get_or_init(|| {
        use sha2::{Digest,Sha256};
        use std::io::Read;
        fn collect(path:std::path::PathBuf, files:&mut std::collections::BTreeMap<String,String>, seen:&mut BTreeSet<std::path::PathBuf>) {
            let real=path.canonicalize().unwrap();
            if !seen.insert(real.clone()) {return;}
            if real.is_dir() {
                for entry in std::fs::read_dir(&real).unwrap() {collect(entry.unwrap().path(),files,seen);}
            } else {
                let mut file=std::fs::File::open(&real).unwrap();let mut hash=Sha256::new();let mut buffer=[0u8;65536];
                loop {let n=file.read(&mut buffer).unwrap();if n==0 {break;}hash.update(&buffer[..n]);}
                files.insert(path.display().to_string(),format!("{:x}",hash.finalize()));
            }
        }
        let mut files=std::collections::BTreeMap::new();let mut seen=BTreeSet::new();let mut environment=std::collections::BTreeMap::new();
        collect(std::env::current_exe().unwrap(),&mut files,&mut seen);
        for key in ["HIRSEL_PRODUCTS","HIRSEL_RUNTIME_ARCHIVES","HIRSEL_LINK_KIT","HIRSEL_GLIBC_FLOOR","LLVM_SYS_231_PREFIX"] {
            if let Some(value)=std::env::var_os(key) {
                environment.insert(key,value.to_string_lossy().into_owned());
                let path=std::path::PathBuf::from(value);
                if key=="LLVM_SYS_231_PREFIX" {
                    for tool in ["bin/clang","bin/ld.lld"] {collect(path.join(tool),&mut files,&mut seen);}
                } else {collect(path,&mut files,&mut seen);}
            }
        }
        serde_json::json!({"compiler_identity":format!("{:?}",hirsel_driver::COMPILER_IDENTITY),"files_sha256":files,"environment":environment,
            "observation_schema":OBSERVATION_SCHEMA,"matrix":MATRIX_VERSION,
            "observation_limits":"Native and H do not expose a separate resource/finalizer/capability event stream or arbitrary returned graph: those fields INCOMPLETE. Here visible destructor order and payload values are explicit output tokens, entry returns unit; roots tracing disabled/empty; no file stages (Pending/AfterWrite N/A)."})
    })
}

#[test]
fn generic_loop_matrix_31_permanent_manifest_twins() {
    with_compiler_stack(|| {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/generic_loop_matrix_31");
        for name in ["complete", "incomplete"] {
            let mut program =
                hirsel_driver::load_program_source(&root.join(format!("{name}.json")), None)
                    .unwrap();
            let checked = program.check(LoweringPolicy::Reachable).unwrap();
            let errors: Vec<_> = checked
                .diagnostics
                .iter()
                .filter(|d| d.severity == hirsel_diag::Severity::Error)
                .collect();
            if name == "complete" {
                assert!(errors.is_empty(), "{errors:?}");
                program.check_for_execution().unwrap();
            } else {
                assert_eq!(errors.len(), 1, "{errors:?}");
                assert_eq!(errors[0].rule.as_str(), "MODE-2");
                assert_eq!(
                    checked.sources.slice(errors[0].primary),
                    Some("take source")
                );
            }
        }
    });
}

#[test]
fn generic_loop_matrix_31_unique_error_mapper_producer() {
    with_compiler_stack(|| {
        let source = include_str!("../fixtures/generic_loop_matrix_31/error-mapper.hir");
        let modules = Modules {
            sources: vec![("main.hir".into(), vec![], source.into())],
        };
        let directory = artifacts("error-mapper");
        let case = &cases()[0];
        modules.persist(directory.path(),"original",case,"ToyAnalyzer cross-lane minimized witness: source accepts, expect exact mapper result receipt and V accept");
        let mut program = hirsel_driver::load_program_source(
            &directory.path().join("original/hirsel.json"),
            None,
        )
        .unwrap();
        let checked = program.check(LoweringPolicy::Comptime).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        program
            .check_for_execution()
            .expect("source-accepted unique mapper result must export");
    });
}

#[test]
fn generic_loop_matrix_31_incomplete_error_propagation() {
    with_compiler_stack(|| {
        for source in [
            include_str!("../fixtures/generic_loop_matrix_31/propagate-incomplete.hir"),
            include_str!("../fixtures/generic_loop_matrix_31/decode.hir"),
        ] {
            let mut engine = crate::source_std::engine();
            let file = engine.sources.add("main.hir", source.into()).unwrap();
            let inferred = check_progress(&mut engine, file, &default_context()).unwrap();
            assert!(
                inferred
                    .diagnostics
                    .iter()
                    .all(|d| d.severity != hirsel_diag::Severity::Error),
                "{:?}",
                inferred.diagnostics
            );
            // Private producers prove the omitted uniqueness from their bodies.
            // Exported twins must still reject every missing written contract.
            let source = source
                .replace("fn source(", "pub fn source(")
                .replace("fn number(", "pub fn number(")
                .replace("type Problem =", "pub type Problem =");
            let mut engine = crate::source_std::engine();
            let file = engine.sources.add("main.hir", source.clone()).unwrap();
            // V must not be the first phase to reject the missing source contract.
            let output = check_progress(&mut engine, file, &default_context());
            let checked=output.expect("incomplete error annotation needs a source diagnostic, not a V operational rejection");
            let errors: Vec<_> = checked
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.severity == hirsel_diag::Severity::Error)
                .collect();
            let original_decode = source.contains("pub fn decode");
            assert_eq!(
                errors.len(),
                if original_decode { 7 } else { 1 },
                "{errors:?}"
            );
            for error in errors {
                assert_eq!(error.rule.as_str(), "MODE-2", "{error:?}");
                let site = checked.sources.slice(error.primary).unwrap();
                assert!(
                    site.starts_with(if original_decode {
                        "number("
                    } else {
                        "source()"
                    }) && site.ends_with('?'),
                    "{site:?}"
                );
            }
        }
    });
}
#[test]
fn generic_loop_matrix_31_projected_callable_result() {
    with_compiler_stack(|| {
        let source = include_str!("../fixtures/generic_loop_matrix_31/call-result.hir");
        let mut engine = crate::source_std::engine();
        let file = engine.sources.add("main.hir", source.into()).unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let input = engine.export_native_input().unwrap();
        engine.import_native_input(input).unwrap();
    });
}

#[test]
fn generic_loop_matrix_31_error_contract_controls() {
    with_compiler_stack(|| {
        let bad = include_str!("../fixtures/generic_loop_matrix_31/propagate-incomplete.hir");
        let cases = [
            (
                "nested",
                bad.replace(
                    "source() -> Result[u64, Problem]",
                    "source() -> Result[u64, unique Problem]",
                ),
                true,
            ),
            (
                "whole",
                bad.replace(
                    "source() -> Result[u64, Problem]",
                    "source() -> unique Result[u64, Problem]",
                ),
                true,
            ),
            (
                "generic_mapper",
                bad.replace(
                    "source() -> Result[u64, Problem]",
                    "source() -> Result[u64, unique Problem]",
                )
                .replace("Ok(source()?)", "Ok(source()? identity)")
                    + "\nfn identity[T](take value:T)->T {take value}\n",
                true,
            ),
            (
                "ordinary_mapper",
                bad.replace("Ok(source()?)", "Ok(source()? mapper)")
                    + "\nfn mapper(take value:Problem)->Problem {take value}\n",
                false,
            ),
            (
                "unique_mapper",
                bad.replace("Ok(source()?)", "Ok(source()? mapper)")
                    + "\nfn mapper(take value:Problem)->unique Problem {unique value}\n",
                true,
            ),
            (
                "decode_complete",
                include_str!("../fixtures/generic_loop_matrix_31/decode.hir").replace(
                    "-> Result[u64, ReplayError]",
                    "-> Result[u64, unique ReplayError]",
                ),
                true,
            ),
        ];
        let mut reds = vec![];
        for (name, text, accepted) in cases {
            let mut engine = crate::source_std::engine();
            let file = engine.sources.add(format!("{name}.hir"), text).unwrap();
            match check_progress(&mut engine, file, &default_context()) {
                Ok(checked) => {
                    let errors: Vec<_> = checked
                        .diagnostics
                        .iter()
                        .filter(|d| d.severity == hirsel_diag::Severity::Error)
                        .collect();
                    if accepted && !errors.is_empty()
                        || !accepted && !errors.iter().any(|d| d.rule.as_str() == "MODE-2")
                    {
                        reds.push(format!("{name}: {errors:?}"));
                    }
                    if accepted && errors.is_empty() {
                        let input = engine.export_native_input().unwrap();
                        engine.import_native_input(input).unwrap();
                    }
                }
                Err(e) => reds.push(format!("{name}: {e:?}")),
            }
        }
        assert!(reds.is_empty(), "{reds:#?}");
    });
}

#[test]
fn generic_loop_matrix_31_transferred_producers_four_engines() {
    with_compiler_stack(|| {
        let directory = artifacts("transferred-runtime");
        let mut configurations = matrix();
        configurations.push(Configuration {
            options: Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
            engine: Engine::Native {
                opt: OptLevel::None,
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        });
        let runner =
            history::Runner::with_matrix(&directory.path().join("runs"), configurations).unwrap();
        let mapper = format!(
            "{}\n{}",
            include_str!("../fixtures/generic_loop_matrix_31/error-mapper.hir")
                .replace("pub fn main()", "fn mapped()"),
            "pub fn main(rt:Io)->Result[(),GrantError] { let stdout=io.stdout(rt)?; match mapped() {Error(mapped_text)=>{let _=out.print(stdout,mapped_text)},Ok(())=>{let _=out.print(stdout,\"unexpected\")}};Ok(())}"
        );
        let projected =
            include_str!("../fixtures/generic_loop_matrix_31/call-result.hir").to_owned();
        for (name, source, output) in [
            ("mapper", mapper, b"example".as_slice()),
            ("projected", projected, b"".as_slice()),
        ] {
            let case = &cases()[0];
            let modules = Modules {
                sources: vec![("main.hir".into(), vec![], source)],
            };
            modules.persist(directory.path(),&format!("{name}-original"),case,&format!("model exit=0 stdout={output:?} no failures/roots/block/budget/signal; new lowering witness includes diagnostic/O0"));
            let report = run(&runner, name, &modules).unwrap();
            let mut expected = expected(&Model::default());
            expected.output = output.to_vec();
            assert_eq!(report.cells.len(), 5);
            assert!(
                report
                    .cells
                    .iter()
                    .all(|cell| cell.category == history::Category::Observation
                        && cell.observation.as_ref() == Some(&expected)),
                "{name}: {report:?}"
            );
            eprintln!("GENERIC_LOOP_TRANSFERRED_RUNTIME {name} cells=5 report={report:?}");
        }
    });
}

#[test]
fn generic_loop_matrix_31_generated_manifest_replay() {
    with_compiler_stack(|| {
        let directory = artifacts("manifest-replay");
        let case = &cases()[1];
        let modules = emit(case, &history(case));
        modules.persist(
            directory.path(),
            "generated",
            case,
            "same standalone@0 package/module set as QueryEngine source and native input",
        );
        let mut program = hirsel_driver::load_program_source(
            &directory.path().join("generated/hirsel.json"),
            None,
        )
        .unwrap();
        assert_eq!(program.entry.0.package, default_context().package);
        let source = program.check(LoweringPolicy::Comptime).unwrap();
        assert!(source.diagnostics.is_empty(), "{:?}", source.diagnostics);
        program.check_for_execution().unwrap();
        let input = program.engine.export_native_input().unwrap();
        program.engine.import_native_input(input).unwrap();
    });
}
