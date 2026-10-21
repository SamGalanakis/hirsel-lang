//! Opt-in shadow diagnostics over explicit, immutable old/new graph products.
//!
//! Dependency/result separation and absent-data fallback follow Ekstazi
//! b6ee1f2, DependencyAnalyzer.java:270–315. Unlike dynamic coverage, every
//! dependency class needs producer evidence. No test outcome is stored/reused.
//! Identity and SCCs use Hirsel's existing ProgramRevision and graph helpers.
use hirsel_run_schema::{ProgramNode, ProgramRevision};
use hirsel_syntax::ContentHash;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Classes a producer must establish (including evidence of absence) for every
/// node. A capability summary, source-check Reads or last-run coverage alone
/// cannot establish these. Unknown classes must be left out.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum DependencyClass {
    Calls,
    FunctionValues,
    BodiesAndConstants,
    PrivateRepresentations,
    ImplicitFinalizers,
    SelectedInstances,
    GeneratorInputs,
    CoreAndRuntime,
}
impl DependencyClass {
    pub const ALL: [Self; 8] = [
        Self::Calls,
        Self::FunctionValues,
        Self::BodiesAndConstants,
        Self::PrivateRepresentations,
        Self::ImplicitFinalizers,
        Self::SelectedInstances,
        Self::GeneratorInputs,
        Self::CoreAndRuntime,
    ];
}

/// Required declared execution inputs; values are canonical bytes from the
/// producer's existing encoding. An absent field is unknown, never a default.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Input {
    Arguments,
    FixtureProvider,
    SeedScheduler,
    FuelStackBudgets,
    RuntimeTestProtocol,
    Selection,
    ToolchainBackend,
}
impl Input {
    pub const ALL: [Self; 7] = [
        Self::Arguments,
        Self::FixtureProvider,
        Self::SeedScheduler,
        Self::FuelStackBudgets,
        Self::RuntimeTestProtocol,
        Self::Selection,
        Self::ToolchainBackend,
    ];
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Inputs {
    pub snapshot: ContentHash,
    pub root: String,
    pub values: BTreeMap<Input, Vec<u8>>,
    /// Input decoding/validation failure, including corrupt records.
    pub failure: Option<String>,
    /// Explicit deterministic fixture/environment contract, even for pure code.
    /// Live filesystem/network/time, FFI, or an unmodeled environment is Err.
    pub environment: Result<Vec<u8>, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectedTest {
    /// Stable versioned definition identity, independent of generated entry ids.
    pub id: String,
    pub root: String,
    pub inputs: Inputs,
}

/// Explicit producer evidence, bound to the exact snapshot. An empty/missing
/// record is incomplete. Failures/cancellation belong in `failure`.
#[derive(Clone, Debug)]
pub struct Evidence {
    pub snapshot: ContentHash,
    pub established: BTreeSet<DependencyClass>,
    pub failure: Option<String>,
}

/// This tool does not obtain completeness from QueryEngine::semantic_program_nodes.
/// The caller supplies independently established class evidence. Unsupported
/// production graphs should supply no evidence and receive visible unknowns.
pub struct Snapshot {
    pub identity: ContentHash,
    nodes: BTreeMap<String, ProgramNode>,
    evidence: BTreeMap<String, Evidence>,
    selected: BTreeMap<String, SelectedTest>,
    errors: BTreeSet<String>,
}
impl Snapshot {
    /// Exact owned string/byte payload length, excluding containers, allocator
    /// headers and spare capacity. Reported separately from the dense index.
    pub fn payload_bytes(&self) -> usize {
        let graph: usize = self
            .nodes
            .values()
            .map(|n| {
                n.identity.len()
                    + n.content.iter().map(Vec::len).sum::<usize>()
                    + n.dependencies.iter().map(String::len).sum::<usize>()
            })
            .sum();
        let evidence: usize = self
            .evidence
            .iter()
            .map(|(id, e)| {
                id.len()
                    + 32
                    + e.established.len() * size_of::<DependencyClass>()
                    + e.failure.as_ref().map_or(0, String::len)
            })
            .sum();
        graph
            + evidence
            + self
                .selected
                .values()
                .map(|t| {
                    t.id.len()
                        + t.root.len()
                        + 32
                        + t.inputs.root.len()
                        + t.inputs.values.values().map(Vec::len).sum::<usize>()
                        + t.inputs.failure.as_ref().map_or(0, String::len)
                        + match &t.inputs.environment {
                            Ok(v) => v.len(),
                            Err(e) => e.len(),
                        }
                })
                .sum::<usize>()
    }

    pub fn new(
        identity: ContentHash,
        nodes: impl IntoIterator<Item = ProgramNode>,
        evidence: BTreeMap<String, Evidence>,
        selected: impl IntoIterator<Item = SelectedTest>,
    ) -> Self {
        let mut result = Self {
            identity,
            nodes: BTreeMap::new(),
            evidence,
            selected: BTreeMap::new(),
            errors: BTreeSet::new(),
        };
        for mut node in nodes {
            node.dependencies.sort();
            node.dependencies.dedup();
            if let Some(previous) = result.nodes.insert(node.identity.clone(), node.clone())
                && previous != node
            {
                result
                    .errors
                    .insert(format!("conflicting node {}", node.identity));
            }
        }
        for test in selected {
            if result.selected.insert(test.id.clone(), test).is_some() {
                result.errors.insert("duplicate selected test".into());
            }
        }
        result
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Verdict {
    Affected,
    Unaffected,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Side {
    Old,
    New,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Witness {
    pub side: Side,
    /// Ordered root-to-change path, shortest on this side; ties by changed ID then predecessor ID.
    pub path: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Impact {
    pub test: String,
    pub verdict: Verdict,
    /// A bounded set of witnessed changed IDs, not every transitive change.
    pub changed_dependencies: BTreeSet<String>,
    pub witnesses: Vec<Witness>,
    pub reasons: BTreeSet<String>,
}
impl Impact {
    /// A diagnostic recommendation only. Ordinary execution never consults it.
    pub fn rerun(&self) -> bool {
        self.verdict != Verdict::Unaffected
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexStats {
    pub nodes: usize,
    pub edges: usize,
    pub components: usize,
    /// Dense reverse-edge/SCC/witness index payload; excludes allocator
    /// headers and owned source graph/input bytes. Not total process RSS.
    pub payload_bytes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Report {
    pub old_snapshot: ContentHash,
    pub new_snapshot: ContentHash,
    /// Canonical full graph product identities, using the existing encoder.
    pub old_graph: Option<ContentHash>,
    pub new_graph: Option<ContentHash>,
    pub old_selection: Vec<SelectedTest>,
    pub new_selection: Vec<SelectedTest>,
    pub unknown_dependencies: BTreeSet<String>,
    pub old_index: IndexStats,
    pub new_index: IndexStats,
    pub results: Vec<Impact>,
}

struct Index<'a> {
    names: Vec<&'a String>,
    positions: BTreeMap<&'a str, usize>,
    _reverse: Vec<Vec<usize>>,
    _component: Vec<usize>,
    next_change: Vec<Option<usize>>,
    next_unknown: Vec<Option<usize>>,
    reasons: BTreeMap<usize, String>,
    stats: IndexStats,
}
impl<'a> Index<'a> {
    fn new(snapshot: &'a Snapshot, changed: &BTreeSet<String>) -> Self {
        let names: Vec<_> = snapshot.nodes.keys().collect();
        let positions: BTreeMap<_, _> = names
            .iter()
            .enumerate()
            .map(|(i, n)| (n.as_str(), i))
            .collect();
        let mut edges = vec![vec![]; names.len()];
        let mut reverse = edges.clone();
        let mut reasons = BTreeMap::new();
        for (i, name) in names.iter().enumerate() {
            let mut missing = BTreeSet::new();
            match snapshot.evidence.get(*name) {
                None => {
                    missing.insert("missing closure evidence".to_string());
                }
                Some(evidence) => {
                    if evidence.snapshot != snapshot.identity {
                        missing.insert("stale closure evidence".into());
                    }
                    if let Some(failure) = &evidence.failure {
                        missing.insert(failure.clone());
                    }
                    for class in DependencyClass::ALL {
                        if !evidence.established.contains(&class) {
                            missing.insert(format!("unestablished {class:?}"));
                        }
                    }
                }
            }
            for dependency in &snapshot.nodes[*name].dependencies {
                if let Some(&j) = positions.get(dependency.as_str()) {
                    edges[i].push(j);
                    reverse[j].push(i);
                } else {
                    missing.insert(format!("missing dependency {dependency}"));
                }
            }
            if !missing.is_empty() {
                reasons.insert(
                    i,
                    format!(
                        "{name}: {}",
                        missing.into_iter().collect::<Vec<_>>().join("; ")
                    ),
                );
            }
        }
        let components = hirsel_graph::components(&edges);
        let mut membership = vec![0usize; names.len()];
        for (component, members) in components.iter().enumerate() {
            for &member in members {
                membership[member] = component;
            }
        }
        // Count/store membership during construction rather than a root×node
        // transitive matrix. Reachability is two batched reverse BFS passes.
        let next_change = propagate(
            &reverse,
            changed
                .iter()
                .filter_map(|n| positions.get(n.as_str()).copied()),
        );
        let next_unknown = propagate(&reverse, reasons.keys().copied());
        let edge_count = edges.iter().map(Vec::len).sum::<usize>();
        let stats = IndexStats {
            nodes: names.len(),
            edges: edge_count,
            components: components.len(),
            payload_bytes: (edge_count + membership.len()) * size_of::<usize>()
                + (next_change.len() + next_unknown.len()) * size_of::<Option<usize>>(),
        };
        Self {
            names,
            positions,
            _reverse: reverse,
            _component: membership,
            next_change,
            next_unknown,
            reasons,
            stats,
        }
    }
    fn path(&self, root: &str, next: &[Option<usize>]) -> Vec<String> {
        let Some(&mut_start) = self.positions.get(root) else {
            return vec![];
        };
        let mut i = mut_start;
        if next[i].is_none() {
            return vec![];
        }
        let mut path = vec![self.names[i].clone()];
        while let Some(j) = next[i] {
            if i == j {
                break;
            }
            i = j;
            path.push(self.names[i].clone());
        }
        path
    }
    fn unknown(&self, root: &str) -> Option<String> {
        let path = self.path(root, &self.next_unknown);
        let end = path.last()?;
        Some(format!(
            "{} via {}",
            self.reasons[&self.positions[end.as_str()]],
            path.join(" -> ")
        ))
    }
}

/// Ordered multi-source BFS, ties by source then stable predecessor identity.
/// Each node retains one successor toward its nearest changed/unknown seed.
fn propagate(reverse: &[Vec<usize>], roots: impl IntoIterator<Item = usize>) -> Vec<Option<usize>> {
    let roots: BTreeSet<_> = roots.into_iter().collect();
    let mut next = vec![None; reverse.len()];
    let mut queue = VecDeque::new();
    for root in roots {
        next[root] = Some(root);
        queue.push_back(root);
    }
    while let Some(node) = queue.pop_front() {
        for &user in &reverse[node] {
            if next[user].is_none() {
                next[user] = Some(node);
                queue.push_back(user);
            }
        }
    }
    next
}

fn graph_identity(snapshot: &Snapshot) -> Option<ContentHash> {
    // A tool-local batch root uses the existing closure encoding unchanged.
    // Every graph row is included once; no independent root closure hashing.
    let root = "$hirsel/shadow-impact-batch@1";
    if snapshot.nodes.contains_key(root) {
        return None;
    }
    let batch = ProgramNode {
        identity: root.into(),
        content: vec![],
        dependencies: snapshot.nodes.keys().cloned().collect(),
    };
    ProgramRevision::encode(root, snapshot.nodes.values().cloned().chain([batch]))
        .ok()
        .map(|r| ContentHash(r.digest()))
}

/// Compare both graph directions and all selection/launch inputs. `Unaffected`
/// states equality of complete available closures/declared inputs, never reusable
/// outcome certification. No execution, authority or result-cache contract changes.
pub fn compare(old: &Snapshot, new: &Snapshot) -> Report {
    let changed: BTreeSet<_> = old
        .nodes
        .keys()
        .chain(new.nodes.keys())
        .filter(|name| old.nodes.get(*name) != new.nodes.get(*name))
        .cloned()
        .collect();
    let before = Index::new(old, &changed);
    let after = Index::new(new, &changed);
    let old_graph = graph_identity(old);
    let new_graph = graph_identity(new);
    let ids: BTreeSet<_> = old.selected.keys().chain(new.selected.keys()).collect();
    let mut results = vec![];
    for id in ids {
        let mut result = Impact {
            test: id.clone(),
            verdict: Verdict::Unaffected,
            changed_dependencies: BTreeSet::new(),
            witnesses: vec![],
            reasons: BTreeSet::new(),
        };
        for (side, snapshot, index, graph) in [
            (Side::Old, old, &before, old_graph),
            (Side::New, new, &after, new_graph),
        ] {
            for error in &snapshot.errors {
                result.reasons.insert(format!("{side:?}: {error}"));
            }
            if graph.is_none() {
                result
                    .reasons
                    .insert(format!("{side:?}: invalid graph product"));
            }
            let Some(test) = snapshot.selected.get(id) else {
                result
                    .changed_dependencies
                    .insert(format!("selection:{id}"));
                continue;
            };
            if !snapshot.nodes.contains_key(&test.root) {
                result
                    .reasons
                    .insert(format!("{side:?}: missing root {}", test.root));
            }
            if test.inputs.snapshot != snapshot.identity || test.inputs.root != test.root {
                result
                    .reasons
                    .insert(format!("{side:?}: stale or corrupt input binding"));
            }
            if let Some(reason) = &test.inputs.failure {
                result
                    .reasons
                    .insert(format!("{side:?}: input record: {reason}"));
            }
            for field in Input::ALL {
                if !test.inputs.values.contains_key(&field) {
                    result
                        .reasons
                        .insert(format!("{side:?}: missing input {field:?}"));
                }
            }
            if matches!(&test.inputs.environment, Ok(contract) if contract.is_empty()) {
                result.reasons.insert(format!(
                    "{side:?}: empty deterministic environment contract"
                ));
            }
            if let Err(reason) = &test.inputs.environment {
                result
                    .reasons
                    .insert(format!("{side:?}: environment: {reason}"));
            }
            if let Some(reason) = index.unknown(&test.root) {
                result.reasons.insert(format!("{side:?}: {reason}"));
            }
            let path = index.path(&test.root, &index.next_change);
            if let Some(last) = path.last() {
                result.changed_dependencies.insert(last.clone());
                result.witnesses.push(Witness { side, path });
            }
        }
        if let (Some(a), Some(b)) = (old.selected.get(id), new.selected.get(id)) {
            if a.root != b.root {
                result.changed_dependencies.insert("input:root".into());
            }
            for field in Input::ALL {
                if a.inputs.values.get(&field) != b.inputs.values.get(&field) {
                    result
                        .changed_dependencies
                        .insert(format!("input:{field:?}"));
                }
            }
            if a.inputs.environment != b.inputs.environment {
                result
                    .changed_dependencies
                    .insert("input:environment".into());
            }
        }
        result.verdict = if !result.reasons.is_empty() {
            Verdict::Unknown
        } else if !result.changed_dependencies.is_empty() {
            Verdict::Affected
        } else {
            Verdict::Unaffected
        };
        results.push(result);
    }
    let unknown_dependencies = before
        .reasons
        .values()
        .map(|r| format!("Old: {r}"))
        .chain(after.reasons.values().map(|r| format!("New: {r}")))
        .collect();
    Report {
        unknown_dependencies,
        old_snapshot: old.identity,
        new_snapshot: new.identity,
        old_graph,
        new_graph,
        old_selection: old.selected.values().cloned().collect(),
        new_selection: new.selected.values().cloned().collect(),
        old_index: before.stats,
        new_index: after.stats,
        results,
    }
}

/// One fresh pair and its original dependency prediction.
#[derive(Debug)]
pub struct OraclePair<T> {
    pub test: String,
    pub predicted: Verdict,
    pub old: Result<T, String>,
    pub new: Result<T, String>,
}

/// Fresh shadow oracle: always invokes both sides for every union-selected test,
/// including unknown/affected/deleted/added entries. The oracle reports absence,
/// cancellation and operational failure as Err. Errors never leave Unaffected.
/// Equality of observations is evidence only; compare's dependency/input assertions
/// must also be tested, including edits that keep their outputs equal.
pub fn validate<T: PartialEq>(
    old: &Snapshot,
    new: &Snapshot,
    mut run: impl FnMut(Side, &str) -> Result<T, String>,
) -> (Report, Vec<OraclePair<T>>) {
    let mut report = compare(old, new);
    let mut observations = vec![];
    for impact in &mut report.results {
        let predicted = impact.verdict;
        let before = run(Side::Old, &impact.test);
        let after = run(Side::New, &impact.test);
        for (side, observation) in [(Side::Old, &before), (Side::New, &after)] {
            if let Err(reason) = observation {
                impact.verdict = Verdict::Unknown;
                impact.reasons.insert(format!("{side:?}: oracle: {reason}"));
            }
        }
        if let (Ok(a), Ok(b)) = (&before, &after)
            && a != b
            && impact.verdict == Verdict::Unaffected
        {
            impact.verdict = Verdict::Unknown;
            impact
                .reasons
                .insert("oracle disagreement with unaffected prediction".into());
        }
        observations.push(OraclePair {
            test: impact.test.clone(),
            predicted,
            old: before,
            new: after,
        });
    }
    (report, observations)
}
