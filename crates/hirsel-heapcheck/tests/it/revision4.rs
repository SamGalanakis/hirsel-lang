//! LIR schema revision 4 on the ledger: a flat construction encodes a String
//! slice behind its length prefix, a String part is viewed as a slice of the
//! block holding it, and a flat list is counted and indexed at its cursor.
use crate::fixtures;

use fixtures::*;
use hirsel_heapcheck::build::{Layouts, path};
use hirsel_heapcheck::{Config, Program, Run};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{
    CopyRecipe, DropStep, FieldLayout, LayoutId, LayoutKind, ScalarKind, ScanStep, SizeSpec,
    SizeTerm, TargetSpec,
};
use hirsel_lir::stage::Rc;
use hirsel_lir::statics::StaticData;

fn execute(p: &Program<Rc>) -> Run {
    run(p, &path("main"), &[], &mut Printer, &Config::default())
}

fn output(run: &Run) -> String {
    let audit = run
        .result
        .as_ref()
        .unwrap_or_else(|v| panic!("expected a clean audit: {v}"));
    String::from_utf8(audit.observation.output.clone()).unwrap()
}

/// A variable part's extent: its u64 length prefix and the bytes it counts.
fn prefixed() -> Vec<SizeTerm> {
    vec![
        SizeTerm::Constant(8),
        SizeTerm::ReadLength {
            offset: 0,
            scalar: ScalarKind::U64,
        },
        SizeTerm::Add,
    ]
}

fn field(ordinal: u32, offset: u64, layout: LayoutId) -> FieldLayout {
    FieldLayout {
        ordinal,
        offset,
        layout,
        unaligned: false,
    }
}

/// The FlatFormatV0 layouts of `flat type Item { text: String, n: u64 }` and
/// `flat type Items { items: List[Item] }`, and the String slice.
struct Flat {
    c: Common,
    slice: LayoutId,
    item: LayoutId,
    item_block: LayoutId,
    items: LayoutId,
    items_block: LayoutId,
}

fn flat_layouts() -> (Layouts, Flat) {
    let mut l = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let c = common(&mut l);
    let byte = l.scalar(ScalarKind::U8);
    // Literal Slice owners contain a count followed by bytes. Their backing
    // payload descriptor is distinct from the three-word Slice value.
    let bytes = l.record(&[c.u64]);
    {
        let r = l.get_mut(bytes);
        r.kind = LayoutKind::Sequence { element: byte };
        r.size = SizeSpec::Dynamic(prefixed());
        r.copy = CopyRecipe::NotCopyable;
    }
    let pointer = l.scalar(ScalarKind::Pointer);
    let slice = l.record(&[pointer, c.u64, c.u64]);
    {
        let r = l.get_mut(slice);
        r.kind = LayoutKind::Slice { element: byte };
        r.scan = vec![ScanStep::SliceOwner { offset: 0 }];
        r.drop = vec![DropStep::Release { offset: 0 }];
        r.copy = CopyRecipe::Dup;
    }
    let flat = |l: &mut Layouts, fields: Vec<FieldLayout>, size: Vec<SizeTerm>| {
        let id = l.record(&[]);
        let r = l.get_mut(id);
        r.kind = LayoutKind::Flat;
        r.size = SizeSpec::Dynamic(size);
        r.align = 1;
        r.fields = fields;
        r.copy = CopyRecipe::FlatBytes;
        id
    };
    let text = flat(&mut l, vec![field(0, 8, byte)], prefixed());
    let mut n = field(1, 8, c.u64);
    n.unaligned = true;
    // The record's extent: the text's prefixed extent at 0, then 8 bytes.
    let item = flat(
        &mut l,
        vec![field(0, 0, text), n],
        vec![
            SizeTerm::Constant(0),
            SizeTerm::Dup,
            SizeTerm::At(prefixed()),
            SizeTerm::Add,
            SizeTerm::Constant(8),
            SizeTerm::Add,
        ],
    );
    let element = flat(&mut l, vec![field(0, 8, item)], prefixed());
    let list = flat(&mut l, vec![field(0, 16, element)], prefixed());
    let items = flat(
        &mut l,
        vec![field(0, 0, list)],
        vec![
            SizeTerm::Constant(0),
            SizeTerm::Dup,
            SizeTerm::At(prefixed()),
            SizeTerm::Add,
        ],
    );
    let item_block = l.block(item);
    let items_block = l.block(items);
    for b in [item_block, items_block] {
        l.get_mut(b).copy = CopyRecipe::FlatBytes;
    }
    (
        l,
        Flat {
            c,
            slice,
            item,
            item_block,
            items,
            items_block,
        },
    )
}

impl F<Rc> {
    /// A prepared payload address of the block `base`.
    fn at(&mut self, base: ValueId, steps: &[ProjectionStep], layout: LayoutId) -> AddressId {
        let repr = if layout == self.c.u64 {
            U64
        } else {
            ValueRepr::Aggregate
        };
        let a = self
            .b
            .address(AddressBase::Value(base), steps, repr, layout);
        let failure = self.raises("MEM-14", self.propagate);
        self.inst(Inst::Address {
            address: a,
            failure,
        });
        a
    }

    fn length(&mut self, address: AddressId) -> ValueId {
        let result = self.b.plain(U64, self.c.u64);
        self.inst(Inst::FlatLength { result, address });
        result
    }
}

/// `Item(text: "flat", n: 7)`: `n` lies after the prefix 4 and the four
/// bytes, the text views as a 4-byte slice of the block, and its length is
/// its byte count.
#[test]
fn a_flat_construction_encodes_a_slice_behind_its_prefix() {
    let (l, x) = flat_layouts();
    let text_encoding = l.get(x.item).fields[0].layout;
    let (target, table) = l.finish();
    let mut f = F::<Rc>::new("main", x.c);
    let k = f.b.constant(
        ValueRepr::Aggregate,
        x.slice,
        ConstantValue::String("flat".into()),
    );
    let text = f.b.plain(ValueRepr::Aggregate, x.slice);
    f.inst(Inst::Const {
        result: text,
        constant: k,
    });
    let n = f.int(7);
    let sym = f.b.symbol(SymbolKind::Type, path("Item"));
    let site = f.b.construct(
        ConstructorKind::Record(sym),
        x.item,
        &[(0, text), (1, n)],
        HEAP,
    );
    let item = f.b.owned(BLOCK, x.item_block);
    let failure = f.propagates();
    f.inst(Inst::Construct {
        result: item,
        construct: site,
        failure,
    });
    let c = x.c;
    let m = f.load(item, &[ProjectionStep::Field(1)], U64, c.u64);
    f.print(m);
    let at = f.at(item, &[ProjectionStep::Field(0)], text_encoding);
    let loan = f.b.loan(at, false);
    f.inst(Inst::BeginLoan {
        loan,
        address: at,
        mutable: false,
        if_inactive: false,
    });
    let view =
        f.b.value(ValueRepr::Aggregate, x.slice, Holding::Borrowed(loan));
    f.inst(Inst::FlatView {
        result: view,
        address: at,
    });
    let bytes = f.length(at);
    f.print(bytes);
    f.inst(Inst::EndLoan {
        loan,
        if_active: false,
    });
    f.release(item, x.item);
    f.ret(None);
    let main = f.finish(&table);
    assert_eq!(
        output(&execute(&program(target, table, vec![main]))),
        "7\n4\n"
    );
}

/// Regional flat bytes have an immortal owner, so borrowing and copying
/// a view remain valid only while the region holds its storage.
fn regional_flat_view(mode: ReadMode, after_exit: bool) -> Run {
    let (mut l, x) = flat_layouts();
    let item_ref = l.uncounted(x.item);
    let text_encoding = l.get(x.item).fields[0].layout;
    let (target, table) = l.finish();
    let mut f = F::<Rc>::new("main", x.c);
    let region = f.b.region();
    let mark = f.b.owned(ValueRepr::RegionMark, x.c.u64);
    f.inst(Inst::RegionEnter {
        result: mark,
        region,
    });
    let k = f.b.constant(
        ValueRepr::Aggregate,
        x.slice,
        ConstantValue::String("é".into()),
    );
    let text = f.b.plain(ValueRepr::Aggregate, x.slice);
    f.inst(Inst::Const {
        result: text,
        constant: k,
    });
    let n = f.int(7);
    let sym = f.b.symbol(SymbolKind::Type, path("Item"));
    let site = f.b.construct(
        ConstructorKind::Record(sym),
        x.item,
        &[(0, text), (1, n)],
        Placement::Fixed(StorageOwner::LocalRegion(region)),
    );
    f.b.unit.tables.constructs[site.index()].destination = DestinationClass::Region;
    let item = f.b.plain(ValueRepr::Uncounted, item_ref);
    let failure = f.propagates();
    f.inst(Inst::Construct {
        result: item,
        construct: site,
        failure,
    });
    let at = f.at(item, &[ProjectionStep::Field(0)], text_encoding);
    let loan = f.b.loan(at, false);
    f.inst(Inst::BeginLoan {
        loan,
        address: at,
        mutable: false,
        if_inactive: false,
    });
    let view =
        f.b.value(ValueRepr::Aggregate, x.slice, Holding::Borrowed(loan));
    f.inst(Inst::FlatView {
        result: view,
        address: at,
    });
    let length = f.b.plain(U64, x.c.u64);
    f.inst(Inst::SeqLen {
        result: length,
        value: view,
    });
    f.print(length);
    let mut copied = None;
    {
        let slot = f.b.slot(ValueRepr::Aggregate, x.slice, SlotKind::Temporary);
        let address =
            f.b.address(AddressBase::Slot(slot), &[], ValueRepr::Aggregate, x.slice);
        f.inst(Inst::StorageLive { slot });
        let version = f.b.plain(ValueRepr::Aggregate, x.slice);
        f.inst(Inst::Store {
            address,
            value: view,
            version,
        });
        if mode == ReadMode::Borrow {
            let read =
                f.b.value(ValueRepr::Aggregate, x.slice, Holding::Borrowed(loan));
            f.inst(Inst::Read {
                result: read,
                address,
                reaching: version,
                mode,
            });
            let length = f.b.plain(U64, x.c.u64);
            f.inst(Inst::SeqLen {
                result: length,
                value: read,
            });
        } else {
            let copy = f.b.owned(ValueRepr::Aggregate, x.slice);
            let failure = f.propagates();
            f.inst(Inst::Copy {
                result: copy,
                address,
                reaching: version,
                failure,
            });
            copied = Some(copy);
        }
    }
    f.inst(Inst::EndLoan {
        loan,
        if_active: false,
    });
    let failure = f.propagates();
    if !after_exit && let Some(copy) = copied {
        f.release(copy, x.slice);
    }
    f.inst(Inst::RegionExit {
        mark,
        region,
        failure,
    });
    if after_exit {
        let length = f.b.plain(U64, x.c.u64);
        f.inst(Inst::SeqLen {
            result: length,
            value: copied.unwrap(),
        });
        f.print(length);
    }
    f.ret(None);
    let main = f.finish(&table);
    execute(&program(target, table, vec![main]))
}

#[test]
fn a_regional_flat_view_can_be_borrowed_and_copied_until_region_exit() {
    assert_eq!(output(&regional_flat_view(ReadMode::Borrow, false)), "2\n");
    assert_eq!(output(&regional_flat_view(ReadMode::Take, false)), "2\n");
    assert_eq!(
        regional_flat_view(ReadMode::Take, true).violation(),
        Some(hirsel_heapcheck::ViolationKind::RegionEscape)
    );
}

/// `Items` holding `[("ab", 1), ("xyz", 2)]`: two elements, the second's `n`
/// past the first element's prefix; index `past` fails as out of bounds.
fn indexed(index: u128) -> Run {
    let (l, x) = flat_layouts();
    let list = l.get(x.items).fields[0].layout;
    let (target, table) = l.finish();
    let mut f = F::<Rc>::new("main", x.c);
    let sym = f.b.symbol(SymbolKind::Constant, path("ITEMS"));
    let s = f.b.owned(BLOCK, x.items_block);
    f.inst(Inst::ReadStatic {
        result: s,
        symbol: sym,
    });
    let at = f.at(s, &[ProjectionStep::Field(0)], list);
    let count = f.length(at);
    f.print(count);
    let i = f.int(index);
    let c = x.c;
    let at = f.at(
        s,
        &[
            ProjectionStep::Field(0),
            ProjectionStep::Index(i),
            ProjectionStep::Field(0),
            ProjectionStep::Field(1),
        ],
        c.u64,
    );
    let n = f.b.plain(U64, c.u64);
    f.inst(Inst::Read {
        result: n,
        address: at,
        reaching: s,
        mode: ReadMode::Bits,
    });
    f.print(n);
    f.release(s, x.items);
    f.ret(None);
    let main = f.finish(&table);
    let mut p = program(target, table, vec![main]);
    let item = |text: &[u8], n: u64| {
        let mut out = (text.len() as u64).to_le_bytes().to_vec();
        out.extend(text);
        out.extend(n.to_le_bytes());
        let mut element = (out.len() as u64).to_le_bytes().to_vec();
        element.extend(out);
        element
    };
    let mut elements = 2u64.to_le_bytes().to_vec();
    elements.extend(item(b"ab", 1));
    elements.extend(item(b"xyz", 2));
    let mut bytes = (elements.len() as u64).to_le_bytes().to_vec();
    bytes.extend(elements);
    p.statics.push(StaticData::plain(
        Symbol {
            kind: SymbolKind::Constant,
            definition: path("ITEMS"),
            instance: None,
        },
        x.items,
        bytes,
    ));
    execute(&p)
}

#[test]
fn a_flat_list_element_lies_at_its_cursor() {
    assert_eq!(output(&indexed(1)), "2\n2\n");
    assert_eq!(output(&indexed(0)), "2\n1\n");
}

#[test]
fn an_index_past_a_flat_list_fails_its_bounds_check() {
    let run = indexed(2);
    let audit = run.result.as_ref().expect("a clean audit of the failure");
    assert_eq!(audit.observation.output, b"2\n");
    let rules: Vec<_> = audit
        .observation
        .failures
        .iter()
        .map(|f| f.rule.as_str())
        .collect();
    assert_eq!(rules, ["MEM-14"]);
}

/// Element 1 of `ITEMS` by `FlatAt` at `offset`, or (`None`) at the end of
/// element 0 as `FlatNext` gives it (LIR schema revision 7): prints the end
/// and element 1's `n`.
fn walked(offset: Option<u128>) -> Run {
    walked_address(offset, 0)
}

fn walked_address(offset: Option<u128>, kind: u8) -> Run {
    let (l, x) = flat_layouts();
    let list = l.get(x.items).fields[0].layout;
    let element = l.get(list).fields[0].layout;
    let (target, table) = l.finish();
    let mut f = F::<Rc>::new("main", x.c);
    let sym = f.b.symbol(SymbolKind::Constant, path("ITEMS"));
    let s = f.b.owned(BLOCK, x.items_block);
    f.inst(Inst::ReadStatic {
        result: s,
        symbol: sym,
    });
    let zero = f.int(0);
    let mut first_path = vec![
        ProjectionStep::Field(0),
        ProjectionStep::FlatAt {
            index: zero,
            offset: zero,
        },
    ];
    let first_layout = if kind == 1 {
        first_path.extend([ProjectionStep::Field(0), ProjectionStep::Field(0)]);
        table.layouts[x.item.0 as usize].fields[0].layout
    } else {
        element
    };
    let first = f.at(s, &first_path, first_layout);
    let first = if kind == 2 {
        let alias = f.b.address(
            AddressBase::Address(first),
            &[],
            ValueRepr::Aggregate,
            element,
        );
        let failure = f.raises("MEM-14", f.propagate);
        f.inst(Inst::Address {
            address: alias,
            failure,
        });
        alias
    } else {
        first
    };
    let end = f.b.plain(U64, x.c.u64);
    f.inst(Inst::FlatNext {
        result: end,
        address: first,
    });
    f.print(end);
    let offset = match offset {
        Some(n) => f.int(n),
        None => end,
    };
    let one = f.int(1);
    let c = x.c;
    let at = f.at(
        s,
        &[
            ProjectionStep::Field(0),
            ProjectionStep::FlatAt { index: one, offset },
            ProjectionStep::Field(0),
            ProjectionStep::Field(1),
        ],
        c.u64,
    );
    let n = f.b.plain(U64, c.u64);
    f.inst(Inst::Read {
        result: n,
        address: at,
        reaching: s,
        mode: ReadMode::Bits,
    });
    f.print(n);
    f.release(s, x.items);
    f.ret(None);
    let main = f.finish(&table);
    let mut p = program(target, table, vec![main]);
    let item = |text: &[u8], n: u64| {
        let mut out = (text.len() as u64).to_le_bytes().to_vec();
        out.extend(text);
        out.extend(n.to_le_bytes());
        let mut element = (out.len() as u64).to_le_bytes().to_vec();
        element.extend(out);
        element
    };
    let mut elements = 2u64.to_le_bytes().to_vec();
    elements.extend(item(b"ab", 1));
    elements.extend(item(b"xyz", 2));
    let mut bytes = (elements.len() as u64).to_le_bytes().to_vec();
    bytes.extend(elements);
    p.statics.push(StaticData::plain(
        Symbol {
            kind: SymbolKind::Constant,
            definition: path("ITEMS"),
            instance: None,
        },
        x.items,
        bytes,
    ));
    execute(&p)
}

#[test]
fn a_flat_walk_names_each_element_at_its_content_offset() {
    // Element 0 is its 8-byte prefix, "ab" behind its own prefix, and n.
    assert_eq!(output(&walked(None)), "26\n2\n");
    assert_eq!(output(&walked(Some(26))), "26\n2\n");
    assert_eq!(
        walked(Some(25)).violation(),
        Some(hirsel_heapcheck::ViolationKind::Malformed)
    );
}

#[test]
fn an_element_end_survives_an_address_alias_but_does_not_replace_a_fields_extent() {
    let alias = walked_address(None, 2);
    assert_eq!(output(&alias), "26\n2\n");
    assert_eq!(alias.counters.flat_prefix_reads, 2);
    // FlatNext at the first record's text ends before its n field. Keep
    // the second element's correct offset explicit, so both audits succeed.
    assert_eq!(output(&walked_address(Some(26), 1)), "18\n2\n");
}

/// `copy` of a variable flat value: a new block of its extent, independent
/// of the original, both released clean.
#[test]
fn a_variable_flat_value_copies_its_extent() {
    let (l, x) = flat_layouts();
    let (target, table) = l.finish();
    let mut f = F::<Rc>::new("main", x.c);
    let sym = f.b.symbol(SymbolKind::Constant, path("ITEM"));
    let s = f.b.owned(BLOCK, x.item_block);
    f.inst(Inst::ReadStatic {
        result: s,
        symbol: sym,
    });
    let slot = f.b.slot(BLOCK, x.item_block, SlotKind::Temporary);
    let whole =
        f.b.address(AddressBase::Slot(slot), &[], BLOCK, x.item_block);
    let failure = f.propagates();
    f.inst(Inst::Address {
        address: whole,
        failure,
    });
    f.inst(Inst::StorageLive { slot });
    let version = f.b.plain(BLOCK, x.item_block);
    f.inst(Inst::Store {
        address: whole,
        value: s,
        version,
    });
    let copy = f.b.owned(BLOCK, x.item_block);
    let failure = f.propagates();
    f.inst(Inst::Copy {
        result: copy,
        address: whole,
        reaching: version,
        failure,
    });
    let c = x.c;
    let n = f.load(copy, &[ProjectionStep::Field(1)], U64, c.u64);
    f.print(n);
    f.release(copy, x.item);
    let back = f.b.owned(BLOCK, x.item_block);
    f.inst(Inst::Read {
        result: back,
        address: whole,
        reaching: version,
        mode: ReadMode::Take,
    });
    f.inst(Inst::StorageDead {
        slot,
        if_live: false,
    });
    f.release(back, x.item);
    f.ret(None);
    let main = f.finish(&table);
    let mut p = program(target, table, vec![main]);
    let mut bytes = 3u64.to_le_bytes().to_vec();
    bytes.extend(b"abc");
    bytes.extend(9u64.to_le_bytes());
    p.statics.push(StaticData::plain(
        Symbol {
            kind: SymbolKind::Constant,
            definition: path("ITEM"),
            instance: None,
        },
        x.item,
        bytes,
    ));
    let run = execute(&p);
    assert_eq!(output(&run), "9\n");
    let audit = run.result.unwrap();
    assert!(audit.exemptions.len() == 1, "{:?}", audit.exemptions);
}
