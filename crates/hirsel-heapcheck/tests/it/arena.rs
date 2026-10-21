//! Arena destinations: what H checks on its own,
//! sharing no code with C. A token is a checked cell naming a ledger region;
//! an arena block is an immortal allocation of that region; closing the arena
//! drops what its blocks hold and frees them. Each broken program has a valid
//! twin that passes the terminal audit with the same output.
use crate::fixtures;

use fixtures::*;
use hirsel_heapcheck::build::{Layouts, path};
use hirsel_heapcheck::{Config, Program, Run, ViolationKind};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{LayoutId, TargetSpec};
use hirsel_lir::stage::Rc;

const IN_ARENA: Placement = Placement::Fixed(StorageOwner::Arena(ArenaInstanceId(0)));
const TOKEN: ValueRepr = ValueRepr::Arena;

fn execute(p: &Program<Rc>) -> Run {
    run(p, &path("main"), &[], &mut Printer, &Config::default())
}

/// `main` with an open arena whose token sits in a local slot, as a
/// parameter's would, and that slot as arena instance 0.
struct Open {
    f: F<Rc>,
    l: Layouts,
    mark: ValueId,
    region: RegionInstanceId,
    token: ValueId,
    arena: LayoutId,
}

fn open() -> Open {
    let mut l = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let c = common(&mut l);
    let arena = l.arena();
    let mut f = F::<Rc>::new("main", c);
    let region = f.b.region();
    let mark = f.b.owned(ValueRepr::RegionMark, c.u64);
    f.inst(Inst::RegionEnter {
        result: mark,
        region,
    });
    let token = f.b.plain(TOKEN, arena);
    f.inst(Inst::ArenaToken {
        result: token,
        mark,
        budget: None,
    });
    let slot = f.b.slot(TOKEN, arena, SlotKind::Local);
    f.b.unit.tables.arenas.push(ExternalInstance { slot });
    f.inst(Inst::StorageLive { slot });
    Open {
        f,
        l,
        mark,
        region,
        token,
        arena,
    }
}

impl Open {
    /// Store `value` into the token slot through an address typed
    /// `repr`/`layout`.
    fn store(&mut self, value: ValueId, repr: ValueRepr, layout: LayoutId) {
        let slot = self.f.b.unit.tables.arenas[0].slot;
        let at = self.f.b.address(AddressBase::Slot(slot), &[], repr, layout);
        let failure = self.f.propagates();
        self.f.inst(Inst::Address {
            address: at,
            failure,
        });
        let version = self.f.b.plain(repr, layout);
        self.f.inst(Inst::Store {
            address: at,
            value,
            version,
        });
    }
    fn close(&mut self) {
        let failure = self.f.propagates();
        self.f.inst(Inst::RegionExit {
            mark: self.mark,
            region: self.region,
            failure,
        });
    }
    /// A block of `layout` built in arena instance 0.
    fn build(&mut self, layout: LayoutId, fields: &[(u32, ValueId)]) -> ValueId {
        let k = self
            .f
            .b
            .construct(ConstructorKind::Tuple, layout, fields, IN_ARENA);
        self.f.b.unit.tables.constructs[k.index()].destination =
            DestinationClass::Arena(self.f.b.unit.tables.arenas[0].slot);
        let v = self.f.b.owned(BLOCK, layout);
        let failure = self.f.propagates();
        self.f.inst(Inst::Construct {
            result: v,
            construct: k,
            failure,
        });
        v
    }
    fn finish(self) -> Program<Rc> {
        let table = self.l.table.clone();
        let main = self.f.finish(&table);
        let (target, table) = self.l.finish();
        program(target, table, vec![main])
    }
}

/// `print(Box(6).value)` with the box built in the arena: before the arena
/// closes (valid), or after, through the token still in its slot (broken).
fn closed(broken: bool) -> Program<Rc> {
    let mut o = open();
    let token = o.token;
    let arena = o.arena;
    o.store(token, TOKEN, arena);
    if broken {
        o.close();
    }
    let six = o.f.int(6);
    let boxed = o.f.c.boxed;
    let v = o.build(boxed, &[(0, six)]);
    let n = o.f.unbox(v);
    o.f.print(n);
    if !broken {
        o.close();
    }
    o.f.ret(None);
    o.finish()
}

#[test]
fn an_arena_construction_needs_its_arena_open() {
    let run = execute(&closed(false));
    let audit = run
        .result
        .as_ref()
        .unwrap_or_else(|v| panic!("the valid twin: {v}"));
    assert_eq!(String::from_utf8_lossy(&audit.observation.output), "6\n");
    assert_eq!(
        execute(&closed(true)).violation(),
        Some(ViolationKind::UseAfterFree)
    );
}

/// The token slot holds the token (valid) or the integer bits 6 stored
/// through a `u64` address (forged).
fn forged(broken: bool) -> Program<Rc> {
    let mut o = open();
    let six = o.f.int(6);
    if broken {
        let u64 = o.f.c.u64;
        o.store(six, U64, u64);
    } else {
        let (token, arena) = (o.token, o.arena);
        o.store(token, TOKEN, arena);
    }
    let boxed = o.f.c.boxed;
    let v = o.build(boxed, &[(0, six)]);
    let n = o.f.unbox(v);
    o.f.print(n);
    o.close();
    o.f.ret(None);
    o.finish()
}

#[test]
fn an_arena_token_is_a_checked_cell_not_bits() {
    let run = execute(&forged(false));
    assert!(run.result.is_ok(), "the valid twin: {:?}", run.result);
    assert_eq!(
        execute(&forged(true)).violation(),
        Some(ViolationKind::ForgedPointer)
    );
}

/// An arena block holding a counted heap child (MEM-4: the arena releases
/// the heap blocks it holds when it closes). The valid twin closes the arena;
/// the broken one never does, so the child and the arena outlive the task.
fn held(broken: bool) -> Program<Rc> {
    let mut o = open();
    let (token, arena) = (o.token, o.arena);
    o.store(token, TOKEN, arena);
    let boxed = o.f.c.boxed;
    let holder = o.l.record(&[boxed]);
    let holder_box = o.l.block(holder);
    let six = o.f.int(6);
    let child = o.f.boxed(six);
    let h = o.build(holder_box, &[(0, child)]);
    let inner = o.f.load(h, &[ProjectionStep::Field(0)], BLOCK, boxed);
    let n = o.f.unbox(inner);
    o.f.print(n);
    if !broken {
        o.close();
    }
    o.f.ret(None);
    o.finish()
}

#[test]
fn closing_an_arena_releases_the_heap_blocks_it_holds() {
    let run = execute(&held(false));
    let audit = run
        .result
        .as_ref()
        .unwrap_or_else(|v| panic!("the valid twin: {v}"));
    assert_eq!(String::from_utf8_lossy(&audit.observation.output), "6\n");
    assert_eq!(
        execute(&held(true)).violation(),
        Some(ViolationKind::LiveRegion)
    );
}

/// An arena block stored in a heap block that outlives the arena (broken),
/// or its value copied into a fresh heap box (valid).
fn escaped(broken: bool) -> Program<Rc> {
    let mut o = open();
    let (token, arena) = (o.token, o.arena);
    o.store(token, TOKEN, arena);
    let boxed = o.f.c.boxed;
    let holder = o.l.record(&[boxed]);
    let holder_box = o.l.block(holder);
    let six = o.f.int(6);
    let v = o.build(boxed, &[(0, six)]);
    let (kept, layout) = if broken {
        let k =
            o.f.b
                .construct(ConstructorKind::Tuple, holder_box, &[(0, v)], HEAP);
        let h = o.f.b.owned(BLOCK, holder_box);
        let failure = o.f.propagates();
        o.f.inst(Inst::Construct {
            result: h,
            construct: k,
            failure,
        });
        (h, holder_box)
    } else {
        let n = o.f.unbox(v);
        (o.f.boxed(n), boxed)
    };
    o.close();
    o.f.release(kept, layout);
    o.f.ret(None);
    o.finish()
}

#[test]
fn an_arena_block_never_outlives_its_arena() {
    let run = execute(&escaped(false));
    assert!(run.result.is_ok(), "the valid twin: {:?}", run.result);
    assert_eq!(
        execute(&escaped(true)).violation(),
        Some(ViolationKind::RegionEscape)
    );
}

/// Two arenas: the inner closes first (valid), or the outer does (broken).
fn nested(broken: bool) -> Program<Rc> {
    let mut o = open();
    let u64 = o.f.c.u64;
    let inner = o.f.b.region();
    let mark = o.f.b.owned(ValueRepr::RegionMark, u64);
    o.f.inst(Inst::RegionEnter {
        result: mark,
        region: inner,
    });
    let exit_inner = |o: &mut Open| {
        let failure = o.f.propagates();
        o.f.inst(Inst::RegionExit {
            mark,
            region: inner,
            failure,
        });
    };
    if broken {
        o.close();
        exit_inner(&mut o);
    } else {
        exit_inner(&mut o);
        o.close();
    }
    o.f.ret(None);
    o.finish()
}

#[test]
fn arenas_close_innermost_first() {
    let run = execute(&nested(false));
    assert!(run.result.is_ok(), "the valid twin: {:?}", run.result);
    assert_eq!(
        execute(&nested(true)).violation(),
        Some(ViolationKind::Malformed)
    );
}

/// Same physical heap child and output as `held`: forging the elision flag
/// abandons its credit when the arena closes. H must execute that omission,
/// then discover the leaked cell independently of C or any result summary.
#[test]
fn an_elided_arena_drop_cannot_hide_a_heap_child() {
    for recount in [false, true] {
        let mut p = held(false);
        let config = Config {
            full_recount_every_step: recount,
            ..Config::default()
        };
        let clean = run(&p, &path("main"), &[], &mut Printer, &config);
        assert!(clean.result.is_ok(), "{:?}", clean.result);
        p.units[0]
            .tables
            .constructs
            .last_mut()
            .unwrap()
            .arena_no_drop = true;
        let broken = run(&p, &path("main"), &[], &mut Printer, &config);
        assert_eq!(broken.output, clean.output);
        assert_eq!(
            broken.violation(),
            Some(ViolationKind::LeakedCredit),
            "{:?}",
            broken.result
        );
    }
}

#[test]
fn elided_drops_of_same_arena_children_pass_the_terminal_audit() {
    let mut o = open();
    let (token, arena) = (o.token, o.arena);
    o.store(token, TOKEN, arena);
    let six = o.f.int(6);
    let boxed = o.f.c.boxed;
    let child = o.build(boxed, &[(0, six)]);
    let holder = o.l.record(&[boxed]);
    let holder_box = o.l.block(holder);
    let _parent = o.build(holder_box, &[(0, child)]);
    o.f.b
        .unit
        .tables
        .constructs
        .last_mut()
        .unwrap()
        .arena_no_drop = true;
    o.close();
    o.f.ret(None);
    let p = o.finish();
    for recount in [false, true] {
        let config = Config {
            full_recount_every_step: recount,
            ..Config::default()
        };
        let result = run(&p, &path("main"), &[], &mut Printer, &config);
        assert!(result.result.is_ok(), "{:?}", result.result);
    }
}

fn forged_result_summary(broken: bool) -> Program<Rc> {
    use hirsel_core_contract::Passing;
    use hirsel_lir::abi_types::*;
    let mut o = open();
    let (token, arena) = (o.token, o.arena);
    o.store(token, TOKEN, arena);
    let boxed = o.f.c.boxed;
    let mut callee = F::<Rc>::new("child", o.f.c);
    let argument = callee.b.plain(TOKEN, arena);
    callee.b.parameter(argument);
    let six = callee.int(6);
    let child = callee.boxed(six);
    callee.ret(Some(child));
    let pointer = PhysicalSlot {
        ty: PhysicalType::Pointer,
        size: 8,
        align: 8,
    };
    let mut abi = o.l.table.fn_abis[0].clone();
    abi.result = Some(AbiResult {
        borrowed_parameters: vec![],
        layout: boxed,
        register: Some(hirsel_lir::layout_types::ScalarKind::Pointer),
        slots: vec![pointer],
    });
    abi.parameters = vec![AbiParameter {
        source: 0,
        convention: Passing::Borrow,
        passing: ParameterPassing::ByValue,
        layout: arena,
        slots: vec![pointer],
        extension: ScalarExtension::None,
    }];
    abi.destination = true;
    abi.result_arena = broken.then_some(0);
    let id = FnAbiId(o.l.table.fn_abis.len() as u32);
    o.l.table.fn_abis.push(abi);
    callee.b.unit.header.abi = id;
    let result = o.f.b.owned(BLOCK, boxed);
    let target = o.f.b.direct(path("child"));
    o.f.call(
        target,
        &[(Passing::Borrow, ArgumentValue::Value(token))],
        Some(result),
    );
    o.f.b.unit.tables.calls.last_mut().unwrap().abi = id;
    let holder = o.l.record(&[boxed]);
    let holder_box = o.l.block(holder);
    let h = o.build(holder_box, &[(0, result)]);
    o.f.b
        .unit
        .tables
        .constructs
        .last_mut()
        .unwrap()
        .arena_no_drop = broken;
    let inner = o.f.load(h, &[ProjectionStep::Field(0)], BLOCK, boxed);
    let n = o.f.unbox(inner);
    o.f.print(n);
    o.close();
    o.f.ret(None);
    let table = o.l.table.clone();
    let callee = callee.finish(&table);
    let main = o.f.finish(&table);
    let (target, table) = o.l.finish();
    program(target, table, vec![main, callee])
}

#[test]
fn a_forged_result_summary_does_not_conceal_a_leaked_heap_credit() {
    for recount in [false, true] {
        let config = Config {
            full_recount_every_step: recount,
            ..Config::default()
        };
        let clean = run(
            &forged_result_summary(false),
            &path("main"),
            &[],
            &mut Printer,
            &config,
        );
        assert!(clean.result.is_ok(), "{:?}", clean.result);
        let broken = run(
            &forged_result_summary(true),
            &path("main"),
            &[],
            &mut Printer,
            &config,
        );
        assert_eq!(broken.output, clean.output);
        assert_eq!(
            broken.violation(),
            Some(ViolationKind::LeakedCredit),
            "{:?}",
            broken.result
        );
    }
}
