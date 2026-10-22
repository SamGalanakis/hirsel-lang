//! The runtime borrow witness follows the selected phi input, not its label.
use crate::fixtures::*;
use hirsel_heapcheck::build::{Layouts, path};
use hirsel_heapcheck::{Config, ViolationKind};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::TargetSpec;
use hirsel_lir::stage::Rc;

fn projected(expired: bool, detached: bool) -> hirsel_heapcheck::Run {
    let mut layouts = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let c = common(&mut layouts);
    let payload = layouts.record(&[c.boxed]);
    let outer_layout = layouts.block(payload);
    let (target, table) = layouts.finish();
    let mut f = F::<Rc>::new("main", c);
    let n = f.int(42);
    let child = f.boxed(n);
    let construct =
        f.b.construct(ConstructorKind::Tuple, outer_layout, &[(0, child)], HEAP);
    let owner = f.b.owned(BLOCK, outer_layout);
    let failure = f.propagates();
    f.inst(Inst::Construct {
        result: owner,
        construct,
        failure,
    });
    let slot = f.b.slot(BLOCK, outer_layout, SlotKind::Local);
    let at =
        f.b.address(AddressBase::Slot(slot), &[], BLOCK, outer_layout);
    f.inst(Inst::StorageLive { slot });
    f.inst(Inst::Store {
        address: at,
        value: owner,
        version: owner,
    });
    let label = f.b.loan(at, false);
    let actual = f.b.loan(at, false);
    for loan in [label, actual] {
        f.inst(Inst::BeginLoan {
            loan,
            address: at,
            mutable: false,
            if_inactive: false,
        });
    }
    let incoming = f.b.value(BLOCK, outer_layout, Holding::Borrowed(actual));
    f.inst(Inst::Read {
        result: incoming,
        address: at,
        reaching: owner,
        mode: ReadMode::Borrow,
    });
    f.inst(Inst::EndLoan {
        loan: label,
        if_active: false,
    });
    let selected = f.b.value(BLOCK, outer_layout, Holding::Borrowed(label));
    let next = f.b.block(BlockKind::Body, &[selected]);
    let edge = f.b.edge(next, &[incoming]);
    f.b.term(f.cur, Terminator::Jump(edge));
    f.cur = next;
    if expired {
        f.inst(Inst::EndLoan {
            loan: actual,
            if_active: false,
        });
    }
    if detached {
        // Keep the actual loan active while replacing its backing storage.
        // The selected phi must retain that dynamic witness and reject its
        // changed backing even though the declaration label already ended.
        let held = f.b.owned(BLOCK, outer_layout);
        f.inst(Inst::Read {
            result: held,
            address: at,
            reaching: owner,
            mode: ReadMode::Take,
        });
        f.inst(Inst::StorageDead {
            slot,
            if_live: false,
        });
        f.inst(Inst::StorageLive { slot });
    }
    let address = f.b.address(
        AddressBase::Value(selected),
        &[ProjectionStep::Field(0)],
        BLOCK,
        c.boxed,
    );
    let failure = f.propagates();
    f.inst(Inst::Address { address, failure });
    let child = f.b.value(BLOCK, c.boxed, Holding::Borrowed(label));
    f.inst(Inst::Read {
        result: child,
        address,
        reaching: selected,
        mode: ReadMode::Borrow,
    });
    let n = f.unbox(child);
    f.print(n);
    f.inst(Inst::EndLoan {
        loan: actual,
        if_active: false,
    });
    let take = f.b.owned(BLOCK, outer_layout);
    f.inst(Inst::Read {
        result: take,
        address: at,
        reaching: owner,
        mode: ReadMode::Take,
    });
    f.inst(Inst::StorageDead {
        slot,
        if_live: false,
    });
    f.release(take, outer_layout);
    f.ret(None);
    let unit = f.finish(&table);
    run(
        &program(target, table, vec![unit]),
        &path("main"),
        &[],
        &mut Printer,
        &Config::default(),
    )
}
#[test]
fn borrow_cursor_8_projection_uses_selected_live_loan() {
    assert_eq!(
        projected(false, false).result.unwrap().observation.output,
        b"42\n"
    );
}
#[test]
fn borrow_cursor_8_projection_cannot_revive_selected_expired_loan() {
    assert_eq!(
        projected(true, false).result.unwrap_err().kind,
        ViolationKind::BorrowExpired
    );
}

#[test]
fn borrow_cursor_8_projection_cannot_reuse_changed_backing() {
    assert_eq!(
        projected(false, true).result.unwrap_err().kind,
        ViolationKind::ViewDetached
    );
}
