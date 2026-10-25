//! Bounded snapshots, not Rust borrow/drop proofs. Test shapes adapted from
//! Rust's vec.rs:257-292 (empty/equal/short/long copies) and rustc_mir_dataflow
//! initialized.rs:255-303 / framework/tests.rs:1-68 (before/after call, distinct
//! normal and failure edges, disconnected branch). Existing nested-unique
//! rejection vectors remain in unique_copy_nested_20; no pointer inequality or
//! IntoIter semantics are imported. No upstream implementation is copied.
use crate::rc_support::history::*;

#[derive(Clone, Copy, Debug)]
enum Shape {
    List,
    Tuple,
    Record,
    Variant,
}

#[derive(Clone, Debug)]
enum Op {
    Construct(Vec<Vec<u64>>),
    Copy,
    Take,
    Overwrite(Vec<u64>),
    Branch(bool, Vec<u64>, Vec<u64>),
    Call(Vec<u64>, bool),
    ObserveSaved,
    ObserveWork,
}

// Immutable value snapshots: no refcounts, ownership proofs or compiler state.
#[derive(Clone, Debug, Default)]
struct Model {
    owner: Option<Vec<Vec<u64>>>,
    saved: Option<Vec<Vec<u64>>>,
    work: Option<Vec<Vec<u64>>>,
    output: String,
    failed: bool,
}

impl Model {
    fn step(&self, op: &Op) -> Option<Self> {
        // Statements following a failing call still have to be well typed.
        let mut next = self.clone();
        let emit = !self.failed;
        match op {
            Op::Construct(rows) if self.owner.is_none() && self.work.is_none() => {
                next.owner = Some(rows.clone())
            }
            Op::Copy if self.saved.is_none() => next.saved = Some(self.owner.as_ref()?.clone()),
            Op::Take if self.work.is_none() => {
                next.work = Some(self.owner.as_ref()?.clone());
                next.owner = None;
            }
            Op::Overwrite(row) | Op::Call(row, _) => {
                let rows = next.work.as_mut()?;
                *rows.first_mut()? = row.clone();
                if let Op::Call(_, fail) = op {
                    next.failed |= *fail;
                }
            }
            Op::Branch(flag, yes, no) => {
                *next.work.as_mut()?.first_mut()? = if *flag { yes } else { no }.clone();
            }
            Op::ObserveSaved | Op::ObserveWork => {
                let rows = if matches!(op, Op::ObserveSaved) {
                    self.saved.as_ref()?
                } else {
                    self.work.as_ref()?
                };
                if emit {
                    next.output.push('[');
                    for row in rows {
                        next.output.push('[');
                        for n in row {
                            next.output.push(char::from(b'a' + u8::try_from(*n).ok()?));
                        }
                        next.output.push(']');
                    }
                    next.output.push(']');
                }
            }
            _ => return None,
        }
        Some(next)
    }
}

fn model(ops: &[Op]) -> Option<Model> {
    let mut state = Model::default();
    for op in ops {
        state = state.step(op)?;
    }
    // Two resources are declared in order before the history and dropped on
    // normal and failure exits in reverse order. This is observable on all engines.
    state.output.push_str("21");
    Some(state)
}

fn row(values: &[u64]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(",")
    )
}

const PRELUDE: &str = r#"
type Holder { rows: List[List[u64]] }
resource type Trace { out: Stdout, text: String }
 drop(x) { let _ = out.print(x.out, x.text) }
fn observe(out:Stdout, rows:List[List[u64]]) {
 let _=out.print(out,"[")
 for xs in rows {
  let _=out.print(out,"[")
  for n in xs { let names:List[String]=["a","b","c","d","e","f","g","h","i","j","k","l","m","n","o","p"];let _=out.print(out,names[n]) }
  let _=out.print(out,"]")
 }
 let _=out.print(out,"]")
}
fn change(mut rows:List[List[u64]], take replacement:List[u64], fail:bool) {
 rows[0]=take replacement
 if fail { let zero:u64=0; let _=1u64/zero }
}
pub fn main(rt:Io)->Result[(),GrantError] {
 let stdout=io.stdout(rt)?
 let first=Trace(out:stdout,text:"1")
 let second=Trace(out:stdout,text:"2")
"#;

fn source(shape: Shape, ops: &[Op]) -> String {
    let mut text = PRELUDE.to_owned();
    for op in ops {
        let statement = match op {
            Op::Construct(rows) => {
                let value = format!(
                    "[{}]",
                    rows.iter().map(|r| row(r)).collect::<Vec<_>>().join(",")
                );
                match shape {
                    Shape::List => format!("let owner:List[List[u64]]={value}"),
                    Shape::Tuple => format!("let owner:(List[List[u64]],u64)=({value},0)"),
                    Shape::Record => format!("let owner:Holder=Holder(rows:{value})"),
                    Shape::Variant => format!("let owner:Option[List[List[u64]]]=Some({value})"),
                }
            }
            Op::Copy => match shape {
                Shape::List => "let saved=copy owner".into(),
                Shape::Tuple => "let (saved,saved_tag)=copy owner".into(),
                Shape::Record => "let saved=copy owner.rows".into(),
                Shape::Variant => {
                    "let saved:List[List[u64]]=match copy owner {Some(xs)=>take xs,None=>[]}".into()
                }
            },
            Op::Take => match shape {
                Shape::List => "let mut work=take owner".into(),
                Shape::Tuple => "let (rows,moved_tag)=take owner;let mut work=take rows".into(),
                Shape::Record => "let Holder(rows:rows)=take owner;let mut work=take rows".into(),
                Shape::Variant => {
                    "let mut work:List[List[u64]]=match take owner {Some(xs)=>take xs,None=>[]}"
                        .into()
                }
            },
            Op::Overwrite(xs) => format!("work[0]={}", row(xs)),
            Op::Branch(flag, yes, no) => format!(
                "if {flag} {{work[0]={}}} else {{work[0]={}}}",
                row(yes),
                row(no)
            ),
            Op::Call(xs, fail) => format!("change(mut work,{}, {fail})", row(xs)),
            Op::ObserveSaved => "observe(stdout,saved)".into(),
            Op::ObserveWork => "observe(stdout,work)".into(),
        };
        text.push_str(&statement);
        text.push_str(";\n");
    }
    text.push_str("Ok(())\n}\n");
    text
}

fn history(seed: u64) -> (Shape, Vec<Op>) {
    let shape = [Shape::List, Shape::Tuple, Shape::Record, Shape::Variant][seed as usize % 4];
    let profile = seed / 4;
    let initial = [
        vec![],
        vec![1, 2],
        vec![1, 2, 3],
        vec![1],
        vec![2, 3],
        vec![1, 2],
    ][profile as usize]
        .clone();
    let replacement = [
        vec![3, 4, 5],
        vec![4, 5],
        vec![6],
        vec![3, 4, 5],
        vec![],
        vec![4, 5],
    ][profile as usize]
        .clone();
    let ops = vec![
        Op::Construct(vec![initial, vec![7]]),
        Op::Copy,
        Op::Take,
        Op::ObserveSaved,
        Op::Overwrite(replacement.clone()),
        Op::ObserveWork,
        Op::Branch(profile % 2 == 0, vec![8], vec![9, 10]),
        Op::ObserveWork,
        Op::ObserveSaved,
        Op::Call(replacement, profile == 5),
        Op::ObserveWork,
        Op::ObserveSaved,
    ];
    (shape, ops)
}

#[test]
fn fixed_seeds_all_engines() {
    let artifacts = std::env::var_os("HIRSEL_HISTORY_ARTIFACTS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ownership-history-21")
        });
    let runner = Runner::new(&artifacts.join("runs"))
        .expect("toolchain setup: incomplete, not a language failure");
    let mut counts = std::collections::BTreeMap::new();
    let mut mismatches = Vec::new();
    for seed in 0..24 {
        let (shape, ops) = history(seed);
        assert!(ops.len() <= 12);
        let expected = model(&ops).unwrap();
        let original_source = source(shape, &ops);
        let report = runner.run(&format!("seed-{seed}"), &original_source);
        for cell in &report.cells {
            *counts
                .entry(format!("{}:{:?}", cell.engine, cell.category))
                .or_insert(0usize) += 1;
        }
        let disagreements = report.disagreements(expected.output.as_bytes(), expected.failed);
        eprintln!(
            "seed={seed} shape={shape:?} operations={} failure={} disagreements={disagreements:?}",
            ops.len(),
            expected.failed
        );
        if !disagreements.is_empty() {
            persist(
                &artifacts,
                &format!("seed-{seed}-original"),
                seed,
                &original_source,
                &format!("{ops:?}\n{report:#?}\nexpected={expected:?}"),
            );
            if !report.validated {
                mismatches.push((seed, disagreements));
                continue; // generator validity/setup failure, never an engine disagreement
            }
            for (index, signature) in disagreements.iter().enumerate() {
                let mut reduced_report = None;
                let reduced = minimize(&ops, |candidate| {
                    let Some(expected) = model(candidate) else {
                        return false;
                    };
                    let result =
                        runner.run(&format!("reduce-{seed}-{index}"), &source(shape, candidate));
                    if !result.validated {
                        return false;
                    }
                    let preserves = result
                        .disagreements(expected.output.as_bytes(), expected.failed)
                        .contains(signature);
                    if preserves {
                        reduced_report = Some(result)
                    }
                    preserves
                });
                persist(
                    &artifacts,
                    &format!("seed-{seed}-minimal-{index}"),
                    seed,
                    &source(shape, &reduced),
                    &format!("signature={signature:?}\n{reduced:?}\n{reduced_report:#?}"),
                );
            }
            mismatches.push((seed, disagreements));
        }
    }
    eprintln!(
        "ownership_history_21 coverage: {counts:?}; normal=20 failure=4 shapes=4 seeds=24 max_depth=3 max_operations=12"
    );
    assert!(
        mismatches.is_empty(),
        "{mismatches:?}; replay artifacts: {}",
        artifacts.display()
    );
    assert_eq!(counts.len(), 4);
    assert!(counts.values().all(|&count| count == 24));
}

#[test]
fn reducer_preserves_validity_and_disagreement() {
    let (_, ops) = history(0);
    let reduced = minimize(&ops, |candidate| {
        model(candidate).is_some() && candidate.iter().any(|op| matches!(op, Op::ObserveSaved))
    });
    assert_eq!(reduced.len(), 3); // construct, copy, observe; dangling uses cannot survive
    assert!(model(&reduced).is_some());
    assert_eq!(
        format!("{reduced:?}"),
        format!(
            "{:?}",
            minimize(&ops, |candidate| model(candidate).is_some()
                && candidate.iter().any(|op| matches!(op, Op::ObserveSaved)))
        )
    );
}

// Planning receipts are source/V classifications, not runtime coverage. These
// spellings predate the generator and pin its receiving-contract assumptions.
