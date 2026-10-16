//! Flat sequences encode borrowed ordinary sequences without moving their holders.
//! These controls build IR directly; the element mismatch must still reject.
use crate::support;
use hirsel_core_contract::Passing;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::*;
use hirsel_lir::stage::Rc;
use support::*;

fn construction(wrong_element: bool, release_source: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[], Some(l::FLAT_ITEMS_BLK));
    let payload = LayoutId(t.table.layouts.len() as u32);
    let mut sequence = t.table.layouts[l::SEQ.0 as usize].clone();
    let element = if wrong_element {
        l::BLK
    } else {
        l::FLAT_ITEM_BLK
    };
    sequence.kind = LayoutKind::Sequence { element };
    sequence.scan = vec![ScanStep::Sequence {
        count_offset: 0,
        offset: 16,
        stride: 8,
        element,
    }];
    sequence.drop = vec![DropStep::ElementsReverse {
        count_offset: 0,
        offset: 16,
        stride: 8,
        element,
    }];
    t.table.layouts.push(sequence);
    let source = LayoutId(t.table.layouts.len() as u32);
    let mut block = t.table.layouts[l::SEQBLK.0 as usize].clone();
    block.kind = LayoutKind::Block { payload };
    t.table.layouts.push(block);
    let abi = t.unit.header.abi;
    // The ABI fixture builder selects the matching physical slots for this
    // extra taken parameter, so the ownership control has exactly one credit.
    let new_abi = t.abi(&[(Passing::Take, source)], Some(l::FLAT_ITEMS_BLK));
    t.unit.header.abi = new_abi;
    assert_eq!(abi.0, 0);
    let input = t.value_as(ValueRepr::Block, source, Holding::Owned);
    t.param(input);
    let symbol = t.symbol(SymbolKind::Type);
    let placement = t.heap();
    let site = t.construct_site(
        ConstructorKind::Record(symbol),
        l::FLAT_ITEMS,
        &[(0, input)],
        placement,
    );
    let result = t.owned(l::FLAT_ITEMS_BLK);
    let target = t.releasing(&[input]);
    let failure = t.failure(target);
    t.push(
        ENTRY,
        Inst::Construct {
            result,
            construct: site,
            failure,
        },
    );
    if release_source {
        t.release(ENTRY, input, &[result]);
    }
    t.ret(ENTRY, Some(result));
    t.check()
}

#[test]
fn flat_sequence_construction_checks_the_source_element_layout() {
    assert!(construction(false, true).is_empty());
    assert!(construction(true, true).contains(&K::Type("constructed fields")));
}

#[test]
fn flat_sequence_construction_borrows_the_source_credit() {
    assert!(construction(false, true).is_empty());
    assert!(
        construction(false, false)
            .iter()
            .any(|k| matches!(k, K::Leak(_)))
    );
}
