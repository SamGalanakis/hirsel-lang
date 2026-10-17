//! PS-13: the set of removed checks equals the accepted set.
//!
//! A discharged unit is compared with the reference import of the same
//! instance, which left every check in. Each unit's inventory counts its
//! failure sites by TIR anchor `(body, block, statement)`, opcode and rule,
//! re-derived from the instructions and terminators alone; nothing reads the
//! pass's own answer. The discharged unit may lack exactly the planned sites'
//! entries and nothing else: no other failure site may disappear or appear,
//! and no planned entry may remain.
use crate::discharge::{Discharged, Form};
use crate::effect::Opcode;
use crate::ir::{FailureId, Inst, LirUnit, ProjectionStep};
use crate::stage::Value;
use std::collections::{BTreeMap, BTreeSet};

type Entry = ((u32, u32, u32), Opcode, Option<String>, Option<u32>);

fn inventory(unit: &LirUnit<Value>) -> BTreeMap<Entry, usize> {
    let mut out = BTreeMap::new();
    let mut add = |failure: FailureId, opcode: Opcode, rule: Option<String>, place: Option<u32>| {
        // Environmental edges and cleanup plumbing are not source checks.
        // Removing a guard can also remove its otherwise-unused cleanup path,
        // or change where a shared cleanup is first emitted.
        if rule.is_none() {
            return;
        }
        let edge = unit.tables.failures.get(failure.index());
        let anchor = edge.and_then(|e| unit.tables.source_anchors.get(e.anchor.index()));
        let key = anchor.map_or((u32::MAX, u32::MAX, u32::MAX), |a| {
            (a.body, a.block, a.statement)
        });
        *out.entry((key, opcode, rule, place)).or_default() += 1;
    };
    for (b, block) in unit.blocks.iter().enumerate() {
        for inst in block.instructions.get(&unit.instructions).unwrap_or(&[]) {
            if let Inst::Address { address, failure } = inst {
                let info = &unit.addresses[address.index()];
                for (i, step) in info
                    .projection
                    .get(&unit.pools.projections)
                    .unwrap_or(&[])
                    .iter()
                    .enumerate()
                {
                    if info.unchecked.contains(&(i as u32)) {
                        continue;
                    }
                    let rule = match step {
                        ProjectionStep::Index(_) | ProjectionStep::FlatAt { .. } => Some("MEM-14"),
                        ProjectionStep::VariantField { .. } => Some("MEM-13"),
                        _ => None,
                    };
                    if let Some(rule) = rule {
                        add(
                            *failure,
                            Opcode::Address,
                            Some(rule.into()),
                            info.source_place,
                        );
                    }
                }
            } else if let Some(failure) = inst.failure() {
                add(
                    failure,
                    inst.opcode(),
                    unit.tables
                        .failures
                        .get(failure.index())
                        .and_then(|e| e.rule.clone()),
                    None,
                );
            }
        }
        if let Some(terminator) = unit.terminators.get(b)
            && let Some(failure) = terminator.failure()
        {
            add(
                failure,
                terminator.opcode(),
                unit.tables
                    .failures
                    .get(failure.index())
                    .and_then(|e| e.rule.clone()),
                None,
            );
        }
    }
    out
}

fn entry(site: &Discharged) -> Entry {
    let at = (site.body, site.block, site.statement);
    match &site.form {
        Form::Check(rule) => (at, Opcode::Check, Some(rule.clone()), None),
        Form::Integer => (at, Opcode::Binary, Some("SEM-1".into()), None),
        Form::Projection { place, rule } => (at, Opcode::Address, Some(rule.clone()), Some(*place)),
        Form::Negation => (at, Opcode::Unary, Some("SEM-1".into()), None),
    }
}

/// Compare `discharged` with its `reference` import under `plan`. Returns
/// the planned sites this unit removed (a site of another body of the
/// definition is not in it), or every difference.
pub fn validate_discharge(
    reference: &LirUnit<Value>,
    discharged: &LirUnit<Value>,
    plan: &BTreeSet<Discharged>,
) -> Result<BTreeSet<Discharged>, Vec<String>> {
    let before = inventory(reference);
    let after = inventory(discharged);
    let planned: BTreeMap<Entry, &Discharged> = plan.iter().map(|s| (entry(s), s)).collect();
    let mut errors = vec![];
    let mut removed = BTreeSet::new();
    for key in before.keys().chain(after.keys()).collect::<BTreeSet<_>>() {
        let (was, is) = (
            before.get(key).copied().unwrap_or(0),
            after.get(key).copied().unwrap_or(0),
        );
        let ((body, block, statement), opcode, rule, _) = key;
        let at = format!(
            "{opcode:?} {} at body {body} block {block} statement {statement}",
            rule.as_deref().unwrap_or("(no rule)")
        );
        match planned.get(key) {
            Some(site) if is == 0 && was > 0 => {
                removed.insert((*site).clone());
            }
            Some(_) if is > 0 => errors.push(format!("accepted {at} was not removed")),
            Some(_) => {}
            None if is < was => errors.push(format!("{at} was removed without accepted evidence")),
            None if is > was => errors.push(format!("{at} is not in the reference import")),
            None => {}
        }
    }
    // PS-2 also lists origin bindings, loan events and drops that name a
    // projected place without accessing it. They have no guard in either
    // import. An actual access is independently inventoried above.
    removed.extend(
        plan.iter()
            .filter(|s| {
                matches!(s.form, Form::Projection { .. })
                    && !before.contains_key(&entry(s))
                    && !after.contains_key(&entry(s))
            })
            .cloned(),
    );
    if errors.is_empty() {
        Ok(removed)
    } else {
        Err(errors)
    }
}
