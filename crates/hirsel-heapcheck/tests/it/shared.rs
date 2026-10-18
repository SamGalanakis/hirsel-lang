//! `Shared[T]` reads (MEM-11): a copy read out of a frozen value is an
//! uncredited holder of a frozen block, given back by its release, and no such
//! copy may outlive the last root, whose thaw would hand the block back unique.
use crate::fixtures;

use fixtures::*;
use hirsel_heapcheck::build::{Layouts, path};
use hirsel_heapcheck::{Config, Program, ViolationKind};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::TargetSpec;
use hirsel_lir::stage::Rc;

/// Freeze a box holding 1, copy the box out of the frozen value through the
/// root's slot, print through the copy, then release the copy and the root; with
/// `outlived`, the root first.
fn copy_out(outlived: bool) -> Program<Rc> {
    let mut l = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let c = common(&mut l);
    let shared = l.shared(c.boxed);
    let (target, table) = l.finish();
    let mut f = F::<Rc>::new("main", c);
    let one = f.int(1);
    let x = f.boxed(one);
    let root = f.b.owned(BLOCK, shared);
    f.inst(Inst::Freeze {
        result: root,
        value: x,
    });
    // `shared.with` reads through the root's slot, as its library body does.
    let slot = f.b.slot(BLOCK, shared, SlotKind::Local);
    f.inst(Inst::StorageLive { slot });
    let whole = f.b.address(AddressBase::Slot(slot), &[], BLOCK, shared);
    let failure = f.propagates();
    f.inst(Inst::Address {
        address: whole,
        failure,
    });
    let stored = f.b.plain(BLOCK, shared);
    f.inst(Inst::Store {
        address: whole,
        value: root,
        version: stored,
    });
    let at = f.b.address(
        AddressBase::Slot(slot),
        &[ProjectionStep::Field(0)],
        BLOCK,
        c.boxed,
    );
    let failure = f.propagates();
    f.inst(Inst::Address {
        address: at,
        failure,
    });
    let copied = f.b.owned(BLOCK, c.boxed);
    let failure = f.propagates();
    f.inst(Inst::Copy {
        result: copied,
        address: at,
        reaching: stored,
        failure,
    });
    let v = f.unbox(copied);
    f.print(v);
    let root = f.b.owned(BLOCK, shared);
    f.inst(Inst::Read {
        result: root,
        address: whole,
        reaching: stored,
        mode: ReadMode::Take,
    });
    f.inst(Inst::StorageDead {
        slot,
        if_live: false,
    });
    if outlived {
        f.release(root, shared);
        f.release(copied, c.boxed);
    } else {
        f.release(copied, c.boxed);
        f.release(root, shared);
    }
    f.ret(None);
    let main = f.finish(&table);
    program(target, table, vec![main])
}

#[test]
fn a_copy_read_out_of_a_frozen_value_never_outlives_the_last_root() {
    let run = |p: &Program<Rc>| run(p, &path("main"), &[], &mut Printer, &Config::default());
    let fine = run(&copy_out(false));
    let audit = fine.result.as_ref().unwrap_or_else(|v| {
        panic!(
            "expected a clean audit: {v} at step {} trace {:?}",
            fine.steps, fine.trace
        )
    });
    assert_eq!(audit.observation.output, b"1\n");
    let outlived = run(&copy_out(true));
    assert_eq!(outlived.violation(), Some(ViolationKind::WriteShared));
    assert!(format!("{:?}", outlived.result).contains("outlive its last root"));
}
