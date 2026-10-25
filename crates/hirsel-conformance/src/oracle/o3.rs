//! O3, recompute counts: across a resident edit, the definitions a scenario
//! names `unchanged` recompute nothing, and those it names `recomputed` do (so
//! the scenario exercises what it claims).
//!
//! Counts include physical Lower/body V work and exact memo identities touched.
//! Raw memo operations are reported independently. An independent cold check
//! supplies current read values; missing/conflicting observations fail closed.
use super::{Report, Verdict, edits::Scenario};
use hirsel_driver::session::{FileChange, Session};
use hirsel_engine::{DefinitionQueries, QueryEngine, QueryKind};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

const KINDS: [QueryKind; 5] = [
    QueryKind::Resolve,
    QueryKind::Signature,
    QueryKind::Check,
    QueryKind::Modes,
    QueryKind::Lower,
];

pub fn run(scenarios: &[Scenario]) -> Report {
    let mut report = Report::new("O3");
    for scenario in scenarios {
        if let Err(why) = hirsel_engine::with_compiler_stack(|| one(scenario, &mut report)) {
            report.add(format!("{}: setup", scenario.name), Verdict::Fail(why));
        }
    }
    report
}

fn one(scenario: &Scenario, report: &mut Report) -> Result<(), String> {
    let single = |tree: &std::collections::BTreeMap<String, String>| match tree.get(&scenario.entry)
    {
        Some(text) if tree.len() == 1 && !scenario.with_std => Ok(text.clone()),
        _ => Err("O3 scenarios are one standalone file today".to_string()),
    };
    let (before, after) = (single(&scenario.before)?, single(&scenario.after)?);
    if scenario.config_before != scenario.config_after {
        return Err("O3 scenarios edit source only".into());
    }
    let context = hirsel_engine::default_context();
    let base = std::env::temp_dir().join(format!(
        "hirsel-o3-{}-{}",
        std::process::id(),
        scenario.name
    ));
    std::fs::create_dir_all(&base).map_err(|e| e.to_string())?;
    let entry = base.join(&scenario.entry);
    std::fs::write(&entry, &before).map_err(|e| e.to_string())?;
    let config = || {
        let mut config = scenario
            .config_before
            .compile(Arc::new(hirsel_engine::store::NoneStore));
        // A check lowers only compile-time evaluation.
        config.lowering = hirsel_engine::LoweringPolicy::Comptime;
        config
    };
    let mut session = Session::open_configured(&entry, config());
    session.check().map_err(|e| e.to_string())?;
    let changed = changed_items(&before, &after)?;
    let names: Vec<&String> = scenario
        .unchanged
        .iter()
        .chain(&scenario.recomputed)
        .collect();
    let definitions = names
        .iter()
        .map(|name| {
            let mut path = hirsel_engine::entry_path(&context);
            path.0.name = name.as_str().into();
            path
        })
        .collect::<Vec<_>>();
    let inspect =
        |session: &mut Session| -> Result<Arc<hirsel_driver::requests::CompileInspection>, String> {
            let selected = session
                .request_applied(hirsel_driver::requests::CompileOperation::Inspect {
                    definitions: definitions.clone(),
                })
                .map_err(|error| error.to_string())?;
            match &selected.result {
                hirsel_driver::requests::CompileResult::Inspect(inspection) => {
                    Ok(inspection.clone())
                }
                _ => Err("inspection request returned another operation".into()),
            }
        };
    let counts =
        |inspection: &hirsel_driver::requests::CompileInspection| -> Result<Vec<[u64; 5]>, String> {
            definitions
                .iter()
                .map(|definition| {
                    let work = inspection
                        .definitions
                        .iter()
                        .find(|work| &work.definition == definition)
                        .ok_or_else(|| format!("inspection omitted definition {definition}"))?;
                    let mut result = [0; 5];
                    for (slot, kind) in result.iter_mut().zip(KINDS) {
                        *slot = work
                            .recomputations
                            .iter()
                            .find(|(observed, _)| *observed == kind)
                            .ok_or_else(|| format!("inspection omitted {kind:?} for {definition}"))?
                            .1;
                    }
                    Ok(result)
                })
                .collect()
        };
    let validations = |inspection: &hirsel_driver::requests::CompileInspection| -> Result<Vec<(u64,u64)>, String> {
        definitions.iter().map(|definition| {
            let work = inspection.definitions.iter().find(|work| &work.definition == definition)
                .ok_or_else(|| format!("inspection omitted physical work for {definition}"))?;
            Ok((work.body_validations, work.lower_calls))
        }).collect()
    };
    let initial = inspect(&mut session)?;
    let snapshot = initial.memos.clone();
    let stats = initial.stats.clone();
    let first = counts(&initial)?;
    let first_work = validations(&initial)?;
    std::fs::write(&entry, &after).map_err(|e| e.to_string())?;
    session.apply(vec![FileChange {
        path: entry.clone(),
        bytes: Some(after.into_bytes()),
    }]);
    session.check_applied().map_err(|e| e.to_string())?;
    let touched = session.stats().touched;
    let operations = session.stats().memo_operations;
    let touched_memos = session.touched_memos().to_vec();
    let second_inspection = inspect(&mut session)?;
    let second = counts(&second_inspection)?;
    let second_work = validations(&second_inspection)?;
    for ((((name, first), second), first_work), second_work) in names
        .iter()
        .zip(first)
        .zip(second)
        .zip(first_work)
        .zip(second_work)
    {
        let mut grew: Vec<String> = KINDS
            .iter()
            .zip(first.iter().zip(second))
            .filter(|(_, (a, b))| b > *a)
            .map(|(kind, (a, b))| format!("{} +{}", kind.phase_name(), b - a))
            .collect();
        if second_work.0 > first_work.0 {
            grew.push(format!("body V +{}", second_work.0 - first_work.0));
        }
        if second_work.1 > first_work.1 {
            grew.push(format!("physical lower +{}", second_work.1 - first_work.1));
        }
        let unchanged = scenario.unchanged.contains(name);
        let verdict = match (unchanged, grew.is_empty()) {
            (true, true) | (false, false) => Verdict::Pass,
            (true, false) => Verdict::Fail(format!("recomputed: {}", grew.join(", "))),
            (false, true) => Verdict::Fail("recomputed nothing, so the edit misses it".into()),
        };
        let claim = if unchanged { "unchanged" } else { "recomputed" };
        report.add(format!("{} [{name} {claim}]", scenario.name), verdict);
    }
    let now = &second_inspection.stats;
    let invalidations: u64 = now
        .invalidated
        .iter()
        .zip(stats.invalidated)
        .map(|(a, b)| a - b)
        .sum::<u64>()
        + now.invalidated_text
        - stats.invalidated_text;
    report.add(
        format!("{} [slow-path invalidations]", scenario.name),
        if invalidations > 0 {
            Verdict::Pass
        } else {
            Verdict::Fail("edit invalidated no memo".into())
        },
    );
    let current = &second_inspection.memos;
    // Retained dirty memos may still record the previous revision's values.
    // A fresh check supplies independent current observations, after the
    // resident's physical work and counters have been captured above.
    let mut cold = Session::open_configured(&entry, config());
    cold.check().map_err(|e| format!("reference check: {e}"))?;
    let reference = inspect(&mut cold)?.memos.clone();
    let mut observed = BTreeMap::new();
    for (owner, reads) in &reference {
        for (key, value) in reads.iter() {
            // V's contextual reads are interpreted in their owner's package.
            let scoped = (owner.def.path().0.package.clone(), *key);
            if let Some(previous) = observed.insert(scoped, *value)
                && previous != *value
            {
                return Err(format!(
                    "reference check observed conflicting values for {key:?} in {}",
                    owner.def.path()
                ));
            }
        }
    }
    let mut changed_values = BTreeSet::new();
    let mut missing = BTreeMap::<_, BTreeSet<_>>::new();
    let mut affected = BTreeSet::new();
    for (memo, reads) in &snapshot {
        if changed.contains(&memo.def) {
            affected.insert(*memo);
            continue;
        }
        for (read, previous) in reads.iter() {
            let scoped = (memo.def.path().0.package.clone(), *read);
            match observed.get(&scoped) {
                Some(current) if current != previous => {
                    changed_values.insert(scoped);
                    affected.insert(*memo);
                }
                Some(_) => {}
                None => {
                    missing.entry(scoped).or_default().insert(*memo);
                }
            }
        }
    }
    let newly_created: BTreeSet<_> = current
        .iter()
        .filter(|(key, _)| !snapshot.iter().any(|(old, _)| old == key))
        .map(|(key, _)| *key)
        .collect();
    affected.extend(&newly_created);
    // A changed definition may attempt a query that fails without retaining
    // a memo (for example Lower after a signature edit makes its body invalid).
    // That physical work still belongs to the changed definition's affected
    // set, even though neither retained snapshot can inventory the record.
    affected.extend(
        touched_memos
            .iter()
            .filter(|memo| changed.contains(&memo.def))
            .copied(),
    );
    report.add(
        format!("{} [reference observations]", scenario.name),
        if missing.is_empty() {
            Verdict::Pass
        } else {
            Verdict::Fail(format!(
                "{} missing read keys; {} verified changed values, {} new memo identities; missing inventory is not a changed input: {missing:?}",
                missing.len(), changed_values.len(), newly_created.len()
            ))
        },
    );
    report.add(
        format!("{} [touched]", scenario.name),
        if touched <= affected.len() as u64
            && touched_memos.iter().all(|memo| affected.contains(memo))
        {
            Verdict::Pass
        } else {
            Verdict::Fail(format!(
                "{touched} distinct memo identities ({operations} raw operations) exceed {} changed/subscriber memos; unrelated: {:?}",
                affected.len(), touched_memos.iter().filter(|memo| !affected.contains(memo)).collect::<Vec<_>>()
            ))
        },
    );
    eprintln!(
        "O3 {}: {touched}/{} distinct memos, {operations} raw operations, typed invalidations {:?}, text {}",
        scenario.name,
        affected.len(),
        now.invalidated
            .iter()
            .zip(stats.invalidated)
            .map(|(a, b)| a - b)
            .collect::<Vec<_>>(),
        now.invalidated_text - stats.invalidated_text
    );
    let _ = std::fs::remove_dir_all(&base);
    Ok(())
}

fn changed_items(before: &str, after: &str) -> Result<BTreeSet<hirsel_engine::DefId>, String> {
    let parse = |text: &str| -> Result<BTreeMap<_, _>, String> {
        let mut engine = QueryEngine::new(
            hirsel_engine::default_environment(),
            hirsel_driver::compiler_adapters(),
        );
        let file = engine
            .sources
            .add("o3.hir", text.into())
            .ok_or("source too large")?;
        let parsed = engine
            .parse_file(file, &hirsel_engine::default_context())
            .map_err(|e| format!("parse: {e:?}"))?;
        Ok(parsed
            .value
            .items
            .iter()
            .map(|item| {
                (
                    hirsel_engine::DefId::intern(&item.item.definition),
                    item.text_hash,
                )
            })
            .collect())
    };
    let before = parse(before)?;
    let after = parse(after)?;
    Ok(before
        .keys()
        .chain(after.keys())
        .filter(|id| before.get(id) != after.get(id))
        .copied()
        .collect())
}
