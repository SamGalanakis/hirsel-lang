//! A for loop over a direct List of a dynamic Packed value (MEM-13): the flat
//! walk carries an offset from the first element instead of walking to each
//! element from the start. Real imported loops, H's boundary-read counts, and
//! hand-built C controls with accepted twins.
use crate::rc_support as support;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::LayoutKind;
use hirsel_lir::stage::{Rc, Value};
use hirsel_lir::validate::{Inputs, ViolationKind};
use hirsel_rc::flat_walk::Options;
use support::*;

const TYPES: &str = "type Inner = Num(n: u64) | Pair(a: u64, b: u64)
type Choice = Small(value: u64) | Large(value: u64, extra: u64) | Node(inner: Packed[Inner])
";

/// `total` walks `xs` with `body`; `run` builds `n` elements by push and sums them.
fn source(body: &str, n: u64) -> String {
    format!(
        "{TYPES}fn total(xs: List[Packed[Choice]]) -> u64 {{
  let mut sum = 0u64
  let mut k = 0u64
  {body}
  sum
}}
fn run() -> u64 {{
  let mut xs: List[Packed[Choice]] = []
  let mut i = 0u64
  while i < {n}u64 {{
    if i % 3u64 == 0u64 {{
      list.push(mut xs, Large(value: i, extra: 2u64))
    }} else if i % 3u64 == 1u64 {{
      list.push(mut xs, Small(value: i))
    }} else {{
      list.push(mut xs, Node(inner: Num(n: i)))
    }}
    i = i + 1u64
  }}
  total(xs)
}}
"
    )
}

const SUM: &str = "for x in xs { sum = sum + worth(x, k)\nk = k + 1u64 }";
/// Element `k` is Large, Small or Node by `k % 3`, as `run` pushes them.
const WORTH: &str = "fn worth(x: Packed[Choice], k: u64) -> u64 {
  if k % 3u64 == 0u64 {
    x.Large.value * x.Large.extra
  } else if k % 3u64 == 1u64 {
    x.Small.value
  } else {
    x.Node.inner.Num.n
  }
}
";

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

fn indexed(u: &LirUnit<Value>) -> bool {
    u.addresses.iter().any(|a| {
        a.projection
            .get(&u.pools.projections)
            .unwrap()
            .iter()
            .any(|p| matches!(p, ProjectionStep::Index(_)))
    })
}

fn full(body: &str, n: u64) -> String {
    format!("{WORTH}{}", source(body, n))
}

#[test]
fn a_for_loop_over_a_sequential_list_carries_an_offset_and_the_option_keeps_index() {
    let src = full(SUM, 3);
    let (_, on) = lowered(&src, true);
    let (_, off) = lowered(&src, false);
    let walked = unit_named(&on, "total");
    assert_eq!(count(walked), 1, "{}", dump(walked));
    assert!(!indexed(walked), "{}", dump(walked));
    assert!(walked.addresses.iter().any(|a| {
        a.projection
            .get(&walked.pools.projections)
            .unwrap()
            .iter()
            .any(|p| matches!(p, ProjectionStep::FlatAt { .. }))
    }));
    assert_eq!(count(unit_named(&off, "total")), 0);
    assert!(indexed(unit_named(&off, "total")));
}

fn stages(src: &str, enabled: bool) -> hirsel_heapcheck::Program<Rc> {
    let (p, l) = lowered(src, enabled);
    let operations = l.operation_registry(&p);
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
                operations: &operations,
                target: &target(),
                layouts: &table,
                manifest: &manifest(&p),
            },
        );
        assert!(v.is_empty(), "{v:?}\n{}", dump(&rc));
        units.push(rc);
    }
    hirsel_heapcheck::Program {
        invocation_bodies: operations.invocation_bodies.clone(),
        invocations: operations.invocations.clone(),
        pretransfer_adapters: operations.pretransfer_adapters.clone(),
        target: target(),
        layouts: table,
        units,
        statics: l.statics,
    }
}

fn execute(p: &hirsel_heapcheck::Program<Rc>, check_every_step: bool) -> hirsel_heapcheck::Run {
    hirsel_heapcheck::run(
        p,
        &path("run"),
        &[],
        &mut hirsel_heapcheck::NoOperations,
        &hirsel_heapcheck::Config {
            step_limit: 60_000_000,
            check_every_step,
            ..Default::default()
        },
    )
}

fn returned(r: &hirsel_heapcheck::Run) -> Option<hirsel_heapcheck::Observed> {
    r.result.as_ref().unwrap().observation.returned.clone()
}

/// What `run` returns over `n` elements, element `i` holding the value `worth` gives it.
fn expected(n: u64) -> u64 {
    (0..n).map(|i| if i % 3 == 0 { 2 * i } else { i }).sum()
}

#[test]
fn observations_agree_with_the_walk_on_and_off_including_early_exits() {
    for body in [
        SUM,
        "for x in xs { if k == 7u64 { break }\nsum = sum + worth(x, k)\nk = k + 1u64 }",
        "for x in xs { k = k + 1u64\nif k == 7u64 { continue }\nsum = sum + worth(x, k - 1u64) }",
        "for x in xs { let mut j = 0u64\nfor y in xs { sum = sum + worth(y, j)\nj = j + 1u64 }\nsum = sum + worth(x, k)\nk = k + 1u64 }",
    ] {
        let src = full(body, 12);
        let on = execute(&stages(&src, true), true);
        let off = execute(&stages(&src, false), true);
        let (on, off) = (on.result.unwrap(), off.result.unwrap());
        assert!(on.observation.equivalent(&off.observation), "{body}");
    }
    let r = execute(&stages(&full(SUM, 12), true), true);
    assert_eq!(
        returned(&r),
        Some(hirsel_heapcheck::Observed::Integer {
            scalar: hirsel_lir::layout_types::ScalarKind::U64,
            bits: u128::from(expected(12)),
        })
    );
}

#[test]
fn extent_reads_are_linear_for_ten_thousand_packed_elements() {
    // H builds the list by ten thousand checked pushes first; the native
    // scaling control (hirsel-driver packed_walk.rs) runs a hundred thousand.
    let r = execute(&stages(&full(SUM, 10_000), true), false);
    assert!(r.result.is_ok(), "{:?}", r.result);
    assert_eq!(r.counters.flat_prefix_reads, 10_000);
    assert_eq!(
        returned(&r),
        Some(hirsel_heapcheck::Observed::Integer {
            scalar: hirsel_lir::layout_types::ScalarKind::U64,
            bits: u128::from(expected(10_000)),
        })
    );
    // Without the walk, element i is reached past i earlier extents at each of
    // the loop's two prepares: the exact quadratic count, on bounded inputs.
    for n in [40, 80] {
        let r = execute(&stages(&full(SUM, n), false), false);
        assert!(r.result.is_ok(), "{:?}", r.result);
        assert_eq!(r.counters.flat_prefix_reads, n * (n - 1), "n={n}");
    }
}

#[test]
fn every_injected_failure_audits_with_the_walk_enabled_and_disabled() {
    let src = full(SUM, 4);
    for enabled in [true, false] {
        let p = stages(&src, enabled);
        let campaign = hirsel_heapcheck::fault_campaign(
            &p,
            &path("run"),
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

/// The C forgeries of `control`.
#[derive(Clone, Copy, PartialEq)]
enum Forgery {
    None,
    /// Element 1's offset is its index, not element 0's end.
    OffsetIsIndex,
    /// Element 2 is reached with element 0's end: the wrong predecessor.
    WrongPredecessor,
    /// `FlatNext` names a Packed value nested in an element: the end of an
    /// extent inside the element, not the element's.
    InnerEnd,
}

/// Entirely hand-built instructions over a production layout table: element 0
/// at offset zero, its end, element 1 at that end, and the end of a place
/// inside element 0.
fn control(forgery: Forgery) -> Vec<ViolationKind> {
    let src = format!("{TYPES}fn sum(xs: List[Packed[Choice]]) {{}}\n");
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
    let LayoutKind::Block { payload } = l.table.layouts[root_ty.layout.0 as usize].kind else {
        panic!()
    };
    let LayoutKind::Sequential { element } = l.table.layouts[payload.0 as usize].kind else {
        panic!("a List of Packed values is sequential")
    };
    let node = l.table.layouts[element.0 as usize]
        .tag
        .as_ref()
        .unwrap()
        .variants
        .iter()
        .find(|v| v.ordinal == 2)
        .unwrap();
    let inner = node.fields[0].layout;
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
        LirType {
            repr: ValueRepr::Aggregate,
            layout: inner,
        },
    ];
    u.values = vec![ValueInfo {
        ty: LirTypeId(0),
        holding: Holding::Plain,
    }];
    u.values.extend(vec![
        ValueInfo {
            ty: LirTypeId(1),
            holding: Holding::Plain
        };
        4
    ]);
    u.parameters = vec![ValueId(0)];
    u.scopes.push(ScopeInfo { parent: None });
    u.tables.source_anchors.push(SourceAnchor {
        body: 0,
        block: 0,
        statement: 0,
        site: SiteRange {
            source: 0,
            start: 0,
            end: 1,
        },
    });
    let (zero, one, next, end) = (ValueId(1), ValueId(2), ValueId(3), ValueId(4));
    u.tables.constants = vec![
        ConstantData {
            ty: LirTypeId(1),
            value: ConstantValue::Integer(0),
        },
        ConstantData {
            ty: LirTypeId(1),
            value: ConstantValue::Integer(if forgery == Forgery::WrongPredecessor {
                2
            } else {
                1
            }),
        },
    ];
    let offset = if forgery == Forgery::OffsetIsIndex {
        one
    } else {
        next
    };
    for (projection, ty) in [
        (
            vec![ProjectionStep::FlatAt {
                index: zero,
                offset: zero,
            }],
            LirTypeId(2),
        ),
        (
            vec![ProjectionStep::FlatAt { index: one, offset }],
            LirTypeId(2),
        ),
        (
            vec![
                ProjectionStep::FlatAt {
                    index: zero,
                    offset: zero,
                },
                ProjectionStep::VariantField {
                    variant: 2,
                    field: 0,
                },
            ],
            LirTypeId(3),
        ),
    ] {
        let projection = PoolRange::push(&mut u.pools.projections, projection);
        u.addresses.push(AddressInfo {
            source_place: None,
            unchecked: Vec::new(),
            base: AddressBase::Value(ValueId(0)),
            projection,
            ty,
        });
    }
    for _ in 0..3 {
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
        Inst::Address {
            address: AddressId(2),
            failure: FailureId(2),
        },
        // The twin takes element 0's end again.
        Inst::FlatNext {
            result: end,
            address: AddressId(if forgery == Forgery::InnerEnd { 2 } else { 0 }),
        },
    ];
    u.blocks = vec![
        Block {
            kind: BlockKind::Body,
            parameters: PoolRange::EMPTY,
            instructions: PoolRange::new(0, 7),
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
            operations: &l.operation_registry(&p),
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
fn hand_built_wrong_offset_wrong_predecessor_and_inner_end_controls_have_an_accepted_twin() {
    assert!(
        control(Forgery::None).is_empty(),
        "{:?}",
        control(Forgery::None)
    );
    for forgery in [Forgery::OffsetIsIndex, Forgery::WrongPredecessor] {
        assert_eq!(
            control(forgery),
            [ViolationKind::Type("flat walk offset replay")]
        );
    }
    assert_eq!(
        control(Forgery::InnerEnd),
        [ViolationKind::Type("flat element end")]
    );
}

/// Pop and take-iteration stay unavailable over a sequential list: it is
/// walk-only, so either would be O(n) per element. The List[u64] twins lower.
#[test]
fn pop_and_take_iteration_of_a_sequential_list_are_unavailable_with_unsequential_twins() {
    let pop = "let n: u64 = match list.pop(mut xs) {\n Some(x) => 1u64,\n None() => 0u64,\n}\nn + list.len(xs)";
    let take = "let mut s = 0u64\nfor x in take xs { s = s + 1u64 }\ns";
    for body in [pop, take] {
        for (ty, value) in [("Packed[Choice]", "Small(value: 1u64)"), ("u64", "1u64")] {
            let src = format!(
                "{TYPES}fn f() -> u64 {{\nlet mut xs: List[{ty}] = [{value}, {value}]\n{body}\n}}\n"
            );
            let lowered = hirsel_rc::import::lower(&compile(&src), &target());
            if ty == "u64" {
                assert!(lowered.is_ok(), "{body}: {:?}", lowered.err());
            } else {
                let error = format!("{:?}", lowered.err());
                assert!(error.contains("walk-only (MEM-13)"), "{body}: {error}");
            }
        }
    }
}
