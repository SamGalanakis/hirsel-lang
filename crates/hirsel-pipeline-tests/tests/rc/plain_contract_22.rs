//! FACT-1/2/3 plain-value execution order and optimization invariance.
//! Vectors adapted from Rust core/src/macros/mod.rs:15-69 and
//! coretests/tests/macros.rs:28-41 at f6c1107f (MIT OR Apache-2.0):
//! unconditional equality/inequality assertions survive release optimization.
//! Hirsel alone supplies clause placement and failure rules/sites; no panic
//! text, debug_assert elision, resource effects or Rust drop rules are ported.
use crate::rc_support::history::{Category, Runner, persist};
use hirsel_driver::native::*;

#[derive(Clone, Copy, Debug)]
enum Predicate {
    Equal(u32, u32),
    Unequal(u32, u32),
    Divide(u32, u32),
}

impl Predicate {
    fn evaluate(self) -> Result<bool, &'static str> {
        match self {
            Self::Equal(a, b) => Ok(a == b),
            Self::Unequal(a, b) => Ok(a != b),
            Self::Divide(a, b) => a.checked_div(b).map(|n| n == 1).ok_or("SEM-1"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Phase {
    Requires,
    Ensures,
    EarlyEnsures,
    Where,
}

#[derive(Clone, Copy, Debug)]
struct Case {
    name: &'static str,
    phase: Phase,
    predicate: Predicate,
    body_fails: bool,
    continuation_fails: bool,
}

/// First failure in the source sequence, independent of compiler check nodes.
/// A predicate's evaluation failure wins over the clause's false-result rule.
fn model(case: Case) -> Option<(&'static str, &'static str)> {
    let predicate = || match case.predicate.evaluate() {
        Err(rule) => Some((rule, "predicate")),
        Ok(false) => Some((
            if matches!(case.phase, Phase::Where) {
                "FACT-2"
            } else {
                "FACT-1"
            },
            "clause",
        )),
        Ok(true) => None,
    };
    let body = case.body_fails.then_some(("SEM-1", "body"));
    let first = match case.phase {
        Phase::Requires => predicate().or(body),
        Phase::Ensures | Phase::EarlyEnsures => body.or_else(predicate),
        Phase::Where => predicate(),
    };
    first.or(case.continuation_fails.then_some(("SEM-1", "continuation")))
}

const CASES: [Case; 10] = [
    Case {
        name: "requires_true_body",
        phase: Phase::Requires,
        predicate: Predicate::Equal(1, 1),
        body_fails: true,
        continuation_fails: false,
    },
    Case {
        name: "requires_false_before_body",
        phase: Phase::Requires,
        predicate: Predicate::Equal(1, 2),
        body_fails: true,
        continuation_fails: false,
    },
    Case {
        name: "requires_predicate_failure",
        phase: Phase::Requires,
        predicate: Predicate::Divide(1, 0),
        body_fails: true,
        continuation_fails: false,
    },
    Case {
        name: "ensures_true",
        phase: Phase::Ensures,
        predicate: Predicate::Unequal(1, 2),
        body_fails: false,
        continuation_fails: false,
    },
    Case {
        name: "ensures_false_before_caller",
        phase: Phase::Ensures,
        predicate: Predicate::Unequal(1, 1),
        body_fails: false,
        continuation_fails: true,
    },
    Case {
        name: "ensures_early_false",
        phase: Phase::EarlyEnsures,
        predicate: Predicate::Equal(1, 2),
        body_fails: false,
        continuation_fails: true,
    },
    Case {
        name: "ensures_predicate_failure",
        phase: Phase::Ensures,
        predicate: Predicate::Divide(1, 0),
        body_fails: false,
        continuation_fails: true,
    },
    Case {
        name: "where_record_true",
        phase: Phase::Where,
        predicate: Predicate::Equal(1, 1),
        body_fails: false,
        continuation_fails: false,
    },
    Case {
        name: "where_record_false",
        phase: Phase::Where,
        predicate: Predicate::Unequal(1, 1),
        body_fails: false,
        continuation_fails: true,
    },
    Case {
        name: "where_predicate_failure",
        phase: Phase::Where,
        predicate: Predicate::Divide(1, 0),
        body_fails: false,
        continuation_fails: true,
    },
];

/// Each row has a helper with nonconstant formal operands and a literal twin.
/// Plain records exercise clause parameter/result transport without ownership.
fn source(case: Case, helper: bool) -> String {
    let (a, b, op) = match case.predicate {
        Predicate::Equal(a, b) => (a, b, "=="),
        Predicate::Unequal(a, b) => (a, b, "!="),
        Predicate::Divide(a, b) => (a, b, "/"),
    };
    let divide = matches!(case.predicate, Predicate::Divide(..));
    let expression = |left: &str, right: &str| {
        let expr = format!("{left} {op} {right}");
        if divide {
            format!("{expr} == 1u32")
        } else {
            expr
        }
    };
    let literals = expression(&format!("{a}u32"), &format!("{b}u32"));
    let mut text = "type Pair { left: u32, right: u32 }\n".to_string();
    if helper {
        text.push_str(&format!(
            "fn predicate(a:u32,b:u32)->bool {{\n  {}\n}}\n",
            expression("a", "b")
        ));
    }
    let operands = match case.phase {
        Phase::Requires => ("p.left", "p.right"),
        Phase::Ensures | Phase::EarlyEnsures => ("result.left", "p.right"),
        Phase::Where => ("v.left", "v.right"),
    };
    let predicate = if helper {
        format!("predicate({},{})", operands.0, operands.1)
    } else {
        literals
    };
    match case.phase {
        Phase::Where => {
            text.push_str(&format!("distinct type Valid(v:Pair) where {predicate}\n"));
            text.push_str("pub fn main()->Result[(),u8] {\n");
            text.push_str(&format!(
                "  let _ = Valid(Pair(left:{a}u32,right:{b}u32))\n"
            ));
        }
        phase => {
            let clause = if matches!(phase, Phase::Requires) {
                "requires"
            } else {
                "ensures"
            };
            text.push_str(&format!(
                "fn probe(p:Pair)->Pair\n  {clause} {predicate}\n{{\n"
            ));
            if case.body_fails {
                text.push_str("  let _ = 7u32 / 0u32\n");
            }
            if matches!(phase, Phase::EarlyEnsures) {
                text.push_str("  if p.left == 1u32 { return p }\n");
            }
            text.push_str("  p\n}\npub fn main()->Result[(),u8] {\n");
            text.push_str(&format!(
                "  let value = probe(Pair(left:{a}u32,right:{b}u32))\n"
            ));
            // Make returned plain record values observable, too.
            text.push_str(&format!(
                "  if value.left != {a}u32 {{ return Error(2u8) }}\n"
            ));
        }
    }
    if case.continuation_fails {
        text.push_str("  let _ = 9u32 / 0u32\n");
    }
    text.push_str("  Ok(())\n}\n");
    text
}

fn site(source: &str, needle: &str, skip: usize) -> (u32, u32) {
    let start = source.find(needle).unwrap() + skip;
    let prefix = &source[..start];
    (
        prefix.bytes().filter(|b| *b == b'\n').count() as u32 + 1,
        prefix.rsplit('\n').next().unwrap().len() as u32 + 1,
    )
}

fn run(index: usize, helper: bool) {
    let case = CASES[index];
    let source = source(case, helper);
    let expected = model(case);
    let expected_site = expected.map(|(_, stage)| match stage {
        "predicate" if helper => ("predicate", site(&source, "a / b", 0)),
        "predicate" => {
            let needle = if matches!(case.phase, Phase::Where) {
                "where "
            } else if matches!(case.phase, Phase::Requires) {
                "requires "
            } else {
                "ensures "
            };
            (
                if matches!(case.phase, Phase::Where) {
                    // fact::fact_path materializes the where expression as
                    // its own callable, so evaluation failures name that body.
                    "Valid$fact$0"
                } else {
                    "probe"
                },
                site(&source, needle, needle.len()),
            )
        }
        "clause" if matches!(case.phase, Phase::Where) => ("main", site(&source, "Valid(Pair", 0)),
        "clause" => {
            let needle = if matches!(case.phase, Phase::Requires) {
                "requires "
            } else {
                "ensures "
            };
            ("probe", site(&source, needle, needle.len()))
        }
        "body" => ("probe", site(&source, "7u32 / 0u32", 0)),
        "continuation" => ("main", site(&source, "9u32 / 0u32", 0)),
        _ => unreachable!(),
    });
    let directory = tempfile::tempdir().unwrap();
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
    let runner = Runner::with_matrix(directory.path(), matrix).unwrap();
    let name = format!(
        "{}-{}",
        case.name,
        if helper { "helper" } else { "constant" }
    );
    let report = runner.run(&name, &source);
    // Report::disagreements intentionally models SEM-1 only. FACT uses this
    // lane's model; unavailable tools/source/V rejection never count as a pass.
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
                        && o.exit_code == if expected.is_some() { None } else { Some(0) }
                        && o.failures.len() == usize::from(expected.is_some())
                        && o.failures.iter().all(|f| {
                            let (rule, _) = expected.unwrap();
                            let (definition, (line, column)) = expected_site.unwrap();
                            f.rule == rule
                                && f.definition == crate::rc_support::path(definition)
                                && f.file == "main.hir"
                                && f.line == line
                                && f.column == column
                                && f.operation.is_none()
                        })
                })
        });
    let receipt = serde_json::json!({"name":name,"source":source,"expected":expected,"site":expected_site,"accepted":valid,
        "cells":report.cells.iter().map(|c|serde_json::json!({"engine":c.engine,"category":format!("{:?}",c.category),"observation":c.observation.as_ref().map(|o|o.render()),"detail":c.detail})).collect::<Vec<_>>()});
    eprintln!("PLAIN_CONTRACT_22_RECEIPT {receipt}");
    if !valid {
        persist(
            &std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("plain-contract-22"),
            &name,
            0,
            &source,
            &format!("{receipt}\n{report:#?}"),
        );
    }
    assert!(
        valid,
        "{name}: {expected:?} at {expected_site:?}\n{source}\n{report:#?}"
    );
}

macro_rules! cases {
    ($($name:ident: $index:literal),* $(,)?) => {$(mod $name {
        #[test] fn helper() { super::run($index, true); }
        #[test] fn constant() { super::run($index, false); }
    })*};
}
cases! {
    requires_true_body: 0,
    requires_false_before_body: 1,
    requires_predicate_failure: 2,
    ensures_true: 3,
    ensures_false_before_caller: 4,
    ensures_early_false: 5,
    ensures_predicate_failure: 6,
    where_record_true: 7,
    where_record_false: 8,
    where_predicate_failure: 9,
}
