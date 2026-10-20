//! Root lifetime anchors (S-14, lane R7) over driver-derived programs: content that
//! reaches a `Shared` root keeps its drop at its TIR cleanup anchor through placement,
//! insertion and elision, and the stage validator's RC-ROOT rule refuses the mutations
//! that would end such a holder early: a release at its last use, and a copy cancelled
//! into a move across the thaw that observes the holders.
use crate::rc_support as support;

use hirsel_lir::ir::{AddressBase, BlockKind, Inst, LirUnit, ReadMode};
use hirsel_lir::layout_types::LayoutTable;
use hirsel_lir::stage::Rc;
use hirsel_lir::validate::{Inputs, ViolationKind};
use hirsel_rc::insert::{Editor, Proofs};
use hirsel_rc::instance::{InstanceId, Program};
use hirsel_rc::placement::Options;
use support::*;

/// `s` is unused after its copy, but holds its root until its scope's cleanup: the
/// thaw of the copy finds two holders (the interpreter's `Err`).
const COPY_THEN_THAW: &str = r#"
fn probe() -> bool {
  let built: String = "value"
  let s: Shared[String] = shared.freeze(take built)
  let t = copy s
  match shared.thaw(take t) {
    Ok(text) => true,
    Error(root) => false,
  }
}
"#;

/// The same through a record holding two roots.
const AGGREGATE: &str = r#"
type Pair {
  left: Shared[String],
  right: Shared[String],
}

fn probe() -> bool {
  let a: String = "left"
  let b: String = "right"
  let pair = Pair(left: shared.freeze(take a), right: shared.freeze(take b))
  let Pair(left: l, right: r) = copy pair
  match shared.thaw(take l) {
    Ok(text) => true,
    Error(root) => false,
  }
}
"#;

/// `probe` through placement with regions, insertion and elision.
fn rc_probe(source: &str) -> (Program, LayoutTable, LirUnit<Rc>) {
    let (program, lowered) = lower_valid(source);
    let table = lowered.table.clone();
    let (i, unit) = lowered
        .units
        .iter()
        .enumerate()
        .find(|(_, u)| u.header.definition.0.name == "probe")
        .expect("probe");
    let id = InstanceId(i as u32);
    let placed = hirsel_rc::placement::place(
        unit.clone(),
        &table,
        &Options::for_instance(true, &lowered.instances, id),
    )
    .expect("placement");
    let proofs = Proofs::for_instance(&lowered.instances, id);
    let rc = hirsel_rc::insert::insert(placed, &table, &proofs).expect("insertion");
    let rc = hirsel_rc::elide::elide(rc, &table).expect("elision");
    (program, table, rc)
}

fn check(program: &Program, table: &LayoutTable, unit: &LirUnit<Rc>) -> Vec<ViolationKind> {
    let manifest = manifest(program);
    let target = target();
    let inputs = Inputs {
        memo: Default::default(),
        discharge: None,
        operations: &program.operation_registry(),
        target: &target,
        layouts: table,
        manifest: &manifest,
    };
    hirsel_lir::validate::check(unit, &inputs)
        .into_iter()
        .map(|v| v.kind)
        .collect()
}

/// The copy of `s` in COPY_THEN_THAW: its block and index, and the normal-exit cleanup
/// drop of the slot it reads (block and index).
fn copy_and_anchor(ed: &Editor<Rc>) -> ((usize, usize), (usize, usize)) {
    let copies: Vec<_> = (0..ed.code.len())
        .flat_map(|b| (0..ed.code[b].len()).map(move |i| (b, i)))
        .filter(|&(b, i)| matches!(ed.code[b][i], Inst::Copy { .. }))
        .collect();
    assert_eq!(copies.len(), 1, "one copy of the root");
    let (b, i) = copies[0];
    let Inst::Copy { address, .. } = ed.code[b][i] else {
        unreachable!()
    };
    let AddressBase::Slot(slot) = ed.unit.addresses[address.index()].base else {
        panic!("the copy reads a slot");
    };
    let anchor = (0..ed.code.len())
        .filter(|&c| ed.unit.blocks[c].kind == BlockKind::Cleanup)
        .flat_map(|c| (0..ed.code[c].len()).map(move |j| (c, j)))
        .find(|&(c, j)| match ed.code[c][j] {
            Inst::Drop { address, .. } => {
                ed.unit.addresses[address.index()].base == AddressBase::Slot(slot)
            }
            _ => false,
        })
        .expect("the slot's drop at its cleanup anchor");
    ((b, i), anchor)
}

#[test]
fn a_copied_root_keeps_its_holder_to_its_cleanup_anchor() {
    for source in [COPY_THEN_THAW, AGGREGATE] {
        let (program, table, rc) = rc_probe(source);
        let violations = check(&program, &table, &rc);
        assert!(violations.is_empty(), "{violations:?}\n{}", dump(&rc));
        // The copy stays a Dup: no holder moves across the thaw.
        let ed = Editor::new(rc.clone());
        assert!(
            ed.code
                .iter()
                .flatten()
                .any(|i| matches!(i, Inst::Copy { .. })),
            "the copy was elided into a move\n{}",
            dump(&rc)
        );
    }
    let ed = Editor::new(rc_probe(COPY_THEN_THAW).2);
    copy_and_anchor(&ed);
}

#[test]
fn rc_root_refuses_a_root_released_at_its_last_use() {
    let (program, table, rc) = rc_probe(COPY_THEN_THAW);
    let mut ed = Editor::new(rc);
    let ((b, i), (c, j)) = copy_and_anchor(&ed);
    // Move the slot's drop from its cleanup anchor to its last use, the copy.
    let drop = ed.code[c].remove(j);
    ed.code[b].insert(i + 1, drop);
    let mutated = ed.finish();
    let violations = check(&program, &table, &mutated);
    assert!(
        violations
            .iter()
            .any(|v| matches!(v, ViolationKind::RootOutsideAnchor(_))),
        "{violations:?}\n{}",
        dump(&mutated)
    );
}

#[test]
fn rc_root_refuses_a_copy_cancelled_across_a_thaw() {
    let (program, table, rc) = rc_probe(COPY_THEN_THAW);
    let mut ed = Editor::new(rc);
    let ((b, i), (c, j)) = copy_and_anchor(&ed);
    // Cancel the copy's Dup against the drop at the anchor, past the thaw: the copy
    // becomes a move and the anchor's drop goes.
    let Inst::Copy {
        result,
        address,
        reaching,
        ..
    } = ed.code[b][i]
    else {
        unreachable!()
    };
    ed.code[b][i] = Inst::Read {
        result,
        address,
        reaching,
        mode: ReadMode::Take,
    };
    ed.code[c].remove(j);
    let mutated = ed.finish();
    let violations = check(&program, &table, &mutated);
    assert!(
        violations
            .iter()
            .any(|v| matches!(v, ViolationKind::RootCopyCancelled(_))),
        "{violations:?}\n{}",
        dump(&mutated)
    );
}
