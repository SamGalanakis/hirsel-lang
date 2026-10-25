//! A List lent `mut` to `list.pop`, which allocates nothing and so cannot
//! detach, is made writable by the caller: an arena List is never written in
//! place, even through a `unique` parameter.
use crate::rc_support as support;
use hirsel_lir::ir::{Inst, Terminator};
use hirsel_rc::{insert::Proofs, placement::Options};

const SOURCE: &str = "fn pop(mut xs: unique List[String]) -> Option[String] uses {} {
  list.pop(mut xs)
}

fn length(text: Option[String]) -> u64 uses {} {
  match text {
    Some(t) => string.byte_len(t),
    None => 0,
  }
}

fn inside(a: Arena) -> u64 uses {} {
  let mut xs: unique List[String] in a = [\"arena\", \"text\"]
  length(list.pop(mut xs)) + length(pop(mut xs))
}

pub fn main() -> Result[(), u64] uses {} {
  Error(arena.with(fn(a: Arena) -> u64 { inside(a) }))
}
";

/// Each call to a `mut` callee in `name` follows a writability test.
fn tests_before_calls(lowered: &hirsel_rc::import::Lowered, name: &str) {
    let ids: Vec<_> = support::instances_of(lowered, name)
        .into_iter()
        .filter(|&id| {
            !lowered
                .instances
                .view(id)
                .key
                .definition()
                .is_core("list.pop")
        })
        .collect();
    assert_eq!(ids.len(), 1);
    let id = ids[0];
    let placed = hirsel_rc::placement::place(
        lowered.units[id.index()].clone(),
        &lowered.table,
        &Options::for_instance(true, &lowered.instances, id),
    )
    .unwrap();
    let rc = hirsel_rc::insert::insert(
        placed,
        &lowered.table,
        &Proofs::for_instance(&lowered.instances, id),
    )
    .unwrap();
    let mut calls = 0;
    let mut tested = 0;
    for (b, term) in rc.terminators.iter().enumerate() {
        let Terminator::Call { call, .. } = *term else {
            continue;
        };
        let site = &rc.tables.calls[call.index()];
        let arguments = site.arguments.get(&rc.pools.call_arguments).unwrap();
        if !arguments
            .iter()
            .any(|a| a.passing == hirsel_core_contract::Passing::Mut)
        {
            continue;
        }
        calls += 1;
        // The test's join block, or this block itself, starts after it.
        let preceded = (0..rc.blocks.len()).any(|p| {
            rc.blocks[p]
                .instructions
                .get(&rc.instructions)
                .unwrap()
                .iter()
                .any(|i| matches!(i, Inst::IsWritable { .. }))
                && rc.terminators[p]
                    .normal_edges(&rc.pools)
                    .iter()
                    .any(|e| rc.tables.edges[e.index()].target.index() == b)
        });
        tested += usize::from(preceded);
    }
    assert!(
        calls > 0 && tested == calls,
        "{name}: {tested} of {calls}\n{}",
        support::dump(&rc)
    );
}

#[test]
fn callers_detach_lists_the_storage_primitives_write() {
    let (program, lowered) = support::lower_valid(SOURCE);
    tests_before_calls(&lowered, "inside");
    tests_before_calls(&lowered, "pop");
    let _ = program;
}
