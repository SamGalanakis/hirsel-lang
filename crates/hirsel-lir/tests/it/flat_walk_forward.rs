//! Reaching sequence versions may cross unchanged forwarding joins.
use crate::support::*;
use hirsel_core_contract::Passing;
use hirsel_lir::ir::*;
use hirsel_lir::stage::Value;

fn control(forged: bool) -> Vec<K> {
    let mut t = Lir::<Value>::new(&[(Passing::Borrow, l::FLAT_ITEMS_BLK)], None);
    let input = t.plain(l::FLAT_ITEMS_BLK);
    t.param(input);
    let root = t.slot(l::FLAT_ITEMS_BLK, SlotKind::Parameter(0), ROOT);
    let index = t.plain(l::U64);
    let offset = t.plain(l::U64);
    let version = t.plain(l::FLAT_ITEMS_BLK);
    let header = t.builder.block(BlockKind::Body, &[index, offset, version]);
    let forwarded = t.plain(l::FLAT_ITEMS_BLK);
    let latch = t.builder.block(BlockKind::Body, &[forwarded]);
    let exit = t.builder.block(BlockKind::Body, &[]);
    let zero = t.plain(l::U64);
    let one = t.plain(l::U64);
    for (value, n) in [(zero, 0), (one, 1)] {
        let constant = ConstantId(t.builder.unit.tables.constants.len() as u32);
        let ty = t.builder.unit.values[value.index()].ty;
        t.builder.unit.tables.constants.push(ConstantData {
            ty,
            value: ConstantValue::Integer(n),
        });
        t.builder.push(
            ENTRY,
            Inst::Const {
                result: value,
                constant,
            },
        );
    }
    t.builder.jump(ENTRY, header, &[zero, zero, input]);
    let address = t.address(
        AddressBase::Slot(root),
        &[
            ProjectionStep::Field(0),
            ProjectionStep::FlatAt {
                index,
                offset: if forged { index } else { offset },
            },
        ],
        l::FLAT_WRAP,
    );
    let failure = t.fails();
    t.builder.push(header, Inst::Address { address, failure });
    let next = t.plain(l::U64);
    t.builder.push(
        header,
        Inst::FlatNext {
            result: next,
            address,
        },
    );
    let increment = t.plain(l::U64);
    let failure = t.fails();
    t.builder.push(
        header,
        Inst::Binary {
            result: increment,
            op: BinaryOp::Add,
            left: index,
            right: one,
            failure,
        },
    );
    t.branch(header, (latch, &[version]), (exit, &[]));
    t.builder.jump(latch, header, &[increment, next, forwarded]);
    t.builder.ret(exit, None);
    t.check()
}

#[test]
fn unchanged_forwarded_versions_preserve_flat_walk_evidence() {
    assert!(control(false).is_empty(), "{:?}", control(false));
    assert!(
        control(true)
            .iter()
            .any(|k| *k == K::Type("flat walk offset replay"))
    );
}
