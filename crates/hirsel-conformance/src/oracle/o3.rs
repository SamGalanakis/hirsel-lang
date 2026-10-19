//! O3, recompute counts: across a resident edit, the definitions a scenario
//! names `unchanged` recompute nothing, and those it names `recomputed` do (so
//! the scenario exercises what it claims).
//!
//! Today the counts are the engine's per-definition, per-query recompute counts
//! (resolve, signature, check, modes, lower) over one resident single-file
//! engine. V's recomputes, the invalidation reasons by `ReadKey` kind and
//! `SessionStats.touched` are TODO until arch-engine provides them.
use super::{Report, Switch, Verdict, edits::Scenario};
use hirsel_engine::{QueryEngine, QueryKind};

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
    let mut engine = QueryEngine::new(
        hirsel_engine::default_environment(),
        hirsel_driver::compiler_adapters(),
    );
    let check = |engine: &mut QueryEngine, text: String| {
        let file = engine
            .sources
            .add(&scenario.entry, text)
            .ok_or("source too large")?;
        hirsel_engine::check_progress(engine, file, &context).map_err(|e| format!("check: {e:?}"))
    };
    check(&mut engine, before)?;
    let names: Vec<&String> = scenario
        .unchanged
        .iter()
        .chain(&scenario.recomputed)
        .collect();
    let counts = |engine: &QueryEngine| -> Vec<[usize; 5]> {
        names
            .iter()
            .map(|name| {
                let mut path = hirsel_engine::entry_path(&context);
                path.0.name = name.as_str().into();
                KINDS.map(|kind| engine.recompute_count(&path, kind))
            })
            .collect()
    };
    let first = counts(&engine);
    check(&mut engine, after)?;
    let second = counts(&engine);
    for ((name, first), second) in names.iter().zip(first).zip(second) {
        let grew: Vec<String> = KINDS
            .iter()
            .zip(first.iter().zip(second))
            .filter(|(_, (a, b))| b > *a)
            .map(|(kind, (a, b))| format!("{} +{}", kind.phase_name(), b - a))
            .collect();
        let unchanged = scenario.unchanged.contains(name);
        let verdict = match (unchanged, grew.is_empty()) {
            (true, true) | (false, false) => Verdict::Pass,
            (true, false) => Verdict::Fail(format!("recomputed: {}", grew.join(", "))),
            (false, true) => Verdict::Fail("recomputed nothing, so the edit misses it".into()),
        };
        let claim = if unchanged { "unchanged" } else { "recomputed" };
        report.add(format!("{} [{name} {claim}]", scenario.name), verdict);
    }
    // V's recomputes and the reasons by ReadKey kind, then the touch bound
    // (at most changed + subscribers memos).
    report.add(
        format!("{} [validation and read kinds]", scenario.name),
        Verdict::Todo(Switch::SlowPathCounters),
    );
    report.add(
        format!("{} [touched]", scenario.name),
        Verdict::Todo(Switch::Touched),
    );
    Ok(())
}
