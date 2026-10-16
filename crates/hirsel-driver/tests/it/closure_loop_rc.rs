//! A scoped closure called in a loop does no per-call retain or release
//! (snapshot-elision, bcb67a10): the call borrows it under a read loan that C
//! replays. These controls read the Ready units of the whole native pipeline:
//! the scoped kernel's loops hold no RC instruction, while an escaping
//! closure and a callee replaced during its arguments keep theirs.
use hirsel_driver::compiler_adapters;
use hirsel_engine::{
    QueryEngine, check_progress, default_context, default_environment, with_compiler_stack,
};
use hirsel_lir::ir::{Inst, LirUnit, Terminator};
use hirsel_lir::{Ready, TargetSpec};

/// The Ready units of `source` under the default native options.
fn ready(source: &str) -> Vec<LirUnit<Ready>> {
    let source = source.to_owned();
    with_compiler_stack(move || {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("closure.hir", source).unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let raw = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(raw).unwrap();
        let options = hirsel_driver::native::Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        hirsel_driver::native::build(&validated, &options)
            .unwrap()
            .raw_units()
    })
}

/// The RC instructions in blocks on a normal-edge cycle of any unit.
fn loop_rc(units: &[LirUnit<Ready>]) -> Vec<String> {
    let mut found = Vec::new();
    for u in units {
        let successors: Vec<Vec<usize>> = u
            .terminators
            .iter()
            .map(|t| {
                let edges = match t {
                    Terminator::Jump(e) => vec![*e],
                    Terminator::Branch { yes, no, .. } => vec![*yes, *no],
                    Terminator::Switch {
                        arms, otherwise, ..
                    } => arms
                        .get(&u.pools.switch_arms)
                        .unwrap_or(&[])
                        .iter()
                        .map(|a| a.edge)
                        .chain([*otherwise])
                        .collect(),
                    Terminator::Call { normal, .. } => normal.iter().copied().collect(),
                    _ => Vec::new(),
                };
                edges
                    .into_iter()
                    .map(|e| u.tables.edges[e.index()].target.index())
                    .collect()
            })
            .collect();
        for (b, block) in u.blocks.iter().enumerate() {
            // `b` is on a cycle when it is reachable from its own successors.
            let mut seen = vec![false; u.blocks.len()];
            let mut stack = successors[b].clone();
            let mut cyclic = false;
            while let Some(n) = stack.pop() {
                if n == b {
                    cyclic = true;
                    break;
                }
                if !std::mem::replace(&mut seen[n], true) {
                    stack.extend(&successors[n]);
                }
            }
            if !cyclic {
                continue;
            }
            for inst in block.instructions.get(&u.instructions).unwrap_or(&[]) {
                if matches!(
                    inst,
                    Inst::Dup { .. }
                        | Inst::Copy { .. }
                        | Inst::Release { .. }
                        | Inst::Reset { .. }
                        | Inst::Drop { .. }
                ) {
                    found.push(format!("{} b{b}: {inst:?}", u.header.definition.0.name));
                }
            }
        }
    }
    found
}

const APPLY: &str =
    "fn apply(value: u64, f: fn(u64) -> u64 uses {}) -> u64 uses {f} { f(value) }\n";

/// The measured closure-iter/scoped kernel (bench/native/programs).
#[test]
fn a_scoped_closure_called_through_a_helper_in_a_loop_does_no_rc_work() {
    let source = format!(
        "{APPLY}fn kernel(n: u64) -> u64 {{
  let offset: u64 = 3
  let factor: u64 = 2
  let add = fn(value: u64) -> u64 uses {{}} {{ value + offset }}
  let multiply = fn(value: u64) -> u64 uses {{}} {{ value * factor }}
  let mut total: u64 = 0
  for i in 0u64..n {{
    let value = apply(i, add)
    if value % 2u64 == 0u64 {{ total += apply(value, multiply) }}
  }}
  total
}}
fn main() -> u64 {{ kernel(8u64) }}
"
    );
    assert_eq!(loop_rc(&ready(&source)), Vec::<String>::new());
}

/// A closure owning a counted capture, called directly and through a helper.
#[test]
fn a_scoped_closure_with_a_counted_environment_does_no_rc_work_in_its_loop() {
    let source = format!(
        "{APPLY}fn kernel(n: u64) -> u64 {{
  let offsets: List[u64] = [3u64]
  let add = fn(value: u64) -> u64 uses {{}} {{ value + offsets[0u64] }}
  let mut total: u64 = 0
  for i in 0u64..n {{ total += add(i) + apply(i, add) }}
  total
}}
fn main() -> u64 {{ kernel(8u64) }}
"
    );
    assert_eq!(loop_rc(&ready(&source)), Vec::<String>::new());
}

/// The negative twin: a global closure copied out of the loop on every
/// iteration escapes, so its per-iteration credit stays. (A scoped literal
/// cannot escape at all: CLO-1 rejects the copy.)
#[test]
fn an_escaping_closure_keeps_its_loop_copy() {
    let source = format!(
        "{APPLY}fn make(base: u64) -> fn(u64) -> u64 uses {{}} {{
  let offsets: List[u64] = [base]
  fn(value: u64) -> u64 uses {{}} {{ value + offsets[0u64] }}
}}
fn kernel(n: u64) -> List[fn(u64) -> u64 uses {{}}] {{
  let add = make(3u64)
  let mut kept: List[fn(u64) -> u64 uses {{}}] = []
  let mut total: u64 = 0
  for i in 0u64..n {{
    total += apply(i, add)
    list.push(mut kept, copy add)
  }}
  kept
}}
fn main() -> u64 {{ list.len(kernel(8u64)) }}
"
    );
    let rc = loop_rc(&ready(&source));
    assert!(
        rc.iter().any(|i| i.contains("Copy") || i.contains("Dup")),
        "{rc:#?}"
    );
}

/// The negative twin: a callee replaced while its arguments are evaluated
/// keeps the owned snapshot of the closure it selected.
#[test]
fn a_callee_replaced_during_its_arguments_keeps_its_snapshot() {
    let source = "fn make(base: u64) -> fn(u64) -> u64 uses {} {
  let payload: List[u64] = [base]
  fn(x: u64) -> u64 uses {} { payload[0u64] + x }
}
fn kernel(n: u64) -> u64 {
  let mut f = make(3u64)
  let mut total: u64 = 0
  for i in 0u64..n { total += f({ f = make(i); i }) }
  total
}
fn main() -> u64 { kernel(4u64) }
";
    let rc = loop_rc(&ready(source));
    assert!(
        rc.iter().any(|i| i.contains("Copy") || i.contains("Dup")),
        "{rc:#?}"
    );
}
