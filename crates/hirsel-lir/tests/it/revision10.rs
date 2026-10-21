//! Inert Path values compare/hash; capability handles remain incomparable.
use crate::support;
use hirsel_core_contract::Passing;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{LayoutKind, ScanStep};
use hirsel_lir::stage::Rc;
use support::*;

fn path_or_handle(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[], Some(l::BOOL));
    let mut record = t.table.layouts[l::U64.0 as usize].clone();
    record.kind = if mutant {
        LayoutKind::Handle
    } else {
        LayoutKind::Path
    };
    if mutant {
        record.scan = vec![ScanStep::Handle {
            offset: 0,
            kind: hirsel_lir::layout_types::HandleKind::Unknown,
            ty: None,
            obtains: None,
        }];
    }
    let layout = hirsel_lir::layout_types::LayoutId(t.table.layouts.len() as u32);
    t.table.layouts.push(record);
    t.builder.unit.header.abi = t.abi(
        &[(Passing::Borrow, layout), (Passing::Borrow, layout)],
        Some(l::BOOL),
    );
    let a = t.plain(layout);
    t.param(a);
    let b = t.plain(layout);
    t.param(b);
    for ty in &mut t.builder.unit.types {
        if ty.layout == layout {
            ty.repr = if mutant {
                ValueRepr::Handle
            } else {
                ValueRepr::Scalar(hirsel_lir::layout_types::ScalarKind::U64)
            };
        }
    }
    let result = t.plain(l::BOOL);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Compare {
            result,
            op: CompareOp::Lt,
            left: a,
            right: b,
            layout,
            failure,
        },
    );
    t.builder.ret(ENTRY, Some(result));
    t.check()
}
#[test]
fn path_comparison_does_not_make_capability_handles_comparable() {
    pair(path_or_handle, |kind| {
        *kind == K::Type("structural comparison of an incomparable layout")
    });
}
