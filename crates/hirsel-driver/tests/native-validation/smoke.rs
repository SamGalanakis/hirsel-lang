//! Pass-fault differential over the shipped native smoke selection. Control units
//! must be accepted; each surviving fault the stage ladder rejects must also be
//! rejected at Ready. A normalized-away fault must yield the identical final unit after packing live operand ranges.
use super::*;
use hirsel_lir::ir::{Inst, LirUnit, ReadMode};
use hirsel_lir::stage::{Stage, StageKind};
use hirsel_lir::validate::{Inputs, ValidationContext};
use hirsel_rc::cleanup::Finalizers;
use hirsel_rc::insert::{Editor, Proofs};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum Fault {
    MissingParameter,
    MissingIncrement,
    MissingDecrement,
}

fn mutants<S: Stage>(
    unit: &LirUnit<S>,
    inputs: &Inputs<'_>,
    context: &ValidationContext<'_>,
) -> Vec<(Fault, LirUnit<S>)> {
    let mut out = Vec::new();
    if !unit.parameters.is_empty() {
        let mut u = unit.clone();
        u.parameters.pop();
        out.push((Fault::MissingParameter, u));
    }
    let mut seen = std::collections::BTreeSet::new();
    let ed = Editor::new(unit.clone());
    for (b, code) in ed.code.iter().enumerate() {
        for (i, inst) in code.iter().enumerate() {
            let fault = match inst {
                Inst::Dup { .. } | Inst::Copy { .. } => Fault::MissingIncrement,
                Inst::Release { .. } => Fault::MissingDecrement,
                _ => continue,
            };
            if seen.contains(&fault) {
                continue;
            }
            let mut ed = Editor::new(unit.clone());
            ed.code[b][i] = match *inst {
                Inst::Dup { value } | Inst::Release { value, .. } => Inst::Share { value },
                Inst::Copy {
                    result,
                    address,
                    reaching,
                    ..
                } => Inst::Read {
                    result,
                    address,
                    reaching,
                    mode: ReadMode::Take,
                },
                _ => unreachable!(),
            };
            let mutant = ed.finish();
            // The first increment may be a static/immortal no-op. Select
            // the first mutation whose lost credit the stage ladder catches.
            if !context.check(&mutant, inputs).is_empty() {
                seen.insert(fault);
                out.push((fault, mutant));
            }
        }
    }
    out
}

// Editor::finish prunes dead cleanup blocks but intentionally retains the
// append-only values pool. Compare every live operand exactly, independent of
// abandoned pool entries and their offsets. These are the only ValueId ranges
// in the IR (Block.parameters, Edge.arguments and FailureEdge.dispose).
fn pack_values<S: Stage>(mut unit: LirUnit<S>) -> LirUnit<S> {
    use hirsel_lir::ir::PoolRange;
    let old = std::mem::take(&mut unit.pools.values);
    let mut pack = |range: &mut PoolRange<hirsel_lir::ir::ValueId>| {
        *range = PoolRange::push(
            &mut unit.pools.values,
            range
                .get(&old)
                .expect("validated operand range")
                .iter()
                .copied(),
        );
    };
    for block in &mut unit.blocks {
        pack(&mut block.parameters);
    }
    for edge in &mut unit.tables.edges {
        pack(&mut edge.arguments);
    }
    for failure in &mut unit.tables.failures {
        pack(&mut failure.dispose);
    }
    unit
}

fn judge<S: Stage>(
    unit: &LirUnit<S>,
    inputs: &Inputs<'_>,
    context: &ValidationContext<'_>,
    finish: impl Fn(LirUnit<S>) -> Result<LirUnit<hirsel_lir::stage::Ready>, hirsel_rc::PassError>,
    counts: &mut BTreeMap<(StageKind, Fault), [usize; 3]>,
) {
    let control = context.check(unit, inputs);
    assert!(
        control.is_empty(),
        "{:?} {} control: {control:?}",
        S::KIND,
        unit.header.definition
    );
    for (fault, mutant) in mutants(unit, inputs, context) {
        let ladder = context.check(&mutant, inputs);
        if ladder.is_empty() {
            continue;
        }
        let count = counts.entry((S::KIND, fault)).or_default();
        match finish(mutant) {
            Err(_) => count[0] += 1,
            Ok(ready) => {
                let errors = context.check(&ready, inputs);
                if errors.is_empty() {
                    // Cleanup can erase an intermediate defect with its entire
                    // path. Acceptance is allowed only when the full final unit
                    // has identical live operands and all other fields to the clean
                    // control, not merely valid or behaviorally equivalent.
                    let clean = finish(unit.clone()).expect("the clean continuation");
                    assert!(
                        pack_values(clean) == pack_values(ready),
                        "Ready accepted surviving {}/{fault:?} fault: {ladder:?}",
                        unit.header.definition
                    );
                    count[2] += 1;
                    eprintln!(
                        "normalized {:?}/{fault:?} in {} to identical live Ready fields",
                        S::KIND,
                        unit.header.definition
                    );
                } else {
                    count[1] += 1;
                }
            }
        }
    }
}

#[test]
fn ready_only_rejects_smoke_pass_faults() {
    hirsel_engine::with_compiler_stack(|| {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let list =
            std::fs::read_to_string(root.join("native-tests/regressions/smoke.txt")).unwrap();
        let mut counts = BTreeMap::new();
        let mut programs = 0;
        for path in list
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty() && !s.starts_with('#'))
        {
            eprintln!("fault campaign: {path}");
            let mut loaded = crate::load_program(&root.join(path)).unwrap();
            loaded.check_for_execution().unwrap();
            let input = loaded.engine.export_native_input().unwrap();
            let validated = loaded.engine.import_native_input(input).unwrap();
            let program = super::program(&validated);
            let target = TargetSpec::x86_64_unknown_linux_gnu();
            let lowered = hirsel_rc::import::lower(&program, &target).unwrap();
            let manifest = super::manifest(validated.snapshot()).unwrap();
            let operations = program.operation_registry();
            let table = &lowered.table;
            let inputs = Inputs {
                target: &target,
                layouts: table,
                manifest: &manifest,
                operations: &operations,
                discharge: None,
            };
            let context = ValidationContext::new(&inputs);
            let finalizers = Finalizers::planned(&lowered.instances, table);
            for (i, value) in lowered.units.into_iter().enumerate() {
                let id = lowered.unit_origins[i];
                let placement =
                    hirsel_rc::placement::Options::for_instance(true, &lowered.instances, id);
                let proofs = Proofs::for_instance(&lowered.instances, id);
                let reuse = hirsel_rc::reuse::Options::default();
                let to_ready = |rc| {
                    hirsel_rc::ready::ready(
                        hirsel_rc::reuse::reuse(rc, table, &reuse)?,
                        table,
                        &finalizers,
                    )
                };
                let to_rc = |placed| {
                    hirsel_rc::elide::elide(
                        hirsel_rc::insert::insert(placed, table, &proofs)?,
                        table,
                    )
                };
                judge(
                    &value,
                    &inputs,
                    &context,
                    |v| {
                        to_ready(to_rc(
                            hirsel_rc::placement::place(v, table, &placement)
                                .map_err(|e| hirsel_rc::PassError::Malformed(e.to_string()))?,
                        )?)
                    },
                    &mut counts,
                );
                let placed = hirsel_rc::placement::place(value, table, &placement).unwrap();
                judge(
                    &placed,
                    &inputs,
                    &context,
                    |p| to_ready(to_rc(p)?),
                    &mut counts,
                );
                let rc = to_rc(placed).unwrap();
                judge(&rc, &inputs, &context, to_ready, &mut counts);
                let reused = hirsel_rc::reuse::reuse(rc, table, &reuse).unwrap();
                judge(
                    &reused,
                    &inputs,
                    &context,
                    |u| hirsel_rc::ready::ready(u, table, &finalizers),
                    &mut counts,
                );
                let ready = hirsel_rc::ready::ready(reused, table, &finalizers).unwrap();
                judge(&ready, &inputs, &context, Ok, &mut counts);
            }
            programs += 1;
        }
        assert!(
            programs > 100,
            "smoke selection unexpectedly shrank: {programs}"
        );
        for stage in StageKind::ALL {
            assert!(
                counts.keys().any(|(s, _)| *s == stage),
                "no faults at {stage:?}"
            );
        }
        for fault in [
            Fault::MissingParameter,
            Fault::MissingIncrement,
            Fault::MissingDecrement,
        ] {
            assert!(
                counts.keys().any(|(_, f)| *f == fault),
                "no {fault:?} controls"
            );
        }
        eprintln!(
            "{programs} smoke programs; mutants [pass rejected, Ready rejected, identical live-unit normalization]: {counts:?}"
        );
    });
}
