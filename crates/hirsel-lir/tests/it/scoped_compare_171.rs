use crate::support::*;
use hirsel_core_contract::Passing;
use hirsel_lir::{LayoutId, abi_types::ParameterPath, ir::*, stage::Ready};

fn compare(layout: LayoutId, op: CompareOp, restricted: usize, escape: bool) -> Vec<K> {
    let mut t = Lir::<Ready>::new(
        &[(Passing::Borrow, layout), (Passing::Borrow, layout)],
        Some(if escape { layout } else { l::BOOL }),
    );
    let abi = &mut t.table.fn_abis[t.builder.unit.header.abi.0 as usize];
    abi.nonretaining = vec![ParameterPath {
        parameter: restricted as u32,
        fields: vec![],
    }];
    abi.scoped_contract = hirsel_lir::abi::scoped_contract_digest(&abi.nonretaining, &[]);
    let left = t.plain(layout);
    t.param(left);
    let right = t.plain(layout);
    t.param(right);
    let result = t.plain(l::BOOL);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Compare {
            result,
            op,
            left,
            right,
            layout,
            failure,
        },
    );
    t.builder.ret(
        ENTRY,
        Some(if escape {
            [left, right][restricted]
        } else {
            result
        }),
    );
    t.check()
}

#[test]
fn borrowed_data_comparison_does_not_retain_either_operand() {
    for layout in [l::SLICE, l::BLK, l::PAIR, l::OPT, l::SEQBLK] {
        for op in [
            CompareOp::Eq,
            CompareOp::Ne,
            CompareOp::Lt,
            CompareOp::Le,
            CompareOp::Gt,
            CompareOp::Ge,
        ] {
            for restricted in 0..2 {
                let errors = compare(layout, op, restricted, false);
                assert!(
                    errors.is_empty(),
                    "{layout:?} {op:?} {restricted}: {errors:?}"
                );
            }
        }
    }
}

#[test]
fn comparison_does_not_erase_an_operand_escape() {
    for restricted in 0..2 {
        let errors = compare(l::SLICE, CompareOp::Eq, restricted, true);
        assert!(
            errors.iter().any(|error| matches!(
                error,
                K::ScopedRetention {
                    operation: "return or error payload",
                    ..
                }
            )),
            "{errors:?}"
        );
    }
}

#[test]
fn a_scoped_callable_cannot_be_compared_as_data() {
    for restricted in 0..2 {
        let errors = compare(l::ERASED, CompareOp::Eq, restricted, false);
        assert!(
            errors.contains(&K::Type("structural comparison of an incomparable layout")),
            "{errors:?}"
        );
    }
}
