//! Static data (CT-2): every constant a unit reads is encoded as immortal
//! blocks at its layouts. Expectations are written from the declarations: a
//! value's bytes at its layout, and one block per payload it reaches.
use crate::rc_support as support;

use hirsel_lir::layout_types::{LayoutKind, ScalarKind};
use hirsel_lir::statics::{StaticData, StaticReference};
use support::*;

fn only(lowered: &hirsel_rc::import::Lowered, name: &str) -> StaticData {
    let found: Vec<_> = lowered
        .statics
        .iter()
        .filter(|s| s.symbol.definition.0.name == name)
        .cloned()
        .collect();
    assert_eq!(
        found.len(),
        1,
        "one static for {name}: {:?}",
        lowered.statics
    );
    found.into_iter().next().unwrap()
}

#[test]
fn a_scalar_constant_is_one_block_of_its_bytes() {
    let (_, lowered) = lower(
        "const FIRST: u32 = comptime 3\nconst SECOND: u32 = comptime (FIRST + 4)\nfn main() -> u32 { SECOND }",
    );
    let data = only(&lowered, "SECOND");
    assert_eq!(data.blocks.len(), 1);
    let block = &data.blocks[0];
    assert_eq!(
        lowered.table.layouts[block.layout.0 as usize].kind,
        LayoutKind::Scalar(ScalarKind::U32)
    );
    assert_eq!(block.bytes, 7u32.to_le_bytes());
    assert!(block.references.is_empty());
    // FIRST is read only while SECOND is evaluated: no unit reads it.
    assert_eq!(lowered.statics.len(), 1);
}

#[test]
fn a_constant_reaches_its_list_and_byte_blocks_through_reference_cells() {
    let (_, lowered) = lower(
        "type R {\n  xs: List[Bytes],\n}\nconst C: R = R(xs: [b\"x\", b\"yz\"])\nfn main() -> u64 { let built = copy (C).xs\n list.len(built) }",
    );
    let data = only(&lowered, "C");
    let layouts = &lowered.table.layouts;
    let kind = |i: usize| &layouts[data.blocks[i].layout.0 as usize].kind;
    // R, its list's sequence payload, and one byte sequence per element.
    assert_eq!(data.blocks.len(), 4, "{data:?}");
    assert_eq!(*kind(0), LayoutKind::Aggregate);
    let xs = layouts[data.blocks[0].layout.0 as usize].fields[0].offset;
    assert_eq!(
        data.blocks[0].references,
        [StaticReference {
            offset: xs,
            target: 1
        }]
    );
    let list = &data.blocks[1];
    assert!(matches!(kind(1), LayoutKind::Sequence { .. }));
    assert_eq!(list.bytes[..8], 2u64.to_le_bytes());
    let targets: Vec<u32> = list.references.iter().map(|r| r.target).collect();
    assert_eq!(targets, [2, 3]);
    for (block, text) in [(2, b"x".as_slice()), (3, b"yz".as_slice())] {
        let bytes = &data.blocks[block].bytes;
        assert!(matches!(kind(block), LayoutKind::Sequence { .. }));
        assert_eq!(bytes[..8], (text.len() as u64).to_le_bytes());
        assert_eq!(&bytes[8..], text);
    }
}

#[test]
fn a_dynamic_flat_constant_is_one_pointer_free_extent() {
    let (_, lowered) = lower(
        "flat type Item { text: String, n: u64 }\nflat type Items { items: List[Item] }\nconst DATA: Items = comptime Items(items: [Item(text: \"\", n: 3), Item(text: \"abc\", n: 7)])\nfn main() -> u64 { let xs = DATA\n list.len(xs.items) }",
    );
    let data = only(&lowered, "DATA");
    assert_eq!(data.blocks.len(), 1, "flat children occupy the same extent");
    let block = &data.blocks[0];
    assert!(block.references.is_empty());
    assert_eq!(
        lowered.table.layouts[block.layout.0 as usize].kind,
        LayoutKind::Flat
    );
    assert_eq!(
        hirsel_lir::layout::extent(&lowered.table, block.layout, &block.bytes).unwrap(),
        block.bytes.len() as u64,
    );
    // The existing layout's sequence prefix covers the count and the two
    // length-prefixed records, rather than any ordinary sequence pointers.
    assert_eq!(u64::from_le_bytes(block.bytes[..8].try_into().unwrap()), 59);
    assert_eq!(
        u64::from_le_bytes(block.bytes[8..16].try_into().unwrap()),
        2
    );
    assert_eq!(
        u64::from_le_bytes(block.bytes[16..24].try_into().unwrap()),
        16
    );
    assert_eq!(
        u64::from_le_bytes(block.bytes[24..32].try_into().unwrap()),
        0
    );
    assert_eq!(
        u64::from_le_bytes(block.bytes[32..40].try_into().unwrap()),
        3
    );
    assert_eq!(
        u64::from_le_bytes(block.bytes[40..48].try_into().unwrap()),
        19
    );
    assert_eq!(&block.bytes[56..59], b"abc");
    assert_eq!(
        u64::from_le_bytes(block.bytes[59..67].try_into().unwrap()),
        7
    );
}
