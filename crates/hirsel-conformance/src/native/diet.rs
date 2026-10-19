//! Derived native campaign inputs and static cost inventory.
use crate::{Expect, agree, discover, native, parse_header, spec_rules};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[path = "diet_observe.rs"]
mod observation;
pub use observation::worker as observation_worker;

/// The checked-in cover the native campaigns read. Deriving it re-observes
/// the whole corpus through 14 native builds per program, so it is frozen and
/// regenerated on demand (`tools/bless.sh native-selection`), not per run.
pub const FROZEN: &str = "native-tests/regressions/native-selection.json";

/// A program cover. Historical findings are retained independently of gain.

pub struct Selection {
    pub selected: BTreeSet<String>,
    pub protected: BTreeSet<String>,
    considered: BTreeSet<String>,
}

impl Selection {
    /// Buck2 supplies the frozen cover (`HIRSEL_NATIVE_SELECTION`); direct CLI
    /// use reads the checked-in file. Either is completed against `root`.
    pub fn load(root: &Path) -> Result<Self, String> {
        let path = std::env::var_os("HIRSEL_NATIVE_SELECTION")
            .map_or_else(|| root.join(FROZEN), std::path::PathBuf::from);
        let mut selection = Self::read(&path)?;
        selection.complete(root)?;
        Ok(selection)
    }
    /// A frozen cover goes stale as the corpus grows: every current historical
    /// pin and every program the cover never observed is selected too.
    fn complete(&mut self, root: &Path) -> Result<(), String> {
        let pins = protected(root)?;
        self.protected.extend(pins.iter().cloned());
        self.selected.extend(pins);
        let considered = &self.considered;
        let new: Vec<_> = universe(root)?
            .into_iter()
            .filter(|path| !considered.contains(path))
            .collect();
        self.selected.extend(new);
        Ok(())
    }
    pub fn read(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let value: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        if !value["shard"].is_null() {
            return Err("a partial native observation is not a program selection".into());
        }
        if value["version"] != 2 {
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
        let considered = paths("considered")?;
        if !protected.is_subset(&selected) || !selected.is_subset(&considered) {
            return Err("native selection dropped a historical finding".into());
        }
        if selected.is_empty() {
            return Err("native selection is empty".into());
        }
        Ok(Self {
            selected,
            protected,
            considered,
        })
    }
}

/// The checked-in form of a derived cover: what it chose, what it pinned and
/// every program it observed, without the per-program feature table.
pub fn freeze(derived: &serde_json::Value) -> Result<serde_json::Value, String> {
    let considered: BTreeSet<&str> = derived["programs"]
        .as_array()
        .ok_or("derived cover has no programs")?
        .iter()
        .map(|p| p["path"].as_str().ok_or("invalid program path"))
        .collect::<Result<_, _>>()?;
    Ok(
        json!({"version": 2, "selected": derived["selected"], "protected": derived["protected"],
              "considered": considered, "counts": derived["counts"]}),
    )
}

/// Every program a derivation observes: the native baseline, the run cases,
/// the regression fixtures and the historical pins.
fn universe(root: &Path) -> Result<BTreeSet<String>, String> {
    let mut paths: BTreeSet<String> = native::baseline(root)?.into_iter().map(|c| c.id).collect();
    paths.extend(protected(root)?);
    let rules = spec_rules(root)?;
    let mut files = Vec::new();
    discover(&root.join("conformance"), &mut files)?;
    for path in files {
        let relative = path.strip_prefix(root).map_err(|e| e.to_string())?;
        let source = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        if matches!(
            parse_header(relative, &source, &rules)?.expect,
            Expect::Run { .. }
        ) {
            paths.insert(relative.display().to_string());
        }
    }
    Ok(paths)
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

#[derive(Default)]
struct Collected {
    inventory: serde_json::Value,
    total: usize,
    costs: BTreeMap<String, usize>,
    pinned: BTreeSet<String>,
    dictionary: BTreeMap<String, usize>,
    features: BTreeMap<String, BTreeSet<usize>>,
    errors: BTreeMap<String, Vec<String>>,
}

impl Collected {
    fn json(&self) -> serde_json::Value {
        let mut names = vec![String::new(); self.dictionary.len()];
        for (feature, index) in &self.dictionary {
            names[*index] = feature.clone();
        }
        let programs: Vec<_> = self
            .features
            .iter()
            .map(|(path, seen)| {
                json!({
                    "path": path, "features": seen, "errors": self.errors[path],
                    "variants": self.costs.get(path).copied().unwrap_or(0)
                })
            })
            .collect();
        json!({"version": 1, "protected": self.pinned, "features": names,
               "programs": programs, "programs_total": self.total, "inventory": self.inventory})
    }
}

/// Derive the cover in one process (tests and small corpora; Buck2 shards it).
pub fn derive(root: &Path) -> Result<serde_json::Value, String> {
    derive_shard(root, None, None)
}

/// Observe a stable stripe independently; reduction happens only after merge.
pub fn derive_shard(
    root: &Path,
    shard: Option<crate::Shard>,
    inventory_input: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    derive_shard_with(root, shard, inventory_input, observation::isolated)
}

fn derive_shard_with(
    root: &Path,
    shard: Option<crate::Shard>,
    inventory_input: Option<serde_json::Value>,
    observe: impl Fn(&Path) -> Result<observation::Observed, String>,
) -> Result<serde_json::Value, String> {
    let inventory = inventory_input.map_or_else(|| inventory(root), Ok)?;
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
    let total = cases.len();
    cases.retain(|id, _| shard.is_none_or(|s| s.holds(id)));
    let mut dictionary = BTreeMap::<String, usize>::new();
    let mut features = BTreeMap::new();
    let mut errors = BTreeMap::new();
    for (index, (path, case)) in cases.iter().enumerate() {
        eprintln!("native-select: {}/{} {path}", index + 1, cases.len());
        let (seen, failures) = observe(case.path.as_ref().unwrap())?;
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
    let collected = Collected {
        inventory,
        total,
        costs,
        pinned,
        dictionary,
        features,
        errors,
    };
    if let Some(shard) = shard {
        let mut value = collected.json();
        value["shard"] = json!({"index": shard.index, "count": shard.count});
        return Ok(value);
    }
    finish(root, collected)
}

/// Reconcile shard inventories and intern their vocabularies, then choose one
/// global cover. Independently minimized shard unions would not be minimal.
pub fn merge(root: &Path, shards: &[serde_json::Value]) -> Result<serde_json::Value, String> {
    let first = shards.first().ok_or("no native coverage shards")?;
    let count = first["shard"]["count"]
        .as_u64()
        .ok_or("missing coverage shard count")?;
    if count as usize != shards.len() {
        return Err("missing native coverage shards".into());
    }
    let mut c = Collected {
        inventory: first["inventory"].clone(),
        total: first["programs_total"]
            .as_u64()
            .ok_or("missing program count")? as usize,
        ..Default::default()
    };
    for program in c.inventory["programs"]
        .as_array()
        .ok_or("missing inventory")?
    {
        c.costs.insert(
            program["path"].as_str().ok_or("invalid path")?.into(),
            program["variants"]
                .as_u64()
                .ok_or("invalid variant count")? as usize,
        );
    }
    let mut seen = BTreeSet::new();
    for shard in shards {
        let index = shard["shard"]["index"]
            .as_u64()
            .ok_or("missing shard index")?;
        if shard["version"] != 1
            || shard["shard"]["count"] != count
            || index < 1
            || index > count
            || !seen.insert(index)
            || shard["inventory"] != c.inventory
            || shard["programs_total"] != c.total
        {
            return Err("inconsistent native coverage shards".into());
        }
        for pin in shard["protected"].as_array().ok_or("missing pins")? {
            c.pinned.insert(pin.as_str().ok_or("invalid pin")?.into());
        }
        let dictionary = shard["features"].as_array().ok_or("missing vocabulary")?;
        for program in shard["programs"].as_array().ok_or("missing programs")? {
            let path = program["path"].as_str().ok_or("invalid path")?.to_string();
            if !(crate::Shard { index, count }).holds(&path) || c.features.contains_key(&path) {
                return Err("duplicate or misplaced native coverage program".into());
            }
            let mut features = BTreeSet::new();
            for id in program["features"]
                .as_array()
                .ok_or("missing program features")?
            {
                let feature = dictionary
                    .get(id.as_u64().ok_or("invalid feature index")? as usize)
                    .and_then(|f| f.as_str())
                    .ok_or("unknown feature")?;
                let next = c.dictionary.len();
                features.insert(*c.dictionary.entry(feature.into()).or_insert(next));
            }
            let errors = program["errors"]
                .as_array()
                .ok_or("missing stage outcomes")?
                .iter()
                .map(|e| e.as_str().map(str::to_owned).ok_or("invalid stage outcome"))
                .collect::<Result<Vec<_>, _>>()?;
            c.errors.insert(path.clone(), errors);
            c.features.insert(path, features);
        }
    }
    if c.features.len() != c.total {
        return Err("coverage shards omitted a program".into());
    }
    finish(root, c)
}

fn finish(root: &Path, c: Collected) -> Result<serde_json::Value, String> {
    let selected = cover(&c.features, &c.pinned, &c.costs);
    let inventory = &c.inventory;
    let costs = &c.costs;
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
    let generated = native::generate::cases(0, 64);
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
    let mut value = c.json();
    value["selected"] = json!(selected);
    value["counts"] = json!({"baseline_before": baseline.len(), "baseline_after": baseline_selected.len(), "seed": 1,
            "baseline_configurations_before": baseline.len() * matrix.len(), "baseline_configurations_after": slim_attempts,
            "baseline_llvm_before": baseline.len() * matrix.iter().filter(|c| matches!(c.engine, hirsel_driver::native::Engine::Native { .. })).count(), "baseline_llvm_after": slim_llvm,
            "native_full_configurations_after": slim_attempts + generated_attempts + topology_attempts,
            "native_full_llvm_after": slim_llvm + generated_llvm + topology_attempts,
            "agreement_exact_historical_inputs": exact,
            "agreement_run_variants_after": run_variants,
            "agreement_llvm_attempt_upper_bound_after": (run_variants + exact) * 2,
            "agreement_total_eligible_after": after_selection + exact,
            "agreement_variants_before": before_budget, "agreement_after_budget": after_budget, "agreement_after_selection": after_selection});
    Ok(value)
}

/// Replay the exact historical sources once under the exhaustive matrix, then
/// compare the findings retained by every rotating slim plan. The slim view
/// reuses actual exhaustive outcomes; it never invents a synthetic verdict.
pub fn replay_history(
    root: &Path,
    seed: u64,
    filters: &[String],
    shard: Option<crate::Shard>,
) -> Result<serde_json::Value, String> {
    let paths = protected(root)?;
    let historical_programs = paths.len();
    let cases: Vec<_> = paths
        .into_iter()
        .filter(|p| filters.is_empty() || filters.iter().any(|f| p.contains(f)))
        .filter(|p| shard.is_none_or(|s| s.holds(p)))
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
    // Each case names itself as it finishes, so a shard stopped by its pool
    // timeout still shows which sources it completed and how long each took.
    let results = native::run_cases_with(
        &cases,
        &native::scratch("history-replay")?,
        4,
        |case, dir| {
            let started = std::time::Instant::now();
            let result = case.run(&matrix, &toolchain, dir);
            eprintln!(
                "native-select: history {} {:.1}s",
                case.id,
                started.elapsed().as_secs_f64()
            );
            result
        },
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
        json!({"historical_programs": historical_programs, "programs": results.len(), "seeds_per_program": matrix.len(), "configuration_attempts": results.iter().map(|r| r.outcomes.len()).sum::<usize>(), "lost": lost, "unreplayable": unreplayable, "classifications": classifications}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frozen_cover_keeps_its_choice_and_every_observed_program() {
        let derived = json!({"version": 1, "selected": ["a", "b"], "protected": ["b"], "counts": {},
            "programs": [{"path": "a"}, {"path": "b"}, {"path": "c"}]});
        let frozen = freeze(&derived).unwrap();
        let path =
            std::env::temp_dir().join(format!("native-selection-{}.json", std::process::id()));
        std::fs::write(&path, serde_json::to_vec(&frozen).unwrap()).unwrap();
        let selection = Selection::read(&path).unwrap();
        assert_eq!(selection.selected, ["a", "b"].map(String::from).into());
        assert_eq!(
            selection.considered,
            ["a", "b", "c"].map(String::from).into()
        );
        // A cover that selects what it never observed is not a cover of it.
        let mut bad = frozen.clone();
        bad["selected"] = json!(["a", "d"]);
        std::fs::write(&path, serde_json::to_vec(&bad).unwrap()).unwrap();
        assert!(Selection::read(&path).is_err());
        std::fs::remove_file(&path).unwrap();
    }

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
    fn an_agreement_fixture_cannot_replace_a_full_native_witness() {
        let root = std::env::temp_dir().join(format!(
            "hirsel-native-cover-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        for directory in [
            "conformance/LANG",
            "docs/spec",
            "native-tests/regressions",
            "native-tests/topology",
        ] {
            std::fs::create_dir_all(root.join(directory)).unwrap();
        }
        let corpus = "conformance/LANG/LANG-7-witness.hir";
        let fixture = "native-tests/regressions/exact.hir";
        let source = "// rule: LANG-7\n// expect: run\npub fn main(rt: Io, args: Args) -> Result[(), GrantError] { Ok(()) }\n";
        for path in [corpus, fixture] {
            std::fs::write(root.join(path), source).unwrap();
        }
        std::fs::write(root.join("conformance/pending.txt"), "").unwrap();
        std::fs::write(root.join("docs/spec/language.md"), "### LANG-7: Types\n").unwrap();
        for path in [
            "native-tests/regressions/history.tsv",
            "native-tests/regressions/exclusions.tsv",
            "native-tests/regressions/smoke.txt",
            "native-tests/regressions/memcheck.txt",
            "native-tests/topology/cases.txt",
        ] {
            std::fs::write(root.join(path), "").unwrap();
        }
        let value = hirsel_engine::with_compiler_stack(|| {
            derive_shard_with(&root, None, None, |path| Ok(observation::observe(path)))
        })
        .unwrap();
        assert!(
            value["programs"]
                .as_array()
                .unwrap()
                .iter()
                .all(|p| p["errors"].as_array().unwrap().is_empty()),
            "{value:#}"
        );
        let selected: BTreeSet<_> = value["selected"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_str().unwrap())
            .collect();
        assert!(selected.contains(fixture));
        assert!(
            selected.contains(corpus),
            "a fixture runs only in agreement"
        );
        assert!(
            value["features"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f.as_str().unwrap().starts_with("native-full/"))
        );
        let shards: Vec<_> = (1..=2)
            .map(|index| {
                hirsel_engine::with_compiler_stack(|| {
                    derive_shard_with(
                        &root,
                        Some(crate::Shard { index, count: 2 }),
                        Some(value["inventory"].clone()),
                        |path| Ok(observation::observe(path)),
                    )
                })
                .unwrap()
            })
            .collect();
        let merged = merge(&root, &shards).unwrap();
        assert_eq!(merged["selected"], value["selected"]);
        assert_eq!(merged["counts"], value["counts"]);
        assert!(merge(&root, &shards[..1]).is_err());
        std::fs::remove_dir_all(root).unwrap();
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
