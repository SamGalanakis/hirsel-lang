//! LANG-13 plain-value selection, independently of coverage and CFG lowering.
//! Reuses the landed coverage model's bool/product vectors, adapted from
//! rust-analyzer missing_match_arms.rs:80-157, 3131751e (MIT/Apache-2.0).
//! Generation is a deterministic bounded recursive grid (proptest recursive.rs
//! :150-208, 70cf37d4, MIT/Apache-2.0); no random stress or Rust let semantics.
#[allow(dead_code)]
#[path = "../../../hirsel-validate/tests/it/match_coverage_22/model.rs"]
mod model;
use crate::rc_support::history::{Category, Runner, persist};
use hirsel_driver::native::*;
use model::{Arm, Domain, Pattern, Value};
use std::fmt::Write;

/// Evaluate concrete predicates directly, without expanding pattern choices or
/// consulting coverage/usefulness, TIR tables, or a lowered control-flow graph.
fn matches(pattern: &Pattern, value: &Value) -> bool {
    match (pattern, value) {
        (Pattern::Wild, _) => true,
        (Pattern::Exact(a), b) => a == b,
        (Pattern::Pair(a, b), Value::Pair(x, y)) => matches(a, x) && matches(b, y),
        (Pattern::Sum(a, p), Value::Sum(b, v)) => a == b && matches(p, v),
        (Pattern::Or(ps), v) => ps.iter().any(|p| matches(p, v)),
        _ => false,
    }
}
fn selected(arms: &[Arm], value: &Value) -> u32 {
    let index = arms
        .iter()
        .position(|a| matches(&a.pattern, value) && a.guard != Some(false));
    11 * (u32::try_from(index.expect("exhaustive finite input")).unwrap() + 1)
}

/// Boolean leaves, then at most two product/constructor levels; <=8 subjects.
fn domain(index: usize) -> Domain {
    let pair = |a, b| Domain::Pair(Box::new(a), Box::new(b));
    match index {
        0 => Domain::Bool,
        1 => pair(Domain::Bool, Domain::Bool),
        2 => pair(Domain::Bool, pair(Domain::Unit, Domain::Bool)),
        3 => Domain::Sum(Box::new(pair(Domain::Bool, Domain::Bool))),
        _ => unreachable!(),
    }
}
fn slice(domain: &Domain) -> Pattern {
    let alternatives = || {
        Pattern::Or(vec![
            Pattern::Exact(Value::Bool(false)),
            Pattern::Exact(Value::Bool(true)),
        ])
    };
    match domain {
        Domain::Bool => Pattern::Exact(Value::Bool(true)),
        Domain::Pair(_, b) if matches!(**b, Domain::Bool) => Pattern::Pair(
            Box::new(alternatives()),
            Box::new(Pattern::Exact(Value::Bool(false))),
        ),
        Domain::Pair(_, _) => Pattern::Pair(
            Box::new(alternatives()),
            Box::new(Pattern::Pair(
                Box::new(Pattern::Exact(Value::Unit)),
                Box::new(Pattern::Exact(Value::Bool(false))),
            )),
        ),
        Domain::Sum(p) => Pattern::Sum(0, Box::new(slice(p))),
        _ => unreachable!(),
    }
}

/// The nested-if twin uses boolean field predicates. A variant tag is inspected
/// by one conditional let; it does not reuse direct match's arm table.
fn condition(pattern: &Pattern, place: &str) -> String {
    condition_at(pattern, place, 0)
}
fn condition_at(pattern: &Pattern, place: &str, depth: usize) -> String {
    match pattern {
        Pattern::Wild => "true".into(),
        Pattern::Exact(Value::Bool(v)) => format!("{place} == {v}"),
        Pattern::Exact(Value::Unit) => "true".into(),
        Pattern::Pair(a, b) => {
            let left = if depth == 0 {
                "left".into()
            } else {
                format!("left{depth}")
            };
            let right = if depth == 0 {
                "right".into()
            } else {
                format!("right{depth}")
            };
            format!(
                "({{ let ({left},{right}) = {place}\n({}) && ({}) }})",
                condition_at(a, &left, depth + 1),
                condition_at(b, &right, depth + 1)
            )
        }
        Pattern::Sum(tag, p) => format!(
            "(if let {}(payload) = {place} {{ {} }} else {{ false }})",
            if *tag == 0 { "A" } else { "B" },
            condition_at(p, "payload", depth)
        ),
        Pattern::Or(ps) => ps
            .iter()
            .map(|p| format!("({})", condition_at(p, place, depth)))
            .collect::<Vec<_>>()
            .join(" || "),
        _ => unreachable!(),
    }
}

#[derive(Clone, Copy, Debug)]
enum Form {
    Match,
    If,
    IfLet,
    WhileLet,
}

fn selection(index: usize, form: Form, failing: bool) -> (String, Vec<(String, u32)>) {
    let domain = domain(index);
    let pattern = slice(&domain);
    let arms = vec![
        Arm {
            pattern: pattern.clone(),
            guard: Some(false),
        },
        Arm {
            pattern,
            guard: Some(true),
        },
        Arm {
            pattern: Pattern::Wild,
            guard: None,
        },
    ];
    let mut source = domain.declaration();
    writeln!(source, "fn pick(x:{})->u32 {{", domain.source()).unwrap();
    let body = |marker| {
        if failing && marker == 22 {
            "{ let _ = 7u32 / 0u32\n return 22u32 }".to_string()
        } else {
            format!("return {marker}u32")
        }
    };
    match form {
        Form::Match => {
            source.push_str("match x {\n");
            for (i, arm) in arms.iter().enumerate() {
                writeln!(
                    source,
                    "{}{} => {},",
                    arm.pattern.source(),
                    arm.guard.map(|b| format!(" if {b}")).unwrap_or_default(),
                    if i == 1 {
                        body(22)
                    } else {
                        format!("{}u32", 11 * (i + 1))
                    }
                )
                .unwrap();
            }
            source.push_str("}\n");
        }
        _ => {
            for (i, arm) in arms[..2].iter().enumerate() {
                let action = body(11 * (i + 1));
                match form {
                    Form::If => writeln!(
                        source,
                        "if ({}) && {} {{ {action} }}",
                        condition(&arm.pattern, "x"),
                        arm.guard.unwrap()
                    )
                    .unwrap(),
                    Form::IfLet => writeln!(
                        source,
                        "if let {} = x {{ if {} {{ {action} }} }}",
                        arm.pattern.source(),
                        arm.guard.unwrap()
                    )
                    .unwrap(),
                    Form::WhileLet => writeln!(
                        source,
                        "while let {} = x {{ if {} {{ {action} }}\n break }}",
                        arm.pattern.source(),
                        arm.guard.unwrap()
                    )
                    .unwrap(),
                    _ => unreachable!(),
                }
            }
            source.push_str("33u32\n");
        }
    }
    source.push_str("}\npub fn main()->Result[(),u8] {\n");
    let mut expected = vec![];
    // Run unmatched values before matched values, including in failure cases.
    let mut values = domain.values();
    values.sort_by_key(|v| std::cmp::Reverse(selected(&arms, v)));
    let mut failed = false;
    for value in values {
        if failed {
            break;
        }
        let marker = selected(&arms, &value);
        writeln!(source, "expect(pick({}), {marker}u32)", value.source()).unwrap();
        expected.push((value.source(), marker));
        failed = failing && marker == 22;
    }
    source.push_str("Ok(())\n}\n");
    (source, expected)
}

/// Three iterations at most. The inner match continues/breaks the outer loop;
/// the condition's computed pair must be evaluated afresh on every iteration.
fn loop_source(limit: u32, desugared: bool) -> String {
    let head = if desugared {
        "'outer: while true {\nmatch (i < limit, i == 1u32) {\n(true, skip) => {"
    } else {
        "'outer: while let (true, skip) = (i < limit, i == 1u32) {"
    };
    let tail = if desugared {
        "}, _ => break\n}\n}"
    } else {
        "}"
    };
    format!(
        "fn count(limit:u32)->u32 {{\nlet mut i=0u32\nlet mut score=0u32\n{head}\ni=i+1u32\nmatch skip {{\ntrue => continue 'outer,\nfalse => {{ score=score+11u32\nif i == 3u32 {{ break 'outer }} }}\n}}\n{tail}\nscore\n}}\npub fn main()->Result[(),u8] {{\nexpect(count({limit}u32), {}u32)\nOk(())\n}}\n",
        (0..limit).filter(|i| *i != 1).count() * 11
    )
}

fn complete_source(source: &str) -> String {
    format!("fn expect(actual:u32,expected:u32) requires actual == expected {{}}\n{source}")
}

fn execute_case(name: &str, source: &str, expected: serde_json::Value, failing: bool) {
    let source = complete_source(source);
    let source = source.as_str();
    let directory = tempfile::tempdir().unwrap();
    persist(
        &directory.path().join("originals"),
        name,
        0,
        source,
        &format!("expected_markers={expected}\nfailing={failing}"),
    );
    let matrix = [
        Engine::Heap { campaign: true },
        Engine::Native {
            opt: OptLevel::None,
            partition: Partition::Whole,
            runtime: Runtime::Diagnostic,
        },
        Engine::Native {
            opt: OptLevel::Default,
            partition: Partition::Whole,
            runtime: Runtime::Diagnostic,
        },
        Engine::Cranelift {
            partition: Partition::Whole,
            runtime: Runtime::Diagnostic,
        },
    ]
    .into_iter()
    .map(|engine| Configuration {
        options: Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
        engine,
    })
    .collect();
    // Setup failure remains a setup failure, never skipped native agreement.
    let runner = Runner::with_matrix(directory.path(), matrix)
        .unwrap_or_else(|e| panic!("engine setup failure {name}: {e:?}"));
    let report = runner.run(name, source);
    let failure_site = failing.then(|| {
        let prefix = &source[..source.find("7u32 / 0u32").unwrap()];
        (
            prefix.bytes().filter(|b| *b == b'\n').count() as u32 + 1,
            prefix.rsplit('\n').next().unwrap().len() as u32 + 1,
        )
    });
    let valid = report.validated
        && report.cells.len() == 5
        && report.agrees()
        && report.cells.iter().all(|cell| {
            cell.category == Category::Observation
                && cell.observation.as_ref().is_some_and(|o| {
                    o.output.is_empty()
                        && !o.blocked
                        && o.budget.is_none()
                        && o.signaled.is_none()
                        && o.roots.is_empty()
                        && o.exit_code == if failing { None } else { Some(0) }
                        && o.failures.len() == usize::from(failing)
                        && o.failures.iter().all(|f| {
                            f.rule == "SEM-1"
                                && f.definition == crate::rc_support::path("pick")
                                && f.file == "main.hir"
                                && Some((f.line, f.column)) == failure_site
                                && f.operation.is_none()
                        })
                })
        });
    // Preserve every original, not merely a reduced or passing replacement.
    let receipt = serde_json::json!({"name":name,"source":source,"expected_markers":expected,"failure_site":failure_site,"validated":report.validated,"accepted":valid,"cells":report.cells.iter().map(|c|serde_json::json!({"engine":c.engine,"category":format!("{:?}",c.category),"observation":c.observation.as_ref().map(|o|o.render()),"detail":c.detail})).collect::<Vec<_>>()});
    persist(
        &directory.path().join("originals"),
        name,
        0,
        source,
        &receipt.to_string(),
    );
    eprintln!("MATCH_LOWERING_22_RECEIPT {receipt}");
    assert!(valid, "{name}\n{source}\n{report:#?}");
}
fn run_selection(index: usize, form: Form, failing: bool) {
    let (source, markers) = selection(index, form, failing);
    execute_case(
        &format!("domain-{index}-{form:?}-failure-{failing}"),
        &source,
        serde_json::json!(markers),
        failing,
    );
}
macro_rules! selections {
    ($($name:ident: $index:literal),* $(,)?) => {$(mod $name {
        #[test] fn direct_match() { super::run_selection($index, super::Form::Match, false); }
        #[test] fn nested_if() { super::run_selection($index, super::Form::If, false); }
        #[test] fn if_let() { super::run_selection($index, super::Form::IfLet, false); }
        #[test] fn while_let() { super::run_selection($index, super::Form::WhileLet, false); }
    })*};
}
selections! { boolean: 0, product: 1, nested_product: 2, variant_product: 3 }
macro_rules! loops {
    ($($name:ident: $limit:literal),* $(,)?) => {$(mod $name {
        #[test] fn conditional() { super::execute_case(stringify!($name), &super::loop_source($limit, false), serde_json::json!({"iterations":$limit}), false); }
        #[test] fn desugared() { super::execute_case(concat!(stringify!($name), "-desugared"), &super::loop_source($limit, true), serde_json::json!({"iterations":$limit}), false); }
    })*};
}
loops! { zero: 0, one: 1, three: 3 }
#[test]
fn failure_direct_match() {
    run_selection(1, Form::Match, true);
}
#[test]
fn failure_nested_if() {
    run_selection(1, Form::If, true);
}
#[test]
fn failure_if_let() {
    run_selection(1, Form::IfLet, true);
}
#[test]
fn failure_while_let() {
    run_selection(1, Form::WhileLet, true);
}

fn bound_source(form: Form) -> (String, Vec<(String, u32)>) {
    let domain = domain(3);
    // In A bind the second field; in B bind the first. Opposite values expose
    // accidentally reusing another alternative's projected binder in a guard.
    let body = match form {
        Form::Match => {
            "match x { A((true,b)) | B((b,true)) if b => 11u32, A((true,b)) | B((b,true)) => return 22u32, _ => 33u32 }"
        }
        Form::If => {
            "if let A((a,b)) = x { if a { if b { return 11u32 }\nreturn 22u32 } } else { if let B((a,b)) = x { if b { if a { return 11u32 }\nreturn 22u32 } } }\n33u32"
        }
        Form::IfLet => {
            "if let A((true,b)) | B((b,true)) = x { if b { return 11u32 }\nreturn 22u32 }\n33u32"
        }
        Form::WhileLet => {
            "while let A((true,b)) | B((b,true)) = x { if b { return 11u32 }\nreturn 22u32 }\n33u32"
        }
    };
    let mut source = format!(
        "{}fn pick(x:Choice)->u32 {{ {body} }}\npub fn main()->Result[(),u8] {{\n",
        domain.declaration()
    );
    let mut markers = vec![];
    for value in domain.values() {
        let Value::Sum(tag, pair) = &value else {
            unreachable!()
        };
        let Value::Pair(a, b) = pair.as_ref() else {
            unreachable!()
        };
        let (Value::Bool(a), Value::Bool(b)) = (a.as_ref(), b.as_ref()) else {
            unreachable!()
        };
        let (matched, guard) = if *tag == 0 { (*a, *b) } else { (*b, *a) };
        let marker = if !matched {
            33
        } else if guard {
            11
        } else {
            22
        };
        writeln!(source, "expect(pick({}), {marker}u32)", value.source()).unwrap();
        markers.push((value.source(), marker));
    }
    source.push_str("Ok(())\n}\n");
    (source, markers)
}
fn run_bound(form: Form) {
    let (source, markers) = bound_source(form);
    execute_case(
        &format!("bound-{form:?}"),
        &source,
        serde_json::json!(markers),
        false,
    );
}
#[test]
fn bound_direct_match() {
    run_bound(Form::Match);
}
#[test]
fn bound_nested_if() {
    run_bound(Form::If);
}
#[test]
fn bound_if_let() {
    run_bound(Form::IfLet);
}
#[test]
fn bound_while_let() {
    run_bound(Form::WhileLet);
}

#[test]
fn source_batch() {
    use hirsel_engine::{
        QueryEngine, check_progress, default_context, default_environment, with_compiler_stack,
    };
    let mut cases = vec![];
    let forms = [Form::Match, Form::If, Form::IfLet, Form::WhileLet];
    for index in 0..4 {
        for form in forms {
            let (source, markers) = selection(index, form, false);
            cases.push((
                format!("domain-{index}-{form:?}-failure-false"),
                source,
                serde_json::json!(markers),
            ));
        }
    }
    for (name, limit) in [("zero", 0), ("one", 1), ("three", 3)] {
        for desugared in [false, true] {
            cases.push((
                format!("{name}{}", if desugared { "-desugared" } else { "" }),
                loop_source(limit, desugared),
                serde_json::json!({"iterations":limit}),
            ));
        }
    }
    for form in forms {
        let (source, markers) = selection(1, form, true);
        cases.push((
            format!("domain-1-{form:?}-failure-true"),
            source,
            serde_json::json!(markers),
        ));
        let (source, markers) = bound_source(form);
        cases.push((
            format!("bound-{form:?}"),
            source,
            serde_json::json!(markers),
        ));
    }
    assert_eq!(cases.len(), 30);
    with_compiler_stack(move || {
        let directory = tempfile::tempdir().unwrap();
        for (name, source, markers) in cases {
            let source = complete_source(&source);
            persist(
                directory.path(),
                &name,
                0,
                &source,
                &format!("source preflight; expected_markers={markers}"),
            );
            let mut engine =
                QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
            let file = engine.sources.add("main.hir", source.clone()).unwrap();
            let checked = check_progress(&mut engine, file, &default_context())
                .unwrap_or_else(|e| panic!("source setup failure {name}: {e:?}\n{source}"));
            let errors: Vec<_> = checked
                .diagnostics
                .iter()
                .filter(|d| d.severity == hirsel_diag::Severity::Error)
                .collect();
            eprintln!(
                "MATCH_LOWERING_22_SOURCE {}",
                serde_json::json!({"name":name,"accepted":errors.is_empty(),"source_hash":format!("{:?}",hirsel_syntax::ContentHash::of_bytes(source.as_bytes())),"diagnostics":format!("{errors:?}")})
            );
            assert!(
                errors.is_empty(),
                "source setup failure {name}: {errors:?}\n{source}"
            );
        }
    });
}
