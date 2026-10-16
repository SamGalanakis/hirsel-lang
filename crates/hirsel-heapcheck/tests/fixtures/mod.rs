//! Hand-built post-RC programs: each deliberately broken fixture and its
//! corrected twin, built by one function with `fixed` selecting the twin.
//! Every pair prints the same bytes, so only the heap audit tells them apart.
#![allow(dead_code)]
use hirsel_core_contract::{OperationId, Passing};
use hirsel_heapcheck::build::{CLEANUP, Layouts, UnitBuilder, path};
use hirsel_heapcheck::{OpOutcome, OpValue, Operations, Program, ViolationKind};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{LayoutId, LayoutTable, ScalarKind, TargetSpec};
use hirsel_lir::stage::{Rc, Ready, Reuse, Stage};
use hirsel_syntax::ContentHash;

pub fn print_op() -> OperationId {
    OperationId {
        definition: path("print"),
        declaration_digest: ContentHash::of_bytes(b"print(borrow u64)"),
    }
}

/// `print(u64)`: its decimal and a newline.
pub struct Printer;

impl Operations for Printer {
    fn domain(&self) -> hirsel_heapcheck::OperationDomain {
        hirsel_heapcheck::OperationDomain::Simulation
    }
    fn fork_simulation(&self) -> Option<Box<dyn Operations>> {
        Some(Box::new(Self))
    }

    fn call(&mut self, op: &OperationId, args: &[OpValue], output: &mut Vec<u8>) -> OpOutcome {
        match (op == &print_op(), args) {
            (true, [OpValue::Bits { bits, .. }]) => {
                output.extend(format!("{bits}\n").as_bytes());
                OpOutcome::Value(OpValue::Unit)
            }
            _ => OpOutcome::Unsupported,
        }
    }
}

/// The layouts every fixture shares.
#[derive(Clone, Copy)]
pub struct Common {
    pub u64: LayoutId,
    pub bool: LayoutId,
    /// `record { value: u64 }`
    pub cell: LayoutId,
    /// `Box[cell]`
    pub boxed: LayoutId,
}

pub fn common(l: &mut Layouts) -> Common {
    let u64 = l.scalar(ScalarKind::U64);
    let bool = l.scalar(ScalarKind::Bool);
    let cell = l.record(&[u64]);
    let boxed = l.block(cell);
    Common {
        u64,
        bool,
        cell,
        boxed,
    }
}

pub const HEAP: Placement = Placement::Fixed(StorageOwner::Heap(TaskDomainId(0)));
pub const BLOCK: ValueRepr = ValueRepr::Block;
pub const U64: ValueRepr = ValueRepr::Scalar(ScalarKind::U64);
pub const BOOL: ValueRepr = ValueRepr::Scalar(ScalarKind::Bool);

/// A unit builder with a current block, and the common layouts.
pub struct F<S> {
    pub b: UnitBuilder<S>,
    pub cur: BlockId,
    pub c: Common,
    /// Shared failure exit for calls that cannot fail in these fixtures.
    pub propagate: BlockId,
}

impl<S: Stage> F<S> {
    pub fn new(name: &str, c: Common) -> Self {
        let mut b = UnitBuilder::<S>::new(path(name));
        let cur = b.block(BlockKind::Body, &[]);
        let propagate = b.block(BlockKind::FailureCleanup, &[]);
        b.end(propagate, Terminator::Propagate);
        Self {
            b,
            cur,
            c,
            propagate,
        }
    }
    pub fn inst(&mut self, inst: Inst) {
        self.b.push(self.cur, inst);
    }
    pub fn int(&mut self, n: u128) -> ValueId {
        let layout = self.c.u64;
        let k = self.b.constant(U64, layout, ConstantValue::Integer(n));
        let v = self.b.plain(U64, layout);
        self.inst(Inst::Const {
            result: v,
            constant: k,
        });
        v
    }
    /// A failure edge into the shared propagating exit.
    pub fn propagates(&mut self) -> FailureId {
        let e = self.b.edge(self.propagate, &[]);
        self.b.failure(e, None, &[])
    }
    /// A failure edge with a rule into a cleanup block of its own.
    pub fn raises(&mut self, rule: &str, cleanup: BlockId) -> FailureId {
        let e = self.b.edge(cleanup, &[]);
        self.b.failure(e, Some(rule), &[])
    }
    /// A heap `cell { value }` (a counted block holding a record).
    pub fn boxed(&mut self, value: ValueId) -> ValueId {
        let (boxed, cell) = (self.c.boxed, self.c.cell);
        let k = self
            .b
            .construct(ConstructorKind::Tuple, boxed, &[(0, value)], HEAP);
        let v = self.b.owned(BLOCK, boxed);
        let failure = self.propagates();
        let _ = cell;
        self.inst(Inst::Construct {
            result: v,
            construct: k,
            failure,
        });
        v
    }
    /// Read field `field` of the block `base` points to, as plain bits.
    pub fn load(
        &mut self,
        base: ValueId,
        steps: &[ProjectionStep],
        repr: ValueRepr,
        layout: LayoutId,
    ) -> ValueId {
        let a = self
            .b
            .address(AddressBase::Value(base), steps, repr, layout);
        let failure = self.propagates();
        self.inst(Inst::Address {
            address: a,
            failure,
        });
        let v = self.b.plain(repr, layout);
        self.inst(Inst::Read {
            result: v,
            address: a,
            reaching: base,
            mode: ReadMode::Bits,
        });
        v
    }
    /// The u64 in a `Box[cell]`.
    pub fn unbox(&mut self, boxed: ValueId) -> ValueId {
        let layout = self.c.u64;
        self.load(boxed, &[ProjectionStep::Field(0)], U64, layout)
    }
    /// Call and continue in a fresh block.
    pub fn call(
        &mut self,
        callee: Callee,
        args: &[(Passing, ArgumentValue)],
        result: Option<ValueId>,
    ) {
        let call = self.b.call(callee, args, result);
        let next = self.b.block(BlockKind::Body, &[]);
        let normal = self.b.edge(next, &[]);
        let failure = self.propagates();
        self.b.end(
            self.cur,
            Terminator::Call {
                call,
                normal: Some(normal),
                failure,
            },
        );
        self.cur = next;
    }
    pub fn print(&mut self, v: ValueId) {
        let op = self.b.operation(print_op());
        self.call(op, &[(Passing::Borrow, ArgumentValue::Value(v))], None);
    }
    pub fn release(&mut self, v: ValueId, layout: LayoutId) {
        self.release_masked(v, layout, MaskRef::All);
    }
    pub fn release_masked(&mut self, v: ValueId, layout: LayoutId, live: MaskRef) {
        let failure = self.propagates();
        self.inst(Inst::Release {
            value: v,
            layout,
            live,
            anchor: CLEANUP,
            failure,
        });
    }
    pub fn ret(&mut self, v: Option<ValueId>) {
        self.b.end(self.cur, Terminator::Return(v));
    }
    pub fn finish(self, table: &LayoutTable) -> LirUnit<S> {
        self.b.finish(table)
    }
}

pub fn program<S: Stage>(
    target: TargetSpec,
    table: LayoutTable,
    units: Vec<LirUnit<S>>,
) -> Program<S> {
    Program {
        target,
        layouts: table,
        units,
        statics: Vec::new(),
    }
}

fn layouts() -> (Layouts, Common) {
    let mut l = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let c = common(&mut l);
    (l, c)
}

/// A fixture pair: name, the violation the broken twin must report, the
/// output both twins print, and the program for either twin.
pub struct Fixture {
    pub name: &'static str,
    pub expect: ViolationKind,
    pub output: &'static str,
    pub build: fn(bool) -> AnyProgram,
}

pub enum AnyProgram {
    Rc(Program<Rc>),
    Reuse(Program<Reuse>),
    Ready(Program<Ready>),
}

pub const FIXTURES: &[Fixture] = &[
    Fixture {
        name: "omitted-decrement",
        expect: ViolationKind::LeakedCredit,
        output: "7\n",
        build: |fixed| AnyProgram::Rc(omitted_decrement(fixed)),
    },
    Fixture {
        name: "stale-reuse-generation",
        expect: ViolationKind::StaleGeneration,
        output: "7\n9\n",
        build: |fixed| AnyProgram::Reuse(stale_reuse_generation(fixed)),
    },
    Fixture {
        name: "uninitialized-scan",
        expect: ViolationKind::UninitializedScan,
        output: "1\n",
        build: |fixed| AnyProgram::Rc(uninitialized_scan(fixed)),
    },
    Fixture {
        name: "duplicate-finalizer",
        expect: ViolationKind::DuplicateFinalizer,
        output: "5\n",
        build: |fixed| AnyProgram::Ready(duplicate_finalizer(fixed)),
    },
    Fixture {
        name: "leaked-token",
        expect: ViolationKind::LeakedToken,
        output: "3\n4\n",
        build: |fixed| AnyProgram::Reuse(leaked_token(fixed)),
    },
    Fixture {
        name: "borrowed-store",
        expect: ViolationKind::BorrowedStore,
        output: "8\n",
        build: |fixed| AnyProgram::Rc(borrowed_store(fixed)),
    },
    Fixture {
        name: "region-escape",
        expect: ViolationKind::RegionEscape,
        output: "6\n",
        build: |fixed| AnyProgram::Rc(region_escape(fixed)),
    },
    Fixture {
        name: "view-after-detach",
        expect: ViolationKind::ViewDetached,
        output: "1\n",
        build: |fixed| AnyProgram::Rc(view_after_detach(fixed)),
    },
];

/// `let x = Box(7); print(x.value)` and, in the broken twin, no release of x.
pub fn omitted_decrement(fixed: bool) -> Program<Rc> {
    let (l, c) = layouts();
    let boxed = c.boxed;
    let mut f = F::<Rc>::new("main", c);
    let seven = f.int(7);
    let x = f.boxed(seven);
    let v = f.unbox(x);
    f.print(v);
    if fixed {
        f.release(x, boxed);
    }
    f.ret(None);
    let (target, table) = l.finish();
    let main = f.finish(&table);
    program(target, table, vec![main])
}

/// A plain alias of a block survives its reset and reuse; the broken twin
/// reads through it after the storage was reused for another value.
pub fn stale_reuse_generation(fixed: bool) -> Program<Reuse> {
    let (l, c) = layouts();
    let boxed = c.boxed;
    let mut f = F::<Reuse>::new("main", c);
    let seven = f.int(7);
    let x = f.boxed(seven);
    let s = f.b.slot(BLOCK, boxed, SlotKind::Local);
    let at = f.b.address(AddressBase::Slot(s), &[], BLOCK, boxed);
    f.inst(Inst::StorageLive { slot: s });
    f.inst(Inst::Store {
        address: at,
        value: x,
        version: x,
    });
    let alias = f.b.plain(BLOCK, boxed);
    f.inst(Inst::Read {
        result: alias,
        address: at,
        reaching: x,
        mode: ReadMode::Bits,
    });
    let x2 = f.b.owned(BLOCK, boxed);
    f.inst(Inst::Read {
        result: x2,
        address: at,
        reaching: x,
        mode: ReadMode::Take,
    });
    f.inst(Inst::StorageDead {
        slot: s,
        if_live: false,
    });
    let v = f.unbox(alias);
    f.print(v);
    let token = f.b.owned(ValueRepr::Token, boxed);
    let failure = f.propagates();
    f.inst(Inst::Reset {
        result: token,
        value: x2,
        layout: boxed,
        live: MaskRef::All,
        anchor: CLEANUP,
        failure,
    });
    let nine = f.int(9);
    let k =
        f.b.construct(ConstructorKind::Tuple, boxed, &[(0, nine)], HEAP);
    let y = f.b.owned(BLOCK, boxed);
    let failure = f.propagates();
    f.inst(Inst::Reuse {
        result: y,
        token,
        construct: k,
        kept: None,
        failure,
    });
    let v = f.unbox(y);
    f.print(v);
    if !fixed {
        f.unbox(alias);
    }
    f.release(y, boxed);
    f.ret(None);
    let (target, table) = l.finish();
    let main = f.finish(&table);
    program(target, table, vec![main])
}

/// A field moved out of a block, then the block released with every field
/// live (broken) or with the moved field masked out (fixed).
pub fn uninitialized_scan(fixed: bool) -> Program<Rc> {
    let (mut l, c) = layouts();
    let boxed = c.boxed;
    let pair = l.record(&[boxed, boxed]);
    let pair_box = l.block(pair);
    let mut f = F::<Rc>::new("main", c);
    let one = f.int(1);
    let a = f.boxed(one);
    let two = f.int(2);
    let b = f.boxed(two);
    let k =
        f.b.construct(ConstructorKind::Tuple, pair_box, &[(0, a), (1, b)], HEAP);
    let p = f.b.owned(BLOCK, pair_box);
    let failure = f.propagates();
    f.inst(Inst::Construct {
        result: p,
        construct: k,
        failure,
    });
    let at = f.b.address(
        AddressBase::Value(p),
        &[ProjectionStep::Field(0)],
        BLOCK,
        boxed,
    );
    let a2 = f.b.owned(BLOCK, boxed);
    f.inst(Inst::Read {
        result: a2,
        address: at,
        reaching: p,
        mode: ReadMode::Take,
    });
    let v = f.unbox(a2);
    f.print(v);
    let live = if fixed {
        MaskRef::Static(f.b.mask(2, &[1]))
    } else {
        MaskRef::All
    };
    f.release_masked(p, pair_box, live);
    f.release(a2, boxed);
    f.ret(None);
    let (target, table) = l.finish();
    let main = f.finish(&table);
    program(target, table, vec![main])
}

/// A resource finalized explicitly (Ready lowering); the broken twin then
/// releases the whole resource, which runs its finalizer a second time.
pub fn duplicate_finalizer(fixed: bool) -> Program<Ready> {
    let (mut l, c) = layouts();
    let boxed = c.boxed;
    let resource = l.resource(&[boxed], Some(path("Res.drop")));
    let resource_ref = l.uncounted(resource);
    let (target, mut table) = l.finish();
    // The finalizer prints the boxed value it owns.
    let mut fin = F::<Ready>::new("Res.drop", c);
    source_finalizer(&mut fin.b, &mut table, resource_ref);
    let this = fin
        .b
        .value(ValueRepr::Uncounted, resource_ref, Holding::Plain);
    fin.b.parameter(this);
    let inner = fin.load(this, &[ProjectionStep::Field(0)], BLOCK, boxed);
    let v = fin.unbox(inner);
    fin.print(v);
    fin.ret(None);
    let fin = fin.finish(&table);

    let mut f = F::<Ready>::new("main", c);
    let five = f.int(5);
    let x = f.boxed(five);
    let k =
        f.b.construct(ConstructorKind::Tuple, resource, &[(0, x)], HEAP);
    let r = f.b.owned(ValueRepr::Aggregate, resource);
    let failure = f.propagates();
    f.inst(Inst::Construct {
        result: r,
        construct: k,
        failure,
    });
    let s = f.b.slot(ValueRepr::Aggregate, resource, SlotKind::Local);
    let at =
        f.b.address(AddressBase::Slot(s), &[], ValueRepr::Aggregate, resource);
    f.inst(Inst::StorageLive { slot: s });
    f.inst(Inst::Store {
        address: at,
        value: r,
        version: r,
    });
    f.inst(Inst::FinalizeBegin {
        address: at,
        anchor: CLEANUP,
    });
    let drop = f.b.direct(path("Res.drop"));
    // A borrowed address argument writes nothing back; its version stays unused.
    let unwritten = f.b.plain(ValueRepr::Aggregate, resource);
    f.call(
        drop,
        &[(
            Passing::Borrow,
            ArgumentValue::Address {
                address: at,
                version: unwritten,
            },
        )],
        None,
    );
    f.inst(Inst::FinalizeEnd {
        address: at,
        anchor: CLEANUP,
    });
    if fixed {
        let field = f.b.address(
            AddressBase::Slot(s),
            &[ProjectionStep::Field(0)],
            BLOCK,
            boxed,
        );
        let inner = f.b.owned(BLOCK, boxed);
        f.inst(Inst::Read {
            result: inner,
            address: field,
            reaching: r,
            mode: ReadMode::Take,
        });
        f.release(inner, boxed);
    } else {
        let whole = f.b.owned(ValueRepr::Aggregate, resource);
        f.inst(Inst::Read {
            result: whole,
            address: at,
            reaching: r,
            mode: ReadMode::Take,
        });
        f.release(whole, resource);
    }
    f.inst(Inst::StorageDead {
        slot: s,
        if_live: false,
    });
    f.ret(None);
    let main = f.finish(&table);
    program(target, table, vec![main, fin])
}

/// A reset donor's token is neither reused nor disposed (broken).
pub fn leaked_token(fixed: bool) -> Program<Reuse> {
    let (l, c) = layouts();
    let boxed = c.boxed;
    let mut f = F::<Reuse>::new("main", c);
    let three = f.int(3);
    let x = f.boxed(three);
    let v = f.unbox(x);
    f.print(v);
    let token = f.b.owned(ValueRepr::Token, boxed);
    let failure = f.propagates();
    f.inst(Inst::Reset {
        result: token,
        value: x,
        layout: boxed,
        live: MaskRef::All,
        anchor: CLEANUP,
        failure,
    });
    let four = f.int(4);
    let y = f.boxed(four);
    if fixed {
        f.inst(Inst::Dispose { token });
    }
    let v = f.unbox(y);
    f.print(v);
    f.release(y, boxed);
    f.ret(None);
    let (target, table) = l.finish();
    let main = f.finish(&table);
    program(target, table, vec![main])
}

/// `keep(borrow a) -> Box[Box[cell]]` stores its borrowed parameter into a
/// new block; the fixed twin takes a holder (Dup) first.
pub fn borrowed_store(fixed: bool) -> Program<Rc> {
    let (mut l, c) = layouts();
    let boxed = c.boxed;
    let outer = l.record(&[boxed]);
    let outer_box = l.block(outer);
    let table = l.table.clone();

    let mut keep = F::<Rc>::new("keep", c);
    let a = keep.b.value(BLOCK, boxed, Holding::Plain);
    keep.b.parameter(a);
    if fixed {
        keep.inst(Inst::Dup { value: a });
    }
    let k = keep
        .b
        .construct(ConstructorKind::Tuple, outer_box, &[(0, a)], HEAP);
    let h = keep.b.owned(BLOCK, outer_box);
    let failure = keep.propagates();
    keep.inst(Inst::Construct {
        result: h,
        construct: k,
        failure,
    });
    keep.ret(Some(h));
    let keep = keep.finish(&table);

    let mut f = F::<Rc>::new("main", c);
    let eight = f.int(8);
    let a = f.boxed(eight);
    let v = f.unbox(a);
    f.print(v);
    let h = f.b.owned(BLOCK, outer_box);
    let callee = f.b.direct(path("keep"));
    f.call(
        callee,
        &[(Passing::Borrow, ArgumentValue::Value(a))],
        Some(h),
    );
    f.release(h, outer_box);
    f.release(a, boxed);
    f.ret(None);
    let main = f.finish(&table);
    let (target, table) = l.finish();
    program(target, table, vec![main, keep])
}

/// A heap block keeps a reference into a compiler-local region past its exit
/// (broken); the fixed twin copies the value out first.
pub fn region_escape(fixed: bool) -> Program<Rc> {
    let (mut l, c) = layouts();
    let (cell, u64) = (c.cell, c.u64);
    let cell_ref = l.uncounted(cell);
    let holder = l.record(&[cell_ref]);
    let holder_box = l.block(holder);
    let boxed = c.boxed;
    let table = l.table.clone();
    let mut f = F::<Rc>::new("main", c);
    let r = f.b.region();
    let mark = f.b.owned(ValueRepr::RegionMark, u64);
    f.inst(Inst::RegionEnter {
        result: mark,
        region: r,
    });
    let six = f.int(6);
    let at = f.b.plain(ValueRepr::Uncounted, cell_ref);
    let failure = f.propagates();
    f.inst(Inst::RegionReserve {
        result: at,
        mark,
        layout: cell,
        extent: six,
        failure,
    });
    let field = f.b.address(
        AddressBase::Value(at),
        &[ProjectionStep::Field(0)],
        U64,
        u64,
    );
    let version = f.b.plain(U64, u64);
    f.inst(Inst::Store {
        address: field,
        value: six,
        version,
    });
    let v = f.load(at, &[ProjectionStep::Field(0)], U64, u64);
    f.print(v);
    let (h, layout) = if fixed {
        let copied = f.load(at, &[ProjectionStep::Field(0)], U64, u64);
        (f.boxed(copied), boxed)
    } else {
        let k =
            f.b.construct(ConstructorKind::Tuple, holder_box, &[(0, at)], HEAP);
        let h = f.b.owned(BLOCK, holder_box);
        let failure = f.propagates();
        f.inst(Inst::Construct {
            result: h,
            construct: k,
            failure,
        });
        (h, holder_box)
    };
    let failure = f.propagates();
    f.inst(Inst::RegionExit {
        mark,
        region: r,
        failure,
    });
    f.release(h, layout);
    f.ret(None);
    let main = f.finish(&table);
    let (target, table) = l.finish();
    program(target, table, vec![main])
}

/// A view of `p.a` is live while a sibling write detaches `p`'s shared block.
/// The broken twin uses the view after the detach; the fixed twin
/// materializes its value and ends the loan before the write.
pub fn view_after_detach(fixed: bool) -> Program<Rc> {
    let (mut l, c) = layouts();
    let u64 = c.u64;
    let point = l.record(&[u64, u64]);
    let point_box = l.block(point);
    let table = l.table.clone();
    let mut f = F::<Rc>::new("main", c);
    let one = f.int(1);
    let two = f.int(2);
    let k = f.b.construct(
        ConstructorKind::Tuple,
        point_box,
        &[(0, one), (1, two)],
        HEAP,
    );
    let p0 = f.b.owned(BLOCK, point_box);
    let failure = f.propagates();
    f.inst(Inst::Construct {
        result: p0,
        construct: k,
        failure,
    });
    let s = f.b.slot(BLOCK, point_box, SlotKind::Local);
    let at = f.b.address(AddressBase::Slot(s), &[], BLOCK, point_box);
    f.inst(Inst::StorageLive { slot: s });
    f.inst(Inst::Store {
        address: at,
        value: p0,
        version: p0,
    });
    // A second holder: the block is shared.
    let q = f.b.owned(BLOCK, point_box);
    let failure = f.propagates();
    f.inst(Inst::Copy {
        result: q,
        address: at,
        reaching: p0,
        failure,
    });
    let pa =
        f.b.address(AddressBase::Slot(s), &[ProjectionStep::Field(0)], U64, u64);
    let loan = f.b.loan(pa, false);
    f.inst(Inst::BeginLoan {
        loan,
        address: pa,
        mutable: false,
        if_inactive: false,
    });
    let view = f.b.value(U64, u64, Holding::Borrowed(loan));
    f.inst(Inst::Read {
        result: view,
        address: pa,
        reaching: p0,
        mode: ReadMode::Borrow,
    });
    f.print(view);
    if fixed {
        f.inst(Inst::EndLoan {
            loan,
            if_active: false,
        });
    }
    // Detach for the sibling write `p.b = 20`: copy the fields to a new block.
    let old = f.b.owned(BLOCK, point_box);
    f.inst(Inst::Read {
        result: old,
        address: at,
        reaching: p0,
        mode: ReadMode::Take,
    });
    let a = f.load(old, &[ProjectionStep::Field(0)], U64, u64);
    let twenty = f.int(20);
    let k = f.b.construct(
        ConstructorKind::Tuple,
        point_box,
        &[(0, a), (1, twenty)],
        HEAP,
    );
    let p1 = f.b.owned(BLOCK, point_box);
    let failure = f.propagates();
    f.inst(Inst::Construct {
        result: p1,
        construct: k,
        failure,
    });
    f.release(old, point_box);
    f.inst(Inst::Store {
        address: at,
        value: p1,
        version: p1,
    });
    if !fixed {
        let zero = f.int(0);
        let sum = f.b.plain(U64, u64);
        let failure = f.propagates();
        f.inst(Inst::Binary {
            result: sum,
            op: BinaryOp::Add,
            left: view,
            right: zero,
            failure,
        });
        f.inst(Inst::EndLoan {
            loan,
            if_active: false,
        });
    }
    f.release(q, point_box);
    let last = f.b.owned(BLOCK, point_box);
    f.inst(Inst::Read {
        result: last,
        address: at,
        reaching: p1,
        mode: ReadMode::Take,
    });
    f.release(last, point_box);
    f.inst(Inst::StorageDead {
        slot: s,
        if_live: false,
    });
    f.ret(None);
    let main = f.finish(&table);
    let (target, table) = l.finish();
    program(target, table, vec![main])
}

/// Hand-built source drop bodies declare the same Borrow-self ABI as source.
pub fn source_finalizer<S: hirsel_lir::stage::Stage>(
    b: &mut hirsel_heapcheck::build::UnitBuilder<S>,
    table: &mut hirsel_lir::layout_types::LayoutTable,
    layout: hirsel_lir::layout_types::LayoutId,
) {
    use hirsel_lir::abi_types::*;
    let mut abi = table.fn_abis[0].clone();
    abi.parameters = vec![AbiParameter {
        source: 0,
        convention: hirsel_core_contract::Passing::Borrow,
        passing: ParameterPassing::ReadOnlyAddress,
        layout,
        slots: vec![PhysicalSlot {
            ty: PhysicalType::Pointer,
            size: 8,
            align: 8,
        }],
        extension: ScalarExtension::None,
    }];
    b.unit.header.abi = FnAbiId(table.fn_abis.len() as u32);
    b.unit.header.origin = UnitOrigin::Source;
    table.fn_abis.push(abi);
}

/// Run every existing H fixture in incremental and old per-step recount modes.
/// Fixture operations are stateless Printer models; campaigns recreate theirs.
pub fn run<S: Stage>(
    program: &Program<S>,
    entry: &hirsel_syntax::DefPath,
    args: &[hirsel_heapcheck::Arg],
    ops: &mut dyn Operations,
    config: &hirsel_heapcheck::Config,
) -> hirsel_heapcheck::Run {
    let incremental = hirsel_heapcheck::run(
        program,
        entry,
        args,
        ops,
        &hirsel_heapcheck::Config {
            full_recount_every_step: false,
            ..config.clone()
        },
    );
    // The 30k small-stack stress control has always disabled per-step recount;
    // its ordinary-size twin exercises both audits without a quadratic test.
    if config.check_every_step {
        let reference = hirsel_heapcheck::run(
            program,
            entry,
            args,
            ops,
            &hirsel_heapcheck::Config {
                full_recount_every_step: true,
                ..config.clone()
            },
        );
        assert_eq!(incremental, reference, "audit modes disagree");
    }
    if config.check_every_step && ops.domain() == hirsel_heapcheck::OperationDomain::Simulation {
        let campaign = fault_campaign(
            program,
            entry,
            args,
            &mut || {
                ops.fork_simulation()
                    .expect("fixture simulation is cloneable")
            },
            config,
        );
        assert_eq!(
            incremental, campaign.clean,
            "ordinary and campaign clean run disagree"
        );
    }
    incremental
}

pub fn fault_campaign<S: Stage>(
    program: &Program<S>,
    entry: &hirsel_syntax::DefPath,
    args: &[hirsel_heapcheck::Arg],
    ops: &mut dyn FnMut() -> Box<dyn Operations>,
    config: &hirsel_heapcheck::Config,
) -> hirsel_heapcheck::Campaign {
    let incremental = hirsel_heapcheck::fault_campaign(
        program,
        entry,
        args,
        ops,
        &hirsel_heapcheck::Config {
            full_recount_every_step: false,
            ..config.clone()
        },
    );
    for full_recount_every_step in [false, true] {
        for reexecute_campaign in [false, true] {
            let reference = hirsel_heapcheck::fault_campaign(
                program,
                entry,
                args,
                ops,
                &hirsel_heapcheck::Config {
                    full_recount_every_step,
                    reexecute_campaign,
                    compare_campaigns: false,
                    ..config.clone()
                },
            );
            assert_eq!(incremental, reference, "campaign modes disagree");
        }
    }
    incremental
}
