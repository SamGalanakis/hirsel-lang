//! Payload legality is audited independently of the native validator.
use crate::fixtures;
use fixtures::*;
use hirsel_heapcheck::build::{Layouts, path};
use hirsel_heapcheck::{Config, Program, ViolationKind};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{LayoutKind, TargetSpec};
use hirsel_lir::stage::Rc;

fn fixture(boxed: bool, nested: bool) -> Program<Rc> {
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let mut l = Layouts::new(target.clone());
    let c = common(&mut l);
    let inner_l = l.block(c.u64);
    l.get_mut(inner_l).kind = LayoutKind::Box { payload: c.u64 };
    let outer_l = if nested { l.block(inner_l) } else { inner_l };
    l.get_mut(outer_l).kind = if boxed {
        LayoutKind::Box {
            payload: if nested { inner_l } else { c.u64 },
        }
    } else {
        LayoutKind::Block {
            payload: if nested { inner_l } else { c.u64 },
        }
    };
    let mut f = F::<Rc>::new("main", c);
    let value = f.int(1);
    let k =
        f.b.construct(ConstructorKind::Box, inner_l, &[(0, value)], HEAP);
    let inner = f.b.owned(BLOCK, inner_l);
    let failure = f.propagates();
    f.inst(Inst::Construct {
        result: inner,
        construct: k,
        failure,
    });
    let value = if nested {
        let k =
            f.b.construct(ConstructorKind::Box, outer_l, &[(0, inner)], HEAP);
        let outer = f.b.owned(BLOCK, outer_l);
        f.inst(Inst::Construct {
            result: outer,
            construct: k,
            failure,
        });
        outer
    } else {
        inner
    };
    let slot = f.b.slot(BLOCK, outer_l, SlotKind::Local);
    f.inst(Inst::StorageLive { slot });
    let whole = f.b.address(AddressBase::Slot(slot), &[], BLOCK, outer_l);
    f.inst(Inst::Address {
        address: whole,
        failure,
    });
    let version = f.b.plain(BLOCK, outer_l);
    f.inst(Inst::Store {
        address: whole,
        value,
        version,
    });
    let steps = if nested {
        vec![ProjectionStep::Payload, ProjectionStep::Payload]
    } else {
        vec![ProjectionStep::Payload]
    };
    let at = f.b.address(AddressBase::Slot(slot), &steps, U64, c.u64);
    f.inst(Inst::Address {
        address: at,
        failure,
    });
    let old = f.b.plain(U64, c.u64);
    f.inst(Inst::Read {
        result: old,
        address: at,
        reaching: version,
        mode: ReadMode::Take,
    });
    let next = f.int(2);
    let version = f.b.plain(BLOCK, outer_l);
    f.inst(Inst::Store {
        address: at,
        value: next,
        version,
    });
    let read = f.b.plain(U64, c.u64);
    f.inst(Inst::Read {
        result: read,
        address: at,
        reaching: version,
        mode: ReadMode::Bits,
    });
    f.print(read);
    let pointer = f.b.owned(BLOCK, outer_l);
    f.inst(Inst::Read {
        result: pointer,
        address: whole,
        reaching: version,
        mode: ReadMode::Take,
    });
    f.inst(Inst::StorageDead {
        slot,
        if_live: false,
    });
    f.release(pointer, if nested { inner_l } else { c.u64 });
    f.ret(None);
    let (_, table) = l.finish();
    let unit = f.finish(&table);
    program(target, table, vec![unit])
}

#[test]
fn payload_rejects_a_block_kind_and_accepts_whole_payload_writes() {
    for nested in [false, true] {
        let clean = run(
            &fixture(true, nested),
            &path("main"),
            &[],
            &mut Printer,
            &Config::default(),
        );
        assert_eq!(clean.result.unwrap().observation.output, b"2\n");
        let bad = run(
            &fixture(false, nested),
            &path("main"),
            &[],
            &mut Printer,
            &Config::default(),
        );
        assert_eq!(bad.result.unwrap_err().kind, ViolationKind::TypeConfusion);
    }
}
