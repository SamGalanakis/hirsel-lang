//! LIR schema revision 3 on the ledger: structural comparison in the
//! interpreter's order with its Depth bound, the uniqueness test against the
//! ledger's counts, and callable weakening moving the holder credit.
use crate::fixtures;

use fixtures::*;
use hirsel_heapcheck::build::{Layouts, path};
use hirsel_heapcheck::{Config, Program, Run, ViolationKind};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{
    CopyRecipe, FieldLayout, LayoutId, LayoutKind, ScalarKind, SizeSpec, SizeTerm, TargetSpec,
};
use hirsel_lir::stage::{Rc, Stage};
use hirsel_lir::statics::StaticData;

fn execute<S: Stage>(p: &Program<S>) -> Run {
    run(p, &path("main"), &[], &mut Printer, &Config::default())
}

fn output(run: &Run) -> String {
    let audit = run
        .result
        .as_ref()
        .unwrap_or_else(|v| panic!("expected a clean audit: {v}"));
    String::from_utf8(audit.observation.output.clone()).unwrap()
}

impl<S: Stage> F<S> {
    fn compare(&mut self, op: CompareOp, left: ValueId, right: ValueId, layout: LayoutId) {
        let result = self.b.plain(BOOL, self.c.bool);
        let failure = self.raises("MEM-3", self.propagate);
        self.inst(Inst::Compare {
            result,
            op,
            left,
            right,
            layout,
            failure,
        });
        self.print(result);
    }

    /// An inline value of `layout` from `fields`.
    fn inline(&mut self, layout: LayoutId, ctor: ConstructorKind, fields: &[ValueId]) -> ValueId {
        let fields: Vec<_> = fields
            .iter()
            .enumerate()
            .map(|(i, &v)| (i as u32, v))
            .collect();
        let k = self.b.construct(ctor, layout, &fields, HEAP);
        let v = self.b.plain(ValueRepr::Aggregate, layout);
        let failure = self.propagates();
        self.inst(Inst::Construct {
            result: v,
            construct: k,
            failure,
        });
        v
    }

    fn float(&mut self, layout: LayoutId, bits: u64) -> ValueId {
        let repr = ValueRepr::Scalar(ScalarKind::F64);
        let k = self.b.constant(repr, layout, ConstantValue::F64(bits));
        let v = self.b.plain(repr, layout);
        self.inst(Inst::Const {
            result: v,
            constant: k,
        });
        v
    }
}

/// Boxes compare by payload, records field by field, variants by tag ordinal
/// first, floats by IEEE with an unordered pair making all but `Ne` false.
#[test]
fn structural_comparison_follows_the_interpreter_order() {
    let mut l = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let c = common(&mut l);
    let pair = l.record(&[c.u64, c.u64]);
    let option = l.variant(&[&[], &[c.u64]]);
    let f64 = l.scalar(ScalarKind::F64);
    let real = l.record(&[f64]);
    let (target, table) = l.finish();
    let mut f = F::<Rc>::new("main", c);
    let three = f.int(3);
    let five = f.int(5);
    let a = f.boxed(three);
    let b = f.boxed(five);
    f.compare(CompareOp::Lt, a, b, c.boxed);
    f.compare(CompareOp::Eq, a, b, c.boxed);
    f.compare(CompareOp::Ne, a, b, c.boxed);
    f.compare(CompareOp::Ge, a, b, c.boxed);
    f.compare(CompareOp::Eq, a, a, c.boxed);
    f.release(a, c.boxed);
    f.release(b, c.boxed);
    // The first unequal field decides: (3, 5) < (5, 3).
    let x = f.inline(pair, ConstructorKind::Tuple, &[three, five]);
    let y = f.inline(pair, ConstructorKind::Tuple, &[five, three]);
    f.compare(CompareOp::Lt, x, y, pair);
    f.compare(CompareOp::Gt, x, y, pair);
    // A lower tag ordinal orders first whatever the payload.
    let symbol = f.b.symbol(SymbolKind::Type, path("Option"));
    let none = f.inline(
        option,
        ConstructorKind::Variant {
            definition: symbol,
            variant: 0,
        },
        &[],
    );
    let some = f.inline(
        option,
        ConstructorKind::Variant {
            definition: symbol,
            variant: 1,
        },
        &[three],
    );
    f.compare(CompareOp::Lt, none, some, option);
    f.compare(CompareOp::Le, some, some, option);
    let nan = f.float(f64, f64::NAN.to_bits());
    let p = f.inline(real, ConstructorKind::Tuple, &[nan]);
    f.compare(CompareOp::Eq, p, p, real);
    f.compare(CompareOp::Ne, p, p, real);
    f.compare(CompareOp::Ge, p, p, real);
    let zero = f.float(f64, 0.0f64.to_bits());
    let minus = f.float(f64, (-0.0f64).to_bits());
    let z = f.inline(real, ConstructorKind::Tuple, &[zero]);
    let m = f.inline(real, ConstructorKind::Tuple, &[minus]);
    f.compare(CompareOp::Eq, z, m, real);
    f.ret(None);
    let main = f.finish(&table);
    let run = execute(&program(target, table, vec![main]));
    assert_eq!(output(&run), "1\n0\n1\n0\n1\n1\n0\n1\n1\n0\n1\n0\n1\n");
}

/// Records nested `wrappers` deep around a u64: the leaf pair is at depth
/// `wrappers + 1`, and a pair deeper than `COMPARE_MAX_DEPTH` is MEM-3 Depth.
fn nested(wrappers: u64) -> Run {
    let mut l = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let c = common(&mut l);
    let mut layouts = vec![l.record(&[c.u64])];
    for _ in 0..wrappers {
        let inner = *layouts.last().unwrap();
        layouts.push(l.record(&[inner]));
    }
    let (target, table) = l.finish();
    let mut f = F::<Rc>::new("main", c);
    let mut v = f.int(7);
    for &layout in &layouts {
        v = f.inline(layout, ConstructorKind::Tuple, &[v]);
    }
    f.compare(CompareOp::Eq, v, v, *layouts.last().unwrap());
    f.ret(None);
    let main = f.finish(&table);
    execute(&program(target, table, vec![main]))
}

#[test]
fn a_comparison_fails_with_depth_past_the_interpreter_bound() {
    let limit = COMPARE_MAX_DEPTH;
    assert_eq!(output(&nested(limit - 1)), "1\n");
    let deep = nested(limit);
    let audit = deep
        .result
        .as_ref()
        .expect("a language failure audits cleanly");
    assert_eq!(audit.observation.output, b"");
    assert_eq!(audit.observation.failures.len(), 1);
    assert_eq!(audit.observation.failures[0].rule, "MEM-3");
}

/// The ledger's count decides: one holder is unique, a second holder or an
/// immortal static block is not.
#[test]
fn the_uniqueness_test_reads_the_ledger_count() {
    let mut l = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let c = common(&mut l);
    let (target, table) = l.finish();
    let mut f = F::<Rc>::new("main", c);
    let unique = |f: &mut F<Rc>, v: ValueId| {
        let r = f.b.plain(BOOL, f.c.bool);
        f.inst(Inst::IsUnique {
            result: r,
            value: v,
        });
        f.print(r);
    };
    let one = f.int(1);
    let a = f.boxed(one);
    unique(&mut f, a);
    f.inst(Inst::Dup { value: a });
    unique(&mut f, a);
    f.release(a, c.boxed);
    unique(&mut f, a);
    f.release(a, c.boxed);
    let k =
        f.b.constant(BLOCK, c.boxed, ConstantValue::Bytes(vec![9; 8]));
    let s = f.b.plain(BLOCK, c.boxed);
    f.inst(Inst::Const {
        result: s,
        constant: k,
    });
    unique(&mut f, s);
    f.ret(None);
    let main = f.finish(&table);
    assert_eq!(
        output(&execute(&program(target, table, vec![main]))),
        "1\n0\n1\n0\n"
    );
}

/// A closure weakened to its once-call layout carries the environment's
/// credit; the many-call value has none left.
fn weakened(reuse_original: bool) -> Run {
    let mut l = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let c = common(&mut l);
    let env = l.record(&[c.boxed]);
    let many = l.callable(Some(env));
    let once = l.callable(Some(env));
    l.get_mut(once).copy = CopyRecipe::NotCopyable;
    let (target, table) = l.finish();

    let mut get = F::<Rc>::new("get", c);
    let cap = get.b.slot(BLOCK, c.boxed, SlotKind::Capture(0));
    let environment = get.b.unit.captures[0];
    let at = get.b.address(
        AddressBase::Slot(cap),
        &[ProjectionStep::Field(0)],
        U64,
        c.u64,
    );
    let v = get.b.plain(U64, c.u64);
    let failure = get.propagates();
    get.inst(Inst::Address {
        address: at,
        failure,
    });
    get.inst(Inst::Read {
        result: v,
        address: at,
        reaching: environment,
        mode: ReadMode::Bits,
    });
    get.print(v);
    get.ret(None);
    let get = get.finish(&table);

    let mut f = F::<Rc>::new("main", c);
    let n = f.int(42);
    let x = f.boxed(n);
    let k =
        f.b.closure(path("get"), env, &[(x, CaptureUse::Take)], HEAP);
    let fun = f.b.owned(ValueRepr::Callable, many);
    let failure = f.propagates();
    f.inst(Inst::Closure {
        result: fun,
        closure: k,
        failure,
    });
    let one = f.b.owned(ValueRepr::Callable, once);
    f.inst(Inst::Weaken {
        result: one,
        value: fun,
    });
    f.call(Callee::Indirect(one), &[], None);
    f.release(one, once);
    if reuse_original {
        f.release(fun, many);
    }
    f.ret(None);
    let main = f.finish(&table);
    execute(&program(target, table, vec![main, get]))
}

#[test]
fn weakening_moves_the_holder_credit() {
    let run = weakened(false);
    assert_eq!(output(&run), "42\n");
    assert_eq!(run.counters.frees, 2);
    assert_eq!(
        weakened(true).violation(),
        Some(ViolationKind::CreditUnderflow)
    );
}

/// `flat type Item { text: String, n: u64 }` holding `("flat", 7)`: the u64
/// prefix 4, the four bytes, then `n`. `n` lies at its cursor, offset
/// 8 + 4 = 12, not at its static offset 8.
#[test]
fn a_flat_field_lies_at_its_cursor() {
    let mut l = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let c = common(&mut l);
    let dynamic = || {
        SizeSpec::Dynamic(vec![
            SizeTerm::Constant(8),
            SizeTerm::ReadLength {
                offset: 0,
                scalar: ScalarKind::U64,
            },
            SizeTerm::Add,
        ])
    };
    let byte = l.scalar(ScalarKind::U8);
    let text = l.record(&[byte]);
    let item = l.record(&[text, c.u64]);
    {
        let r = l.get_mut(text);
        r.kind = LayoutKind::Flat;
        r.size = dynamic();
        r.align = 1;
        r.fields[0].offset = 8;
    }
    {
        let r = l.get_mut(item);
        r.kind = LayoutKind::Flat;
        r.size = dynamic();
        r.align = 1;
        r.fields = vec![
            FieldLayout {
                ordinal: 0,
                offset: 0,
                layout: text,
                unaligned: false,
            },
            FieldLayout {
                ordinal: 1,
                offset: 8,
                layout: c.u64,
                unaligned: true,
            },
        ];
    }
    let held = l.block(item);
    let (target, table) = l.finish();
    let mut f = F::<Rc>::new("main", c);
    let sym = f.b.symbol(SymbolKind::Constant, path("ITEM"));
    let s = f.b.owned(BLOCK, held);
    f.inst(Inst::ReadStatic {
        result: s,
        symbol: sym,
    });
    let n = f.load(s, &[ProjectionStep::Field(1)], U64, c.u64);
    f.print(n);
    f.release(s, held);
    f.ret(None);
    let main = f.finish(&table);
    let mut p = program(target, table, vec![main]);
    let mut bytes = 4u64.to_le_bytes().to_vec();
    bytes.extend(b"flat");
    bytes.extend(7u64.to_le_bytes());
    p.statics.push(StaticData::plain(
        Symbol {
            kind: SymbolKind::Constant,
            definition: path("ITEM"),
            instance: None,
        },
        item,
        bytes,
    ));
    assert_eq!(output(&execute(&p)), "7\n");
}

/// A closure borrowing a box: the environment field is the borrowed block
/// reference itself, scanned as uncounted and never released by the
/// environment (LIR schema revision 3). Read as owned (`owned_field`), the
/// environment's drop releases a credit it never held.
fn borrowed_capture(owned_field: bool) -> Run {
    use hirsel_lir::layout_types::ScanStep;
    let mut l = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let c = common(&mut l);
    let env = l.record(&[c.boxed]);
    if !owned_field {
        let r = l.get_mut(env);
        r.scan = vec![ScanStep::Uncounted { offset: 0 }];
        r.drop = Vec::new();
        r.copy = CopyRecipe::Bits;
    }
    let many = l.callable(Some(env));
    let (target, table) = l.finish();

    let mut get = F::<Rc>::new("get", c);
    let cap = get.b.slot(BLOCK, c.boxed, SlotKind::Capture(0));
    let environment = get.b.unit.captures[0];
    let at = get.b.address(
        AddressBase::Slot(cap),
        &[ProjectionStep::Field(0)],
        U64,
        c.u64,
    );
    let v = get.b.plain(U64, c.u64);
    let failure = get.propagates();
    get.inst(Inst::Address {
        address: at,
        failure,
    });
    get.inst(Inst::Read {
        result: v,
        address: at,
        reaching: environment,
        mode: ReadMode::Bits,
    });
    get.print(v);
    get.ret(None);
    let get = get.finish(&table);

    let mut f = F::<Rc>::new("main", c);
    let n = f.int(42);
    let x = f.boxed(n);
    let k =
        f.b.closure(path("get"), env, &[(x, CaptureUse::Borrow)], HEAP);
    let fun = f.b.owned(ValueRepr::Callable, many);
    let failure = f.propagates();
    f.inst(Inst::Closure {
        result: fun,
        closure: k,
        failure,
    });
    f.call(Callee::Indirect(fun), &[], None);
    f.release(fun, many);
    f.release(x, c.boxed);
    f.ret(None);
    let main = f.finish(&table);
    execute(&program(target, table, vec![main, get]))
}

#[test]
fn a_borrowed_capture_is_the_view_in_its_environment() {
    let run = borrowed_capture(false);
    assert_eq!(output(&run), "42\n");
    assert_eq!(run.counters.frees, 2);
    assert!(borrowed_capture(true).violation().is_some());
}

fn expired_view_on_an_edge(read_after: bool) -> Run {
    let mut layouts = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let c = common(&mut layouts);
    let (target, table) = layouts.finish();
    let mut f = F::<Rc>::new("main", c);
    let n = f.int(42);
    let owner = f.boxed(n);
    let slot = f.b.slot(BLOCK, c.boxed, SlotKind::Local);
    let at = f.b.address(AddressBase::Slot(slot), &[], BLOCK, c.boxed);
    let version = f.b.plain(BLOCK, c.boxed);
    f.inst(Inst::StorageLive { slot });
    f.inst(Inst::Store {
        address: at,
        value: owner,
        version,
    });
    let loan = f.b.loan(at, false);
    f.inst(Inst::BeginLoan {
        loan,
        address: at,
        mutable: false,
        if_inactive: false,
    });
    let view = f.b.value(BLOCK, c.boxed, Holding::Borrowed(loan));
    f.inst(Inst::Read {
        result: view,
        address: at,
        reaching: version,
        mode: ReadMode::Borrow,
    });
    f.inst(Inst::EndLoan {
        loan,
        if_active: false,
    });
    let parameter = f.b.value(BLOCK, c.boxed, Holding::Borrowed(loan));
    let next = f.b.block(BlockKind::Body, &[parameter]);
    let edge = f.b.edge(next, &[view]);
    f.b.term(f.cur, Terminator::Jump(edge));
    f.cur = next;
    if read_after {
        let value = f.unbox(parameter);
        f.print(value);
    }
    let owner = f.b.owned(BLOCK, c.boxed);
    f.inst(Inst::Read {
        result: owner,
        address: at,
        reaching: version,
        mode: ReadMode::Take,
    });
    f.release(owner, c.boxed);
    f.inst(Inst::StorageDead {
        slot,
        if_live: false,
    });
    f.ret(None);
    let unit = f.finish(&table);
    execute(&program(target, table, vec![unit]))
}

#[test]
fn nonowning_edges_transport_expiration_until_an_actual_read() {
    assert_eq!(output(&expired_view_on_an_edge(false)), "");
    assert_eq!(
        expired_view_on_an_edge(true).violation(),
        Some(ViolationKind::BorrowExpired)
    );
}
