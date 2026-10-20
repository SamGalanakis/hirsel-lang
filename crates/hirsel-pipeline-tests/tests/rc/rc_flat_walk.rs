//! Real imported loops, independent validator mutations, and byte-count evidence.
use crate::rc_support as support;
use hirsel_lir::ir::*;
use hirsel_lir::stage::{Rc, Value};
use hirsel_lir::validate::{Inputs, ViolationKind};
use hirsel_rc::flat_walk::Options;
use support::*;

const TYPES: &str =
    "flat type Item { text: String, n: u64 }\nflat type Items { items: List[Item] }\n";
fn source(body: &str) -> String {
    format!("{TYPES}fn sum(xs: Items) -> u64 {{ let mut total: u64 = 0\n{body}\ntotal\n}}")
}
fn lowered(src: &str, enabled: bool) -> (hirsel_rc::instance::Program, hirsel_rc::import::Lowered) {
    let p = compile(src);
    let l = hirsel_rc::import::lower_with_options(&p, &target(), &Options { enabled }).unwrap();
    let v = violations(&p, &l);
    assert!(
        v.is_empty(),
        "{v:#?}\n{}",
        l.units.iter().map(dump).collect::<Vec<_>>().join("\n")
    );
    (p, l)
}
fn count(u: &LirUnit<Value>) -> usize {
    u.instructions
        .iter()
        .filter(|i| matches!(i, Inst::FlatNext { .. }))
        .count()
}

#[test]
fn a_zero_based_unit_stride_loop_is_rewritten_and_the_option_preserves_index() {
    let src = source("for x in xs.items { total += x.n }");
    let (_, on) = lowered(&src, true);
    let (_, off) = lowered(&src, false);
    assert_eq!(count(unit_named(&on, "sum")), 1);
    assert_eq!(count(unit_named(&off, "sum")), 0);
    assert!(
        unit_named(&on, "sum")
            .slots
            .iter()
            .any(|s| matches!(s.kind, SlotKind::LoopCarried(_)))
    );
    assert!(unit_named(&on, "sum").addresses.iter().any(|a| {
        a.projection
            .get(&unit_named(&on, "sum").pools.projections)
            .unwrap()
            .iter()
            .any(|p| matches!(p, ProjectionStep::FlatAt { .. }))
    }));
}

#[test]
fn a_break_and_a_continue_after_the_binding_keep_the_cursor_valid() {
    for body in [
        "for x in xs.items { if x.n == 2 { break }; total += x.n }",
        "for x in xs.items { if x.n == 2 { continue }; total += x.n }",
        "for x in xs.items { continue }",
    ] {
        let (_, l) = lowered(&source(body), true);
        assert_eq!(
            count(unit_named(&l, "sum")),
            1,
            "{}",
            dump(unit_named(&l, "sum"))
        );
    }
}

#[test]
fn a_continue_before_a_checked_read_declines_with_a_read_first_twin() {
    let src = source("for x in xs.items { total += x.n }");
    let (_, l) = lowered(&src, false);
    let mut unit = unit_named(&l, "sum").clone();
    let candidate = hirsel_rc::flat_walk::discover(&unit, &l.table)
        .pop()
        .unwrap();
    assert!(hirsel_rc::flat_walk::prove(&unit, &l.table, candidate.clone()).is_some());
    // Hand-built control: let the condition's exit bypass the element prepare
    // and jump straight to the increment. Discovery is intentionally read-only
    // and must decline this graph before any edit.
    let header = unit
        .blocks
        .iter()
        .position(|b| b.parameters.len == 3)
        .unwrap();
    let feedback = unit
        .terminators
        .iter()
        .enumerate()
        .find_map(|(b, t)| match t {
            Terminator::Jump(e)
                if b != 3 && unit.tables.edges[e.index()].target.index() == header =>
            {
                Some(b)
            }
            _ => None,
        })
        .unwrap();
    let Terminator::Branch { no, .. } = unit.terminators[header] else {
        panic!()
    };
    unit.tables.edges[no.index()].target = BlockId(feedback as u32);
    assert!(hirsel_rc::flat_walk::prove(&unit, &l.table, candidate).is_none());
    let rewritten = hirsel_rc::flat_walk::rewrite(unit.clone(), &l.table, &Options::default());
    assert_eq!(rewritten, unit);
    let (_, twin) = lowered(&src, true);
    assert_eq!(count(unit_named(&twin, "sum")), 1);
}

#[test]
fn a_nested_walk_keeps_separate_offsets_and_declines_the_outer_loop() {
    let (_, l) = lowered(
        &source("for x in xs.items { for y in xs.items { total += y.n }; total += x.n }"),
        true,
    );
    assert_eq!(count(unit_named(&l, "sum")), 1);
    let (_, twin) = lowered(
        &source("for x in xs.items { total += x.n }\nfor y in xs.items { total += y.n }"),
        true,
    );
    assert_eq!(count(unit_named(&twin, "sum")), 2);
}

fn stages(
    src: &str,
    enabled: bool,
) -> (hirsel_rc::instance::Program, hirsel_heapcheck::Program<Rc>) {
    let (p, l) = lowered(src, enabled);
    let table = l.table;
    let mut units = vec![];
    for (i, u) in l.units.into_iter().enumerate() {
        let id = hirsel_rc::instance::InstanceId(i as u32);
        let placed = hirsel_rc::placement::place(
            u,
            &table,
            &hirsel_rc::placement::Options::for_instance(false, &l.instances, id),
        )
        .unwrap();
        let rc = hirsel_rc::insert::insert(
            placed,
            &table,
            &hirsel_rc::insert::Proofs::for_instance(&l.instances, id),
        )
        .unwrap();
        let rc = hirsel_rc::elide::elide(rc, &table).unwrap();
        let v = hirsel_lir::validate::check(
            &rc,
            &Inputs {
                memo: Default::default(),
                discharge: None,
                operations: &p.operation_registry(),
                target: &target(),
                layouts: &table,
                manifest: &manifest(&p),
            },
        );
        assert!(v.is_empty(), "{v:?}\n{}", dump(&rc));
        units.push(rc);
    }
    (
        p,
        hirsel_heapcheck::Program {
            target: target(),
            layouts: table,
            units,
            statics: l.statics,
        },
    )
}

/// Attach an independently encoded flat input to a compiled entry. The actual
/// loop is still the production source/import/pass pipeline; this adapter only
/// replaces the entry's borrowed parameter by a read of harness static bytes.
fn input_program(src: &str, enabled: bool, n: u64) -> hirsel_heapcheck::Program<Rc> {
    let (_, mut p) = stages(src, enabled);
    let sum = p
        .units
        .iter_mut()
        .find(|u| u.header.definition == path("sum"))
        .unwrap();
    let value = sum.parameters[0];
    sum.parameters.clear();
    let mut abi = p.layouts.fn_abis[sum.header.abi.0 as usize].clone();
    abi.parameters.clear();
    sum.header.abi = hirsel_lir::FnAbiId(p.layouts.fn_abis.len() as u32);
    p.layouts.fn_abis.push(abi);
    let root = sum
        .slots
        .iter()
        .position(|s| s.kind == SlotKind::Parameter(0))
        .unwrap();
    sum.slots[root].kind = SlotKind::Local;
    let ty = sum.values[value.index()].ty;
    let layout = sum.types[ty.index()].layout;
    let hirsel_lir::layout_types::LayoutKind::Block { payload } =
        p.layouts.layouts[layout.0 as usize].kind
    else {
        panic!()
    };
    let symbol = SymbolId(sum.tables.symbols.len() as u32);
    let sym = Symbol {
        kind: SymbolKind::Constant,
        definition: path("input"),
        instance: None,
    };
    sum.tables.symbols.push(sym.clone());
    let address = AddressId(sum.addresses.len() as u32);
    sum.addresses.push(AddressInfo {
        source_place: None,
        unchecked: Vec::new(),
        base: AddressBase::Slot(SlotId(root as u32)),
        projection: PoolRange::EMPTY,
        ty,
    });
    let read = ValueId(sum.values.len() as u32);
    sum.values.push(ValueInfo {
        ty,
        holding: Holding::Plain,
    });
    let mut bytes = n.to_le_bytes().to_vec();
    for i in 0..n {
        let text = vec![b'x'; (i % 7) as usize];
        let length = 16 + text.len() as u64;
        bytes.extend(length.to_le_bytes());
        bytes.extend((text.len() as u64).to_le_bytes());
        bytes.extend(text);
        bytes.extend((i + 1).to_le_bytes());
    }
    let mut encoded = (bytes.len() as u64).to_le_bytes().to_vec();
    encoded.extend(bytes);
    p.statics.push(hirsel_lir::statics::StaticData::plain(
        sym, payload, encoded,
    ));
    let mut ed = hirsel_rc::insert::Editor::new(sum.clone());
    let failure = ed.unit.tables.failures[0].clone();
    let cleanup = ed.unit.tables.edges[failure.cleanup.index()].clone();
    let args = cleanup
        .arguments
        .get(&ed.unit.pools.values)
        .unwrap()
        .to_vec();
    let cleanup = ed.edge(cleanup.target, &args);
    let f = FailureId(ed.unit.tables.failures.len() as u32);
    ed.unit
        .tables
        .failures
        .push(FailureEdge { cleanup, ..failure });
    ed.code[ed.unit.entry.index()].splice(
        0..0,
        [
            Inst::StorageLive {
                slot: SlotId(root as u32),
            },
            Inst::Address {
                address,
                failure: f,
            },
            Inst::ReadStatic {
                result: read,
                symbol,
            },
            Inst::Store {
                address,
                value: read,
                version: value,
            },
        ],
    );
    *sum = ed.finish();
    let digest = p.layouts.digest();
    for u in &mut p.units {
        u.header.layouts = digest;
    }
    p
}
fn run(p: &hirsel_heapcheck::Program<Rc>) -> hirsel_heapcheck::Run {
    execute(p, true)
}
fn execute(p: &hirsel_heapcheck::Program<Rc>, check_every_step: bool) -> hirsel_heapcheck::Run {
    hirsel_heapcheck::run(
        p,
        &path("sum"),
        &[],
        &mut hirsel_heapcheck::NoOperations,
        &hirsel_heapcheck::Config {
            step_limit: 30_000_000,
            check_every_step,
            ..Default::default()
        },
    )
}

#[test]
fn observations_agree_with_the_rewrite_on_and_off_including_early_exits() {
    for body in [
        "for x in xs.items { total += x.n }",
        "for x in xs.items { if x.n == 7 { break }; total += x.n }",
        "for x in xs.items { if x.n == 7 { continue }; total += x.n }",
        "for x in xs.items { for y in xs.items { total += y.n }; total += x.n }",
    ] {
        let src = source(body);
        let on = input_program(&src, true, 12);
        let off = input_program(&src, false, 12);
        let on = run(&on).result.unwrap();
        let off = run(&off).result.unwrap();
        assert!(on.observation.equivalent(&off.observation), "{body}");
    }
}

#[test]
fn the_validator_replays_the_offset_and_rejects_a_hand_built_forgery() {
    let src = source("for x in xs.items { total += x.n }");
    let (p, mut l) = lowered(&src, true);
    let idx = instances_of(&l, "sum")[0].index();
    let unit = &mut l.units[idx];
    let target = target();
    let manifest = manifest(&p);
    let inputs = Inputs {
        memo: Default::default(),
        discharge: None,
        operations: &p.operation_registry(),
        target: &target,
        layouts: &l.table,
        manifest: &manifest,
    };
    assert!(hirsel_lir::validate::check(unit, &inputs).is_empty());
    // Offset := index has the right type, availability, and loan behavior.
    // Removing only the replay rule therefore makes this control pass.
    for step in &mut unit.pools.projections {
        if let ProjectionStep::FlatAt { index, offset } = step {
            *offset = *index;
        }
    }
    let v = hirsel_lir::validate::check(unit, &inputs);
    assert!(!v.is_empty());
    assert!(
        v.iter()
            .all(|v| v.kind == ViolationKind::Type("flat walk offset replay")),
        "{v:?}"
    );
}

#[test]
fn prefix_reads_are_linear_for_one_hundred_thousand_variable_records() {
    let src = source("for x in xs.items { total += x.n }");
    let p = input_program(&src, true, 100_000);
    let r = execute(&p, false);
    assert!(r.result.is_ok(), "{:?}", r.result);
    assert_eq!(r.counters.flat_prefix_reads, 100_000);
    assert_eq!(
        r.result.unwrap().observation.returned,
        Some(hirsel_heapcheck::Observed::Integer {
            scalar: hirsel_lir::layout_types::ScalarKind::U64,
            bits: 5_000_050_000
        })
    );
    // The disabled traversal's exact quadratic count is measured on bounded
    // inputs so the gate doesn't run billions of checked byte reads.
    let mut reads = vec![];
    for n in [40, 80] {
        let p = input_program(&src, false, n);
        let r = execute(&p, false);
        assert!(r.result.is_ok(), "{:?}", r.result);
        assert!(r.counters.flat_prefix_reads >= n * (n - 1) / 2);
        eprintln!(
            "disabled n={n}: {} prefix reads",
            r.counters.flat_prefix_reads
        );
        reads.push(r.counters.flat_prefix_reads);
    }
    assert_eq!(reads[1] * 40 * 39, reads[0] * 80 * 79);
}

/// Entirely hand-built instructions; production layout derivation only supplies
/// the physical type table. This fixture has no source-level counterpart to hide
/// behind: the wrong offset used to pass all the existing C rules.
fn control(forged: u8) -> Vec<ViolationKind> {
    let src = format!("{TYPES}fn sum(xs: Items, ys: Items) {{}} ");
    let p = compile(&src);
    let l = hirsel_rc::import::lower(&p, &target()).unwrap();
    let template = unit_named(&l, "sum");
    let mut u = LirUnit::<Value>::new(template.header.clone());
    let root_ty =
        template.types[template.values[template.parameters[0].index()].ty.index()].clone();
    let u64layout = l
        .table
        .layouts
        .iter()
        .position(|r| {
            r.kind
                == hirsel_lir::layout_types::LayoutKind::Scalar(
                    hirsel_lir::layout_types::ScalarKind::U64,
                )
        })
        .unwrap();
    let hirsel_lir::layout_types::LayoutKind::Block { payload } =
        l.table.layouts[root_ty.layout.0 as usize].kind
    else {
        panic!()
    };
    let list = l.table.layouts[payload.0 as usize].fields[0].layout;
    let element = l.table.layouts[list.0 as usize].fields[0].layout;
    u.types = vec![
        root_ty,
        LirType {
            repr: ValueRepr::Scalar(hirsel_lir::layout_types::ScalarKind::U64),
            layout: hirsel_lir::LayoutId(u64layout as u32),
        },
        LirType {
            repr: ValueRepr::Aggregate,
            layout: element,
        },
    ];
    u.values = vec![
        ValueInfo {
            ty: LirTypeId(0),
            holding: Holding::Plain
        };
        2
    ];
    u.values.extend(vec![
        ValueInfo {
            ty: LirTypeId(1),
            holding: Holding::Plain
        };
        3
    ]);
    u.parameters = vec![ValueId(0), ValueId(1)];
    u.scopes.push(ScopeInfo { parent: None });
    u.tables.source_anchors.push(SourceAnchor {
        body: 0,
        block: 0,
        statement: 0,
        site: SiteRange { start: 0, end: 1 },
    });
    let zero = ValueId(2);
    let one = ValueId(3);
    let next = ValueId(4);
    u.tables.constants = vec![
        ConstantData {
            ty: LirTypeId(1),
            value: ConstantValue::Integer(0),
        },
        ConstantData {
            ty: LirTypeId(1),
            value: ConstantValue::Integer(if forged == 3 { 2 } else { 1 }),
        },
    ];
    for (base, index, offset) in [
        (ValueId(0), zero, zero),
        (
            ValueId(if forged == 2 { 1 } else { 0 }),
            one,
            if forged == 1 { one } else { next },
        ),
    ] {
        let projection = PoolRange::push(
            &mut u.pools.projections,
            [
                ProjectionStep::Field(0),
                ProjectionStep::FlatAt { index, offset },
            ],
        );
        u.addresses.push(AddressInfo {
            source_place: None,
            unchecked: Vec::new(),
            base: AddressBase::Value(base),
            projection,
            ty: LirTypeId(2),
        });
    }
    for _ in 0..2 {
        let cleanup = EdgeId(u.tables.edges.len() as u32);
        u.tables.edges.push(Edge {
            target: BlockId(1),
            arguments: PoolRange::EMPTY,
        });
        u.tables.failures.push(FailureEdge {
            cleanup,
            rule: Some("MEM-14".into()),
            anchor: SourceAnchorId(0),
            dispose: PoolRange::EMPTY,
        });
    }
    u.instructions = vec![
        Inst::Const {
            result: zero,
            constant: ConstantId(0),
        },
        Inst::Const {
            result: one,
            constant: ConstantId(1),
        },
        Inst::Address {
            address: AddressId(0),
            failure: FailureId(0),
        },
        Inst::FlatNext {
            result: next,
            address: AddressId(0),
        },
        Inst::Address {
            address: AddressId(1),
            failure: FailureId(1),
        },
    ];
    u.blocks = vec![
        Block {
            kind: BlockKind::Body,
            parameters: PoolRange::EMPTY,
            instructions: PoolRange::new(0, 5),
            anchor: SourceAnchorId(0),
        },
        Block {
            kind: BlockKind::FailureCleanup,
            parameters: PoolRange::EMPTY,
            instructions: PoolRange::EMPTY,
            anchor: SourceAnchorId(0),
        },
    ];
    u.terminators = vec![Terminator::Return(None), Terminator::Propagate];
    hirsel_lir::validate::check(
        &u,
        &Inputs {
            memo: Default::default(),
            discharge: None,
            operations: &p.operation_registry(),
            target: &target(),
            layouts: &l.table,
            manifest: &manifest(&p),
        },
    )
    .into_iter()
    .map(|v| v.kind)
    .collect()
}
#[test]
fn hand_built_offset_wrong_sequence_and_wrong_predecessor_controls_have_an_accepted_twin() {
    assert!(control(0).is_empty(), "{:?}", control(0));
    for mutant in 1..=3 {
        assert_eq!(
            control(mutant),
            [ViolationKind::Type("flat walk offset replay")]
        );
    }
}

#[test]
fn a_body_write_to_the_sequence_declines_without_mutating_the_unit() {
    let src = source("for x in xs.items { total += x.n }");
    let (_, l) = lowered(&src, false);
    let mut u = unit_named(&l, "sum").clone();
    let candidate = hirsel_rc::flat_walk::discover(&u, &l.table).pop().unwrap();
    assert!(hirsel_rc::flat_walk::prove(&u, &l.table, candidate.clone()).is_some());
    // A proposed write (including an exclusive call through this address)
    // destroys the immutable-version precondition, regardless of what bytes
    // that write would install. The read-only twin above is proved.
    let b=u.blocks.iter().position(|b|b.instructions.get(&u.instructions).unwrap().iter().any(|i|matches!(i,Inst::Address{address,..} if u.addresses[address.index()].projection.get(&u.pools.projections).unwrap().iter().any(|s|matches!(s,ProjectionStep::Index(_)))))).unwrap();
    let a = AddressId(u.addresses.len() as u32);
    let ty = u.slots[0].ty;
    u.addresses.push(AddressInfo {
        source_place: None,
        unchecked: Vec::new(),
        base: AddressBase::Slot(SlotId(0)),
        projection: PoolRange::EMPTY,
        ty,
    });
    let mut ed = hirsel_rc::insert::Editor::new(u);
    let version = ed.value(ty, Holding::Plain);
    ed.code[b].push(Inst::Store {
        address: a,
        value: ed.unit.parameters[0],
        version,
    });
    let u = ed.finish();
    assert!(
        hirsel_rc::flat_walk::discover(&u, &l.table)
            .into_iter()
            .all(|c| hirsel_rc::flat_walk::prove(&u, &l.table, c).is_none())
    );
    assert_eq!(
        hirsel_rc::flat_walk::rewrite(u.clone(), &l.table, &Options::default()),
        u
    );
}

#[test]
fn a_loop_carried_sequence_name_does_not_hide_a_changed_reaching_value() {
    use hirsel_lir::layout_types::{LayoutId, LayoutKind, ScalarKind};
    let src = format!("{TYPES}fn sum(xs: Items, ys: Items, again: bool) {{}} ");
    let p = compile(&src);
    let l = hirsel_rc::import::lower(&p, &target()).unwrap();
    let template = unit_named(&l, "sum");
    let mut u = LirUnit::<Value>::new(template.header.clone());
    let root = template.types[template.values[template.parameters[0].index()].ty.index()].clone();
    let scalar = |kind| {
        LayoutId(
            l.table
                .layouts
                .iter()
                .position(|r| r.kind == LayoutKind::Scalar(kind))
                .unwrap() as u32,
        )
    };
    let LayoutKind::Block { payload } = l.table.layouts[root.layout.0 as usize].kind else {
        panic!()
    };
    let list = l.table.layouts[payload.0 as usize].fields[0].layout;
    let element = l.table.layouts[list.0 as usize].fields[0].layout;
    u.types = vec![
        root,
        LirType {
            repr: ValueRepr::Scalar(ScalarKind::Bool),
            layout: scalar(ScalarKind::Bool),
        },
        LirType {
            repr: ValueRepr::Scalar(ScalarKind::U64),
            layout: scalar(ScalarKind::U64),
        },
        LirType {
            repr: ValueRepr::Aggregate,
            layout: element,
        },
    ];
    u.values = [0, 0, 1, 2, 2, 2, 2, 0, 2, 2]
        .into_iter()
        .map(|ty| ValueInfo {
            ty: LirTypeId(ty),
            holding: Holding::Plain,
        })
        .collect();
    u.parameters = vec![ValueId(0), ValueId(1), ValueId(2)];
    u.scopes.push(ScopeInfo { parent: None });
    u.tables.source_anchors.push(SourceAnchor {
        body: 0,
        block: 0,
        statement: 0,
        site: SiteRange { start: 0, end: 1 },
    });
    let zero = ValueId(3);
    let one = ValueId(4);
    let i = ValueId(5);
    let offset = ValueId(6);
    let sequence = ValueId(7);
    let end = ValueId(8);
    let increment = ValueId(9);
    u.tables.constants = vec![
        ConstantData {
            ty: LirTypeId(2),
            value: ConstantValue::Integer(0),
        },
        ConstantData {
            ty: LirTypeId(2),
            value: ConstantValue::Integer(1),
        },
    ];
    u.addresses.push(AddressInfo {
        source_place: None,
        unchecked: Vec::new(),
        base: AddressBase::Value(sequence),
        projection: PoolRange::push(
            &mut u.pools.projections,
            [
                ProjectionStep::Field(0),
                ProjectionStep::FlatAt { index: i, offset },
            ],
        ),
        ty: LirTypeId(3),
    });
    let entryargs = PoolRange::push(&mut u.pools.values, [zero, zero, ValueId(0)]);
    let feedbackargs = PoolRange::push(&mut u.pools.values, [increment, end, sequence]);
    let params = PoolRange::push(&mut u.pools.values, [i, offset, sequence]);
    u.tables.edges = vec![
        Edge {
            target: BlockId(1),
            arguments: entryargs,
        },
        Edge {
            target: BlockId(1),
            arguments: feedbackargs,
        },
        Edge {
            target: BlockId(2),
            arguments: PoolRange::EMPTY,
        },
        Edge {
            target: BlockId(3),
            arguments: PoolRange::EMPTY,
        },
        Edge {
            target: BlockId(3),
            arguments: PoolRange::EMPTY,
        },
    ];
    u.tables.failures = vec![
        FailureEdge {
            cleanup: EdgeId(3),
            anchor: SourceAnchorId(0),
            rule: Some("MEM-14".into()),
            dispose: PoolRange::EMPTY,
        },
        FailureEdge {
            cleanup: EdgeId(4),
            anchor: SourceAnchorId(0),
            rule: Some("SEM-1".into()),
            dispose: PoolRange::EMPTY,
        },
    ];
    u.instructions = vec![
        Inst::Const {
            result: zero,
            constant: ConstantId(0),
        },
        Inst::Const {
            result: one,
            constant: ConstantId(1),
        },
        Inst::Address {
            address: AddressId(0),
            failure: FailureId(0),
        },
        Inst::FlatNext {
            result: end,
            address: AddressId(0),
        },
        Inst::Binary {
            result: increment,
            op: BinaryOp::Add,
            left: i,
            right: one,
            failure: FailureId(1),
        },
    ];
    u.blocks = vec![
        Block {
            kind: BlockKind::Body,
            parameters: PoolRange::EMPTY,
            instructions: PoolRange::new(0, 2),
            anchor: SourceAnchorId(0),
        },
        Block {
            kind: BlockKind::Body,
            parameters: params,
            instructions: PoolRange::new(2, 3),
            anchor: SourceAnchorId(0),
        },
        Block {
            kind: BlockKind::Body,
            parameters: PoolRange::EMPTY,
            instructions: PoolRange::EMPTY,
            anchor: SourceAnchorId(0),
        },
        Block {
            kind: BlockKind::FailureCleanup,
            parameters: PoolRange::EMPTY,
            instructions: PoolRange::EMPTY,
            anchor: SourceAnchorId(0),
        },
    ];
    u.terminators = vec![
        Terminator::Jump(EdgeId(0)),
        Terminator::Branch {
            condition: ValueId(2),
            yes: EdgeId(1),
            no: EdgeId(2),
        },
        Terminator::Return(None),
        Terminator::Propagate,
    ];
    let inputs = Inputs {
        memo: Default::default(),
        discharge: None,
        operations: &p.operation_registry(),
        target: &target(),
        layouts: &l.table,
        manifest: &manifest(&p),
    };
    assert!(
        hirsel_lir::validate::check(&u, &inputs).is_empty(),
        "{:?}",
        hirsel_lir::validate::check(&u, &inputs)
    );
    u.pools.values[feedbackargs.start as usize + 2] = ValueId(1);
    let violations = hirsel_lir::validate::check(&u, &inputs);
    assert_eq!(
        violations
            .iter()
            .map(|v| v.kind.clone())
            .collect::<Vec<_>>(),
        [ViolationKind::Type("flat walk offset replay")]
    );
}

#[test]
fn every_injected_failure_audits_with_the_rewrite_enabled_and_disabled() {
    let src = source("for x in xs.items { total += x.n }");
    for enabled in [true, false] {
        let p = input_program(&src, enabled, 3);
        let campaign = hirsel_heapcheck::fault_campaign(
            &p,
            &path("sum"),
            &[],
            &mut || Box::new(hirsel_heapcheck::NoOperations),
            &Default::default(),
        );
        assert!(campaign.clean.result.is_ok());
        assert!(!campaign.injected.is_empty());
        for (site, r) in campaign.injected {
            assert!(
                r.result.is_ok(),
                "enabled={enabled} site={site}: {:?}",
                r.result
            );
        }
    }
}

#[test]
fn reads_and_writes_on_a_break_only_path_participate_in_the_proof() {
    let src = source("for x in xs.items { if x.n == 7 { total += x.n\nbreak }; total += x.n }");
    let (_, l) = lowered(&src, true);
    let u = unit_named(&l, "sum");
    assert_eq!(count(u), 1);
    assert!(u.addresses.iter().all(|a| {
        a.projection
            .get(&u.pools.projections)
            .unwrap()
            .iter()
            .all(|p| !matches!(p, ProjectionStep::Index(_)))
    }));
    let on = input_program(&src, true, 12);
    let off = input_program(&src, false, 12);
    assert!(
        run(&on)
            .result
            .unwrap()
            .observation
            .equivalent(&run(&off).result.unwrap().observation)
    );
    let (_, l) = lowered(&src, false);
    let mut u = unit_named(&l, "sum").clone();
    let header = u.blocks.iter().position(|b| b.parameters.len == 3).unwrap();
    // The break-only block ends the iteration scope, then jumps to the exit
    // path that ends the index scope. It cannot reach the feedback edge.
    let exit = u
        .blocks
        .iter()
        .position(|b| {
            b.instructions
                .get(&u.instructions)
                .unwrap()
                .iter()
                .any(|i| matches!(i,Inst::EndScope{scope} if scope.0==2))
        })
        .unwrap();
    let b = u
        .terminators
        .iter()
        .enumerate()
        .find_map(|(b, t)| match t {
            Terminator::Jump(e)
                if u.tables.edges[e.index()].target.index() == exit && b != header =>
            {
                let insts = u.blocks[b].instructions.get(&u.instructions).unwrap();
                insts
                    .iter()
                    .any(|i| matches!(i, Inst::EndLoan { .. }))
                    .then_some(b)
            }
            _ => None,
        })
        .unwrap();
    let at = AddressId(u.addresses.len() as u32);
    let ty = u.slots[0].ty;
    u.addresses.push(AddressInfo {
        source_place: None,
        unchecked: Vec::new(),
        base: AddressBase::Slot(SlotId(0)),
        projection: PoolRange::EMPTY,
        ty,
    });
    let mut ed = hirsel_rc::insert::Editor::new(u);
    let version = ed.value(ty, Holding::Plain);
    ed.code[b].insert(
        0,
        Inst::Store {
            address: at,
            value: ed.unit.parameters[0],
            version,
        },
    );
    let u = ed.finish();
    assert_eq!(
        hirsel_rc::flat_walk::rewrite(u.clone(), &l.table, &Options::default()),
        u
    );
}

#[test]
fn a_write_after_the_index_scope_ends_does_not_prevent_the_walk() {
    let src = source("for x in xs.items { total += x.n }");
    let (_, l) = lowered(&src, false);
    let mut u = unit_named(&l, "sum").clone();
    let (b, end) = u
        .blocks
        .iter()
        .enumerate()
        .find_map(|(b, block)| {
            block
                .instructions
                .get(&u.instructions)
                .unwrap()
                .iter()
                .position(|i| matches!(i, Inst::EndScope { scope } if scope.0 == 2))
                .map(|end| (b, end))
        })
        .unwrap();
    // A proposed whole-sequence write after the index's scope ends is outside
    // the loop proof, in contrast to the break-only write control above.
    let at = AddressId(u.addresses.len() as u32);
    let ty = u.slots[0].ty;
    u.addresses.push(AddressInfo {
        source_place: None,
        unchecked: Vec::new(),
        base: AddressBase::Slot(SlotId(0)),
        projection: PoolRange::EMPTY,
        ty,
    });
    let mut ed = hirsel_rc::insert::Editor::new(u);
    let version = ed.value(ty, Holding::Plain);
    ed.code[b].insert(
        end + 1,
        Inst::Store {
            address: at,
            value: ed.unit.parameters[0],
            version,
        },
    );
    let u = hirsel_rc::flat_walk::rewrite(ed.finish(), &l.table, &Options::default());
    assert_eq!(count(&u), 1);
}

#[test]
fn flat_sequences_materialize_with_ordinary_construction_and_a_linear_walk() {
    for (types, values) in [
        ("flat type Items { values: List[u64] }", "List[u64]"),
        (
            "flat type Item { text: String, n: u64 }\nflat type Items { values: List[Item] }",
            "List[Item]",
        ),
        (
            "flat type Item { n: u64 }\nflat type Items { values: List[Item] }",
            "List[Item]",
        ),
        (
            "flat type Items { values: List[List[u64]] }",
            "List[List[u64]]",
        ),
    ] {
        let src = format!(
            "{types}\nfn unpack(items: Items) -> {values} in items {{ copy items.values }}"
        );
        let _ = stages(&src, true);
        let (_, on) = lowered(&src, true);
        let unit = unit_named(&on, "unpack");
        let (_, off) = lowered(&src, false);
        assert_eq!(count(unit_named(&off, "unpack")), 0);
        // The existing walk proof conservatively leaves nested cycles alone.
        assert_eq!(count(unit), usize::from(types.contains("text: String")));
        // Materialization reserves the count, so its pushes are proved to
        // fit (FACT-3) and append without a capacity test.
        assert!(unit.tables.calls.iter().any(|c| {
            matches!(c.callee, Callee::Direct(s) if [hirsel_syntax::DefPath::core("list.push"), hirsel_rc::spare_push::spare_path()].contains(&unit.tables.symbols[s.index()].definition))
        }));
    }
}

#[test]
fn flat_list_materialization_conformance_passes_rc() {
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance/MEM/MEM-12-flat-list-materialize-run.hir"),
    )
    .unwrap();
    let _ = stages(&src, true);
}

#[test]
fn flat_list_materialization_releases_partial_results_on_every_failure() {
    let src = format!(
        "{TYPES}\nfn unpack(xs: Items) -> List[Item] in xs {{ copy xs.items }}\nfn sum(xs: Items) -> u64 {{ let items = unpack(xs)\nlet mut total = 0u64\nfor item in items {{ total += item.n }}\ntotal }}"
    );
    for enabled in [true, false] {
        let program = input_program(&src, enabled, 3);
        let campaign = hirsel_heapcheck::fault_campaign(
            &program,
            &path("sum"),
            &[],
            &mut || Box::new(hirsel_heapcheck::NoOperations),
            &Default::default(),
        );
        assert!(campaign.clean.result.is_ok(), "{:?}", campaign.clean.result);
        assert!(!campaign.injected.is_empty());
        for (site, run) in campaign.injected {
            assert!(run.result.is_ok(), "{site:?}: {:?}", run.result);
        }
    }
}

#[test]
fn materializing_a_local_flat_list_keeps_cleanup_in_its_source_scope() {
    let src = format!(
        "{TYPES}fn sum() -> u64 {{
let xs = Items(items: [Item(text: \"a\", n: 1), Item(text: \"bbb\", n: 2)])
let mut items = copy xs.items
list.reserve(mut items, 0)
let mut total = 0u64
for item in items {{ total += item.n }}
total + list.len(xs.items)
}}"
    );
    for enabled in [true, false] {
        let (_, program) = stages(&src, enabled);
        let campaign = hirsel_heapcheck::fault_campaign(
            &program,
            &path("sum"),
            &[],
            &mut || Box::new(hirsel_heapcheck::NoOperations),
            &Default::default(),
        );
        let clean = campaign.clean.result.as_ref().unwrap();
        assert_eq!(
            clean.observation.returned,
            Some(hirsel_heapcheck::Observed::Integer {
                scalar: hirsel_lir::layout_types::ScalarKind::U64,
                bits: 5,
            })
        );
        assert!(!campaign.injected.is_empty());
        for (site, run) in campaign.injected {
            assert!(run.result.is_ok(), "{site}: {:?}", run.result);
        }
    }
}
