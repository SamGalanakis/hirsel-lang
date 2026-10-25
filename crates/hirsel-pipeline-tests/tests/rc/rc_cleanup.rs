//! Cleanup lowering over real driver-derived programs: every Ready unit passes
//! C's Ready validator, and the heap executor runs it with a clean terminal
//! audit, agreeing with the Rc program it was lowered from.
use crate::rc_support as support;

use hirsel_heapcheck::{Config, NoOperations, Observation};
use hirsel_lir::ir::{
    Block, BlockId, BlockKind, FailureEdge, FailureId, Inst, LirUnit, PoolRange, Terminator,
};
use hirsel_lir::layout_types::LayoutTable;
use hirsel_lir::stage::{Rc, Ready, Stage};
use hirsel_lir::validate::{Inputs, Violation, ViolationKind};
use hirsel_rc::cleanup::Finalizers;
use hirsel_rc::insert::Proofs;
use hirsel_rc::instance::{InstanceId, Program};
use hirsel_rc::placement::Options;
use support::*;

struct Built {
    operations: hirsel_layout::operation::OperationRegistry,
    program: Program,
    table: LayoutTable,
    rc: Vec<LirUnit<Rc>>,
    ready: Vec<LirUnit<Ready>>,
}

#[test]
fn operand_cleanup_retires_invocation_before_resource_finalizers() {
    let (built, observation) = ready(
        r#"resource type R {}
drop(x) {}
fn number() -> u64 { 1u64 }
fn touch(mut xs: List[u64]) {}
pub fn main() -> u64 {
  let r = R()
  let mut xs: List[List[u64]] = [[number()]]
  touch(mut xs[0u64])
  7u64
}
"#,
    );
    assert!(observation.failures.is_empty());
    campaign(&built);
}

#[test]
fn a_join_keeps_the_untaken_error_payload_for_cleanup() {
    let source = r#"pub fn main(rt: Io, args: Args) -> Result[(), LaunchError] {
  let text: String = match string.from_bytes(b"ok") { Ok(s) => take s, Error(_) => "bad" }
  out.print(io.stdout(rt)? Grant, text) ? Output
  Ok(())
}
pub type LaunchError = Grant(GrantError) | Output(OutputError)
"#;
    validate_driver_ready(source);
}

#[test]
fn guarded_product_variants_keep_nested_cleanup() {
    validate_driver_ready(include_str!(
        "../../../../conformance/DROP/DROP-2-guarded-product-take-run.hir"
    ));
}

#[test]
fn plain_error_payloads_remain_valid_beside_conditional_owners() {
    validate_driver_ready(include_str!(
        "../../../../conformance/PROV/PROV-2-runtime-handle-narrow.hir"
    ));
    validate_driver_ready(include_str!(
        "../../../../conformance/MODE/MODE-2-pattern-physical-variant-ok.hir"
    ));
}

/// Drop flags reroute every switch of `make`, and finishing then drops
/// edges: no arm may keep an edge id from before the compaction.
#[test]
fn a_fallible_resource_call_beside_a_resource_construction_keeps_its_edges() {
    validate_driver_ready(include_str!(
        "../../../../conformance/DROP/DROP-2-noalloc-drop-beside-net-connect-try-run.hir"
    ));
    validate_driver_ready(include_str!(
        "../../../../conformance/DROP/DROP-2-noalloc-drop-beside-net-connect-match-run.hir"
    ));
    validate_driver_ready(include_str!(
        "../../../../conformance/DROP/DROP-2-noalloc-drop-beside-finalized-try-run.hir"
    ));
}

fn validate_driver_ready(source: &'static str) {
    let built = try_build(source).unwrap();
    let rows = hirsel_engine::with_compiler_stack(move || {
        let mut engine = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            hirsel_driver::compiler_adapters(),
        );
        let file = engine.sources.add("test.hir", source.to_string()).unwrap();
        hirsel_engine::check_progress(&mut engine, file, &hirsel_engine::default_context())
            .unwrap();
        hirsel_driver::native::manifest(&engine.export_native_input().unwrap()).unwrap()
    });
    let target = target();
    let inputs = Inputs {
        memo: Default::default(),
        discharge: None,
        operations: &built.operations,
        target: &target,
        layouts: &built.table,
        manifest: &rows,
    };
    for unit in &built.ready {
        let violations = hirsel_lir::validate::check(unit, &inputs);
        assert!(violations.is_empty(), "{violations:?}\n{}", dump(unit));
    }
}

/// Lower to Rc (regions on, elision on), then to Ready.
fn try_build(source: &str) -> Result<Built, String> {
    try_build_with(source, false)
}

/// As `try_build`; with `reuse`, through drop specialization and reuse
/// before cleanup lowering.
fn try_build_with(source: &str, reuse: bool) -> Result<Built, String> {
    let (program, lowered, rc) = rc_units(source)?;
    let operations = lowered.operation_registry(&program);
    let table = lowered.table.clone();
    let finalizers = Finalizers::new(&lowered.instances, &rc);
    let ready = rc
        .iter()
        .map(|u| {
            if reuse {
                let options = hirsel_rc::reuse::Options { reuse: true };
                let reused = hirsel_rc::reuse::reuse(u.clone(), &table, &options)
                    .map_err(|e| format!("reuse: {e:?}"))?;
                hirsel_rc::cleanup::lower(reused, &table, &finalizers)
                    .map_err(|e| format!("cleanup: {e:?}"))
            } else {
                hirsel_rc::cleanup::lower(u.clone(), &table, &finalizers)
                    .map_err(|e| format!("cleanup: {e:?}"))
            }
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Built {
        operations,
        program,
        table,
        rc,
        ready,
    })
}

type RcUnits = (Program, hirsel_rc::import::Lowered, Vec<LirUnit<Rc>>);

fn rc_units(source: &str) -> Result<RcUnits, String> {
    let program = try_compile(source)?;
    let lowered =
        hirsel_rc::import::lower(&program, &target()).map_err(|e| format!("lower: {e:?}"))?;
    let table = lowered.table.clone();
    let mut rc = Vec::new();
    for (i, u) in lowered.units.iter().enumerate() {
        let id = InstanceId(i as u32);
        let options = Options::for_instance(true, &lowered.instances, id);
        let placed = hirsel_rc::placement::place(u.clone(), &table, &options)
            .map_err(|e| format!("place: {e:?}"))?;
        let proofs = Proofs::for_instance(&lowered.instances, id);
        let r = hirsel_rc::insert::insert(placed, &table, &proofs)
            .map_err(|e| format!("insert: {e:?}"))?;
        rc.push(hirsel_rc::elide::elide(r, &table).map_err(|e| format!("elide: {e:?}"))?);
    }
    Ok((program, lowered, rc))
}

fn check<S: Stage>(b: &Built, units: &[LirUnit<S>]) -> Vec<(String, Vec<Violation>)> {
    let manifest = manifest(&b.program);
    let target = target();
    let inputs = Inputs {
        memo: Default::default(),
        discharge: None,
        operations: &b.operations,
        target: &target,
        layouts: &b.table,
        manifest: &manifest,
    };
    units
        .iter()
        .map(|u| {
            (
                u.header.definition.0.name.to_string(),
                hirsel_lir::validate::check(u, &inputs),
            )
        })
        .filter(|(_, v)| !v.is_empty())
        .collect()
}

fn execute<S: Stage>(
    operations: &hirsel_layout::operation::OperationRegistry,
    table: &LayoutTable,
    units: &[LirUnit<S>],
) -> hirsel_heapcheck::Run {
    let program = hirsel_heapcheck::Program {
        invocation_bodies: operations.invocation_bodies.clone(),
        invocations: operations.invocations.clone(),
        pretransfer_adapters: operations.pretransfer_adapters.clone(),
        target: target(),
        layouts: table.clone(),
        units: units.to_vec(),
        statics: Vec::new(),
    };
    hirsel_heapcheck::run(
        &program,
        &path("main"),
        &[],
        &mut NoOperations,
        &Config::default(),
    )
}

/// Build `source`, require C to accept every Ready unit and H to run the Ready
/// program with a clean audit and the Rc program's observation.
fn ready(source: &str) -> (Built, Observation) {
    let b = try_build(source).unwrap_or_else(|e| panic!("{source}\n{e}"));
    let v = check(&b, &b.ready);
    if !v.is_empty() {
        let dumps: Vec<String> = b.ready.iter().map(dump).collect();
        panic!("{source}\n{}\n{v:#?}", dumps.join("\n"));
    }
    let run = execute(&b.operations, &b.table, &b.ready);
    let audit = run.result.unwrap_or_else(|v| {
        let dumps: Vec<String> = b.ready.iter().map(dump).collect();
        panic!("{v}\n{source}\n{}", dumps.join("\n"))
    });
    let rc = execute(&b.operations, &b.table, &b.rc)
        .result
        .unwrap_or_else(|v| panic!("rc: {v}\n{source}"));
    assert!(
        audit.observation.equivalent(&rc.observation),
        "{source}\nready {:?} != rc {:?}",
        audit.observation,
        rc.observation
    );
    (b, audit.observation)
}

#[test]
fn a_counted_local_is_released_by_its_layout() {
    ready(
        "type P { a: u64 }\npub fn main() -> u64 {\n  let b = box P(a: 1)\n  let c = b\n  7\n}\n",
    );
}

/// Fault-inject every failure-capable site of the Ready program: each failure
/// path must pass H's terminal audit.
fn campaign(b: &Built) {
    if let Some((site, v)) = first_injected_violation(&b.operations, &b.table, &b.ready) {
        // Whether the Rc program it was lowered from fails the same way.
        let rc = first_injected_violation(&b.operations, &b.table, &b.rc)
            .map(|(site, v)| format!("site {site:?}: {v}"));
        let dumps: Vec<String> = b.ready.iter().map(dump).collect();
        panic!(
            "site {site:?}: {v}\nRc campaign: {rc:?}\n{}",
            dumps.join("\n")
        );
    }
}

fn first_injected_violation<S: Stage>(
    operations: &hirsel_layout::operation::OperationRegistry,
    table: &LayoutTable,
    units: &[LirUnit<S>],
) -> Option<(hirsel_heapcheck::Perturbation, hirsel_heapcheck::Violation)> {
    let program = hirsel_heapcheck::Program {
        invocation_bodies: operations.invocation_bodies.clone(),
        invocations: operations.invocations.clone(),
        pretransfer_adapters: operations.pretransfer_adapters.clone(),
        target: target(),
        layouts: table.clone(),
        units: units.to_vec(),
        statics: Vec::new(),
    };
    let c = hirsel_heapcheck::fault_campaign(
        &program,
        &path("main"),
        &[],
        &mut || Box::new(NoOperations),
        &Config::default(),
    );
    c.first_violation().map(|(site, v)| (site, v.clone()))
}

fn finalized(trace: &hirsel_heapcheck::Trace) -> Vec<String> {
    trace
        .iter()
        .filter(|e| matches!(e, hirsel_heapcheck::Event::Finalize { .. }))
        .map(|e| format!("{e:?}"))
        .collect()
}

fn rules(o: &Observation) -> Vec<&str> {
    o.failures.iter().map(|f| f.rule.as_str()).collect()
}

/// A resource whose drop body divides by its `n`, with a counted field.
const T: &str = "type P { a: u64 }
resource type T {
  n: u64,
  xs: box P,
}
drop(x) {
  let _ = 10u64 / x.n
  ()
}
";

/// The Ready program finalizes as the Rc program does, in the same order.
fn same_finalizers(b: &Built) {
    let ready = execute(&b.operations, &b.table, &b.ready);
    let rc = execute(&b.operations, &b.table, &b.rc);
    assert_eq!(finalized(&ready.trace), finalized(&rc.trace));
}

#[test]
fn a_counted_field_resource_is_finalized_then_its_field_released() {
    let (b, o) = ready(&format!(
        "{T}pub fn main() -> u64 {{\n  let t = T(n: 1, xs: box P(a: 2))\n  7\n}}\n"
    ));
    assert!(o.failures.is_empty());
    same_finalizers(&b);
    campaign(&b);
}

#[test]
fn nested_failed_finalizers_keep_the_first_failure_primary_and_release_every_field() {
    let (b, o) = ready(&format!(
        "{T}type Pair {{ first: T, second: T }}
pub fn main() -> u64 {{
  let p = Pair(first: T(n: 0, xs: box P(a: 1)), second: T(n: 0, xs: box P(a: 2)))
  7
}}
"
    ));
    // Fields in reverse: `second` fails first and stays primary; `first`
    // still runs and adds its failure to the chain.
    assert_eq!(rules(&o), ["SEM-1", "SEM-1"]);
    same_finalizers(&b);
    campaign(&b);
}

#[test]
fn an_extracted_return_is_released_when_its_cleanup_fails() {
    let (b, o) = ready(&format!(
        "{T}fn f() -> box P {{
  let t = T(n: 0, xs: box P(a: 1))
  box P(a: 5)
}}
pub fn main() -> u64 {{
  let r = f()
  7
}}
"
    ));
    assert_eq!(rules(&o), ["SEM-1"]);
    campaign(&b);
}

#[test]
fn a_later_argument_failure_releases_the_earlier_arguments() {
    let (b, o) = ready(
        "type P { a: u64 }
fn g(take a: box P, b: u64) -> u64 {
  b
}
pub fn main() -> u64 {
  let z = 0u64
  g(box P(a: 1), 10u64 / z)
}
",
    );
    assert_eq!(rules(&o), ["SEM-1"]);
    campaign(&b);
}

#[test]
fn a_partly_moved_record_releases_only_its_present_fields() {
    let (b, o) = ready(
        "type P { a: u64 }
type Q { a: box P, b: box P, c: box P }
fn keep(take x: box P) -> u64 {
  1
}
pub fn main() -> u64 {
  let q = Q(a: box P(a: 1), b: box P(a: 2), c: box P(a: 3))
  let Q(a: _, b: x, c: _) = take q
  keep(take x)
}
",
    );
    assert!(o.failures.is_empty());
    campaign(&b);
}

#[test]
fn an_out_of_scope_order_cleanup_is_rejected() {
    let source = format!(
        "{T}pub fn main() -> u64 {{
  let first = T(n: 1, xs: box P(a: 1))
  let second = T(n: 2, xs: box P(a: 2))
  7
}}
"
    );
    let (valid, _) = ready(&source);
    // Swap the two drops of the exit cleanup in the Rc unit; lowering keeps
    // the order it is given, so the Ready unit carries the swap.
    let (_, lowered, mut rc) = rc_units(&source).unwrap();
    let main = instances_of(&lowered, "main")[0].index();
    let unit = &mut rc[main];
    let block = unit
        .blocks
        .iter()
        .find(|b| {
            b.kind == BlockKind::Cleanup
                && b.instructions
                    .get(&unit.instructions)
                    .unwrap()
                    .iter()
                    .filter(|i| matches!(i, Inst::Drop { .. }))
                    .count()
                    == 2
        })
        .expect("an exit cleanup with both drops");
    let start = block.instructions.start as usize;
    let range = start..start + block.instructions.len as usize;
    let drops: Vec<usize> = range
        .filter(|&i| matches!(unit.instructions[i], Inst::Drop { .. }))
        .collect();
    let (x, y) = (drops[0], drops[1]);
    let (dx, dy) = (unit.instructions[x], unit.instructions[y]);
    // The addresses swap, and each drop's failure successor runs the rest
    // of the swapped list: the second keeps its own (nothing is left), the
    // first fails into a new block dropping what the second now drops. The
    // first's old successor drops the first slot, and other failure sites
    // share it, so it cannot be reused.
    let (
        Inst::Drop {
            address: first,
            if_live,
            anchor: x_anchor,
            failure: x_failure,
        },
        Inst::Drop {
            address: second,
            anchor: y_anchor,
            failure: y_failure,
            ..
        },
    ) = (dx, dy)
    else {
        unreachable!("two drops")
    };
    let rest = unit.tables.failures[y_failure.index()].clone();
    let failed = BlockId(unit.blocks.len() as u32);
    let into_rest = |unit: &mut LirUnit<Rc>| {
        let edge = hirsel_lir::ir::EdgeId(unit.tables.edges.len() as u32);
        unit.tables
            .edges
            .push(unit.tables.edges[rest.cleanup.index()].clone());
        edge
    };
    let next = into_rest(unit);
    let fails = FailureId(unit.tables.failures.len() as u32);
    unit.tables.failures.push(FailureEdge {
        cleanup: next,
        ..rest.clone()
    });
    let start = unit.instructions.len() as u32;
    unit.instructions.push(Inst::Drop {
        address: first,
        if_live: false,
        anchor: y_anchor,
        failure: fails,
    });
    let jump = into_rest(unit);
    unit.blocks.push(Block {
        kind: BlockKind::FailureCleanup,
        parameters: PoolRange::EMPTY,
        instructions: PoolRange::new(start, 1),
        anchor: unit.blocks[0].anchor,
    });
    unit.terminators.push(Terminator::Jump(jump));
    let into_failed = unit.tables.failures[x_failure.index()].cleanup;
    unit.tables.edges[into_failed.index()].target = failed;
    unit.instructions[x] = Inst::Drop {
        address: second,
        if_live,
        anchor: x_anchor,
        failure: x_failure,
    };
    unit.instructions[y] = Inst::Drop {
        address: first,
        if_live,
        anchor: y_anchor,
        failure: y_failure,
    };
    let finalizers = Finalizers::new(&lowered.instances, &rc);
    let ready: Vec<_> = rc
        .iter()
        .map(|u| hirsel_rc::cleanup::lower(u.clone(), &valid.table, &finalizers).unwrap())
        .collect();
    let mutant = Built {
        operations: valid.operations.clone(),
        program: valid.program.clone(),
        table: valid.table.clone(),
        rc: rc.clone(),
        ready,
    };
    let rejected = check(&mutant, &mutant.ready);
    assert!(
        rejected.iter().any(|(_, v)| v
            .iter()
            .any(|v| matches!(v.kind, ViolationKind::CleanupOrder(_)))),
        "{rejected:?}"
    );
    let rc_rejected = check(&mutant, &mutant.rc);
    assert!(
        rc_rejected.iter().any(|(_, v)| {
            v.iter()
                .any(|v| matches!(v.kind, ViolationKind::CleanupOrder(_)))
        }),
        "{rc_rejected:?}"
    );
    // H observes the swapped finalizer order.
    let swapped = finalized(&execute(&mutant.operations, &mutant.table, &mutant.ready).trace);
    assert_ne!(
        swapped,
        finalized(&execute(&valid.operations, &valid.table, &valid.ready).trace)
    );
}

#[test]
fn an_owned_string_local_is_copied_and_released_by_its_layout() {
    let (b, _) = ready(
        "pub fn main() -> u64 {
  let s: String = \"text\"
  let t = copy s
  let u = copy t
  7
}
",
    );
    campaign(&b);
}

#[test]
fn a_record_of_two_counted_fields_is_copied_by_fields_and_released_inline() {
    let (b, o) = ready(
        "type P { a: u64 }
type R { a: box P, b: box P }
fn sum(r: R) -> u64 {
  1
}
pub fn main() -> u64 {
  let r = R(a: box P(a: 1), b: box P(a: 2))
  let s = copy r
  sum(r) + sum(s)
}
",
    );
    assert!(o.failures.is_empty());
    campaign(&b);
}

#[test]
fn a_callable_environment_is_released_with_its_callable() {
    let (b, _) = ready(
        "type P { a: u64 }
pub fn main() -> u64 {
  let p = box P(a: 1)
  let f = fn() -> u64 {
    let q = copy p
    1
  }
  let g = copy f
  f() + g()
}
",
    );
    campaign(&b);
}

/// `r.a = make(n)`: the new value is prepared before the replace, so a
/// failing preparation leaves the old content in place for the cleanup to
/// release; a successful one displaces the old box, which is released.
fn assign(n: u64) -> String {
    format!(
        "type P {{ a: u64 }}
type R {{ a: box P, b: box P }}
fn make(n: u64) -> box P {{
  box P(a: 10 / n)
}}
pub fn main() -> u64 {{
  let mut r = R(a: box P(a: 1), b: box P(a: 2))
  r.a = make({n}u64)
  7
}}
"
    )
}

#[test]
fn a_two_phase_replace_releases_the_displaced_content() {
    let (b, o) = ready(&assign(5));
    assert!(o.failures.is_empty());
    campaign(&b);
}

#[test]
fn a_two_phase_replace_with_failing_preparation_keeps_and_releases_the_old_content() {
    let (b, o) = ready(&assign(0));
    assert_eq!(rules(&o), ["SEM-1"]);
    campaign(&b);
}

/// Edit a block and repack the arena, including adjacent empty blocks.
fn edit_block(u: &mut LirUnit<Ready>, block: usize, edit: impl FnOnce(&mut Vec<Inst>)) {
    let mut rows: Vec<Vec<Inst>> = u
        .blocks
        .iter()
        .map(|row| row.instructions.get(&u.instructions).unwrap().to_vec())
        .collect();
    edit(&mut rows[block]);
    u.instructions.clear();
    for (info, row) in u.blocks.iter_mut().zip(rows) {
        info.instructions = PoolRange::new(u.instructions.len() as u32, row.len() as u32);
        u.instructions.extend(row);
    }
}

fn insert_at(u: &mut LirUnit<Ready>, block: usize, index: usize, inst: Inst) {
    edit_block(u, block, |row| row.insert(index, inst));
}

/// The explicit finalization of a counted-field resource, and three
/// mutants of it that C rejects as `FinalizeState` and H rejects as it
/// executes them: a second `FinalizeBegin`, a `FinalizeEnd` without its
/// begin, and the slot's `StorageDead` while the finalizer runs.
#[test]
fn c_and_h_agree_on_each_finalization_state_mutant() {
    let (b, _) = ready(&format!(
        "{T}pub fn main() -> u64 {{\n  let t = T(n: 1, xs: box P(a: 2))\n  7\n}}\n"
    ));
    let main = b
        .ready
        .iter()
        .position(|u| u.header.definition.0.name == "main")
        .unwrap();
    // Target the compiler's actual in-place lifecycle. Its normal cleanup
    // already drains the counted field and closes the finalized shell.
    let unit = &b.ready[main];
    let (bb, bi, address, anchor) = unit
        .blocks
        .iter()
        .enumerate()
        .filter(|(_, block)| block.kind != BlockKind::FailureCleanup)
        .find_map(|(bb, block)| {
            block
                .instructions
                .get(&unit.instructions)
                .unwrap()
                .iter()
                .enumerate()
                .find_map(|(bi, inst)| match *inst {
                    Inst::FinalizeBegin { address, anchor } => Some((bb, bi, address, anchor)),
                    _ => None,
                })
        })
        .expect("normal cleanup begins the source resource finalizer");
    let slot = match unit.addresses[address.index()].base {
        hirsel_lir::ir::AddressBase::Slot(slot) => slot,
        other => panic!("{other:?}"),
    };
    let accepted = check(&b, &b.ready);
    assert!(
        accepted.is_empty(),
        "explicit lifecycle accepted twin: {accepted:?}\n{}",
        dump(&b.ready[main])
    );
    let positive = execute(&b.operations, &b.table, &b.ready);
    assert!(positive.result.is_ok(), "{:?}", positive.result);
    let twice = {
        let mut u = b.ready[main].clone();
        let inst = u.instructions[u.blocks[bb].instructions.start as usize + bi];
        insert_at(&mut u, bb, bi, inst);
        u
    };
    let end_only = {
        let mut u = b.ready[main].clone();
        // Keep the original Begin for independent source-call coverage;
        // the extra End runs first, without any active finalization.
        insert_at(&mut u, bb, bi, Inst::FinalizeEnd { address, anchor });
        u
    };
    let dead_while_finalizing = {
        let mut u = b.ready[main].clone();
        insert_at(
            &mut u,
            bb,
            bi + 1,
            Inst::StorageDead {
                slot,
                if_live: false,
            },
        );
        u
    };
    for (name, mutant) in [
        ("begin twice", twice),
        ("end without begin", end_only),
        ("storage dead while finalizing", dead_while_finalizing),
    ] {
        let mut units = b.ready.clone();
        units[main] = mutant;
        let c: Vec<_> = check(&b, &units)
            .into_iter()
            .flat_map(|(_, v)| v)
            .map(|v| v.kind)
            .collect();
        assert!(
            c.iter()
                .any(|k| matches!(k, ViolationKind::FinalizeState(_))),
            "{name}: C {c:?}"
        );
        let h = execute(&b.operations, &b.table, &units);
        assert!(h.result.is_err(), "{name}: H accepted it");
    }
}

/// A match takes one part of an active variant's payload; the variant's
/// other counted part is released through a tag-checked `VariantField`
/// address prepared where the cleanup runs.
/// As `ready`, for a program whose Rc form the heap checker cannot run as a
/// reference (its semantic `Drop` walks a whole drop program over a part
/// moved out): C accepts every Ready unit and H runs them with a clean audit.
fn ready_alone(source: &str) -> (Built, Observation) {
    let b = try_build(source).unwrap_or_else(|e| panic!("{source}\n{e}"));
    let v = check(&b, &b.ready);
    if !v.is_empty() {
        let dumps: Vec<String> = b.ready.iter().map(dump).collect();
        panic!("{source}\n{}\n{v:#?}", dumps.join("\n"));
    }
    let audit = execute(&b.operations, &b.table, &b.ready)
        .result
        .unwrap_or_else(|v| panic!("{v}\n{source}"));
    (b, audit.observation)
}

#[test]
fn a_variant_with_a_taken_part_releases_its_other_fields() {
    let (b, o) = ready_alone(
        "type P { a: u64 }
fn keep(take x: box P) -> u64 {
  1
}
type Two = Pair(box P, box P)
fn pick(take o: Two) -> u64 {
  match take o {
    Pair(x, _) => keep(take x),
  }
}
pub fn main() -> u64 {
  pick(Pair(box P(a: 1), box P(a: 2)))
}
",
    );
    assert!(o.failures.is_empty());
    assert!(matches!(
        o.returned,
        Some(hirsel_heapcheck::Observed::Integer { bits: 1, .. })
    ));
    assert!(
        b.ready.iter().any(|u| u.addresses.iter().any(|a| a
            .projection
            .get(&u.pools.projections)
            .unwrap()
            .iter()
            .any(|s| matches!(s, hirsel_lir::ir::ProjectionStep::VariantField { .. })))),
        "no variant field address"
    );
    campaign(&b);
}

/// Two `mut` calls write `p` back on their failure edges too: whichever
/// call fails, its cleanup releases the version it wrote back.
#[test]
fn a_failing_mut_call_releases_the_version_it_wrote_back() {
    for d in [1, 0] {
        let (b, o) = ready(&format!(
            "type P {{ a: u64 }}
fn bump(mut x: box P, d: u64) -> () {{
  x = box P(a: 10 / d)
}}
pub fn main() -> u64 {{
  let mut p = box P(a: 1)
  bump(mut p, 1u64)
  bump(mut p, {d}u64)
  7
}}
"
        ));
        assert_eq!(rules(&o).len(), usize::from(d == 0));
        campaign(&b);
    }
}

/// Copy-on-write of an element under a record field joins the record's
/// versions where the detach and the shared path meet, and only the drop
/// reads the slot after that: cleanup lowering adds the version join the
/// release needs, and C accepts the result. (The heap checker does not run
/// counted arrays yet, so C is the witness here.)
#[test]
fn versions_that_meet_only_at_a_drop_get_a_join_for_its_release() {
    let source = "type Rec {
  items: Array[u64],
}
pub fn main() -> u64 {
  let mut r = Rec(items: [1, 2])
  let old = r.items
  r.items[0] = 9
  if old[0] == 1 {
    7
  } else {
    8
  }
}
";
    let b = try_build(source).unwrap_or_else(|e| panic!("{e}"));
    let v = check(&b, &b.ready);
    assert!(v.is_empty(), "{v:#?}");
    // The Rc unit had no reaching version at that drop; the Ready unit's
    // join is a block parameter whose arguments are versions.
    let joins = |u: &LirUnit<Ready>| u.blocks.iter().map(|b| b.parameters.len).sum::<u32>();
    let main = |units: &[LirUnit<Ready>]| {
        units
            .iter()
            .find(|u| u.header.definition.0.name == "main")
            .map(joins)
            .unwrap()
    };
    let rc_params: u32 =
        b.rc.iter()
            .find(|u| u.header.definition.0.name == "main")
            .map(|u| u.blocks.iter().map(|b| b.parameters.len).sum())
            .unwrap();
    assert!(main(&b.ready) > rc_params);
}

/// Build through the Reuse stage; C accepts every Ready unit and H runs
/// the Ready program with a clean audit, the Rc program's observation and
/// the Reuse program's reuse counters.
fn ready_via_reuse(source: &str) -> (Built, hirsel_heapcheck::Run) {
    let b = try_build_with(source, true).unwrap_or_else(|e| panic!("{source}\n{e}"));
    let v = check(&b, &b.ready);
    if !v.is_empty() {
        let dumps: Vec<String> = b.ready.iter().map(dump).collect();
        panic!("{source}\n{}\n{v:#?}", dumps.join("\n"));
    }
    let run = execute(&b.operations, &b.table, &b.ready);
    let audit = run.result.clone().unwrap_or_else(|v| {
        let dumps: Vec<String> = b.ready.iter().map(dump).collect();
        panic!("{v}\n{source}\n{}", dumps.join("\n"))
    });
    let rc = execute(&b.operations, &b.table, &b.rc)
        .result
        .unwrap_or_else(|v| panic!("rc: {v}\n{source}"));
    assert!(
        audit.observation.equivalent(&rc.observation),
        "{source}\nready {:?} != rc {:?}",
        audit.observation,
        rc.observation
    );
    campaign(&b);
    (b, run)
}

const PQ: &str = "type P { a: u64, b: u64 }\ntype Q { b: box P, n: u64 }\n";

#[test]
fn a_reused_box_lowers_to_ready_and_still_reuses() {
    let (b, run) = ready_via_reuse(&format!(
        "{PQ}fn mk(n: u64) -> box P {{ box P(a: n, b: n) }}\n\
         fn swap(take t: box P, n: u64) -> box P {{ let u = t; box P(a: n, b: 2) }}\n\
         pub fn main() -> u64 {{ let t: box P = mk(1); let u = swap(take t, 4); 3 }}"
    ));
    assert!(b.ready.iter().any(|u| {
        u.instructions
            .iter()
            .any(|i| matches!(i, Inst::Reuse { .. }))
    }));
    assert_eq!(run.counters.reuses, 1);
}

#[test]
fn a_failing_finalizer_with_a_live_token_lowers_to_ready() {
    let (_, run) = ready_via_reuse(&format!(
        "{PQ}resource type R {{ n: u64 }}\ndrop(x) {{\n  let _ = 1u8 / 0u8\n}}\n\
         fn hold(take t: box P) -> u64 {{\n  let k = {{\n    let r = R(n: 1)\n    \
         let u = t\n    1u64\n  }}\n  let v: box P = box P(a: 3, b: k)\n  3\n}}\n\
         pub fn main() -> u64 {{\n  let t: box P = box P(a: 1, b: 2)\n  hold(take t)\n}}"
    ));
    let audit = run.result.unwrap();
    assert_eq!(audit.observation.failures.len(), 1);
}

#[test]
fn both_variant_arms_move_their_payload_before_joined_cleanup() {
    let source = r#"
fn check(take r: Result[String, Bytes]) -> bool {
  match take r {
    Ok(s) => s == "ok"
    Error(bytes) => bytes == b"\xff"
  }
}
pub fn main() -> u64 {
  let valid = check(Ok("ok")) && check(Error(b"\xff"))
  1u64 / if valid { 1u64 } else { 0u64 }
}
"#;
    // Exercise Ready directly: H's pre-Ready semantic Drop currently has no
    // active-variant field mask for this partly moved scrutinee.
    for reuse in [false, true] {
        let built = try_build_with(source, reuse).unwrap();
        let violations = check(&built, &built.ready);
        assert!(
            violations.is_empty(),
            "{violations:#?}\n{}",
            built.ready.iter().map(dump).collect::<Vec<_>>().join("\n")
        );
        let audit = execute(&built.operations, &built.table, &built.ready)
            .result
            .unwrap();
        assert!(audit.observation.failures.is_empty());
        campaign(&built);
    }
}

#[test]
fn variant_matches_keep_their_proved_impossible_defaults_unsplit() {
    let source = r#"
fn decode(take data: Bytes, valid: bool) -> Result[String, Bytes] {
  if valid { Ok("ok") } else { Error(take data) }
}
pub fn main() -> u64 {
  let valid_source: Bytes = b"ok"
  let invalid_source: Bytes = b"\xff"
  let valid: bool = match decode(take valid_source, true) {
    Ok(s) => s == "ok"
    Error(bytes) => false
  }
  let invalid: bool = match decode(take invalid_source, false) {
    Ok(s) => false
    Error(bytes) => bytes == b"\xff"
  }
  1u64 / if valid && invalid { 1u64 } else { 0u64 }
}
"#;
    for reuse in [false, true] {
        let built = try_build_with(source, reuse).unwrap();
        let violations = check(&built, &built.ready);
        assert!(
            violations.is_empty(),
            "{violations:#?}\n{}",
            built.ready.iter().map(dump).collect::<Vec<_>>().join("\n")
        );
        assert!(
            execute(&built.operations, &built.table, &built.ready)
                .result
                .unwrap()
                .observation
                .failures
                .is_empty()
        );
        campaign(&built);
    }
}

#[test]
fn a_resource_option_is_empty_after_none_or_moved_some_at_a_join() {
    let source = r#"
resource type R { n: u64 }
drop(r) { () }
fn consume(take value: Option[R]) -> bool {
  match take value {
    None => false
    Some(r) => { let _ = take r; true }
  }
}
pub fn main() -> u64 {
  let first: Option[R] = Some(R(n: 1))
  let second: Option[R] = None
  let valid = consume(take first) && !consume(take second)
  1u64 / if valid {1u64} else {0u64}
}
"#;
    for reuse in [false, true] {
        let built = try_build_with(source, reuse).unwrap();
        let violations = check(&built, &built.ready);
        assert!(
            violations.is_empty(),
            "{violations:#?}\n{}",
            built.ready.iter().map(dump).collect::<Vec<_>>().join("\n")
        );
        assert!(
            execute(&built.operations, &built.table, &built.ready)
                .result
                .unwrap()
                .observation
                .failures
                .is_empty()
        );
        campaign(&built);
    }
}
#[test]
fn closure_returned_zero_sized_resources_discharge_every_slot() {
    let source =
        include_str!("../../../../conformance/DROP/DROP-2-closure-zero-sized-destructure-run.hir");
    validate_driver_ready(source);
    let source = source
        .replace("pub fn main(t: Test) -> Result[(), ()]", "fn main() -> u64")
        .replace("Ok(())", "7u64");
    let built = try_build(&source).unwrap();
    assert!(check(&built, &built.ready).is_empty());
    let program = hirsel_heapcheck::Program {
        invocation_bodies: built.operations.invocation_bodies.clone(),
        invocations: built.operations.invocations.clone(),
        pretransfer_adapters: built.operations.pretransfer_adapters.clone(),
        target: target(),
        layouts: built.table,
        units: built.ready,
        statics: vec![],
    };
    for full_recount_every_step in [false, true] {
        let config = Config {
            full_recount_every_step,
            ..Config::default()
        };
        let campaign = hirsel_heapcheck::fault_campaign(
            &program,
            &path("main"),
            &[],
            &mut || Box::new(NoOperations),
            &config,
        );
        assert!(
            campaign.first_violation().is_none(),
            "{:?}",
            campaign.first_violation()
        );
        assert_eq!(
            campaign
                .clean
                .trace
                .iter()
                .filter(|event| matches!(event, hirsel_heapcheck::Event::Finalize { .. }))
                .count(),
            2
        );
    }
}

#[test]
fn resource_variant_cleanup_preserves_own_finalizer_and_active_payload() {
    for (value, failures, finalizers) in [
        ("Held(value: T(n: 1, xs: box P(a: 2)))", 0, 2),
        ("Empty", 0, 1),
        ("Held(value: T(n: 0, xs: box P(a: 2)))", 1, 2),
    ] {
        for parent_fails in [false, true] {
            let drop = if parent_fails {
                "failure.check(false, Arithmetic())"
            } else {
                "()"
            };
            let (b, o) = ready(&format!(
                "{T}resource type Wrapped = Held(value: T) | Empty\ndrop(w) {{ {drop} }}\npub fn main() -> u64 {{ let w: Wrapped = {value}; 7 }}"
            ));
            assert_eq!(o.failures.len(), failures + usize::from(parent_fails));
            assert_eq!(
                finalized(&execute(&b.operations, &b.table, &b.ready).trace).len(),
                finalizers
            );
            same_finalizers(&b);
            campaign(&b);
        }
    }
}

#[test]
fn keyed_detach_cleanup_keeps_each_original_admission_identity() {
    let (b, observation) = ready(
        r#"
type State { values:Map[String,u64] }
fn fill(mut state:State)->u64 {
 map.insert(mut state.values,"one",1u64)
 map.insert(mut state.values,"two",2u64)
 map.len(state.values)
}
pub fn main()->u64 {
 let mut state=State(values:map.empty())
 let original=copy state
 let count=fill(mut state)
 count+map.len(original.values)
}
"#,
    );
    assert!(observation.failures.is_empty());
    assert!(matches!(
        observation.returned,
        Some(hirsel_heapcheck::Observed::Integer { bits: 2, .. })
    ));
    campaign(&b);
}
