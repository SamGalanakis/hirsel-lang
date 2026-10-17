//! Drop specialization and reuse tokens over hand-built Rc units: shapes the
//! v1 source cannot state (children copied out of a box before its release,
//! variants and records in boxes, irreducible loops, a failing finalizer
//! while a token is live). Every unit passes C before and after the pass, the
//! heap executor runs it with an empty terminal ledger on the normal path and
//! on every injected failure, reuse-on and reuse-off observe the same thing,
//! and counters witness what the pass did. Injected token misuse is rejected
//! by C, beside its accepted twin.
use crate::rc_support as support;

use hirsel_core_contract::Manifest;
use hirsel_heapcheck::build::{CLEANUP, Layouts, fixture_unit, path};
use hirsel_heapcheck::{Arg, Config, Counters, NoOperations, Observation, Program, ViolationKind};
use hirsel_lir::abi_types::*;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::*;
use hirsel_lir::runtime_abi::HEADER_V2;
use hirsel_lir::stage::{Rc, Reuse, Stage};
use hirsel_lir::testing::UnitBuilder;
use hirsel_lir::validate::{Inputs, ViolationKind as K};
use hirsel_rc::reuse::Options;
use support::{dump, target};

const HEAP: Placement = Placement::Fixed(StorageOwner::Heap(TaskDomainId(0)));
const BLOCK: ValueRepr = ValueRepr::Block;
const U64: ValueRepr = ValueRepr::Scalar(ScalarKind::U64);
const BOOL: ValueRepr = ValueRepr::Scalar(ScalarKind::Bool);

#[derive(Clone, Copy)]
struct L {
    u64: LayoutId,
    bool: LayoutId,
    /// `Box[cell]`
    boxed: LayoutId,
    /// `Box[record { a: Box[cell], b: Box[cell] }]`
    pair: LayoutId,
    /// `Box[variant { Two(Box[cell], Box[cell]) | One(Box[cell]) }]`
    either: LayoutId,
    /// `Box[record { inner: record { p: Box[cell], n: u64 }, q: Box[cell] }]`
    nested: LayoutId,
    /// `record { p: Box[cell], n: u64 }`
    inner: LayoutId,
    /// Each block layout's payload.
    payloads: [(LayoutId, LayoutId); 4],
}

impl L {
    /// A block's payload; an inline value is released at its own layout.
    fn payload(&self, layout: LayoutId) -> LayoutId {
        self.payloads
            .iter()
            .find(|(b, _)| *b == layout)
            .map_or(layout, |p| p.1)
    }
}

fn layouts() -> (Layouts, L) {
    let mut l = Layouts::new(target());
    let u64 = l.scalar(ScalarKind::U64);
    let bool = l.scalar(ScalarKind::Bool);
    let cell = l.record(&[u64]);
    let boxed = l.block(cell);
    let pairp = l.record(&[boxed, boxed]);
    let pair = l.block(pairp);
    let eitherp = l.variant(&[&[boxed, boxed], &[boxed]]);
    let either = l.block(eitherp);
    let inner = l.record(&[boxed, u64]);
    let nestedp = l.record(&[inner, boxed]);
    let nested = nestedp;
    // C composes an inline child's programs into its parent's, shifted.
    let q = l.get(nested).fields[1].offset;
    let r = l.get_mut(nested);
    r.scan = vec![
        ScanStep::Counted { offset: 0 },
        ScanStep::Counted { offset: q },
    ];
    r.drop = vec![
        DropStep::Release { offset: q },
        DropStep::Release { offset: 0 },
    ];
    let nested = l.block(nestedp);
    // The block header: a reuse token's layout.
    let fields: Vec<FieldLayout> = HEADER_V2
        .iter()
        .enumerate()
        .map(|(ordinal, &(_, kind, offset))| FieldLayout {
            ordinal: ordinal as u32,
            offset,
            layout: l.scalar(kind),
            unaligned: false,
        })
        .collect();
    let mut header = l.get(u64).clone();
    header.kind = LayoutKind::Header;
    header.size = SizeSpec::Fixed(hirsel_lir::runtime_abi::HEADER_SIZE);
    header.align = 8;
    header.fields = fields;
    header.abi.scalars.clear();
    l.table.layouts.push(header);
    (
        l,
        L {
            u64,
            bool,
            boxed,
            pair,
            either,
            nested,
            inner,
            payloads: [
                (boxed, cell),
                (pair, pairp),
                (either, eitherp),
                (nested, nestedp),
            ],
        },
    )
}

/// A unit builder with a current block and the owned blocks held there:
/// every failure edge gets a cleanup that releases exactly those.
struct F {
    b: UnitBuilder<Rc>,
    cur: BlockId,
    l: L,
    propagate: BlockId,
    owned: Vec<(ValueId, LayoutId)>,
}

impl F {
    fn new(name: &str, l: L) -> Self {
        let mut b = UnitBuilder::<Rc>::new(fixture_unit(path(name)));
        b.unit.header.abi = FnAbiId(1);
        let cur = b.block(BlockKind::Body, &[]);
        let propagate = b.block(BlockKind::FailureCleanup, &[]);
        b.term(propagate, Terminator::Propagate);
        Self {
            b,
            cur,
            l,
            propagate,
            owned: Vec::new(),
        }
    }
    fn inst(&mut self, inst: Inst) {
        self.b.push(self.cur, inst);
    }
    fn propagates(&mut self) -> FailureId {
        self.fails(None)
    }
    /// A failure edge into a cleanup releasing what is owned now.
    fn fails(&mut self, rule: Option<&str>) -> FailureId {
        let cleanup = self.cleanup(self.owned.clone());
        let e = self.b.edge(cleanup, &[]);
        self.b.failure(e, rule, &[])
    }
    /// Release `owned` last first, each release's failure continuing with
    /// the rest.
    fn cleanup(&mut self, owned: Vec<(ValueId, LayoutId)>) -> BlockId {
        let Some((&(value, layout), rest)) = owned.split_last() else {
            return self.propagate;
        };
        let next = self.cleanup(rest.to_vec());
        let block = self.b.block(BlockKind::FailureCleanup, &[]);
        let e = self.b.edge(next, &[]);
        let failure = self.b.failure(e, None, &[]);
        self.b.push(
            block,
            Inst::Release {
                value,
                layout: self.l.payload(layout),
                live: MaskRef::All,
                anchor: CLEANUP,
                failure,
            },
        );
        let e = self.b.edge(next, &[]);
        self.b.term(block, Terminator::Jump(e));
        block
    }
    fn own(&mut self, v: ValueId, layout: LayoutId) {
        self.owned.push((v, layout));
    }
    fn disown(&mut self, v: ValueId) {
        let i = self
            .owned
            .iter()
            .rposition(|&(o, _)| o == v)
            .expect("owned");
        self.owned.remove(i);
    }
    fn int(&mut self, n: u128) -> ValueId {
        let k = self.b.constant(U64, self.l.u64, ConstantValue::Integer(n));
        let v = self.b.plain(U64, self.l.u64);
        self.inst(Inst::Const {
            result: v,
            constant: k,
        });
        v
    }
    fn boolean(&mut self, b: bool) -> ValueId {
        let k = self.b.constant(BOOL, self.l.bool, ConstantValue::Bool(b));
        let v = self.b.plain(BOOL, self.l.bool);
        self.inst(Inst::Const {
            result: v,
            constant: k,
        });
        v
    }
    fn construct(
        &mut self,
        kind: ConstructorKind,
        layout: LayoutId,
        fields: &[(u32, ValueId)],
    ) -> ValueId {
        for &(_, v) in fields {
            if self.owned.iter().any(|&(o, _)| o == v) {
                self.disown(v);
            }
        }
        let failure = self.propagates();
        let k = self.b.construct(kind, self.l.payload(layout), fields, HEAP);
        let v = self.b.owned(BLOCK, layout);
        self.inst(Inst::Construct {
            result: v,
            construct: k,
            failure,
        });
        self.own(v, layout);
        v
    }
    /// A heap `cell { n }`.
    fn boxed(&mut self, n: u128) -> ValueId {
        let n = self.int(n);
        self.construct(ConstructorKind::Tuple, self.l.boxed, &[(0, n)])
    }
    fn address(
        &mut self,
        base: ValueId,
        steps: &[ProjectionStep],
        repr: ValueRepr,
        layout: LayoutId,
    ) -> AddressId {
        let a = self
            .b
            .address(AddressBase::Value(base), steps, repr, layout);
        let failure = self.propagates();
        self.inst(Inst::Address {
            address: a,
            failure,
        });
        a
    }
    /// The u64 in a `Box[cell]`.
    fn unbox(&mut self, boxed: ValueId) -> ValueId {
        let a = self.address(boxed, &[ProjectionStep::Field(0)], U64, self.l.u64);
        let v = self.b.plain(U64, self.l.u64);
        self.inst(Inst::Read {
            result: v,
            address: a,
            reaching: boxed,
            mode: ReadMode::Bits,
        });
        v
    }
    /// Counted copies of the boxes at `steps` of `d`'s payload: their
    /// addresses first, then the copies.
    fn children(&mut self, d: ValueId, steps: &[ProjectionStep]) -> Vec<ValueId> {
        let boxed = self.l.boxed;
        let addresses: Vec<_> = steps
            .iter()
            .map(|&step| self.address(d, &[step], BLOCK, boxed))
            .collect();
        addresses
            .into_iter()
            .map(|a| {
                let v = self.b.owned(BLOCK, boxed);
                let failure = self.propagates();
                self.inst(Inst::Copy {
                    result: v,
                    address: a,
                    reaching: d,
                    failure,
                });
                self.own(v, boxed);
                v
            })
            .collect()
    }
    fn release(&mut self, v: ValueId, layout: LayoutId) {
        self.disown(v);
        let failure = self.propagates();
        self.inst(Inst::Release {
            value: v,
            layout: self.l.payload(layout),
            live: MaskRef::All,
            anchor: CLEANUP,
            failure,
        });
    }
    fn add(&mut self, left: ValueId, right: ValueId) -> ValueId {
        let v = self.b.plain(U64, self.l.u64);
        let failure = self.propagates();
        self.inst(Inst::Binary {
            result: v,
            op: BinaryOp::Add,
            left,
            right,
            failure,
        });
        v
    }
    fn ret(&mut self, v: ValueId) {
        self.b.term(self.cur, Terminator::Return(Some(v)));
    }
}

struct Fixture {
    target: TargetSpec,
    table: LayoutTable,
    units: Vec<LirUnit<Rc>>,
    args: Vec<Arg>,
}

/// `main`'s ABI: `(flag: bool)` when it has an argument, a u64 result.
fn fixture(mut l: Layouts, units: Vec<LirUnit<Rc>>, args: Vec<Arg>) -> Fixture {
    let u64 = l
        .table
        .layouts
        .iter()
        .position(|r| r.kind == LayoutKind::Scalar(ScalarKind::U64));
    let bool = l
        .table
        .layouts
        .iter()
        .position(|r| r.kind == LayoutKind::Scalar(ScalarKind::Bool));
    let (u64, bool) = (
        LayoutId(u64.unwrap() as u32),
        LayoutId(bool.unwrap() as u32),
    );
    let mut abi = l.table.fn_abis[0].clone();
    abi.key.callable = hirsel_syntax::ContentHash::of_bytes(b"drop specialization main");
    abi.result = Some(AbiResult {
        borrowed_parameters: vec![],
        layout: u64,
        slots: vec![hirsel_lir::abi::slot(&l.target, ScalarKind::Pointer)],
    });
    if !args.is_empty() {
        abi.parameters.push(AbiParameter {
            source: 0,
            convention: hirsel_core_contract::Passing::Borrow,
            passing: ParameterPassing::ByValue,
            layout: bool,
            slots: vec![hirsel_lir::abi::slot(&l.target, ScalarKind::Bool)],
            extension: hirsel_lir::abi::extension_of(ScalarKind::Bool),
        });
    }
    l.table.fn_abis.push(abi);
    let digest = l.table.digest();
    let units = units
        .into_iter()
        .map(|mut u| {
            u.header.layouts = digest;
            u
        })
        .collect();
    let (target, table) = l.finish();
    Fixture {
        target,
        table,
        units,
        args,
    }
}

fn manifest() -> Manifest {
    Manifest::skeleton(Vec::new())
}

fn violations<S: Stage>(f: &Fixture, units: &[LirUnit<S>]) -> Vec<K> {
    let manifest = manifest();
    let inputs = Inputs {
        discharge: None,
        operations: &(),
        target: &f.target,
        layouts: &f.table,
        manifest: &manifest,
    };
    units
        .iter()
        .flat_map(|u| hirsel_lir::validate::check(u, &inputs))
        .map(|v| v.kind)
        .collect()
}

fn reuse(f: &Fixture, on: bool) -> Vec<LirUnit<Reuse>> {
    let before = violations(f, &f.units);
    let dumps = || f.units.iter().map(dump).collect::<Vec<_>>().join("\n");
    assert!(before.is_empty(), "Rc: {before:?}\n{}", dumps());
    let units: Vec<_> = f
        .units
        .iter()
        .map(|u| {
            hirsel_rc::reuse::reuse(u.clone(), &f.table, &Options { reuse: on }).expect("reuse")
        })
        .collect();
    let after = violations(f, &units);
    assert!(
        after.is_empty(),
        "Reuse ({on}): {after:?}\n{}",
        units.iter().map(dump).collect::<Vec<_>>().join("\n")
    );
    units
}

fn program(f: &Fixture, units: Vec<LirUnit<Reuse>>) -> Program<Reuse> {
    Program {
        target: f.target.clone(),
        layouts: f.table.clone(),
        units,
        statics: Vec::new(),
    }
}

/// What one reuse mode did on the clean run.
struct Outcome {
    units: Vec<LirUnit<Reuse>>,
    observation: Observation,
    counters: Counters,
}

/// Both modes, with every failure-capable site injected in turn: clean
/// audits throughout, and the same observation and exemption count.
fn run_both(f: &Fixture) -> [Outcome; 2] {
    let mut out = [true, false].map(|on| {
        let units = reuse(f, on);
        let p = program(f, units.clone());
        let campaign = hirsel_heapcheck::fault_campaign(
            &p,
            &path("main"),
            &f.args,
            &mut || Box::new(NoOperations),
            &Config::default(),
        );
        let dumps = || units.iter().map(dump).collect::<Vec<_>>().join("\n");
        let audit = campaign
            .clean
            .result
            .clone()
            .unwrap_or_else(|v| panic!("reuse {on}: {v}\n{}", dumps()));
        for (site, run) in &campaign.injected {
            if let Err(v) = &run.result {
                panic!("reuse {on}, failing site {site}: {v}\n{}", dumps());
            }
        }
        (units, audit, campaign.clean.counters)
    });
    let [(_, on, _), (_, off, _)] = &mut out;
    assert!(
        on.observation.equivalent(&off.observation),
        "{:?} != {:?}",
        on.observation,
        off.observation
    );
    assert_eq!(on.exemptions.len(), off.exemptions.len());
    out.map(|(units, audit, counters)| Outcome {
        units,
        observation: audit.observation,
        counters,
    })
}

fn count(units: &[LirUnit<Reuse>], f: impl Fn(&Inst) -> bool) -> usize {
    units
        .iter()
        .map(|u| u.instructions.iter().filter(|i| f(i)).count())
        .sum()
}

fn returned(o: &Observation) -> u128 {
    match o.returned {
        Some(hirsel_heapcheck::Observed::Integer { bits, .. }) => bits,
        ref other => panic!("not an integer: {other:?}"),
    }
}

/// The kept field ordinals of every Reuse.
fn kept(units: &[LirUnit<Reuse>]) -> Vec<Vec<u32>> {
    let mut out = Vec::new();
    for u in units {
        for inst in &u.instructions {
            if let Inst::Reuse { kept, .. } = *inst {
                let bits = kept.map_or(Vec::new(), |m| {
                    let mask = &u.tables.masks[m.index()];
                    let words = mask.words.get(&u.pools.mask_words).unwrap_or(&[]);
                    (0..mask.fields)
                        .filter(|&i| words[i as usize / 64] >> (i % 64) & 1 == 1)
                        .collect()
                });
                out.push(bits);
            }
        }
    }
    out
}

#[derive(Clone, Copy, Default)]
struct Pair {
    /// Another holder keeps the donor alive: the shared path.
    shared: bool,
    /// The children go back into the other fields.
    swapped: bool,
    /// A check fails between the donor's release and the construction.
    fail_after_release: bool,
}

/// `x = copy d.a; y = copy d.b; release d; e = Box((x, y))` (or `(y, x)`),
/// returning `10 * x + y` read before the construction.
fn pair(shape: Pair) -> Fixture {
    let (l, lay) = layouts();
    let mut f = F::new("main", lay);
    let a = f.boxed(1);
    let b = f.boxed(2);
    let d = f.construct(ConstructorKind::Tuple, lay.pair, &[(0, a), (1, b)]);
    if shape.shared {
        f.inst(Inst::Dup { value: d });
        f.own(d, lay.pair);
    }
    let [x, y] = f.children(d, &[ProjectionStep::Field(0), ProjectionStep::Field(1)])[..] else {
        unreachable!()
    };
    f.release(d, lay.pair);
    let rx = f.unbox(x);
    let ry = f.unbox(y);
    let ten = f.int(10);
    let tens = f.b.plain(U64, lay.u64);
    let failure = f.propagates();
    f.inst(Inst::Binary {
        result: tens,
        op: BinaryOp::Mul,
        left: rx,
        right: ten,
        failure,
    });
    let r = f.add(tens, ry);
    if shape.fail_after_release {
        // x, y and the other holder of d are owned when the check fails.
        let no = f.boolean(false);
        let failure = f.fails(Some("CHECK-1"));
        f.inst(Inst::Check {
            condition: no,
            failure,
        });
    }
    let fields = if shape.swapped {
        [(0, y), (1, x)]
    } else {
        [(0, x), (1, y)]
    };
    let e = f.construct(ConstructorKind::Tuple, lay.pair, &fields);
    f.release(e, lay.pair);
    if shape.shared {
        f.release(d, lay.pair);
    }
    f.ret(r);
    let main = f.b.finish_with_layouts(&l.table);
    fixture(l, vec![main], Vec::new())
}

/// A unique donor moves its children out instead of copying them (no
/// Dup), its storage is reused, and both children are kept in place: the
/// construction writes neither (a selective write).
#[test]
fn a_unique_donor_moves_its_children_and_keeps_them_in_place() {
    let [on, off] = run_both(&pair(Pair::default()));
    assert_eq!(returned(&on.observation), 12);
    assert_eq!(off.counters.dups, 2, "reuse off copies both children");
    assert_eq!(on.counters.dups, 0, "the unique path elides both dups");
    assert_eq!(on.counters.reuses, 1);
    assert_eq!(on.counters.allocations + 1, off.counters.allocations);
    assert_eq!(kept(&on.units), vec![vec![0, 1]]);
    assert_eq!(count(&on.units, |i| matches!(i, Inst::IsUnique { .. })), 1);
    assert_eq!(count(&off.units, |i| matches!(i, Inst::Reuse { .. })), 0);
}

/// A shared donor takes the shared path: the children are copied and the
/// block only decremented, so the construction gets no storage.
#[test]
fn a_shared_donor_copies_its_children_and_donates_nothing() {
    let [on, off] = run_both(&pair(Pair {
        shared: true,
        ..Pair::default()
    }));
    assert_eq!(returned(&on.observation), 12);
    assert_eq!(on.counters.dups, off.counters.dups);
    assert_eq!(on.counters.reuses, 0);
    assert_eq!(on.counters.allocations, off.counters.allocations);
}

/// Swapped labelled fields are not the bits already in place: every field
/// is written.
#[test]
fn swapped_children_are_written_not_kept() {
    let [on, _] = run_both(&pair(Pair {
        swapped: true,
        ..Pair::default()
    }));
    assert_eq!(on.counters.reuses, 1);
    assert_eq!(kept(&on.units), vec![Vec::<u32>::new()]);
}

/// A failure after the reset disposes the live token on the failure edge.
#[test]
fn a_failure_after_the_reset_disposes_the_token() {
    for shared in [false, true] {
        let [on, _] = run_both(&pair(Pair {
            shared,
            fail_after_release: true,
            ..Pair::default()
        }));
        assert!(!on.observation.failures.is_empty());
        assert_eq!(on.counters.reuses, 0);
        assert_eq!(on.counters.disposes, u64::from(!shared));
    }
}

/// `d = Box(Two(a, b)); x = copy d.Two.0; y = copy d.Two.1; release d;`
/// then `Box(Two(x, y))` (same active variant: kept) or `Box(One(x))` and
/// `release y` (another variant: written).
fn variant(same: bool) -> Fixture {
    let (l, lay) = layouts();
    let mut f = F::new("main", lay);
    let def = f.b.symbol(SymbolKind::Type, path("Either"));
    let two = ConstructorKind::Variant {
        definition: def,
        variant: 0,
    };
    let one = ConstructorKind::Variant {
        definition: def,
        variant: 1,
    };
    let a = f.boxed(3);
    let b = f.boxed(4);
    let d = f.construct(two, lay.either, &[(0, a), (1, b)]);
    let steps = [0, 1].map(|field| ProjectionStep::VariantField { variant: 0, field });
    let [x, y] = f.children(d, &steps)[..] else {
        unreachable!()
    };
    f.release(d, lay.either);
    let r = f.unbox(x);
    let e = if same {
        f.construct(two, lay.either, &[(0, x), (1, y)])
    } else {
        let e = f.construct(one, lay.either, &[(0, x)]);
        f.release(y, lay.boxed);
        e
    };
    f.release(e, lay.either);
    f.ret(r);
    let main = f.b.finish_with_layouts(&l.table);
    fixture(l, vec![main], Vec::new())
}

#[test]
fn a_child_is_kept_only_in_the_same_active_variant() {
    let [on, _] = run_both(&variant(true));
    assert_eq!(on.counters.reuses, 1);
    assert_eq!(kept(&on.units), vec![vec![0, 1]]);
    let [on, _] = run_both(&variant(false));
    assert_eq!(on.counters.reuses, 1);
    assert_eq!(kept(&on.units), vec![Vec::<u32>::new()]);
}

/// A donor whose payload holds a pointer inside an inline record: the
/// reset runs the layout's drop program, nested child included.
#[test]
fn a_donor_with_nested_inline_pointers_releases_them_at_the_reset() {
    let (l, lay) = layouts();
    let mut f = F::new("main", lay);
    let fresh = |f: &mut F, n| (f.boxed(n), f.int(n), f.boxed(n + 1));
    let (p, k, q) = fresh(&mut f, 1);
    let inner = nested_inner(&mut f, p, k);
    let d = f.construct(ConstructorKind::Tuple, lay.nested, &[(0, inner), (1, q)]);
    f.release(d, lay.nested);
    let (p, k, q) = fresh(&mut f, 5);
    let inner = nested_inner(&mut f, p, k);
    let e = f.construct(ConstructorKind::Tuple, lay.nested, &[(0, inner), (1, q)]);
    let a = f.address(
        e,
        &[ProjectionStep::Field(0), ProjectionStep::Field(1)],
        U64,
        lay.u64,
    );
    let r = f.b.plain(U64, lay.u64);
    f.inst(Inst::Read {
        result: r,
        address: a,
        reaching: e,
        mode: ReadMode::Bits,
    });
    f.release(e, lay.nested);
    f.ret(r);
    let main = f.b.finish_with_layouts(&l.table);
    let [on, off] = run_both(&fixture(l, vec![main], Vec::new()));
    assert_eq!(returned(&on.observation), 5);
    assert_eq!(on.counters.reuses, 1);
    assert_eq!(on.counters.allocations + 1, off.counters.allocations);
}

/// The inline `record { p, n }` of the nested layout, as an owned value.
fn nested_inner(f: &mut F, p: ValueId, n: ValueId) -> ValueId {
    let inner = f.l.inner;
    f.disown(p);
    let k =
        f.b.construct(ConstructorKind::Tuple, inner, &[(0, p), (1, n)], HEAP);
    let v = f.b.owned(ValueRepr::Aggregate, inner);
    let failure = f.propagates();
    f.inst(Inst::Construct {
        result: v,
        construct: k,
        failure,
    });
    f.own(v, inner);
    v
}

/// `d = box 1; branch flag { yes: k = box 4; release d; join(k) | no:
/// join(d) }; join(h): n = box 3; release h`: the yes edge brings d's
/// token to the join, the no edge none.
fn diamond() -> (Layouts, LirUnit<Rc>) {
    let (l, lay) = layouts();
    let mut f = F::new("main", lay);
    let flag = f.b.plain(BOOL, lay.bool);
    f.b.parameter(flag);
    let d = f.boxed(1);
    let h = f.b.owned(BLOCK, lay.boxed);
    let yes = f.b.block(BlockKind::Body, &[]);
    let no = f.b.block(BlockKind::Body, &[]);
    let join = f.b.block(BlockKind::Body, &[h]);
    let (ey, en) = (f.b.edge(yes, &[]), f.b.edge(no, &[]));
    f.b.term(
        f.cur,
        Terminator::Branch {
            condition: flag,
            yes: ey,
            no: en,
        },
    );
    f.cur = yes;
    let k = f.boxed(4);
    f.release(d, lay.boxed);
    let e = f.b.edge(join, &[k]);
    f.b.term(yes, Terminator::Jump(e));
    f.disown(k);
    f.cur = no;
    f.own(d, lay.boxed);
    let e = f.b.edge(join, &[d]);
    f.b.term(no, Terminator::Jump(e));
    f.disown(d);
    f.cur = join;
    f.own(h, lay.boxed);
    let n = f.boxed(3);
    let r = f.unbox(n);
    f.release(h, lay.boxed);
    f.release(n, lay.boxed);
    f.ret(r);
    let main = f.b.finish_with_layouts(&l.table);
    (l, main)
}

#[test]
fn a_diamond_joins_a_token_with_none() {
    for flag in [true, false] {
        let (l, main) = diamond();
        let args = vec![Arg::Bits {
            scalar: ScalarKind::Bool,
            bits: u128::from(flag),
        }];
        let [on, off] = run_both(&fixture(l, vec![main], args));
        assert_eq!(returned(&on.observation), 3);
        assert_eq!(on.counters.reuses, u64::from(flag), "flag {flag}");
        assert_eq!(
            on.counters.allocations + u64::from(flag),
            off.counters.allocations
        );
        // The join takes the token as a block parameter.
        let joins = on.units[0]
            .blocks
            .iter()
            .filter(|b| {
                b.parameters
                    .get(&on.units[0].pools.values)
                    .unwrap_or(&[])
                    .iter()
                    .any(|&p| {
                        let ty = on.units[0].values[p.index()].ty;
                        on.units[0].types[ty.index()].repr == ValueRepr::Token
                    })
            })
            .count();
        assert_eq!(joins, 1);
    }
}

/// An irreducible loop: the entry branches into both `a` and `b`, which
/// branch into each other until a counter reaches 3. Each body releases
/// a box and the other builds one, so a token would cross a feedback edge.
fn irreducible(flag: bool) -> Fixture {
    let (l, lay) = layouts();
    let mut f = F::new("main", lay);
    let enter_a = f.boolean(flag);
    let zero = f.int(0);
    let d = f.boxed(7);
    let ia = f.b.plain(U64, lay.u64);
    let da = f.b.owned(BLOCK, lay.boxed);
    let ib = f.b.plain(U64, lay.u64);
    let db = f.b.owned(BLOCK, lay.boxed);
    let a = f.b.block(BlockKind::Body, &[ia, da]);
    let b = f.b.block(BlockKind::Body, &[ib, db]);
    let exit = f.b.block(BlockKind::Body, &[]);
    let (ea, eb) = (f.b.edge(a, &[zero, d]), f.b.edge(b, &[zero, d]));
    f.b.term(
        f.cur,
        Terminator::Branch {
            condition: enter_a,
            yes: ea,
            no: eb,
        },
    );
    f.disown(d);
    for (this, i, held, other) in [(a, ia, da, b), (b, ib, db, a)] {
        f.cur = this;
        f.own(held, lay.boxed);
        f.release(held, lay.boxed);
        let one = f.int(1);
        let next = f.add(i, one);
        let fresh = f.boxed(9);
        let three = f.int(3);
        let more = f.b.plain(BOOL, lay.bool);
        let failure = f.propagates();
        f.inst(Inst::Binary {
            result: more,
            op: BinaryOp::Lt,
            left: next,
            right: three,
            failure,
        });
        let done = f.b.block(BlockKind::Body, &[]);
        let (back, out) = (f.b.edge(other, &[next, fresh]), f.b.edge(done, &[]));
        f.disown(fresh);
        f.b.term(
            this,
            Terminator::Branch {
                condition: more,
                yes: back,
                no: out,
            },
        );
        f.cur = done;
        f.own(fresh, lay.boxed);
        f.release(fresh, lay.boxed);
        let e = f.b.edge(exit, &[]);
        f.b.term(done, Terminator::Jump(e));
    }
    f.cur = exit;
    let r = f.int(3);
    f.ret(r);
    let main = f.b.finish_with_layouts(&l.table);
    fixture(l, vec![main], Vec::new())
}

#[test]
fn no_token_crosses_an_irreducible_loops_feedback_edges() {
    for flag in [true, false] {
        let [on, _] = run_both(&irreducible(flag));
        assert_eq!(returned(&on.observation), 3);
        // No block parameter of the loop carries a token.
        let u = &on.units[0];
        for b in &u.blocks {
            for &p in b.parameters.get(&u.pools.values).unwrap_or(&[]) {
                let ty = u.values[p.index()].ty;
                assert_ne!(u.types[ty.index()].repr, ValueRepr::Token);
            }
        }
    }
}

/// Each injected misuse of a correct Reuse unit, beside the unit itself.
#[test]
fn injected_token_misuse_is_rejected() {
    let f = pair(Pair {
        fail_after_release: true,
        ..Pair::default()
    });
    let good = reuse(&f, true);
    assert!(violations(&f, &good).is_empty());

    // A failure edge forgets the live token: leaked on that path.
    let mut leak = good.clone();
    for fe in &mut leak[0].tables.failures {
        fe.dispose.len = 0;
    }
    assert!(!violations(&f, &leak).is_empty(), "leaked token");

    // The construction names the reset's token directly instead of the
    // join's parameter: unavailable on the shared path, and the parameter
    // leaks.
    let f = pair(Pair::default());
    let good = reuse(&f, true);
    let mut bypass = good.clone();
    let u = &mut bypass[0];
    let reset = u
        .instructions
        .iter()
        .find_map(|i| match *i {
            Inst::Reset { result, .. } => Some(result),
            _ => None,
        })
        .expect("a reset");
    for inst in &mut u.instructions {
        if let Inst::Reuse { token, .. } = inst {
            *token = reset;
        }
    }
    assert!(
        !violations(&f, &bypass).is_empty(),
        "token bypasses the join"
    );

    // A kept field whose operand is not the take of that slot.
    let f = pair(Pair {
        swapped: true,
        ..Pair::default()
    });
    let good = reuse(&f, true);
    let mut bad = good.clone();
    let u = &mut bad[0];
    let words = PoolRange::push(&mut u.pools.mask_words, [0b11u64]);
    u.tables.masks.push(FieldMask { fields: 2, words });
    let all = MaskId(u.tables.masks.len() as u32 - 1);
    for inst in &mut u.instructions {
        if let Inst::Reuse { kept, .. } = inst {
            *kept = Some(all);
        }
    }
    let v = violations(&f, &bad);
    assert!(v.iter().any(|k| matches!(k, K::KeptField { .. })), "{v:?}");
    // H rejects the skipped store as well: the swapped bits differ.
    let run = hirsel_heapcheck::run(
        &program(&f, bad),
        &path("main"),
        &[],
        &mut NoOperations,
        &Config::default(),
    );
    assert_eq!(run.violation(), Some(ViolationKind::SkippedStore));
    let run = hirsel_heapcheck::run(
        &program(&f, good),
        &path("main"),
        &[],
        &mut NoOperations,
        &Config::default(),
    );
    assert!(run.result.is_ok());
}
