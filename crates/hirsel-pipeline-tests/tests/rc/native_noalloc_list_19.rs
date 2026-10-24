//! MEM-5: only a proved nonretaining scalar observation permits local List placement.
use crate::rc_support::{dump, lower_valid, manifest, target, try_compile, unit_named};
use hirsel_driver::native::*;
use hirsel_lir::ir::{
    ArgumentValue, Inst, LirUnit, Placement, StorageOwner, Terminator, ValueRepr,
};
use hirsel_lir::stage::Value;
use hirsel_rc::placement::{Options as PlaceOptions, PlaceError, place};

const ORIGINAL: &str = r#"
// Source-valid MEM-5 local list; native placement conservatively treats its
// list.len call as escape. Unrelated to parameter-only Caller receipt repair.
@noalloc
fn scratch() -> u64 { let v: List[u64] = [1, 2]; list.len(v) }
pub fn main() -> Result[(), ()] {
  if scratch() == 2 { Ok(()) } else { Error(()) }
}
"#;

const LENGTHS: &str = r#"
@noalloc fn scratch() -> u64 {
    let empty: List[u64] = []
    let one: List[u64] = [9]
    let many: List[u64] = [1, 2, 3, 4]
    list.len(empty) + list.len(one) + list.len(many)
}
pub fn main() -> Result[(), ()] { if scratch() == 5 { Ok(()) } else { Error(()) } }
"#;

const SCALAR: &str = r#"
@noalloc fn scratch() -> u64 {
    let v: List[u64] = [7, 11]
    let n = list.len(v)
    v[0] + v[n - 1]
}
pub fn main() -> Result[(), ()] { if scratch() == 18 { Ok(()) } else { Error(()) } }
"#;

const SLICES: &str = r#"
@noalloc fn scratch() -> u64 {
    let bytes: Bytes = b"abc"
    let text: String = "flock"
    bytes.len(bytes) + string.byte_len(text)
}
pub fn main() -> Result[(), ()] { if scratch() == 8 { Ok(()) } else { Error(()) } }
"#;

const ELEMENT: &str = r#"
@noalloc fn scratch() -> u64 { let v: List[u64] = [7, 11]; v[1] }
pub fn main() -> Result[(), ()] { if scratch() == 11 { Ok(()) } else { Error(()) } }
"#;

const COMPARE: &str = r#"
@noalloc fn scratch() -> bool { let v: List[u64] = [1, 2]; v == [1, 2] }
pub fn main() -> Result[(), ()] { if scratch() { Ok(()) } else { Error(()) } }
"#;

const NESTED: &str = r#"
@noalloc fn scratch() -> u64 {
    { { let v: List[u64] = [4, 5]; list.len(v) } }
}
pub fn main() -> Result[(), ()] { if scratch() == 2 { Ok(()) } else { Error(()) } }
"#;

const EARLY: &str = r#"
resource type Token { n: u64 }
@noalloc drop(x) { () }
@noalloc fn check(yes: bool) -> Result[(), ()] { if yes { Ok(()) } else { Error(()) } }
@noalloc fn scratch(yes: bool) -> Result[u64, ()] {
    let token = Token(n: 7)
    let v: List[u64] = [1, 2]
    check(yes)?
    Ok(list.len(v) + token.n)
}
pub fn main() -> Result[(), ()] {
    let failed = scratch(false)
    let passed = scratch(true)
    if failed == Error(()) && passed == Ok(9) { Ok(()) } else { Error(()) }
}
"#;

fn matrix() -> Vec<Configuration> {
    [true, false]
        .into_iter()
        .flat_map(|regions| {
            let mut options = Options::new(target());
            options.regions = regions;
            [
                Engine::Heap { campaign: false },
                Engine::Native {
                    opt: OptLevel::None,
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
                Engine::Native {
                    opt: OptLevel::Default,
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
                Engine::Cranelift {
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            ]
            .into_iter()
            .map(move |engine| Configuration {
                options: options.clone(),
                engine,
            })
        })
        .collect()
}

fn region_count<S>(unit: &LirUnit<S>) -> usize {
    unit.tables
        .constructs
        .iter()
        .filter(|c| {
            matches!(
                c.placement,
                Some(Placement::Fixed(StorageOwner::LocalRegion(_)))
            )
        })
        .count()
}

// The full-recount fault campaign checks cleanup at every failing instruction,
// including RegionEnter/Exit and both early Result paths. Native diagnostic
// runtimes independently audit live blocks and regions at process exit.
fn parity(cases: &[(&str, &str)]) {
    use hirsel_driver::*;
    use hirsel_engine::*;
    let matrix = matrix();
    let directory =
        std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("native-noalloc-list-19");
    std::fs::create_dir_all(&directory).unwrap();
    let toolchain = Toolchain::for_matrix(&directory.join("runtime"), &matrix).unwrap();
    for &(name, source) in cases {
        with_compiler_stack(|| {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let context = default_context();
            let file = engine.sources.add("noalloc.hir", source.into()).unwrap();
            let checked = check_progress(&mut engine, file, &context).unwrap();
            assert!(
                checked.diagnostics.is_empty(),
                "{name}: {:?}",
                checked.diagnostics
            );
            let entry = entry_path(&context);
            let reference = run_observed_snapshot(
                &mut engine,
                &RunConfiguration {
                    entry: entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            )
            .unwrap();
            let reference = Observation::of_reference(reference.observation.as_ref().unwrap());
            assert_eq!(reference.exit_code, Some(0), "{name}: {reference:?}");
            assert!(reference.failures.is_empty(), "{name}: {reference:?}");
            let snapshot = engine.export_native_input().unwrap();
            let validated = engine.import_native_input(snapshot).unwrap();
            let mut campaign_runs = 0;
            for regions in [true, false] {
                let mut options = Options::new(target());
                options.regions = regions;
                let ready = build(&validated, &options).unwrap();
                let units = ready.raw_units();
                let scratch = units
                    .iter()
                    .find(|u| u.header.definition.0.name == "scratch")
                    .unwrap();
                if name != "slices" {
                    assert!(region_count(scratch) > 0, "{name}: {}", dump(scratch));
                }
                let heap = run_heap(&ready, &entry, &Sites::new(validated.snapshot())).unwrap();
                assert_eq!(heap.observation, reference, "{name}");
                assert_eq!(heap.counters.allocations, 0, "{name}: {:?}", heap.counters);
                assert_eq!(heap.counters.frees, 0, "{name}");
                assert_eq!(heap.counters.live_heap_bytes, 0, "{name}");
                assert_eq!(
                    heap.counters.finalizers,
                    if name == "early" { 2 } else { 0 }
                );
                let heap = hirsel_heapcheck::Program {
                    invocation_bodies: ready.operations.invocation_bodies.clone(),
                    invocations: ready.operations.invocations.clone(),
                    pretransfer_adapters: ready.operations.pretransfer_adapters.clone(),
                    target: ready.target.clone(),
                    layouts: ready.table.clone(),
                    units,
                    statics: ready.statics.clone(),
                };
                let campaign = hirsel_heapcheck::fault_campaign(
                    &heap,
                    &entry,
                    &[],
                    &mut || Box::new(hirsel_heapcheck::NoOperations),
                    &hirsel_heapcheck::Config {
                        full_recount_every_step: true,
                        ..Default::default()
                    },
                );
                assert!(
                    campaign.first_violation().is_none(),
                    "{name}: {:?}",
                    campaign.first_violation()
                );
                assert_eq!(campaign.injected.len() as u64, campaign.clean.sites);
                for run in std::iter::once(&campaign.clean)
                    .chain(campaign.injected.iter().map(|(_, run)| run))
                {
                    assert_eq!(run.counters.allocations, 0, "{name}: {:?}", run.result);
                    assert_eq!(run.counters.live_heap_bytes, 0, "{name}");
                }
                campaign_runs += campaign.injected.len() + 1;
                eprintln!(
                    "{name} regions={regions}: {} H full-recount fault executions; clean {:?}",
                    campaign.injected.len() + 1,
                    campaign.clean.counters
                );
            }
            let directory = directory.join(name);
            std::fs::create_dir_all(&directory).unwrap();
            let mut executions = 0;
            for (configuration, outcome) in differential(
                &validated, &entry, &reference, &matrix, &toolchain, &directory,
            ) {
                assert!(
                    matches!(outcome, Outcome::Agree),
                    "{name}: {configuration}: {outcome:?}"
                );
                eprintln!("{name}: {configuration}: agrees");
                executions += 1;
            }
            assert_eq!(executions, 8);
            eprintln!(
                "{name}: {executions} engine configurations, {campaign_runs} H fault executions"
            );
        });
    }
}

#[test]
fn original_witness() {
    parity(&[("original", ORIGINAL)]);
}

#[test]
fn empty_nonempty_scalar_and_nested_observations() {
    parity(&[("lengths", LENGTHS), ("scalar", SCALAR), ("nested", NESTED)]);
}

#[test]
fn early_result_cleanup() {
    parity(&[("early", EARLY)]);
}

#[test]
fn sibling_borrowed_observers() {
    parity(&[
        ("slices", SLICES),
        ("element", ELEMENT),
        ("compare", COMPARE),
    ]);
}

const RETURNED: &str =
    "fn main() -> List[u64] { let v: List[u64] = [1, 2]; let n = list.len(v); v }";
const CALL: &str = "@noalloc fn observe(v: List[u64]) -> u64 { list.len(v) }\nfn main() -> u64 { let v: List[u64] = [1, 2]; observe(v) }";
const INDIRECT: &str =
    "fn main(f: @noalloc fn(List[u64]) -> u64 uses {}) -> u64 { let v: List[u64] = [1, 2]; f(v) }";
const MUT: &str = "fn observe(mut v: List[u64]) -> u64 { list.len(v) }\nfn main() -> u64 { let mut v: List[u64] = [1, 2]; observe(mut v) }";
const CAPTURED: &str =
    "fn main() -> u64 { let v: List[u64] = [1, 2]; let f = fn() -> u64 { list.len(v) }; f() }";
const RETAINED: &str =
    "fn main() -> fn() -> u64 uses {} { let v: List[u64] = [1, 2]; fn() -> u64 { list.len(v) } }";

#[test]
fn returning_retaining_unknown_mutable_and_captured_paths_stay_strict() {
    for (source, rule) in [
        ("@noalloc fn main() -> List[u64] { [1, 2] }", "MEM-5"),
        (
            "@noalloc fn main() -> fn() -> u64 uses {} { let v: List[u64] = [1, 2]; fn() -> u64 { list.len(v) } }",
            "MEM-5",
        ),
        (
            include_str!("../../../../conformance/MEM/MEM-5-closure-local-list-escape-bad.hir"),
            "MEM-5",
        ),
        (
            include_str!("../../../../conformance/MEM/MEM-5-implicit-resource-cleanup-bad.hir"),
            "MEM-5",
        ),
    ] {
        let error = try_compile(source).unwrap_err();
        assert!(error.contains(rule), "{error}");
    }
    for source in [RETURNED, CALL, INDIRECT, MUT, CAPTURED, RETAINED] {
        let (_, lowered) = lower_valid(source);
        for regions in [true, false] {
            assert!(
                matches!(
                    place(
                        unit_named(&lowered, "main").clone(),
                        &lowered.table,
                        &PlaceOptions {
                            regions,
                            noalloc: true
                        }
                    ),
                    Err(PlaceError::HeapInNoalloc(_))
                ),
                "{source}"
            );
        }
    }
}

#[test]
fn forged_region_return_is_rejected_by_c() {
    use hirsel_lir::validate::{Inputs, ViolationKind};
    let source = "fn main() -> List[u64] { let v: List[u64] = [1, 2]; let n = list.len(v); [n] }";
    let (program, lowered) = lower_valid(source);
    let mut unit = place(
        unit_named(&lowered, "main").clone(),
        &lowered.table,
        &PlaceOptions {
            regions: true,
            noalloc: false,
        },
    )
    .unwrap();
    assert_eq!(region_count(&unit), 1);
    let manifest = manifest(&program);
    let target = target();
    let operations = lowered.operation_registry(&program);
    let inputs = Inputs {
        target: &target,
        layouts: &lowered.table,
        manifest: &manifest,
        operations: &operations,
        memo: Default::default(),
        discharge: None,
    };
    assert!(hirsel_lir::validate::check(&unit, &inputs).is_empty());
    let value = unit
        .instructions
        .iter()
        .find_map(|i| match i {
            Inst::RegionPublish { value } => Some(*value),
            _ => None,
        })
        .unwrap();
    for term in &mut unit.terminators {
        if matches!(term, Terminator::Return(_)) {
            *term = Terminator::Return(Some(value));
        }
    }
    let violations = hirsel_lir::validate::check(&unit, &inputs);
    assert!(
        violations
            .iter()
            .any(|v| v.kind == ViolationKind::RegionEscape(value)),
        "{violations:?}"
    );
}

// Independent may-reach oracle: rustc_mir_dataflow framework/mod.rs:288-375
// at 19833af7224424e8b53e288097d56b6af43d5bcf (MIT OR Apache-2.0).
// Adapt the dirty/min-rank fixed-point structure to a small SSA/storage graph,
// with a boolean domain; do not read placement's union-find or local-value flags.
// Unlike the production analysis this follows directed reachability from one
// allocation. These fixtures have no overwrites needing point-sensitive kills.
fn escapes(unit: &LirUnit<Value>, source: usize, reverse: bool) -> bool {
    use hirsel_lir::ir::{AddressBase, AddressId};
    let count = unit.values.len();
    let mut graph = vec![vec![]; count + unit.slots.len()];
    let root = |mut address: AddressId| loop {
        match unit.addresses[address.index()].base {
            AddressBase::Slot(s) => break count + s.index(),
            AddressBase::Value(v) => break v.index(),
            AddressBase::Address(a) => address = a,
        }
    };
    let mut sinks = vec![];
    for inst in &unit.instructions {
        match *inst {
            Inst::Store {
                address,
                value,
                version,
            } => {
                graph[value.index()].push(root(address));
                graph[root(address)].push(version.index());
            }
            Inst::Read {
                address, result, ..
            }
            | Inst::Copy {
                address, result, ..
            } if matches!(
                unit.types[unit.values[result.index()].ty.index()].repr,
                ValueRepr::Block | ValueRepr::Aggregate
            ) =>
            {
                graph[root(address)].push(result.index());
            }
            Inst::Construct {
                result, construct, ..
            } => {
                for field in unit.tables.constructs[construct.index()]
                    .fields
                    .get(&unit.pools.fields)
                    .unwrap()
                {
                    graph[field.value.index()].push(result.index());
                }
            }
            Inst::Closure { closure, .. } => {
                sinks.extend(
                    unit.tables.closures[closure.index()]
                        .captures
                        .get(&unit.pools.captures)
                        .unwrap()
                        .iter()
                        .map(|c| c.value.index()),
                );
            }
            _ => (),
        }
        if let Some(failure) = Inst::failure(inst) {
            sinks.extend(
                unit.tables.failures[failure.index()]
                    .dispose
                    .get(&unit.pools.values)
                    .unwrap()
                    .iter()
                    .map(|v| v.index()),
            );
        }
    }
    for edge in &unit.tables.edges {
        for (arg, param) in edge.arguments.get(&unit.pools.values).unwrap().iter().zip(
            unit.blocks[edge.target.index()]
                .parameters
                .get(&unit.pools.values)
                .unwrap(),
        ) {
            graph[arg.index()].push(param.index());
        }
    }
    for term in &unit.terminators {
        match *term {
            Terminator::Return(Some(v)) => sinks.push(v.index()),
            Terminator::Call { call, failure, .. } => {
                let site = &unit.tables.calls[call.index()];
                for a in site.arguments.get(&unit.pools.call_arguments).unwrap() {
                    sinks.push(match a.value {
                        ArgumentValue::Value(v) => v.index(),
                        ArgumentValue::Address { address, .. } => root(address),
                    });
                }
                sinks.extend(site.callee.value().map(|v| v.index()));
                sinks.extend(
                    unit.tables.failures[failure.index()]
                        .dispose
                        .get(&unit.pools.values)
                        .unwrap()
                        .iter()
                        .map(|v| v.index()),
                );
            }
            _ => (),
        }
    }
    // Rank reversal tests both propagation orders, including edges that dirty
    // an earlier rank. The boolean join only dirties a node on false -> true.
    let rank = |node| {
        if reverse {
            graph.len() - 1 - node
        } else {
            node
        }
    };
    let mut states = vec![false; graph.len()];
    states[source] = true;
    let mut dirty = vec![false; graph.len()];
    dirty[rank(source)] = true;
    let mut current = rank(source);
    loop {
        assert!(dirty[current]);
        dirty[current] = false;
        let node = rank(current);
        for &next in &graph[node] {
            if !states[next] {
                states[next] = true;
                dirty[rank(next)] = true;
                current = current.min(rank(next));
            }
        }
        if !dirty[current] {
            let Some(next) = dirty.iter().position(|&d| d) else {
                break;
            };
            current = next;
        }
    }
    sinks.into_iter().any(|sink| states[sink])
}

#[test]
fn independent_escape_fixed_point_agrees_in_both_orders() {
    for (source, name, expected) in [
        (ORIGINAL, "scratch", false),
        (LENGTHS, "scratch", false),
        (SCALAR, "scratch", false),
        (NESTED, "scratch", false),
        (ELEMENT, "scratch", false),
        (COMPARE, "scratch", false),
        (EARLY, "scratch", false),
        (RETURNED, "main", true),
        (CALL, "main", true),
        (INDIRECT, "main", true),
        (MUT, "main", true),
        (CAPTURED, "main", true),
        (RETAINED, "main", true),
    ] {
        let (_, lowered) = lower_valid(source);
        let unit = unit_named(&lowered, name);
        let sources: Vec<_> = unit
            .instructions
            .iter()
            .filter_map(|inst| match *inst {
                Inst::Construct { result, .. }
                    if unit.types[unit.values[result.index()].ty.index()].repr
                        == ValueRepr::Block =>
                {
                    Some(result.index())
                }
                _ => None,
            })
            .collect();
        assert!(!sources.is_empty(), "{source}");
        for &allocation in &sources {
            assert_eq!(escapes(unit, allocation, false), expected, "{source}");
            assert_eq!(escapes(unit, allocation, true), expected, "{source}");
        }
        let placed = place(
            unit.clone(),
            &lowered.table,
            &PlaceOptions {
                regions: true,
                noalloc: false,
            },
        )
        .unwrap();
        assert_eq!(
            region_count(&placed),
            if expected { 0 } else { sources.len() },
            "{source}"
        );
    }
}

#[test]
fn cleanup_dispose_still_prevents_localization() {
    use hirsel_lir::ir::PoolRange;
    let (_, lowered) = lower_valid(ORIGINAL);
    let mut unit = unit_named(&lowered, "scratch").clone();
    let (value, failure) = unit
        .instructions
        .iter()
        .find_map(|inst| match *inst {
            Inst::Construct {
                result, failure, ..
            } => Some((result, failure)),
            _ => None,
        })
        .unwrap();
    // Forge an explicit cleanup escape, keeping the scalar SeqLen consumer.
    unit.tables.failures[failure.index()].dispose =
        PoolRange::push(&mut unit.pools.values, [value]);
    for regions in [true, false] {
        assert!(matches!(
            place(
                unit.clone(),
                &lowered.table,
                &PlaceOptions {
                    regions,
                    noalloc: true
                }
            ),
            Err(PlaceError::HeapInNoalloc(_))
        ));
    }
}

/// Capacity and Hash are generated library instructions, not direct source
/// operations. Revalidate each mutated Ready unit, then compare both emitters
/// against H on the exact uncounted payload and fresh scalar result.
#[test]
fn capacity_and_hash_native_controls() {
    use hirsel_driver::*;
    use hirsel_engine::*;
    use hirsel_lir::validate::{Inputs, validate};
    let matrix = matrix();
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("native-noalloc-observer-controls-19");
    std::fs::create_dir_all(&root).unwrap();
    let tools = Toolchain::for_matrix(&root.join("runtime"), &matrix).unwrap();
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let context = default_context();
        let file = engine.sources.add("observer.hir", ORIGINAL.into()).unwrap();
        assert!(
            check_progress(&mut engine, file, &context)
                .unwrap()
                .diagnostics
                .is_empty()
        );
        let snapshot = engine.export_native_input().unwrap();
        let input = engine.import_native_input(snapshot).unwrap();
        let entry = entry_path(&context);
        let sites = Sites::new(input.snapshot());
        for observer in ["capacity", "hash"] {
            for regions in [true, false] {
                let mut options = Options::new(target());
                options.regions = regions;
                let mut ready = build(&input, &options).unwrap();
                let index = ready
                    .units
                    .iter()
                    .position(|u| u.unit().header.definition.0.name == "scratch")
                    .unwrap();
                let mut unit = ready.units[index].unit().clone();
                let at = unit
                    .instructions
                    .iter()
                    .position(|i| matches!(i, Inst::SeqLen { .. }))
                    .unwrap();
                let Inst::SeqLen { result, value } = unit.instructions[at] else {
                    unreachable!()
                };
                assert_eq!(
                    unit.types[unit.values[value.index()].ty.index()].repr,
                    ValueRepr::Uncounted
                );
                unit.instructions[at] = if observer == "capacity" {
                    Inst::SeqCapacity { result, value }
                } else {
                    let layout = unit.types[unit.values[value.index()].ty.index()].layout;
                    let failure = unit
                        .instructions
                        .iter()
                        .find_map(|inst| match inst {
                            Inst::Construct { failure, .. } => Some(*failure),
                            _ => None,
                        })
                        .unwrap();
                    let failure = observer_failure(&mut unit, failure);
                    Inst::Hash {
                        result,
                        value,
                        layout,
                        failure,
                    }
                };
                let inputs = Inputs {
                    target: &ready.target,
                    layouts: &ready.table,
                    manifest: &ready.manifest,
                    operations: &ready.operations,
                    memo: Default::default(),
                    discharge: None,
                };
                let unit = validate(unit, &inputs)
                    .unwrap_or_else(|e| panic!("{observer}: {:?}", e.violations));
                ready.replace_unit(index, unit).unwrap();
                let heap = heap_campaign(&ready, &entry, &sites).unwrap();
                assert_eq!(heap.counters.allocations, 0);
                assert_eq!(heap.counters.live_heap_bytes, 0);
                for (backend, opt) in [
                    (Backend::Llvm, OptLevel::None),
                    (Backend::Llvm, OptLevel::Default),
                    (Backend::Cranelift, OptLevel::None),
                ] {
                    let directory = root.join(format!("{observer}-{regions}-{backend:?}-{opt:?}"));
                    std::fs::create_dir_all(&directory).unwrap();
                    let objects =
                        emit_backend_objects(&ready, backend, opt, Partition::Whole, false, None)
                            .unwrap();
                    let exe = link_objects(&ready, &entry, &objects, &tools, &directory).unwrap();
                    let run = launch(&exe, &ready.target, &tools, &directory, true).unwrap();
                    assert_eq!(
                        observe(&run, &ready, &sites).unwrap(),
                        heap.observation,
                        "{observer} regions={regions} {backend:?} {opt:?}"
                    );
                    eprintln!(
                        "{observer} regions={regions} {backend:?} {opt:?}: agrees with zero-heap H"
                    );
                }
            }
        }
    });
}

/// Synthetic Hash can fail while the length call's operand loan is active.
/// Give it its own failure/edge identities and end that loan before entering
/// the constructor's older cleanup (which precedes the loan).
pub(super) fn observer_failure<S>(
    unit: &mut LirUnit<S>,
    original: hirsel_lir::ir::FailureId,
) -> hirsel_lir::ir::FailureId {
    use hirsel_lir::ir::{Block, BlockId, BlockKind, Edge, EdgeId, FailureId, PoolRange};
    let mut failure = unit.tables.failures[original.index()].clone();
    let continuation = unit.tables.edges[failure.cleanup.index()].clone();
    let edge = EdgeId(unit.tables.edges.len() as u32);
    unit.tables.edges.push(continuation);
    let loans: Vec<_> = unit
        .instructions
        .iter()
        .filter_map(|inst| match inst {
            Inst::BeginLoan { loan, .. } => Some(*loan),
            _ => None,
        })
        .collect();
    assert_eq!(loans.len(), 1);
    let block = BlockId(unit.blocks.len() as u32);
    unit.blocks.push(Block {
        kind: BlockKind::FailureCleanup,
        parameters: PoolRange::EMPTY,
        instructions: PoolRange::push(
            &mut unit.instructions,
            [Inst::EndLoan {
                loan: loans[0],
                if_active: false,
            }],
        ),
        anchor: failure.anchor,
    });
    unit.terminators.push(Terminator::Jump(edge));
    failure.cleanup = EdgeId(unit.tables.edges.len() as u32);
    unit.tables.edges.push(Edge {
        target: block,
        arguments: PoolRange::EMPTY,
    });
    let id = FailureId(unit.tables.failures.len() as u32);
    unit.tables.failures.push(failure);
    id
}

// A fieldless alternative has the same owning layout as Some(List), but no
// region provenance. Its comparison must not turn it into a regional value.
const EMPTY_VARIANT: &str = r#"
fn probe(yes: bool) -> bool {
    let items: Option[List[u64]] = if yes { Some([1u64]) } else { None() }
    items == Some([1u64])
}
pub fn main() -> Result[(), u64] {
    failure.check(probe(true) && !probe(false), TextInvariant())
    Ok(())
}
"#;

#[test]
fn empty_variant_observers_preserve_regional_construction_provenance() {
    use hirsel_driver::*;
    use hirsel_engine::*;
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let context = default_context();
        let file = engine
            .sources
            .add("empty-variant.hir", EMPTY_VARIANT.into())
            .unwrap();
        let checked = check_progress(&mut engine, file, &context).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let entry = entry_path(&context);
        let reference = run_observed_snapshot(
            &mut engine,
            &RunConfiguration {
                entry: entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        let reference = Observation::of_reference(reference.observation.as_ref().unwrap());
        assert_eq!(reference.exit_code, Some(0));
        assert!(reference.failures.is_empty());
        let snapshot = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(snapshot).unwrap();
        let configurations: Vec<_> = matrix()
            .into_iter()
            .map(|mut c| {
                c.options.validation = Validation::EveryStage;
                c
            })
            .collect();
        let directory = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("empty-variant-observers-20");
        std::fs::create_dir_all(&directory).unwrap();
        let tools = Toolchain::for_matrix(&directory.join("runtime"), &configurations).unwrap();
        let mut executions = 0;
        for (configuration, outcome) in differential(
            &validated,
            &entry,
            &reference,
            &configurations,
            &tools,
            &directory,
        ) {
            assert!(
                matches!(outcome, Outcome::Agree),
                "{configuration}: {outcome:?}"
            );
            executions += 1;
        }
        assert_eq!(executions, 8);
    });
}
