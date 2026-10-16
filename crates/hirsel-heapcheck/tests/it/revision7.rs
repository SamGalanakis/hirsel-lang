//! LIR schema revision 7 on the ledger: element slots read and written below the
//! capacity, `SeqReserve` and `SeqSetLen` on the unique heap block they write (a reserve
//! detaches a shared or static one), and
//! `Recast` back to the layout a block was built at. Each dynamic check has its failing
//! program and the clean twin one change away.
use crate::fixtures;

use fixtures::*;
use hirsel_heapcheck::build::{Layouts, path};
use hirsel_heapcheck::{Config, Program, Run, ViolationKind};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{LayoutId, LayoutKind, ScalarKind, SizeSpec, SizeTerm, TargetSpec};
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

/// `List[u64]`: a counted sequence payload behind a block, and the cursor's opaque
/// source (a block over no payload).
struct Seq {
    c: Common,
    payload: LayoutId,
    list: LayoutId,
    source: LayoutId,
}

fn layouts() -> (Layouts, Seq) {
    let mut l = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let c = common(&mut l);
    // Its count and capacity words (runtime ABI 5).
    let payload = l.record(&[c.u64, c.u64]);
    {
        let r = l.get_mut(payload);
        r.kind = LayoutKind::Sequence { element: c.u64 };
        r.size = SizeSpec::Dynamic(vec![
            SizeTerm::Constant(16),
            SizeTerm::ReadLength {
                offset: 0,
                scalar: ScalarKind::U64,
            },
            SizeTerm::Constant(8),
            SizeTerm::Multiply,
            SizeTerm::Add,
        ]);
        r.align = 8;
    }
    let list = l.block(payload);
    let unit = l.unit();
    let source = l.block(unit);
    (
        l,
        Seq {
            c,
            payload,
            list,
            source,
        },
    )
}

/// The slot holding the list, its root address and current version.
struct Held {
    list: LayoutId,
    slot: SlotId,
    at: AddressId,
    version: ValueId,
}

impl F<Rc> {
    fn list(&mut self, list: LayoutId, elements: &[u128]) -> ValueId {
        let values: Vec<_> = elements
            .iter()
            .enumerate()
            .map(|(i, &n)| (i as u32, self.int(n)))
            .collect();
        let k = self
            .b
            .construct(ConstructorKind::Sequence, list, &values, HEAP);
        let v = self.b.owned(BLOCK, list);
        let failure = self.propagates();
        self.inst(Inst::Construct {
            result: v,
            construct: k,
            failure,
        });
        v
    }

    fn hold(&mut self, list: LayoutId, value: ValueId) -> Held {
        let slot = self.b.slot(BLOCK, list, SlotKind::Local);
        self.inst(Inst::StorageLive { slot });
        let at = self.b.address(AddressBase::Slot(slot), &[], BLOCK, list);
        let failure = self.propagates();
        self.inst(Inst::Address {
            address: at,
            failure,
        });
        let version = self.b.plain(BLOCK, list);
        self.inst(Inst::Store {
            address: at,
            value,
            version,
        });
        Held {
            list,
            slot,
            at,
            version,
        }
    }

    fn element(&mut self, h: &Held, index: u128) -> AddressId {
        let u64 = self.c.u64;
        let i = self.int(index);
        let at = self.b.address(
            AddressBase::Slot(h.slot),
            &[ProjectionStep::Slot(i)],
            U64,
            u64,
        );
        let failure = self.propagates();
        self.inst(Inst::Address {
            address: at,
            failure,
        });
        at
    }

    fn read_slot(&mut self, h: &Held, index: u128) {
        let at = self.element(h, index);
        let v = self.b.plain(U64, self.c.u64);
        self.inst(Inst::Read {
            result: v,
            address: at,
            reaching: h.version,
            mode: ReadMode::Bits,
        });
        self.print(v);
    }

    fn write_slot(&mut self, h: &mut Held, index: u128, n: u128) {
        let at = self.element(h, index);
        let value = self.int(n);
        let list = h.list;
        h.version = self.b.plain(BLOCK, list);
        self.inst(Inst::Store {
            address: at,
            value,
            version: h.version,
        });
    }

    fn reserve(&mut self, h: &mut Held, capacity: u128) {
        let capacity = self.int(capacity);
        let list = h.list;
        h.version = self.b.plain(BLOCK, list);
        self.inst(Inst::SeqReserve {
            address: h.at,
            capacity,
            version: h.version,
        });
    }

    fn set_len(&mut self, h: &mut Held, len: u128) {
        let len = self.int(len);
        let list = h.list;
        h.version = self.b.plain(BLOCK, list);
        self.inst(Inst::SeqSetLen {
            address: h.at,
            len,
            version: h.version,
        });
    }

    /// Prints the list's capacity (`SeqCapacity` of a borrowed read).
    fn print_capacity(&mut self, h: &Held) {
        let list = h.list;
        let v = self.b.plain(BLOCK, list);
        self.inst(Inst::Read {
            result: v,
            address: h.at,
            reaching: h.version,
            mode: ReadMode::Borrow,
        });
        let n = self.b.plain(U64, self.c.u64);
        self.inst(Inst::SeqCapacity {
            result: n,
            value: v,
        });
        self.print(n);
    }

    /// Overwrites the list's capacity word (field 1 of its sequence payload) with `n`.
    fn write_capacity(&mut self, h: &mut Held, n: u128) {
        let at = self.b.address(
            AddressBase::Slot(h.slot),
            &[ProjectionStep::Field(1)],
            U64,
            self.c.u64,
        );
        let failure = self.propagates();
        self.inst(Inst::Address {
            address: at,
            failure,
        });
        // Taken first: a store writes only uninitialized bytes.
        let old = self.b.plain(U64, self.c.u64);
        self.inst(Inst::Read {
            result: old,
            address: at,
            reaching: h.version,
            mode: ReadMode::Take,
        });
        let value = self.int(n);
        let list = h.list;
        h.version = self.b.plain(BLOCK, list);
        self.inst(Inst::Store {
            address: at,
            value,
            version: h.version,
        });
    }

    /// Prints the count, takes the list out of its slot and releases it.
    fn finish_list(&mut self, h: Held, payload: LayoutId) {
        let list = h.list;
        let v = self.b.owned(BLOCK, list);
        self.inst(Inst::Read {
            result: v,
            address: h.at,
            reaching: h.version,
            mode: ReadMode::Take,
        });
        self.inst(Inst::StorageDead {
            slot: h.slot,
            if_live: false,
        });
        let n = self.b.plain(U64, self.c.u64);
        self.inst(Inst::SeqLen {
            result: n,
            value: v,
        });
        self.print(n);
        self.release(v, payload);
    }
}

/// `[7, 8]` in a slot, reserved to four slots, then `body`.
fn slotted(body: impl FnOnce(&mut F<Rc>, &mut Held)) -> Run {
    let (l, s) = layouts();
    let (target, table) = l.finish();
    let mut f = F::<Rc>::new("main", s.c);
    let xs = f.list(s.list, &[7, 8]);
    let mut h = f.hold(s.list, xs);
    f.reserve(&mut h, 4);
    body(&mut f, &mut h);
    f.finish_list(h, s.payload);
    f.ret(None);
    let main = f.finish(&table);
    execute(&program(target, table, vec![main]))
}

#[test]
fn a_slot_past_the_count_reads_only_once_written() {
    assert_eq!(
        output(&slotted(|f, h| {
            f.write_slot(h, 2, 9);
            f.read_slot(h, 2);
            f.set_len(h, 3);
        })),
        "9\n3\n"
    );
    assert_eq!(
        slotted(|f, h| f.read_slot(h, 2)).violation(),
        Some(ViolationKind::UninitializedRead)
    );
}

#[test]
fn a_slot_past_the_capacity_is_out_of_extent() {
    assert_eq!(output(&slotted(|f, h| f.read_slot(h, 1))), "8\n2\n");
    assert_eq!(
        slotted(|f, h| f.read_slot(h, 1 << 20)).violation(),
        Some(ViolationKind::OutOfExtent)
    );
}

#[test]
fn a_length_covers_exactly_the_initialized_slots() {
    // Over a slot never written.
    assert_eq!(
        slotted(|f, h| f.set_len(h, 3)).violation(),
        Some(ViolationKind::UninitializedRead)
    );
    // Abandoning a written one.
    assert_eq!(output(&slotted(|f, h| f.set_len(h, 2))), "2\n");
    assert_eq!(
        slotted(|f, h| f.set_len(h, 1)).violation(),
        Some(ViolationKind::LeakedSlot)
    );
    // Past the capacity.
    assert_eq!(
        slotted(|f, h| f.set_len(h, 1 << 20)).violation(),
        Some(ViolationKind::OutOfExtent)
    );
}

/// Runtime ABI 5: a construction's capacity word is its count, and every reserve
/// leaves it exact (all the slots of the class it lands in), as the runtime does.
#[test]
fn a_capacity_is_the_count_then_exact_after_each_reserve() {
    let (l, s) = layouts();
    let (target, table) = l.finish();
    let mut f = F::<Rc>::new("main", s.c);
    let xs = f.list(s.list, &[7, 8]);
    let mut h = f.hold(s.list, xs);
    f.print_capacity(&h);
    // 16 header bytes and four elements fill the 48-byte class exactly.
    f.reserve(&mut h, 4);
    f.print_capacity(&h);
    // Left alone: still exact.
    f.reserve(&mut h, 3);
    f.print_capacity(&h);
    f.finish_list(h, s.payload);
    f.ret(None);
    let main = f.finish(&table);
    assert_eq!(
        output(&execute(&program(target, table, vec![main]))),
        "2\n4\n4\n2\n"
    );
}

/// `SeqCapacity` checks the capacity word against the storage: a word naming
/// more elements than the block holds would let a push write past it. The twin
/// writes a word within the slots.
#[test]
fn a_capacity_word_beyond_the_slots_is_out_of_extent() {
    let forged = |n| {
        slotted(move |f, h| {
            f.write_capacity(h, n);
            f.print_capacity(h);
        })
    };
    assert_eq!(output(&forged(3)), "3\n2\n");
    assert_eq!(forged(5).violation(), Some(ViolationKind::OutOfExtent));
}

#[test]
fn a_reserve_keeps_the_elements_and_frees_the_old_storage() {
    assert_eq!(
        output(&slotted(|f, h| {
            f.reserve(h, 100);
            f.read_slot(h, 0);
            f.read_slot(h, 1);
        })),
        "7\n8\n2\n"
    );
}

/// A sequence write of `[7, 8]` while a second holder shares it (`shared`), or
/// (`static_list`) of a static list; the twin writes a private heap list.
fn shared_write(set_len: bool, shared: bool, static_list: bool) -> Run {
    let (l, s) = layouts();
    let (target, table) = l.finish();
    let mut f = F::<Rc>::new("main", s.c);
    let xs = if static_list {
        let symbol = f.b.symbol(SymbolKind::Constant, path("XS"));
        let v = f.b.owned(BLOCK, s.list);
        f.inst(Inst::ReadStatic { result: v, symbol });
        v
    } else {
        f.list(s.list, &[7, 8])
    };
    if shared {
        f.inst(Inst::Dup { value: xs });
    }
    let mut h = f.hold(s.list, xs);
    if set_len {
        f.set_len(&mut h, 2);
    } else {
        f.reserve(&mut h, 8);
    }
    f.finish_list(h, s.payload);
    if shared {
        f.release(xs, s.payload);
    }
    f.ret(None);
    let main = f.finish(&table);
    let mut p = program(target, table, vec![main]);
    // Count 2, capacity 2 (runtime ABI 5), then [7, 8].
    let mut bytes = 2u64.to_le_bytes().to_vec();
    bytes.extend(2u64.to_le_bytes());
    bytes.extend(7u64.to_le_bytes());
    bytes.extend(8u64.to_le_bytes());
    p.statics.push(StaticData::plain(
        Symbol {
            kind: SymbolKind::Constant,
            definition: path("XS"),
            instance: None,
        },
        s.payload,
        bytes,
    ));
    execute(&p)
}

#[test]
fn a_length_write_needs_a_unique_heap_block() {
    assert_eq!(output(&shared_write(true, false, false)), "2\n");
    assert_eq!(
        shared_write(true, true, false).violation(),
        Some(ViolationKind::WriteShared)
    );
    assert!(matches!(
        shared_write(true, false, true).violation(),
        Some(ViolationKind::WriteShared | ViolationKind::WritePublished)
    ));
}

/// A reserve of a shared or static list detaches it at the requested
/// capacity: the place holds a unique copy of `[7, 8]`, the other holder's
/// credit and the static data stay intact, and nothing leaks.
#[test]
fn a_reserve_detaches_a_shared_or_static_block() {
    for (shared, static_list) in [(false, false), (true, false), (false, true)] {
        let run = shared_write(false, shared, static_list);
        assert_eq!(run.violation(), None, "{shared} {static_list}");
        assert_eq!(output(&run), "2\n");
    }
}

/// Element 1 of `[7, 8]` located, then a reserve to `capacity`, then a read
/// through the address located before it (or, `again`, located after it).
fn read_across_reserve(capacity: u128, again: bool) -> Run {
    slotted(|f, h| {
        let before = f.element(h, 1);
        f.reserve(h, capacity);
        let at = if again { f.element(h, 1) } else { before };
        let v = f.b.plain(U64, f.c.u64);
        f.inst(Inst::Read {
            result: v,
            address: at,
            reaching: h.version,
            mode: ReadMode::Bits,
        });
        f.print(v);
    })
}

/// Native code keeps the location an address's preparation found
/// (`hirsel-llvm` `prepare`). A reserve that moves the elements leaves an
/// address located before it naming the freed block, so H refuses the use;
/// located again it reads the moved element, and a reserve within the
/// capacity moves nothing.
#[test]
fn an_element_located_before_a_moving_reserve_is_stale() {
    assert_eq!(output(&read_across_reserve(100, true)), "8\n2\n");
    assert_eq!(
        read_across_reserve(100, false).violation(),
        Some(ViolationKind::StaleAddress)
    );
    assert_eq!(output(&read_across_reserve(3, false)), "8\n2\n");
}

/// `[7, 8]` recast to the opaque source and back to the list (`right`) or to a box of a
/// cell, then counted or read.
fn recast(right: bool) -> Run {
    let (l, s) = layouts();
    let (target, table) = l.finish();
    let boxed = s.c.boxed;
    let mut f = F::<Rc>::new("main", s.c);
    let xs = f.list(s.list, &[7, 8]);
    let opaque = f.b.plain(BLOCK, s.source);
    f.inst(Inst::Recast {
        result: opaque,
        value: xs,
    });
    let back = f.b.plain(BLOCK, if right { s.list } else { boxed });
    f.inst(Inst::Recast {
        result: back,
        value: opaque,
    });
    if right {
        let n = f.b.plain(U64, f.c.u64);
        f.inst(Inst::SeqLen {
            result: n,
            value: back,
        });
        f.print(n);
    }
    f.release(xs, s.payload);
    f.ret(None);
    let main = f.finish(&table);
    execute(&program(target, table, vec![main]))
}

#[test]
fn a_recast_returns_to_the_layout_the_block_was_built_at() {
    assert_eq!(output(&recast(true)), "2\n");
    assert_eq!(
        recast(false).violation(),
        Some(ViolationKind::TypeConfusion)
    );
}

fn moved_recast(foreign: bool, reused: bool) -> Run {
    let (mut l, s) = layouts();
    let alias = l.block(s.payload);
    let (target, table) = l.finish();
    let mut f = F::<Rc>::new("main", s.c);
    let xs = f.list(s.list, &[7, 8]);
    let result = f.b.owned(BLOCK, if foreign { s.c.boxed } else { alias });
    f.inst(Inst::Recast { result, value: xs });
    let n = f.b.plain(U64, f.c.u64);
    f.inst(Inst::SeqLen {
        result: n,
        value: if reused { xs } else { result },
    });
    f.print(n);
    f.release(result, s.payload);
    f.ret(None);
    let unit = f.finish(&table);
    execute(&program(target, table, vec![unit]))
}

#[test]
fn an_owned_recast_moves_one_credit_and_checks_the_payload_descriptor() {
    assert_eq!(output(&moved_recast(false, false)), "2\n");
    assert_eq!(
        moved_recast(true, false).violation(),
        Some(ViolationKind::TypeConfusion)
    );
    assert_eq!(
        moved_recast(false, true).violation(),
        Some(ViolationKind::CreditUnderflow)
    );
}

#[test]
fn a_slot_address_has_no_language_failure_site_but_index_does() {
    let probe = |checked: bool| {
        let (l, s) = layouts();
        let (target, table) = l.finish();
        let mut f = F::<Rc>::new("main", s.c);
        let xs = f.list(s.list, &[7, 8]);
        let h = f.hold(s.list, xs);
        let index = f.int(1);
        let at = f.b.address(
            AddressBase::Slot(h.slot),
            &[if checked {
                ProjectionStep::Index(index)
            } else {
                ProjectionStep::Slot(index)
            }],
            U64,
            s.c.u64,
        );
        let failure = f.propagates();
        f.inst(Inst::Address {
            address: at,
            failure,
        });
        let result = f.b.plain(U64, s.c.u64);
        f.inst(Inst::Read {
            result,
            address: at,
            reaching: h.version,
            mode: ReadMode::Bits,
        });
        f.print(result);
        f.finish_list(h, s.payload);
        f.ret(None);
        let main = f.finish(&table);
        execute(&program(target, table, vec![main]))
    };
    let slot = probe(false);
    let index = probe(true);
    assert_eq!(output(&slot), output(&index));
    assert_eq!(slot.sites, 0);
    assert_eq!(index.sites, 1);
    assert_eq!(
        slotted(|f, h| f.read_slot(h, 1 << 20)).violation(),
        Some(ViolationKind::OutOfExtent)
    );
}

/// A (u32, u64) slot has four unwritten padding bytes. Initialization
/// requires both fields and permits those padding bytes to stay unwritten.
fn padded_sequence(missing_field: bool) -> Run {
    let (mut l, s) = layouts();
    let small = l.scalar(ScalarKind::U32);
    let pair = l.record(&[small, s.c.u64]);
    l.get_mut(s.payload).kind = LayoutKind::Sequence { element: pair };
    if let SizeSpec::Dynamic(program) = &mut l.get_mut(s.payload).size {
        program[2] = SizeTerm::Constant(16);
    }
    let (target, table) = l.finish();
    let mut f = F::<Rc>::new("main", s.c);
    let value = f.list(s.list, &[]);
    let mut h = f.hold(s.list, value);
    f.reserve(&mut h, 1);
    let repr = ValueRepr::Scalar(ScalarKind::U32);
    let constant = f.b.constant(repr, small, ConstantValue::Integer(7));
    let first = f.b.plain(repr, small);
    f.inst(Inst::Const {
        result: first,
        constant,
    });
    let second = f.int(9);
    let fields = if missing_field {
        vec![(0, first)]
    } else {
        vec![(0, first), (1, second)]
    };
    let construct = f.b.construct(ConstructorKind::Tuple, pair, &fields, HEAP);
    let value = f.b.plain(ValueRepr::Aggregate, pair);
    let failure = f.propagates();
    f.inst(Inst::Construct {
        result: value,
        construct,
        failure,
    });
    let zero = f.int(0);
    let at = f.b.address(
        AddressBase::Slot(h.slot),
        &[ProjectionStep::Slot(zero)],
        ValueRepr::Aggregate,
        pair,
    );
    let failure = f.propagates();
    f.inst(Inst::Address {
        address: at,
        failure,
    });
    h.version = f.b.plain(BLOCK, s.list);
    f.inst(Inst::Store {
        address: at,
        value,
        version: h.version,
    });
    f.set_len(&mut h, 1);
    f.finish_list(h, s.payload);
    f.ret(None);
    let unit = f.finish(&table);
    execute(&program(target, table, vec![unit]))
}

#[test]
fn padded_element_fields_are_required_and_padding_is_unspecified() {
    assert_eq!(output(&padded_sequence(false)), "1\n");
    assert_eq!(
        padded_sequence(true).violation(),
        Some(ViolationKind::UninitializedRead)
    );
}

/// A field copy duplicates only the owning scan cell and preserves the
/// uncounted field's source permission, even when that target has a finite count.
fn copied_record_with_a_borrowed_field(expired: bool) -> Run {
    use hirsel_lir::layout_types::{CopyRecipe, DropStep, ScanStep};
    let (mut l, s) = layouts();
    let mixed = l.record(&[s.c.boxed, s.c.boxed]);
    let r = l.get_mut(mixed);
    r.copy = CopyRecipe::Fields;
    r.scan = vec![
        ScanStep::Uncounted { offset: 0 },
        ScanStep::Counted { offset: 8 },
    ];
    r.drop = vec![DropStep::Release { offset: 8 }];
    let (target, table) = l.finish();
    let mut f = F::<Rc>::new("main", s.c);
    let n = f.int(42);
    let source = f.boxed(n);
    let source = f.hold(s.c.boxed, source);
    let loan = f.b.loan(source.at, false);
    f.inst(Inst::BeginLoan {
        loan,
        address: source.at,
        mutable: false,
        if_inactive: false,
    });
    let view = f.b.value(BLOCK, s.c.boxed, Holding::Borrowed(loan));
    f.inst(Inst::Read {
        result: view,
        address: source.at,
        reaching: source.version,
        mode: ReadMode::Borrow,
    });
    let owned = f.boxed(n);
    let construct = f.b.construct(
        ConstructorKind::Tuple,
        mixed,
        &[(0, view), (1, owned)],
        HEAP,
    );
    let value = f.b.owned(ValueRepr::Aggregate, mixed);
    let failure = f.propagates();
    f.inst(Inst::Construct {
        result: value,
        construct,
        failure,
    });
    let slot = f.b.slot(ValueRepr::Aggregate, mixed, SlotKind::Local);
    let at =
        f.b.address(AddressBase::Slot(slot), &[], ValueRepr::Aggregate, mixed);
    let failure = f.propagates();
    f.inst(Inst::Address {
        address: at,
        failure,
    });
    f.inst(Inst::StorageLive { slot });
    let version = f.b.plain(ValueRepr::Aggregate, mixed);
    f.inst(Inst::Store {
        address: at,
        value,
        version,
    });
    let copied = f.b.owned(ValueRepr::Aggregate, mixed);
    let failure = f.propagates();
    f.inst(Inst::Copy {
        result: copied,
        address: at,
        reaching: version,
        failure,
    });
    let copied_slot = f.b.slot(ValueRepr::Aggregate, mixed, SlotKind::Local);
    let copied_at = f.b.address(
        AddressBase::Slot(copied_slot),
        &[],
        ValueRepr::Aggregate,
        mixed,
    );
    let failure = f.propagates();
    f.inst(Inst::Address {
        address: copied_at,
        failure,
    });
    f.inst(Inst::StorageLive { slot: copied_slot });
    let copied_version = f.b.plain(ValueRepr::Aggregate, mixed);
    f.inst(Inst::Store {
        address: copied_at,
        value: copied,
        version: copied_version,
    });
    if expired {
        f.inst(Inst::EndLoan {
            loan,
            if_active: false,
        });
    }
    let view_at = f.b.address(
        AddressBase::Slot(copied_slot),
        &[ProjectionStep::Field(0)],
        BLOCK,
        s.c.boxed,
    );
    let failure = f.propagates();
    f.inst(Inst::Address {
        address: view_at,
        failure,
    });
    let copied_view = f.b.plain(BLOCK, s.c.boxed);
    f.inst(Inst::Read {
        result: copied_view,
        address: view_at,
        reaching: copied_version,
        mode: ReadMode::Bits,
    });
    let n = f.unbox(copied_view);
    f.print(n);
    let copied = f.b.owned(ValueRepr::Aggregate, mixed);
    f.inst(Inst::Read {
        result: copied,
        address: copied_at,
        reaching: copied_version,
        mode: ReadMode::Take,
    });
    f.release(copied, mixed);
    let original = f.b.owned(ValueRepr::Aggregate, mixed);
    f.inst(Inst::Read {
        result: original,
        address: at,
        reaching: version,
        mode: ReadMode::Take,
    });
    f.release(original, mixed);
    if !expired {
        f.inst(Inst::EndLoan {
            loan,
            if_active: false,
        });
    }
    let source_value = f.b.owned(BLOCK, s.c.boxed);
    f.inst(Inst::Read {
        result: source_value,
        address: source.at,
        reaching: source.version,
        mode: ReadMode::Take,
    });
    f.inst(Inst::StorageDead {
        slot: source.slot,
        if_live: false,
    });
    f.release(source_value, s.c.cell);
    f.ret(None);
    let unit = f.finish(&table);
    execute(&program(target, table, vec![unit]))
}

#[test]
fn copies_own_their_counted_fields_and_keep_borrowed_field_permissions() {
    assert_eq!(output(&copied_record_with_a_borrowed_field(false)), "42\n");
    assert_eq!(
        copied_record_with_a_borrowed_field(true).violation(),
        Some(ViolationKind::BorrowExpired)
    );
}

/// A scalar copy or borrowed read of an entry must leave the entry initialized,
/// including after the sequence has been detached from a second holder.
fn plain_entry_after_detach(mode: Option<ReadMode>, deep: bool, missing: bool) -> Run {
    let (l, s) = layouts();
    let (target, table) = l.finish();
    let mut f = F::<Rc>::new("main", s.c);
    // Three elements past the count and capacity leave one spare slot in the class.
    let original = f.list(s.list, &[7, 8, 9]);
    let construct = f.b.construct(ConstructorKind::Tuple, s.payload, &[], HEAP);
    let detached = f.b.owned(BLOCK, s.list);
    let failure = f.propagates();
    f.inst(Inst::UniqueCopy {
        result: detached,
        value: original,
        construct,
        deep,
        failure,
    });
    let mut h = f.hold(s.list, detached);
    let at = f.element(&h, if missing { 3 } else { 0 });
    let value = f.b.plain(U64, s.c.u64);
    if let Some(mode) = mode {
        f.inst(Inst::Read {
            result: value,
            address: at,
            reaching: h.version,
            mode,
        });
    } else {
        let failure = f.propagates();
        f.inst(Inst::Copy {
            result: value,
            address: at,
            reaching: h.version,
            failure,
        });
    }
    f.print(value);
    f.set_len(&mut h, 3);
    f.read_slot(&h, 0);
    f.finish_list(h, s.payload);
    f.release(original, s.payload);
    f.ret(None);
    let main = f.finish(&table);
    execute(&program(target, table, vec![main]))
}

#[test]
fn plain_entry_reads_and_copies_preserve_initialization_after_detach() {
    for deep in [false, true] {
        for mode in [None, Some(ReadMode::Bits), Some(ReadMode::Borrow)] {
            assert_eq!(
                output(&plain_entry_after_detach(mode, deep, false)),
                "7\n7\n3\n"
            );
            assert_eq!(
                plain_entry_after_detach(mode, deep, true).violation(),
                Some(ViolationKind::UninitializedRead)
            );
        }
    }
}

/// A one-element sequence construction requests its count and capacity words and
/// that element. The second append uses spare class capacity without a reserve or
/// a new allocation. Copies must keep those newly initialized bytes, including in
/// a nested deep copy.
fn copied_grown_sequence(deep: bool, nested: bool, missing: bool) -> Run {
    let (mut l, s) = layouts();
    let parent = l.record(&[s.list]);
    let parent_block = l.block(parent);
    let (target, table) = l.finish();
    let mut f = F::<Rc>::new("main", s.c);
    let first = f.list(s.list, &[5]);
    let mut h = f.hold(s.list, first);
    if !missing {
        f.write_slot(&mut h, 1, 7);
        f.set_len(&mut h, 2);
    }
    let source = f.b.owned(BLOCK, s.list);
    f.inst(Inst::Read {
        result: source,
        address: h.at,
        reaching: h.version,
        mode: ReadMode::Take,
    });
    f.inst(Inst::StorageDead {
        slot: h.slot,
        if_live: false,
    });
    let (source, layout, payload) = if nested {
        let construct =
            f.b.construct(ConstructorKind::Tuple, parent_block, &[(0, source)], HEAP);
        let value = f.b.owned(BLOCK, parent_block);
        let failure = f.propagates();
        f.inst(Inst::Construct {
            result: value,
            construct,
            failure,
        });
        (value, parent_block, parent)
    } else {
        (source, s.list, s.payload)
    };
    let construct = f.b.construct(ConstructorKind::Tuple, payload, &[], HEAP);
    let copy = f.b.owned(BLOCK, layout);
    let failure = f.propagates();
    f.inst(Inst::UniqueCopy {
        result: copy,
        value: source,
        construct,
        deep,
        failure,
    });
    let mut h = f.hold(layout, copy);
    let index = f.int(1);
    let steps = if nested {
        vec![ProjectionStep::Field(0), ProjectionStep::Slot(index)]
    } else {
        vec![ProjectionStep::Slot(index)]
    };
    let at = f.b.address(AddressBase::Slot(h.slot), &steps, U64, s.c.u64);
    let failure = f.propagates();
    f.inst(Inst::Address {
        address: at,
        failure,
    });
    let value = f.b.plain(U64, s.c.u64);
    f.inst(Inst::Read {
        result: value,
        address: at,
        reaching: h.version,
        mode: ReadMode::Bits,
    });
    f.print(value);
    if !nested {
        let displaced = f.b.slot(U64, s.c.u64, SlotKind::Displaced);
        f.inst(Inst::StorageLive { slot: displaced });
        let old_at = f.b.address(AddressBase::Slot(displaced), &[], U64, s.c.u64);
        let failure = f.propagates();
        f.inst(Inst::Address {
            address: old_at,
            failure,
        });
        let value = f.int(9);
        h.version = f.b.plain(BLOCK, layout);
        let displaced_version = f.b.plain(U64, s.c.u64);
        let failure = f.propagates();
        f.inst(Inst::Replace {
            address: at,
            value,
            displaced,
            version: h.version,
            displaced_version,
            if_live: false,
            failure,
        });
        let old = f.b.plain(U64, s.c.u64);
        f.inst(Inst::Read {
            result: old,
            address: old_at,
            reaching: displaced_version,
            mode: ReadMode::Take,
        });
        f.print(old);
        f.inst(Inst::StorageDead {
            slot: displaced,
            if_live: false,
        });
        f.set_len(&mut h, 2);
        f.read_slot(&h, 1);
    }
    let copied = f.b.owned(BLOCK, layout);
    f.inst(Inst::Read {
        result: copied,
        address: h.at,
        reaching: h.version,
        mode: ReadMode::Take,
    });
    f.inst(Inst::StorageDead {
        slot: h.slot,
        if_live: false,
    });
    f.release(copied, payload);
    f.release(source, payload);
    f.ret(None);
    let main = f.finish(&table);
    execute(&program(target, table, vec![main]))
}

#[test]
fn block_copies_keep_elements_appended_within_spare_capacity() {
    for (deep, nested) in [(false, false), (true, false), (false, true), (true, true)] {
        assert_eq!(
            output(&copied_grown_sequence(deep, nested, false)),
            if nested { "7\n" } else { "7\n7\n9\n" }
        );
        assert_eq!(
            copied_grown_sequence(deep, nested, true).violation(),
            Some(ViolationKind::UninitializedRead)
        );
    }
}
