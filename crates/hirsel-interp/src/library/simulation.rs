//! Explicit deterministic capability implementations; no host I/O or entropy.
use crate::{Error, OperationContext, TraceEvent, Value, invalid};
use hirsel_run_config::Access;
use hirsel_syntax::{DefPath, ItemPath, PackageId};
use hirsel_tir::{CapabilityEvent, RunConfiguration};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub fn core_path(name: &str) -> DefPath {
    let mut parts: Vec<_> = name.split('.').map(str::to_owned).collect();
    let name = parts.pop().unwrap();
    DefPath(ItemPath {
        package: PackageId {
            name: "core".into(),
            version: "1".into(),
        },
        modules: parts,
        name,
    })
}
/// Core types declared Plain whose interpreter value keeps its evidence as a
/// string: a `Path` its text, a `StrPos` its source. That string is the
/// interpreter's own check, not program storage, so the value is never
/// local (MODE-1: plain values hold identity loans only).
pub(crate) fn plain_evidence(definition: &DefPath) -> bool {
    let path = &definition.0;
    path.package.name == "core"
        && path.package.version == "1"
        && path.modules.is_empty()
        && matches!(path.name.as_str(), "Path" | "StrPos")
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Fault {
    Canceled(u32),
    Fault { code: u32, partial: usize },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SimulationConfig {
    /// Explicit script provider and exact rules; peer data never widens authority.
    pub net: Option<hirsel_run_config::NetGrant>,
    pub random_seed: u64,
    pub test_seed: u64,
    pub grants: BTreeSet<String>,
    /// Configured Io projections, independent of the root Io grant.
    pub projections: BTreeSet<String>,
    /// Virtual selectors and inherited operation rights; independent of host designators.
    pub mounts: BTreeMap<String, Access>,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub files: BTreeMap<String, Vec<u8>>,
    pub directories: BTreeSet<String>,
    pub links: BTreeMap<String, String>,
    pub monotonic: u64,
    pub wall: i64,
    /// Codes for invalid/confinement and absent paths are capability inputs.
    pub invalid_path_code: u32,
    pub missing_path_code: u32,
    /// Fault queues are consumed by operation name, in order, never host errno.
    pub faults: BTreeMap<String, VecDeque<Fault>>,
}
impl Default for SimulationConfig {
    fn default() -> Self {
        Self {
            net: None,
            random_seed: 0,
            test_seed: 0,
            grants: ["Io", "Args", "Env"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            mounts: BTreeMap::from([(String::new(), Access::ReadWrite)]),
            projections: ["stdout", "tasks", "net", "random", "dir"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            args: vec![],
            env: BTreeMap::new(),
            files: BTreeMap::new(),
            directories: [String::new()].into_iter().collect(),
            links: BTreeMap::new(),
            monotonic: 0,
            wall: 0,
            invalid_path_code: 1,
            missing_path_code: 2,
            faults: BTreeMap::new(),
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Grant {
    Net {
        world: u64,
        rules: Vec<hirsel_run_config::NetRule>,
    },
    Stream {
        net: u64,
    },
    Root(String),
    Dir {
        tree: u64,
        root: String,
        access: Access,
    },
    File {
        tree: u64,
        path: String,
        dir: u64,
        identity: u64,
    },
    Atomic {
        tree: u64,
        temp: String,
        dst: String,
        dir: u64,
        identity: u64,
    },
}
#[derive(Clone, Debug)]
pub struct Simulation {
    pub(super) net_worlds: BTreeMap<u64, super::net::World>,
    pub(super) net_streams: BTreeMap<u64, super::net::ScriptStream>,
    pub config: SimulationConfig,
    pub events: Vec<CapabilityEvent>,
    pub(crate) grants: BTreeMap<u64, Grant>,
    pub(crate) trees: BTreeMap<u64, (BTreeSet<String>, BTreeMap<String, Vec<u8>>)>,
    pub(crate) resolved_directories: BTreeMap<u64, String>,
    file_identities: BTreeMap<(u64, String), u64>,
    next_file_identity: u64,
    pub(crate) next_id: u64,
    state: u64,
    replay: Vec<CapabilityEvent>,
    replay_index: usize,
    pub(crate) last_monotonic: Option<u64>,
}
/// Final fake filesystem and surviving logical grants, including independent trees.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedSimulation {
    pub trees: BTreeMap<u64, (BTreeSet<String>, BTreeMap<String, Vec<u8>>)>,
    pub handles: BTreeMap<u64, ObservedGrant>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ObservedGrant {
    Net {
        world: u64,
        rules: Vec<hirsel_run_config::NetRule>,
    },
    Stream {
        net: u64,
    },
    Root(String),
    Directory {
        tree: u64,
        root: String,
        access: Access,
    },
    File {
        tree: u64,
        path: String,
        directory: u64,
    },
    AtomicFile {
        tree: u64,
        temporary: String,
        destination: String,
        directory: u64,
    },
}
impl Simulation {
    pub fn observed_state(&self) -> ObservedSimulation {
        let handles = self
            .grants
            .iter()
            .map(|(id, grant)| {
                (
                    *id,
                    match grant {
                        Grant::Net { world, rules } => ObservedGrant::Net {
                            world: *world,
                            rules: rules.clone(),
                        },
                        Grant::Stream { net } => ObservedGrant::Stream { net: *net },
                        Grant::Root(kind) => ObservedGrant::Root(kind.clone()),
                        Grant::Dir { tree, root, access } => ObservedGrant::Directory {
                            tree: *tree,
                            root: root.clone(),
                            access: *access,
                        },
                        Grant::File {
                            tree, path, dir, ..
                        } => ObservedGrant::File {
                            tree: *tree,
                            path: path.clone(),
                            directory: *dir,
                        },
                        Grant::Atomic {
                            tree,
                            temp,
                            dst,
                            dir,
                            ..
                        } => ObservedGrant::AtomicFile {
                            tree: *tree,
                            temporary: temp.clone(),
                            destination: dst.clone(),
                            directory: *dir,
                        },
                    },
                )
            })
            .collect();
        ObservedSimulation {
            trees: self.trees.clone(),
            handles,
        }
    }
    pub fn new(run: &RunConfiguration, mut config: SimulationConfig) -> Result<Self, Error> {
        // Validate snapshot entry names before granting anything. Link targets
        // remain data and are checked at the confined operation boundary.
        let mut files = BTreeMap::new();
        for (path, data) in std::mem::take(&mut config.files) {
            let path =
                relative("", &path).ok_or_else(|| invalid("simulation file escapes root"))?;
            if path.is_empty() {
                return Err(invalid("file at directory root"));
            }
            files.insert(path, data);
        }
        let mut dirs = BTreeSet::from([String::new()]);
        for d in &config.directories {
            dirs.insert(
                relative("", d).ok_or_else(|| invalid("simulation directory escapes root"))?,
            );
        }
        // Acquiring a mount establishes its root even when it has no entries.
        // Preserve root aliases that identify shared snapshot storage.
        for selector in config.mounts.keys() {
            if !config.links.contains_key(selector) {
                dirs.insert(selector.clone());
            }
            parents(selector, &mut dirs);
        }
        for path in config.links.keys() {
            if path.is_empty() || relative("", path).as_ref() != Some(path) {
                return Err(invalid("invalid simulation link name"));
            }
            parents(path, &mut dirs);
        }
        for path in files.keys() {
            parents(path, &mut dirs);
        }
        if files.keys().any(|p| dirs.contains(p)) {
            return Err(invalid("file/directory collision"));
        }
        config.files = files.clone();
        config.directories = dirs.clone();
        if config
            .links
            .keys()
            .any(|p| dirs.contains(p) || files.contains_key(p))
        {
            return Err(invalid("simulation link collision"));
        }
        let random_seed = config.random_seed;
        let net_worlds = super::net::configured_world(config.net.as_ref())?;
        let mut simulation = Self {
            net_worlds,
            net_streams: BTreeMap::new(),
            config,
            events: vec![],
            grants: BTreeMap::new(),
            trees: BTreeMap::from([(0, (dirs, files))]),
            resolved_directories: BTreeMap::new(),
            file_identities: BTreeMap::new(),
            next_file_identity: 1,
            next_id: 1,
            state: random_seed,
            replay: run.inputs.clone(),
            replay_index: 0,
            last_monotonic: None,
        };
        simulation.identify_tree(0);
        Ok(simulation)
    }
    pub(crate) fn identify_tree(&mut self, tree: u64) {
        let paths: Vec<_> = self.trees[&tree].1.keys().cloned().collect();
        for path in paths {
            self.replace_identity(tree, &path);
        }
    }
    pub(crate) fn file_identity(&self, tree: u64, path: &str) -> Option<u64> {
        self.file_identities.get(&(tree, path.into())).copied()
    }
    /// Publication replaces the entry even when every byte is unchanged. Never
    /// recycle identities: unlink/recreate must not resurrect an opened file.
    pub(crate) fn replace_identity(&mut self, tree: u64, path: &str) {
        let identity = self.next_file_identity;
        self.next_file_identity = identity.checked_add(1).expect("file identities exhausted");
        self.file_identities.insert((tree, path.into()), identity);
    }
    pub(crate) fn unlink_identity(&mut self, tree: u64, path: &str) {
        self.file_identities.remove(&(tree, path.into()));
    }
    pub(crate) fn publish_identity(&mut self, tree: u64, from: &str, to: &str) {
        let identity = self
            .file_identities
            .remove(&(tree, from.into()))
            .expect("atomic identity");
        self.file_identities.insert((tree, to.into()), identity);
    }
    pub(crate) fn resolve_mount(&self, selector: &str, path: &str) -> Result<String, u32> {
        let physical = self.resolve_path(0, "", selector, true)?;
        self.resolve_path(0, &physical, path, true)
    }
    /// Resolve only inside the pinned directory supplied by the operation.
    /// This model uses path components and snapshot data, never host handles.
    pub(crate) fn resolve_in(
        &self,
        tree: u64,
        directory: u64,
        logical_root: &str,
        path: &str,
        follow_leaf: bool,
    ) -> Result<String, u32> {
        let root = self
            .resolved_directories
            .get(&directory)
            .map(String::as_str)
            .unwrap_or(logical_root);
        self.resolve_path(tree, root, path, follow_leaf)
    }
    fn resolve_path(
        &self,
        tree: u64,
        root: &str,
        path: &str,
        follow_leaf: bool,
    ) -> Result<String, u32> {
        let invalid = self.config.invalid_path_code;
        if path.starts_with('/') || path.contains(['\\', '\0']) {
            return Err(invalid);
        }
        let mut physical: Vec<_> = root
            .split('/')
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
            .collect();
        let floor = physical.len();
        let mut components: VecDeque<_> = path.split('/').map(str::to_owned).collect();
        let mut followed = 0;
        while let Some(component) = components.pop_front() {
            if component.is_empty() || component == "." {
                continue;
            }
            if component == ".." {
                if physical.len() == floor {
                    return Err(invalid);
                }
                physical.pop();
                continue;
            }
            physical.push(component);
            let name = physical.join("/");
            if tree == 0
                && (follow_leaf || !components.is_empty())
                && let Some(target) = self.config.links.get(&name)
            {
                followed += 1;
                if followed > 40 || target.starts_with('/') || target.contains(['\\', '\0']) {
                    return Err(invalid);
                }
                physical.pop();
                for component in target.split('/').rev() {
                    components.push_front(component.into());
                }
            } else if !components.is_empty() && !self.trees[&tree].0.contains(&name) {
                return Err(self.config.missing_path_code);
            }
        }
        Ok(physical.join("/"))
    }
    pub(crate) fn resolve_stored(
        &self,
        tree: u64,
        directory: u64,
        path: &str,
        follow_leaf: bool,
    ) -> Result<String, u32> {
        let Some(Grant::Dir { root, .. }) = self.grants.get(&directory) else {
            return Err(self.config.invalid_path_code);
        };
        let local = if root.is_empty() {
            path
        } else {
            path.strip_prefix(&format!("{root}/"))
                .ok_or(self.config.invalid_path_code)?
        };
        self.resolve_in(tree, directory, root, local, follow_leaf)
    }
    pub(crate) fn walk_paths(
        &self,
        tree: u64,
        directory: u64,
        logical_root: &str,
    ) -> Result<Vec<String>, u32> {
        let root = self.resolve_in(tree, directory, logical_root, "", true)?;
        let mut work = vec![(root.clone(), String::new(), vec![root.clone()])];
        let mut result = Vec::new();
        while let Some((physical, logical, ancestry)) = work.pop() {
            let mut children = BTreeSet::new();
            for p in self.trees[&tree]
                .0
                .iter()
                .chain(self.trees[&tree].1.keys())
                .chain(self.config.links.keys().filter(|_| tree == 0))
            {
                let (parent, name) = p.rsplit_once('/').unwrap_or(("", p));
                if parent == physical && !name.is_empty() {
                    children.insert(name.to_owned());
                }
            }
            for child in children {
                let relative = if logical.is_empty() {
                    child
                } else {
                    format!("{logical}/{child}")
                };
                let resolved = self.resolve_path(tree, &root, &relative, true)?;
                if self.trees[&tree].0.contains(&resolved) {
                    if ancestry.contains(&resolved) {
                        return Err(self.config.invalid_path_code);
                    }
                    let mut ancestry = ancestry.clone();
                    ancestry.push(resolved.clone());
                    work.push((resolved, relative, ancestry));
                } else if self.trees[&tree].1.contains_key(&resolved) {
                    result.push(relative);
                } else {
                    return Err(self.config.invalid_path_code);
                }
            }
        }
        result.sort();
        Ok(result)
    }
    pub fn grant_root(&mut self, name: &str) -> Result<Value, Error> {
        if !matches!(name, "Io" | "Args" | "Env" | "Unsafe" | "Test")
            || (name != "Test" && !self.config.grants.contains(name))
        {
            return Err(Error::Unavailable(format!("launch cannot grant {name}")));
        }
        Ok(self.handle(name, Grant::Root(name.into())))
    }
    pub(crate) fn handle(&mut self, kind: &str, grant: Grant) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.grants.insert(id, grant);
        Value::Handle {
            kind: kind.into(),
            id,
        }
    }
    pub(crate) fn id(&self, value: &Value, kind: &str) -> Result<u64, Error> {
        if let Value::Handle { kind: k, id } = value
            && k == kind
            && match self.grants.get(id) {
                Some(Grant::Net { .. }) => kind == "Net",
                Some(Grant::Stream { .. }) => kind == "Stream",
                Some(Grant::Root(name)) => name == kind,
                Some(Grant::Dir { .. }) => kind == "Dir",
                Some(Grant::File { .. }) => kind == "File",
                Some(Grant::Atomic { .. }) => kind == "AtomicFile",
                None => false,
            }
        {
            Ok(*id)
        } else {
            Err(invalid(format!("invalid {kind} grant")))
        }
    }
    pub(crate) fn writable(&self, directory: u64) -> bool {
        matches!(
            self.grants.get(&directory),
            Some(Grant::Dir {
                access: Access::ReadWrite,
                ..
            })
        )
    }
    pub(crate) fn directory(&self, value: &Value) -> Result<(u64, String, u64), Error> {
        let id = self.id(value, "Dir")?;
        if let Some(Grant::Dir { tree, root, .. }) = self.grants.get(&id) {
            Ok((*tree, root.clone(), id))
        } else {
            Err(invalid("wrong directory grant"))
        }
    }
    pub(crate) fn fault(&mut self, op: &str) -> Option<Fault> {
        self.config.faults.get_mut(op).and_then(VecDeque::pop_front)
    }
    /// Event payloads are length-framed UTF-8/bits, with no spans/host identities.
    pub(crate) fn record(
        &mut self,
        capability: &str,
        operation: &str,
        input: Vec<u8>,
        output: Vec<u8>,
        context: &mut OperationContext<'_>,
    ) -> Result<(), Error> {
        let event = CapabilityEvent {
            capability: core_path(capability),
            operation: operation.into(),
            input,
            output,
        };
        if !self.replay.is_empty() {
            if self.replay.get(self.replay_index) != Some(&event) {
                return Err(invalid(format!(
                    "capability replay mismatch at event {}",
                    self.replay_index
                )));
            }
            self.replay_index += 1;
        }
        if context.trace_mode == crate::TraceMode::Full {
            let mut bytes = vec![];
            for part in [&event.input, &event.output] {
                bytes.extend_from_slice(&(part.len() as u64).to_le_bytes());
                bytes.extend_from_slice(part);
            }
            context.trace.push(TraceEvent::Operation {
                name: format!("core@1/{operation}"),
                bytes,
            });
        }
        // Standalone kernel fixtures have no scheduler anchor. Executions always
        // provide one; publish the semantic effect after replay acceptance.
        if context.anchor().is_some() {
            context.consume(event.clone())?;
        }
        self.events.push(event);
        Ok(())
    }
    pub fn finish_replay(&self) -> Result<(), Error> {
        if !self.replay.is_empty() && self.replay_index != self.replay.len() {
            Err(invalid("unconsumed capability replay events"))
        } else {
            Ok(())
        }
    }
    /// Typed random/env kernels remain unpublished until their ledger cutover.
    pub fn random_u64(
        &mut self,
        random: &Value,
        context: &mut OperationContext<'_>,
    ) -> Result<u64, Error> {
        self.id(random, "Random")?;
        // SplitMix64: fixed wrapping integer algorithm, seed supplied by run.
        let next_state = self.state.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = next_state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        let z = z ^ (z >> 31);
        self.record(
            "Random",
            "random.u64",
            vec![],
            z.to_le_bytes().to_vec(),
            context,
        )?;
        self.state = next_state;
        Ok(z)
    }
    pub fn env_get(
        &mut self,
        env: &Value,
        key: &str,
        context: &mut OperationContext<'_>,
    ) -> Result<Option<String>, Error> {
        self.id(env, "Env")?;
        let v = self.config.env.get(key).cloned();
        let mut out = vec![u8::from(v.is_some())];
        if let Some(v) = &v {
            out.extend_from_slice(v.as_bytes());
        }
        self.record("Env", "env.get", key.as_bytes().to_vec(), out, context)?;
        Ok(v)
    }
    pub fn grant_test(&mut self) -> Value {
        self.handle("Test", Grant::Root("Test".into()))
    }
}
pub(crate) fn parents(path: &str, dirs: &mut BTreeSet<String>) {
    let mut p = path;
    while let Some((parent, _)) = p.rsplit_once('/') {
        dirs.insert(parent.into());
        p = parent;
    }
}
pub(crate) fn relative(root: &str, path: &str) -> Option<String> {
    if path.starts_with('/') || path.contains('\0') || path.contains('\\') {
        return None;
    }
    let mut parts: Vec<_> = root.split('/').filter(|s| !s.is_empty()).collect();
    let base = parts.len();
    for p in path.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                if parts.len() == base {
                    return None;
                }
                parts.pop();
            }
            _ => parts.push(p),
        }
    }
    Some(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejected_random_event_preserves_seed_and_replay_position() {
        let mut run = RunConfiguration {
            entry: core_path("main"),
            hash_seed: 42,
            inputs: vec![],
        };
        let sources = SourceMap::default();
        let mut stdout = vec![];
        let mut trace = vec![];
        let mut sim = Simulation::new(
            &run,
            SimulationConfig {
                random_seed: 42,
                ..Default::default()
            },
        )
        .unwrap();
        let random = sim.handle("Random", Grant::Root("Random".into()));
        sim.random_u64(
            &random,
            &mut OperationContext {
                configuration: &run,
                sources: &sources,
                stdout: &mut stdout,
                trace: &mut trace,
                trace_mode: Default::default(),
                anchor: None,
            },
        )
        .unwrap();
        run.inputs = sim.events;
        let mut sim = Simulation::new(
            &run,
            SimulationConfig {
                random_seed: 43,
                ..Default::default()
            },
        )
        .unwrap();
        let random = sim.handle("Random", Grant::Root("Random".into()));
        trace.clear();
        assert!(matches!(
            sim.random_u64(
                &random,
                &mut OperationContext {
                    trace_mode: Default::default(),
                    anchor: None,
                    configuration: &run,
                    sources: &sources,
                    stdout: &mut stdout,
                    trace: &mut trace
                }
            ),
            Err(Error::InvalidTir(_))
        ));
        assert_eq!(sim.state, 43);
        assert_eq!(sim.replay_index, 0);
        assert!(sim.events.is_empty());
        assert!(trace.is_empty());
    }
    use hirsel_span::SourceMap;
}

#[cfg(test)]
mod semantic_events {
    use super::*;
    use crate::observation::{Anchor, SemanticRole};
    use hirsel_span::{SourceMap, Span};
    use hirsel_tir::BodyId;
    #[test]
    fn accepted_capability_events_have_source_anchors_and_rejected_replay_has_no_effect() {
        let run = RunConfiguration {
            entry: core_path("main"),
            hash_seed: 0,
            inputs: vec![],
        };
        let mut sim = Simulation::new(&run, Default::default()).unwrap();
        let mut sources = SourceMap::default();
        let file = sources.add("effect.hir", "operation\n".into()).unwrap();
        let site = Span::new(file, 0, 9).unwrap();
        let anchor = Anchor {
            definition: run.entry.clone(),
            body: BodyId(77),
            cleanup: None,
            site,
        };
        let mut trace = vec![];
        let mut stdout = vec![];
        let mut context = OperationContext {
            configuration: &run,
            sources: &sources,
            stdout: &mut stdout,
            trace: &mut trace,
            trace_mode: Default::default(),
            anchor: Some(&anchor),
        };
        sim.record(
            "Stdout",
            "out.print",
            b"input".to_vec(),
            b"output".to_vec(),
            &mut context,
        )
        .unwrap();
        let effect = context
            .trace
            .iter()
            .find_map(|e| {
                if let TraceEvent::Effect(e) = e {
                    Some(e)
                } else {
                    None
                }
            })
            .unwrap();
        let anchor = effect.semantic_anchor(&sources).unwrap();
        assert_eq!(anchor.role, SemanticRole::CapabilityEffect);
        assert_eq!(anchor.source.definition, run.entry);
        assert_eq!((anchor.source.line, anchor.source.column), (1, 1));
        assert!(
            matches!(&effect.effect, crate::observation::Effect::CapabilityConsumed(event) if event == &sim.events[0])
        );
        let before = context.trace.clone();
        sim.replay = vec![sim.events[0].clone()];
        sim.replay_index = 0;
        assert!(
            sim.record(
                "Stdout",
                "out.print",
                b"different".to_vec(),
                b"output".to_vec(),
                &mut context
            )
            .is_err()
        );
        assert_eq!(*context.trace, before);
        assert_eq!(sim.events.len(), 1);
        assert_eq!(sim.replay_index, 0);
    }
}
