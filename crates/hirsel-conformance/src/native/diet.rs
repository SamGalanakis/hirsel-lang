//! Derived native campaign inputs and static cost inventory.
use crate::{Expect, agree, discover, native, parse_header, spec_rules};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// A generated cover. Historical findings are retained independently of gain.
pub struct Selection {
    pub selected: BTreeSet<String>,
    pub protected: BTreeSet<String>,
}

impl Selection {
    /// Buck2 supplies a cached derived artifact. Direct CLI use derives it
    /// into this invocation's scratch root instead of trusting a stale list.
    pub fn load(root: &Path) -> Result<Self, String> {
        if let Some(path) = std::env::var_os("HIRSEL_NATIVE_SELECTION") {
            return Self::read(Path::new(&path));
        }
        let value = hirsel_engine::with_compiler_stack(|| derive(root))?;
        let path = native::scratch("selection")?.join("selection.json");
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).map_err(|e| e.to_string())?;
        Self::read(&path)
    }
    pub fn read(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let value: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        if value["version"] != 1 {
            return Err("unsupported native selection version".into());
        }
        let paths = |key: &str| -> Result<BTreeSet<String>, String> {
            value[key]
                .as_array()
                .ok_or_else(|| format!("native selection has no {key}"))?
                .iter()
                .map(|p| {
                    p.as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| format!("invalid {key} path"))
                })
                .collect()
        };
        let selected = paths("selected")?;
        let protected = paths("protected")?;
        if !protected.is_subset(&selected) {
            return Err("native selection dropped a historical finding".into());
        }
        if selected.is_empty() {
            return Err("native selection is empty".into());
        }
        Ok(Self {
            selected,
            protected,
        })
    }
}

/// Retain all named exclusion cells in addition to feasible pair coverage, so
/// the exact-row ratchet can still discover a stale or changed exclusion.
pub fn configurations(
    matrix: &[hirsel_driver::native::Configuration],
    id: &str,
    seed: u64,
    exhaustive: bool,
    exclusions: &native::Exclusions,
) -> Vec<hirsel_driver::native::Configuration> {
    if exhaustive {
        return matrix.to_vec();
    }
    let mut selected = hirsel_driver::native::pairwise_matrix(matrix, id, seed);
    for config in matrix {
        let mut baseline = config.clone();
        baseline.options.launch = Default::default();
        if exclusions.contains_key(&(id.to_string(), baseline.name()))
            && !selected.iter().any(|s| s.name() == config.name())
        {
            selected.push(config.clone());
        }
    }
    selected
}

/// Deterministic greedy set cover, followed by reverse deletion to a fixed
/// point. Inclusion-minimal after pins; no global-optimality claim is made.
fn cover<T: Ord + Clone>(
    programs: &BTreeMap<String, BTreeSet<T>>,
    pinned: &BTreeSet<String>,
    costs: &BTreeMap<String, usize>,
) -> BTreeSet<String> {
    let mut selected = pinned.clone();
    let mut uncovered: BTreeSet<_> = programs.values().flatten().cloned().collect();
    for path in &selected {
        if let Some(features) = programs.get(path) {
            uncovered.retain(|f| !features.contains(f));
        }
    }
    while !uncovered.is_empty() {
        let next = programs
            .iter()
            .filter(|(p, _)| !selected.contains(*p))
            .max_by_key(|(p, features)| {
                (
                    features.intersection(&uncovered).count(),
                    std::cmp::Reverse(costs.get(*p).copied().unwrap_or(0)),
                    std::cmp::Reverse(*p),
                )
            })
            .expect("the corpus covers its own features");
        uncovered.retain(|f| !next.1.contains(f));
        selected.insert(next.0.clone());
    }
    // Count witnesses once rather than repeatedly rebuilding the union of
    // every other program. Deletion only decreases counts, so a single pass
    // reaches the same fixed point.
    let mut uses = BTreeMap::<&T, usize>::new();
    for path in &selected {
        for feature in programs.get(path).into_iter().flatten() {
            *uses.entry(feature).or_default() += 1;
        }
    }
    for path in selected
        .clone()
        .iter()
        .rev()
        .filter(|p| !pinned.contains(*p))
    {
        if programs[path].iter().all(|f| uses[f] > 1) {
            for feature in &programs[path] {
                *uses.get_mut(feature).unwrap() -= 1;
            }
            selected.remove(path);
        }
    }
    selected
}

fn protected(root: &Path) -> Result<BTreeSet<String>, String> {
    let mut paths = BTreeSet::new();
    let history = std::fs::read_to_string(root.join("native-tests/regressions/history.tsv"))
        .map_err(|e| e.to_string())?;
    for line in history
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let path = line.split('\t').next().unwrap();
        if root.join(path).is_file() {
            paths.insert(path.into());
        }
    }
    for ((path, _), _) in native::read_exclusions(root)? {
        paths.insert(path);
    }
    let mut fixtures = Vec::new();
    discover(&root.join("native-tests/regressions"), &mut fixtures)?;
    for path in fixtures {
        paths.insert(
            path.strip_prefix(root)
                .map_err(|e| e.to_string())?
                .display()
                .to_string(),
        );
    }
    for list in ["smoke.txt", "memcheck.txt"] {
        let text = std::fs::read_to_string(root.join("native-tests/regressions").join(list))
            .map_err(|e| e.to_string())?;
        paths.extend(
            text.lines()
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .map(str::to_owned),
        );
    }
    Ok(paths)
}

/// Build a derived coverage artifact. The Buck2 action key includes the
/// collector binary and every source input; no checked-in selection goes stale.
pub fn derive(root: &Path) -> Result<serde_json::Value, String> {
    let inventory = inventory(root)?;
    let mut cases: BTreeMap<_, _> = native::baseline(root)?
        .into_iter()
        .map(|c| (c.id.clone(), c))
        .collect();
    let baseline_paths: BTreeSet<_> = cases.keys().cloned().collect();
    let mut agreement_paths = BTreeSet::new();
    let mut fixtures = Vec::new();
    discover(&root.join("native-tests/regressions"), &mut fixtures)?;
    for path in fixtures {
        agreement_paths.insert(path.strip_prefix(root).unwrap().display().to_string());
    }
    let mut pinned = protected(root)?;
    let mut costs = BTreeMap::new();
    for program in inventory["programs"].as_array().unwrap() {
        let path = program["path"].as_str().unwrap();
        costs.insert(
            path.to_string(),
            program["variants"].as_u64().unwrap() as usize,
        );
        if program["run"] == true {
            agreement_paths.insert(path.to_string());
        }
        if program["run"] == true && !cases.contains_key(path) {
            cases.insert(
                path.to_string(),
                native::Case {
                    id: path.to_string(),
                    source: std::fs::read_to_string(root.join(path)).map_err(|e| e.to_string())?,
                    path: Some(root.join(path)),
                },
            );
        }
    }
    for path in &pinned {
        if !cases.contains_key(path) {
            cases.insert(
                path.clone(),
                native::Case {
                    id: path.clone(),
                    source: std::fs::read_to_string(root.join(path))
                        .map_err(|e| format!("{path}: {e}"))?,
                    path: Some(root.join(path)),
                },
            );
        }
    }
    let mut dictionary = BTreeMap::<String, usize>::new();
    let mut features = BTreeMap::new();
    let mut errors = BTreeMap::new();
    for (index, (path, case)) in cases.iter().enumerate() {
        eprintln!("native-select: {}/{} {path}", index + 1, cases.len());
        let result = (|| {
            let mut program = hirsel_driver::load_program(case.path.as_ref().unwrap())
                .map_err(|e| e.to_string())?;
            program.check_for_execution().map_err(|e| e.to_string())?;
            let raw = program
                .engine
                .export_native_input()
                .map_err(|e| format!("{e:?}"))?;
            let input = program
                .engine
                .import_native_input(raw)
                .map_err(|e| format!("{e:?}"))?;
            Ok::<_, String>(hirsel_driver::native::backend_features(&input))
        })();
        let (seen, failures) = result.unwrap_or_else(|e| (BTreeSet::new(), vec![e]));
        // A partially observable program is retained, never silently discarded.
        if !failures.is_empty() {
            pinned.insert(path.clone());
        }
        // A feature must have a witness in each campaign that exercised it.
        // Exact regression fixtures run in agreement, while only the passing
        // corpus runs in full native; neither may replace the other's witness.
        let campaigns: Vec<_> = [
            ("native-full", baseline_paths.contains(path)),
            ("agreement-native", agreement_paths.contains(path)),
        ]
        .into_iter()
        .filter_map(|(name, active)| active.then_some(name))
        .collect();
        let indices = seen
            .into_iter()
            .flat_map(|feature| {
                campaigns
                    .iter()
                    .map(move |campaign| format!("{campaign}/{feature}"))
            })
            .map(|feature| {
                let next = dictionary.len();
                *dictionary.entry(feature).or_insert(next)
            })
            .collect::<BTreeSet<_>>();
        features.insert(path.clone(), indices);
        errors.insert(path.clone(), failures);
    }
    let selected = cover(&features, &pinned, &costs);
    let mut names = vec![String::new(); dictionary.len()];
    for (feature, index) in dictionary {
        names[index] = feature;
    }
    let programs: Vec<_> = features.iter().map(|(path, seen)| json!({
        "path": path, "features": seen, "errors": errors[path], "variants": costs.get(path).copied().unwrap_or(0)
    })).collect();
    let matrix = hirsel_driver::native::full_matrix();
    let baseline = native::baseline(root)?;
    let baseline_selected: Vec<_> = baseline
        .iter()
        .filter(|c| selected.contains(&c.id))
        .collect();
    let slim_attempts: usize = baseline_selected
        .iter()
        .map(|c| hirsel_driver::native::pairwise_matrix(&matrix, &c.id, 1).len())
        .sum();
    let slim_llvm: usize = baseline_selected
        .iter()
        .map(|c| {
            hirsel_driver::native::pairwise_matrix(&matrix, &c.id, 1)
                .iter()
                .filter(|c| matches!(c.engine, hirsel_driver::native::Engine::Native { .. }))
                .count()
        })
        .sum();
    let budget = agree::DEFAULT_VARIANTS_PER_PROGRAM;
    let before_budget: usize = costs.values().sum();
    let after_budget: usize = costs.values().map(|n| (*n).min(budget)).sum();
    let after_selection: usize = selected
        .iter()
        .filter_map(|p| costs.get(p))
        .map(|n| (*n).min(budget))
        .sum();
    let generated = native::generate::cases(64);
    let generated_plans: Vec<_> = generated
        .iter()
        .map(|case| hirsel_driver::native::pairwise_matrix(&matrix, &case.id, 1))
        .collect();
    let generated_attempts: usize = generated_plans.iter().map(Vec::len).sum();
    let generated_llvm: usize = generated_plans
        .iter()
        .flatten()
        .filter(|c| matches!(c.engine, hirsel_driver::native::Engine::Native { .. }))
        .count();
    let topology_attempts = inventory["topology_programs"].as_u64().unwrap() as usize * 22;
    let run_variants: usize = inventory["programs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["run"] == true && selected.contains(p["path"].as_str().unwrap()))
        .map(|p| (p["variants"].as_u64().unwrap() as usize).min(budget))
        .sum();
    let mut fixtures = Vec::new();
    discover(&root.join("native-tests/regressions"), &mut fixtures)?;
    let exact = fixtures.len();
    Ok(
        json!({ "version": 1, "selected": selected, "protected": pinned, "features": names, "programs": programs,
        "counts": {"baseline_before": baseline.len(), "baseline_after": baseline_selected.len(), "seed": 1,
            "baseline_configurations_before": baseline.len() * matrix.len(), "baseline_configurations_after": slim_attempts,
            "baseline_llvm_before": baseline.len() * matrix.iter().filter(|c| matches!(c.engine, hirsel_driver::native::Engine::Native { .. })).count(), "baseline_llvm_after": slim_llvm,
            "native_full_configurations_after": slim_attempts + generated_attempts + topology_attempts,
            "native_full_llvm_after": slim_llvm + generated_llvm + topology_attempts,
            "agreement_exact_historical_inputs": exact,
            "agreement_run_variants_after": run_variants,
            "agreement_llvm_attempt_upper_bound_after": (run_variants + exact) * 2,
            "agreement_total_eligible_after": after_selection + exact,
            "agreement_variants_before": before_budget, "agreement_after_budget": after_budget, "agreement_after_selection": after_selection},
        "inventory": inventory }),
    )
}

/// Replay the exact historical sources once under the exhaustive matrix, then
/// compare the findings retained by every rotating slim plan. The slim view
/// reuses actual exhaustive outcomes; it never invents a synthetic verdict.
pub fn replay_history(
    root: &Path,
    seed: u64,
    filters: &[String],
) -> Result<serde_json::Value, String> {
    let paths = protected(root)?;
    let cases: Vec<_> = paths
        .into_iter()
        .filter(|p| filters.is_empty() || filters.iter().any(|f| p.contains(f)))
        .map(|id| {
            Ok(native::Case {
                source: std::fs::read_to_string(root.join(&id))
                    .map_err(|e| format!("{id}: {e}"))?,
                path: Some(root.join(&id)),
                id,
            })
        })
        .collect::<Result<_, String>>()?;
    if cases.is_empty() {
        return Err("history replay selected no programs".into());
    }
    let matrix = hirsel_driver::native::full_matrix();
    let toolchain =
        hirsel_driver::native::Toolchain::for_matrix(&native::runtime_cache()?, &matrix)
            .map_err(|e| e.to_string())?;
    let results = native::run_cases(
        &cases,
        &matrix,
        &toolchain,
        &native::scratch("history-replay")?,
        4,
    );
    let exclusions = native::read_exclusions(root)?;
    let findings = |result: &native::CaseResult| -> BTreeSet<_> {
        [native::Class::Heap, native::Class::Llvm]
            .into_iter()
            .flat_map(|class| {
                result
                    .exclusions(&[class])
                    .into_iter()
                    .map(move |(_, finding)| (class, finding))
            })
            .collect()
    };
    let mut lost = Vec::new();
    let mut classifications = Vec::new();
    let mut unreplayable = Vec::new();
    for result in &results {
        if result.snapshot.is_some() {
            unreplayable.push(json!({"path": result.id, "reason": result.snapshot}));
            continue;
        }
        let full = findings(result);
        // Every historical source remains pinned. Here we test the only other
        // reduction, the legal configuration cover, for a complete seed cycle.
        for offset in 0..matrix.len() as u64 {
            let rotating_seed = seed.wrapping_add(offset);
            let names: BTreeSet<_> =
                configurations(&matrix, &result.id, rotating_seed, false, &exclusions)
                    .iter()
                    .map(|c| c.name())
                    .collect();
            let mut slim = result.clone();
            slim.outcomes.retain(|(name, _, _)| names.contains(name));
            if findings(&slim) != full {
                lost.push(json!({"path": result.id, "seed": rotating_seed}));
            }
        }
        classifications.push(
            json!({"path": result.id, "snapshot": result.snapshot, "finding_kinds": full.len()}),
        );
    }
    Ok(
        json!({"programs": results.len(), "seeds_per_program": matrix.len(), "configuration_attempts": results.iter().map(|r| r.outcomes.len()).sum::<usize>(), "lost": lost, "unreplayable": unreplayable, "classifications": classifications}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cover_preserves_the_union_pins_and_has_no_redundant_unpinned_program() {
        let programs: BTreeMap<_, BTreeSet<_>> = [
            ("historical", vec!["a"]),
            ("cheap", vec!["b", "c"]),
            ("expensive", vec!["b", "c"]),
            ("narrow", vec!["b"]),
            ("unique", vec!["d"]),
        ]
        .into_iter()
        .map(|(p, f)| (p.to_string(), f.into_iter().map(str::to_owned).collect()))
        .collect();
        let pinned = BTreeSet::from(["historical".to_string()]);
        let costs = BTreeMap::from([
            ("cheap".to_string(), 10),
            ("expensive".to_string(), 4_000_000),
        ]);
        let selected = cover(&programs, &pinned, &costs);
        assert_eq!(
            selected,
            BTreeSet::from(["historical".into(), "cheap".into(), "unique".into()])
        );
        let union: BTreeSet<_> = programs.values().flatten().collect();
        let observed: BTreeSet<_> = selected.iter().flat_map(|p| &programs[p]).collect();
        assert_eq!(observed, union);
        for removed in selected.difference(&pinned) {
            let rest: BTreeSet<_> = selected
                .iter()
                .filter(|p| *p != removed)
                .flat_map(|p| &programs[p])
                .collect();
            assert_ne!(rest, union);
        }
    }

    #[test]
    fn excluded_cells_stay_in_a_cover_under_nonzero_launch_inputs() {
        let mut matrix = hirsel_driver::native::full_matrix();
        for c in &mut matrix {
            c.options.launch.hash_seed = 7;
        }
        let selected = configurations(&matrix, "program", 1, false, &native::Exclusions::new());
        let missing = matrix
            .iter()
            .find(|c| !selected.iter().any(|s| s.name() == c.name()))
            .unwrap();
        let mut baseline = missing.clone();
        baseline.options.launch = Default::default();
        let excluded = native::Exclusions::from([(
            ("program".to_string(), baseline.name()),
            native::Exclusion {
                class: "known-bug".into(),
                reason: "historical".into(),
            },
        )]);
        let with_pin = configurations(&matrix, "program", 1, false, &excluded);
        assert!(with_pin.iter().any(|c| c.name() == missing.name()));
        assert!(with_pin.iter().all(|c| c.options.launch.hash_seed == 7));
        assert_eq!(
            configurations(&matrix, "program", 1, true, &excluded).len(),
            matrix.len()
        );
    }
}

pub fn inventory(root: &Path) -> Result<serde_json::Value, String> {
    let mut paths = Vec::new();
    discover(&root.join("conformance"), &mut paths)?;
    paths.sort();
    let rules = spec_rules(root)?;
    let mut programs = Vec::new();
    for path in paths {
        let relative = path.strip_prefix(root).map_err(|e| e.to_string())?;
        let source = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let header = parse_header(relative, &source, &rules)?;
        if matches!(header.expect, Expect::Reject { .. }) {
            continue;
        }
        let Ok(variants) = agree::catalogue_count(&source) else {
            continue;
        };
        programs.push(json!({"path": relative, "variants": variants, "run": matches!(header.expect, Expect::Run { .. })}));
    }
    let matrix = hirsel_driver::native::full_matrix();
    let native_rows = matrix
        .iter()
        .filter(|c| matches!(c.engine, hirsel_driver::native::Engine::Native { .. }))
        .count();
    let baseline = native::baseline(root)?.len();
    let topology = std::fs::read_to_string(root.join("native-tests/topology/cases.txt"))
        .map_err(|e| e.to_string())?
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .count();
    let variants: usize = programs
        .iter()
        .map(|p| p["variants"].as_u64().unwrap() as usize)
        .sum();
    let run_variants: usize = programs
        .iter()
        .filter(|p| p["run"] == true)
        .map(|p| p["variants"].as_u64().unwrap() as usize)
        .sum();
    Ok(json!({
        "baseline_programs": baseline,
        "generated_programs": 128,
        "topology_programs": topology,
        "matrix": matrix.iter().map(|c| c.name()).collect::<Vec<_>>(),
        "native_full_configuration_attempts": (baseline + 128) * matrix.len() + topology * 22,
        "native_full_llvm_attempts": (baseline + 128) * native_rows + topology * 22,
        "agreement_programs": programs.len(),
        "agreement_variants": variants,
        "agreement_run_variants": run_variants,
        "agreement_llvm_attempt_upper_bound": run_variants * 2,
        "programs": programs,
    }))
}
