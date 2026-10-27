//! Explicit deterministic capability implementations; no host I/O or entropy.
use crate::{Error, OperationContext, TraceEvent, Value, invalid};
use hirsel_run_schema::Access;
pub use hirsel_run_schema::FileReadScript;
use hirsel_run_schema::confined_relative;
use hirsel_syntax::DefPath;
use hirsel_tir::{CapabilityEvent, RunConfiguration};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Core types declared Plain whose interpreter value keeps its evidence as a
/// string: a `Path` its text, a `StrPos` its source. That string is the
/// interpreter's own check, not program storage, so the value is never
/// local (MODE-1: plain values hold identity loans only).
pub(crate) fn plain_evidence(definition: &DefPath) -> bool {
    let path = &definition.0;
    path.package.package_is("core", "1")
        && path.modules.is_empty()
        && matches!(path.name.as_str(), "Path" | "StrPos")
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FileError {
    Fault(u32),
    NotFound,
    PermissionDenied,
    AlreadyExists,
    IsDirectory,
    NotDirectory,
    DirectoryNotEmpty,
    CrossDevice,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Fault {
    File { error: FileError, partial: usize },
    Canceled(u32),
    Fault { code: u32, partial: usize },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileMetadata {
    pub kind: u32,
    pub size: u64,
    pub modified_unix_ns: Option<i128>,
    pub permissions: u32,
}
/// Extra suspension points for simulator exploration, never production scheduling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulePoint {
    RootRelease,
    AtomicStore,
}

/// Opt-in tooling; excluded from run identity, semantic observations and Io tapes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SimulationDebug {
    pub races: bool,
    pub root_releases: bool,
    pub atomic_stores: bool,
}
impl SimulationDebug {
    /// Comma-separated point names used by the test CLI and simulation JSON.
    pub fn parse_points(text: &str) -> Option<Self> {
        let mut debug = Self::default();
        for name in text.split(',') {
            match name {
                "root-release" if !debug.root_releases => debug.root_releases = true,
                "atomic-store" if !debug.atomic_stores => debug.atomic_stores = true,
                _ => return None,
            }
        }
        Some(debug)
    }
    pub fn points(self) -> String {
        [
            self.root_releases.then_some("root-release"),
            self.atomic_stores.then_some("atomic-store"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(",")
    }
    pub fn enabled(self, point: SchedulePoint) -> bool {
        match point {
            SchedulePoint::RootRelease => self.root_releases,
            SchedulePoint::AtomicStore => self.atomic_stores,
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SimulationConfig {
    pub debug: SimulationDebug,
    /// Explicit script provider and exact rules; peer data never widens authority.
    pub net: Option<hirsel_run_schema::NetGrant>,
    /// Exact process rules; scripted runs never widen them.
    pub process: Option<hirsel_run_schema::ProcessGrant>,
    pub cpu: Option<hirsel_run_schema::CpuGrant>,
    /// The admitted Tasks policy, independent of whether a tape is present.
    pub tasks_free: bool,
    /// The observed kinds and the `signals.script@1` arrivals; absent, the run has no
    /// Signals and receives no signal.
    pub signals: Option<hirsel_run_schema::SignalsGrant>,
    pub random_seed: u64,
    pub test_seed: u64,
    /// The `telemetry.null@1` identity seed: a stream separate from Random's.
    pub telemetry_seed: u64,
    /// The run's scheduler policy (`inputs.scheduler`); `None` is `fifo@1`.
    pub scheduler: Option<hirsel_run_schema::Scheduler>,
    pub grants: BTreeSet<String>,
    /// Configured Io projections, independent of the root Io grant.
    pub projections: BTreeSet<String>,
    /// Virtual selectors and inherited operation rights; independent of host designators.
    pub mounts: BTreeMap<String, Access>,
    pub args: Vec<String>,
    pub env: BTreeMap<String, Option<String>>,
    pub stdin: Vec<u8>,
    pub files: BTreeMap<String, Vec<u8>>,
    /// Simulation-only readiness gates by resolved opened-file path; empty by default.
    pub file_reads: BTreeMap<String, FileReadScript>,
    pub file_faults: hirsel_run_schema::FileFaults,
    pub file_aliases: BTreeMap<String, String>,
    pub directories: BTreeSet<String>,
    /// Persistent read errors by resolved directory path in the initial tree.
    /// Applied to the initial page and every lazy refill, after confinement.
    pub directory_errors: BTreeMap<String, FileError>,
    pub links: BTreeMap<String, String>,
    pub metadata: BTreeMap<String, FileMetadata>,
    pub monotonic: u64,
    pub wall: i64,
    /// Codes for invalid/confinement paths are capability inputs.
    pub invalid_path_code: u32,
    /// Fault queues are consumed by operation name, in order, never host errno.
    pub faults: BTreeMap<String, VecDeque<Fault>>,
    /// The runners the run's Io projects (RUN-1), sorted by name.
    pub runners: Vec<hirsel_run_schema::RunnerEntry>,
    /// The seed `sim@1` runners draw their faults from (`inputs.fault_seed`).
    pub fault_seed: u64,
    /// This run's instance (RUN-11): call keys and callee seeds derive from it.
    pub instance: [u8; 16],
    /// The callee entries the program names, by full versioned path (RUN-8, RUN-11).
    pub callees: std::sync::Arc<BTreeMap<String, hirsel_run_schema::CalleeFacts>>,
    /// Record every live `Shared` root's count at every scheduling point
    /// (`--trace-roots`, S-14) into `ObservedSimulation::root_trace`.
    pub trace_roots: bool,
}
impl Default for SimulationConfig {
    fn default() -> Self {
        Self {
            debug: SimulationDebug::default(),
            net: None,
            process: None,
            cpu: None,
            tasks_free: false,
            signals: None,
            random_seed: 0,
            test_seed: 0,
            telemetry_seed: 0,
            scheduler: None,
            grants: ["Io", "Args", "Env"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            mounts: BTreeMap::from([(String::new(), Access::ReadWrite)]),
            projections: [
                "stdin",
                "stderr",
                "stdout",
                "tasks",
                "net",
                "process",
                "random",
                "dir",
                "telemetry",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            args: vec![],
            env: BTreeMap::new(),
            stdin: Vec::new(),
            files: BTreeMap::new(),
            file_reads: BTreeMap::new(),
            file_faults: Default::default(),
            file_aliases: BTreeMap::new(),
            directories: [String::new()].into_iter().collect(),
            directory_errors: BTreeMap::new(),
            links: BTreeMap::new(),
            metadata: BTreeMap::new(),
            monotonic: 0,
            wall: 0,
            invalid_path_code: 1,
            faults: BTreeMap::new(),
            runners: vec![],
            fault_seed: 0,
            instance: [0; 16],
            callees: Default::default(),
            trace_roots: false,
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Grant {
    Net {
        world: u64,
        rules: Vec<hirsel_run_schema::NetRule>,
    },
    Stream {
        net: u64,
    },
    Proc {
        world: u64,
        rules: Vec<hirsel_run_schema::ProcessRule>,
    },
    /// A streaming child of Proc `process`, or with `group` a spawned pipeline (a
    /// ChildGroup); scripted ones are in `script_children` and `script_groups`.
    Child {
        process: u64,
        stdin_open: bool,
        group: bool,
    },
    /// A `net.split` half of stream `stream`, whose grant waits aside in `split`.
    Half {
        net: u64,
        stream: u64,
        write: bool,
    },
    UdpSocket {
        net: u64,
    },
    Listener {
        net: u64,
    },
    Root(String),
    /// An `env.narrow` result: exactly these names of the run's Env.
    Env(BTreeSet<String>),
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
        position: u64,
        writable: bool,
    },
    Atomic {
        tree: u64,
        temp: String,
        dst: String,
        dir: u64,
        identity: u64,
    },
    /// A runner: its place and the authority its callees run under (RUN-1, RUN-2).
    Runner(Box<super::runners::RunnerGrant>),
    /// A read-only mapping of `length` bytes of an opened identity; `truncated` once a
    /// check found the identity shorter, after which every check reports it.
    Mapped {
        tree: u64,
        dir: u64,
        identity: u64,
        length: u64,
        truncated: bool,
    },
    /// A change watcher over tree path `root` (with `recursive`, everything below it) and
    /// the events delivered to it.
    Watcher {
        tree: u64,
        dir: u64,
        root: String,
        recursive: bool,
        queue: super::watch::Queue,
    },
    /// A live span of Telemetry `telemetry` and its context.
    Span {
        telemetry: u64,
        context: Vec<u8>,
    },
}
#[derive(Clone, Debug)]
pub struct Simulation {
    pub(super) file_readiness: hirsel_run_schema::FileReadiness,
    pub(super) net_worlds: BTreeMap<u64, super::net::World>,
    pub(super) net_streams: BTreeMap<u64, super::net::ScriptStream>,
    pub(super) process_worlds: BTreeMap<u64, super::process::World>,
    pub(super) process_children: BTreeMap<u64, super::process::Children>,
    /// Spawned scripted children, by Child handle.
    pub(super) process_path: Vec<u32>,
    pub(super) test_clocks: BTreeSet<u64>,
    pub(super) process_sites: BTreeMap<u64, (crate::observation::Anchor, Vec<u32>)>,
    pub(super) script_children: BTreeMap<u64, hirsel_run_schema::ScriptChild>,
    /// Spawned scripted pipelines, by ChildGroup handle.
    pub(super) script_groups: BTreeMap<u64, hirsel_run_schema::ScriptGroup>,
    pub(super) udp_sockets: BTreeMap<u64, super::net::ScriptUdp>,
    pub(super) net_listeners: BTreeMap<u64, super::net::ScriptListener>,
    pub config: SimulationConfig,
    pub events: Vec<CapabilityEvent>,
    pub stderr: Vec<u8>,
    pub(crate) stdin_position: usize,
    pub(crate) grants: BTreeMap<u64, Grant>,
    pub(crate) trees: BTreeMap<u64, (BTreeSet<String>, BTreeMap<String, Vec<u8>>)>,
    pub(crate) resolved_directories: BTreeMap<u64, String>,
    pub(super) detached_directories: BTreeSet<u64>,
    pub(super) mount_pins: BTreeMap<String, String>,
    pub(super) detached_mounts: BTreeSet<String>,
    pub(super) file_identities: BTreeMap<(u64, String), u64>,
    pub(super) file_permissions: BTreeMap<(u64, String), u32>,
    next_file_identity: u64,
    pub(crate) file_contents: BTreeMap<u64, Vec<u8>>,
    /// What a crash of each `test.dir` tree keeps (core-library.md, Durability).
    pub(crate) durable: BTreeMap<u64, hirsel_kernels::disk::Durable>,
    /// File and AtomicFile handles opened before their tree's last crash.
    pub(crate) stale: BTreeSet<u64>,
    pub(crate) next_id: u64,
    state: u64,
    supplied_random: Option<Result<Vec<u8>, ()>>,
    /// `telemetry.null@1`'s identity stream and the identities delivered so far.
    telemetry_ids: hirsel_kernels::telemetry::NullIds,
    telemetry_issued: hirsel_kernels::telemetry::Issued,
    /// A host provider's identities for the next begin (`supply_telemetry`).
    supplied_telemetry: Option<([u8; 16], [u8; 8])>,
    replay: Vec<CapabilityEvent>,
    replay_index: usize,
    pub(crate) last_monotonic: Option<u64>,
    /// `--trace-roots` (S-14), when the configuration asks for it.
    pub(super) root_trace: Option<super::root_trace::RootTrace>,
}
/// Final fake filesystem and surviving logical grants, including independent trees.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedSimulation {
    pub stderr: Vec<u8>,
    pub trees: BTreeMap<u64, (BTreeSet<String>, BTreeMap<String, Vec<u8>>)>,
    pub handles: BTreeMap<u64, ObservedGrant>,
    /// `--trace-roots` (S-14): one line per scheduling point, empty unless asked for.
    pub root_trace: Vec<String>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ObservedGrant {
    /// A runner: the entry's name and the definitions it runs.
    Runner {
        name: String,
        definitions: Option<Vec<String>>,
    },
    Net {
        world: u64,
        rules: Vec<hirsel_run_schema::NetRule>,
    },
    Stream {
        net: u64,
    },
    Proc {
        world: u64,
        rules: Vec<hirsel_run_schema::ProcessRule>,
    },
    Child {
        process: u64,
    },
    /// A `net.split` half of stream `stream`, whose grant waits aside in `split`.
    Half {
        net: u64,
        stream: u64,
        write: bool,
    },
    UdpSocket {
        net: u64,
    },
    Listener {
        net: u64,
    },
    Root(String),
    Env {
        names: BTreeSet<String>,
    },
    Directory {
        tree: u64,
        root: String,
        access: Access,
    },
    File {
        tree: u64,
        path: String,
        directory: u64,
        position: u64,
        writable: bool,
    },
    AtomicFile {
        tree: u64,
        temporary: String,
        destination: String,
        directory: u64,
    },
    Mapped {
        tree: u64,
        directory: u64,
        length: u64,
        truncated: bool,
    },
    Span {
        telemetry: u64,
    },
    Watcher {
        tree: u64,
        directory: u64,
        root: String,
        recursive: bool,
    },
}
impl Simulation {
    /// Supply the next provider chunk without changing any grant or replay state.
    /// Host adapters use this after a typed read; ordinary simulations supply the whole snapshot.
    pub fn supply_stdin(&mut self, bytes: Vec<u8>) {
        self.config.stdin = bytes;
        self.stdin_position = 0;
    }
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
                        Grant::Half { net, stream, write } => ObservedGrant::Half {
                            net: *net,
                            stream: *stream,
                            write: *write,
                        },
                        Grant::Proc { world, rules } => ObservedGrant::Proc {
                            world: *world,
                            rules: rules.clone(),
                        },
                        Grant::Child { process, .. } => ObservedGrant::Child { process: *process },
                        Grant::UdpSocket { net } => ObservedGrant::UdpSocket { net: *net },
                        Grant::Listener { net } => ObservedGrant::Listener { net: *net },
                        Grant::Root(kind) => ObservedGrant::Root(kind.clone()),
                        Grant::Env(names) => ObservedGrant::Env {
                            names: names.clone(),
                        },
                        Grant::Dir { tree, root, access } => ObservedGrant::Directory {
                            tree: *tree,
                            root: root.clone(),
                            access: *access,
                        },
                        Grant::File {
                            tree,
                            path,
                            dir,
                            position,
                            writable,
                            ..
                        } => ObservedGrant::File {
                            tree: *tree,
                            path: path.clone(),
                            directory: *dir,
                            position: *position,
                            writable: *writable,
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
                        Grant::Runner(runner) => ObservedGrant::Runner {
                            name: runner.entry.name.clone(),
                            definitions: runner.definitions.clone(),
                        },
                        Grant::Mapped {
                            tree,
                            dir,
                            length,
                            truncated,
                            ..
                        } => ObservedGrant::Mapped {
                            tree: *tree,
                            directory: *dir,
                            length: *length,
                            truncated: *truncated,
                        },
                        Grant::Watcher {
                            tree,
                            dir,
                            root,
                            recursive,
                            ..
                        } => ObservedGrant::Watcher {
                            tree: *tree,
                            directory: *dir,
                            root: root.clone(),
                            recursive: *recursive,
                        },
                        Grant::Span { telemetry, .. } => ObservedGrant::Span {
                            telemetry: *telemetry,
                        },
                    },
                )
            })
            .collect();
        ObservedSimulation {
            stderr: self.stderr.clone(),
            trees: self.trees.clone(),
            handles,
            root_trace: self
                .root_trace
                .as_ref()
                .map(|t| t.lines.clone())
                .unwrap_or_default(),
        }
    }
    pub fn new(run: &RunConfiguration, mut config: SimulationConfig) -> Result<Self, Error> {
        config.env = hirsel_run_schema::snapshot_env(
            std::mem::take(&mut config.env)
                .into_iter()
                .map(|(key, value)| (key.into(), value.map(Into::into))),
        )
        .map_err(|error| invalid(format!("simulation environment: {error}")))?;
        // Validate snapshot entry names before granting anything. Link targets
        // remain data and are checked at the confined operation boundary.
        hirsel_run_schema::validate_file_faults(&config.file_faults).map_err(invalid)?;
        if config
            .file_faults
            .keys()
            .any(|op| config.faults.contains_key(op.name()))
        {
            return Err(invalid("overlapping legacy and typed file fault queues"));
        }
        let mut files = BTreeMap::new();
        for (path, data) in std::mem::take(&mut config.files) {
            let path = confined_relative("", &path)
                .ok_or_else(|| invalid("simulation file escapes root"))?;
            if path.is_empty() {
                return Err(invalid("file at directory root"));
            }
            files.insert(path, data);
        }
        let mut dirs = BTreeSet::from([String::new()]);
        for d in &config.directories {
            dirs.insert(
                confined_relative("", d)
                    .ok_or_else(|| invalid("simulation directory escapes root"))?,
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
            if path.is_empty() || confined_relative("", path).as_ref() != Some(path) {
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
        let telemetry_seed = config.telemetry_seed;
        let net_worlds = super::net::configured_world(config.net.as_ref())?;
        if let Some(process) = &config.process {
            if let Some(timed) = &process.timed {
                if timed.clock != "run.monotonic" {
                    return Err(invalid("invalid timed clock selector"));
                }
                for child in &timed.children {
                    child.validate().map_err(invalid)?;
                }
            }
            if process.timed.is_some()
                && (!config.grants.contains("MonotonicClock")
                    || config
                        .net
                        .as_ref()
                        .is_some_and(|n| n.provider != "net.script@2"))
            {
                return Err(invalid(
                    "timed processes require an isolated granted virtual monotonic clock",
                ));
            }
        }
        let (process_worlds, process_children) =
            super::process::configured_world(config.process.as_ref())?;
        let root_trace = config.trace_roots.then(Default::default);
        let mut simulation = Self {
            file_readiness: hirsel_run_schema::FileReadiness::new(config.file_reads.clone()),
            root_trace,
            net_worlds,
            net_streams: BTreeMap::new(),
            process_worlds,
            process_children,
            process_path: vec![],
            test_clocks: BTreeSet::new(),
            process_sites: BTreeMap::new(),
            script_children: BTreeMap::new(),
            script_groups: BTreeMap::new(),
            net_listeners: BTreeMap::new(),
            udp_sockets: BTreeMap::new(),
            config,
            events: vec![],
            stderr: Vec::new(),
            stdin_position: 0,
            grants: BTreeMap::new(),
            trees: BTreeMap::from([(0, (dirs, files))]),
            resolved_directories: BTreeMap::new(),
            detached_directories: BTreeSet::new(),
            mount_pins: BTreeMap::new(),
            detached_mounts: BTreeSet::new(),
            file_identities: BTreeMap::new(),
            file_permissions: BTreeMap::new(),
            next_file_identity: 1,
            file_contents: BTreeMap::new(),
            durable: BTreeMap::new(),
            stale: BTreeSet::new(),
            next_id: 1,
            state: random_seed,
            supplied_random: None,
            telemetry_ids: hirsel_kernels::telemetry::NullIds::new(telemetry_seed),
            telemetry_issued: Default::default(),
            supplied_telemetry: None,
            replay: run.inputs.clone(),
            replay_index: 0,
            last_monotonic: None,
        };
        simulation.identify_tree(0);
        simulation.file_permissions.extend(
            simulation
                .config
                .metadata
                .iter()
                .map(|(p, meta)| ((0, p.clone()), meta.permissions)),
        );
        simulation.mount_pins = simulation
            .config
            .mounts
            .keys()
            .filter_map(|selector| {
                simulation
                    .resolve_path(0, "", selector, true)
                    .ok()
                    .map(|root| (selector.clone(), root))
            })
            .collect();
        Ok(simulation)
    }
    pub(crate) fn identify_tree(&mut self, tree: u64) {
        let paths: BTreeSet<_> = self.trees[&tree]
            .1
            .keys()
            .cloned()
            .chain(
                self.config
                    .metadata
                    .iter()
                    .filter_map(|(p, m)| (tree == 0 && m.kind == 0).then_some(p.clone())),
            )
            .collect();
        let metadata = (tree == 0).then(|| std::mem::take(&mut self.config.metadata));
        for path in paths {
            self.replace_identity(tree, &path);
        }
        if let Some(metadata) = metadata {
            self.config.metadata = metadata;
        }
        if tree == 0 {
            for (alias, canonical) in &self.config.file_aliases {
                if let Some(identity) = self
                    .file_identities
                    .get(&(tree, canonical.clone()))
                    .copied()
                {
                    self.file_identities.insert((tree, alias.clone()), identity);
                }
            }
        }
    }
    /// A `test.dir` tree starts durable as created.
    pub(crate) fn durable_start(&mut self, tree: u64) {
        let files = self
            .file_identities
            .iter()
            .filter(|((t, _), _)| *t == tree)
            .map(|((_, path), id)| (path.clone(), (*id, self.file_contents[id].clone())))
            .collect();
        self.durable.insert(
            tree,
            hirsel_kernels::disk::Durable::of(&self.trees[&tree].0, &files),
        );
    }
    /// Note an unsynced write to file `identity` of a `test.dir` tree.
    pub(crate) fn durable_write(&mut self, tree: u64, identity: u64, start: usize, bytes: &[u8]) {
        if let Some(durable) = self.durable.get_mut(&tree)
            && !bytes.is_empty()
        {
            durable.wrote(identity, start, bytes);
        }
    }
    /// Make file `identity` and the directory chain above `path` durable.
    pub(crate) fn durable_sync(&mut self, tree: u64, identity: u64, path: &str) {
        let Some(durable) = self.durable.get_mut(&tree) else {
            return;
        };
        let files = self
            .file_identities
            .iter()
            .filter(|((t, _), _)| *t == tree)
            .map(|((_, path), id)| (path.clone(), *id))
            .collect();
        durable.sync(
            identity,
            &self.file_contents[&identity],
            path,
            &self.trees[&tree].0,
            &files,
        );
    }
    /// `test.crash`: the tree becomes its durable state; its open files and mappings go stale.
    pub(crate) fn crash(&mut self, tree: u64, seed: u64) {
        let crashed = self
            .durable
            .get_mut(&tree)
            .expect("a test.dir tree")
            .crashed(seed);
        self.file_identities.retain(|(t, _), _| *t != tree);
        self.file_permissions.retain(|(t, path), _| {
            *t != tree || crashed.directories.contains(path) || crashed.files.contains_key(path)
        });
        let mut files = BTreeMap::new();
        for (path, (identity, bytes)) in crashed.files {
            self.file_identities.insert((tree, path.clone()), identity);
            self.file_contents.insert(identity, bytes.clone());
            files.insert(path, bytes);
        }
        self.trees.insert(tree, (crashed.directories, files));
        let stale: Vec<u64> = self
            .grants
            .iter()
            .filter_map(|(id, grant)| match grant {
                Grant::File { tree: t, .. }
                | Grant::Atomic { tree: t, .. }
                | Grant::Mapped { tree: t, .. }
                    if *t == tree =>
                {
                    Some(*id)
                }
                _ => None,
            })
            .collect();
        self.stale.extend(stale);
    }
    pub(crate) fn file_identity(&self, tree: u64, path: &str) -> Option<u64> {
        self.file_identities.get(&(tree, path.into())).copied()
    }
    /// Publication replaces the entry even when every byte is unchanged. Never
    /// recycle identities: unlink/recreate must not resurrect an opened file.
    pub(crate) fn replace_identity(&mut self, tree: u64, path: &str) {
        self.file_permissions.remove(&(tree, path.into()));
        if tree == 0 {
            self.config.metadata.remove(path);
        }
        let identity = self.next_file_identity;
        self.next_file_identity = identity.checked_add(1).expect("file identities exhausted");
        self.file_identities.insert((tree, path.into()), identity);
        self.file_contents.insert(
            identity,
            self.trees[&tree].1.get(path).cloned().unwrap_or_default(),
        );
    }
    pub(crate) fn sync_file(&mut self, tree: u64, identity: u64) {
        for ((t, path), _) in self
            .file_identities
            .iter()
            .filter(|(_, id)| **id == identity)
        {
            assert_eq!(*t, tree);
            if tree == 0
                && let Some(meta) = self.config.metadata.remove(path)
            {
                self.file_permissions
                    .entry((tree, path.clone()))
                    .or_insert(meta.permissions);
            }
            self.trees
                .get_mut(&tree)
                .unwrap()
                .1
                .insert(path.clone(), self.file_contents[&identity].clone());
        }
    }
    pub(crate) fn refresh_file(&mut self, tree: u64, path: &str) {
        if tree == 0
            && let Some(meta) = self.config.metadata.remove(path)
        {
            self.file_permissions
                .entry((tree, path.into()))
                .or_insert(meta.permissions);
        }
        let identity = self.file_identity(tree, path).expect("file identity");
        self.file_contents
            .insert(identity, self.trees[&tree].1[path].clone());
    }
    pub(crate) fn unlink_identity(&mut self, tree: u64, path: &str) {
        self.file_permissions.remove(&(tree, path.into()));
        if tree == 0 {
            self.config.metadata.remove(path);
        }
        self.file_identities.remove(&(tree, path.into()));
    }
    pub(crate) fn publish_identity(&mut self, tree: u64, from: &str, to: &str) {
        self.file_permissions.remove(&(tree, to.into()));
        if let Some(bits) = self.file_permissions.remove(&(tree, from.into())) {
            self.file_permissions.insert((tree, to.into()), bits);
        }
        if tree == 0 {
            self.config.metadata.remove(from);
            self.config.metadata.remove(to);
        }
        let identity = self
            .file_identities
            .remove(&(tree, from.into()))
            .expect("atomic identity");
        self.file_identities.insert((tree, to.into()), identity);
    }
    pub(crate) fn file_bytes(&self, tree: u64, path: &str) -> Result<&Vec<u8>, FileError> {
        if self.trees[&tree].0.contains(path) {
            return Err(FileError::IsDirectory);
        }
        self.trees[&tree].1.get(path).ok_or(FileError::NotFound)
    }
    pub(crate) fn file_destination(&self, tree: u64, path: &str) -> Result<(), FileError> {
        if self.trees[&tree].0.contains(path) {
            return Err(FileError::IsDirectory);
        }
        let parent = path.rsplit_once('/').map_or("", |(p, _)| p);
        if self.trees[&tree].0.contains(parent) {
            Ok(())
        } else {
            Err(if self.trees[&tree].1.contains_key(parent) {
                FileError::NotDirectory
            } else {
                FileError::NotFound
            })
        }
    }
    pub(crate) fn resolve_mount(&self, selector: &str, path: &str) -> Result<String, FileError> {
        let physical = self
            .mount_pins
            .get(selector)
            .cloned()
            .map_or_else(|| self.resolve_path(0, "", selector, true), Ok)?;
        if self.detached_mounts.contains(selector) {
            return if confined_relative("", path).as_deref() == Some("") {
                Ok(physical)
            } else {
                Err(FileError::NotFound)
            };
        }
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
    ) -> Result<String, FileError> {
        let root = self
            .resolved_directories
            .get(&directory)
            .map(String::as_str)
            .unwrap_or(logical_root);
        if self.detached_directories.contains(&directory) {
            return if confined_relative("", path).as_deref() == Some("") {
                Ok(root.into())
            } else {
                Err(FileError::NotFound)
            };
        }
        let normalized =
            confined_relative("", path).ok_or(FileError::Fault(self.config.invalid_path_code))?;
        self.resolve_path(tree, root, &normalized, follow_leaf)
    }
    fn resolve_path(
        &self,
        tree: u64,
        root: &str,
        path: &str,
        follow_leaf: bool,
    ) -> Result<String, FileError> {
        let invalid = FileError::Fault(self.config.invalid_path_code);
        if path.starts_with('/') || path.contains(['\\', '\0']) {
            return Err(invalid.clone());
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
                    return Err(invalid.clone());
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
                    return Err(invalid.clone());
                }
                physical.pop();
                for component in target.split('/').rev() {
                    components.push_front(component.into());
                }
            } else if !components.is_empty() && !self.trees[&tree].0.contains(&name) {
                return Err(if self.trees[&tree].1.contains_key(&name) {
                    FileError::NotDirectory
                } else {
                    FileError::NotFound
                });
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
    ) -> Result<String, FileError> {
        let Some(Grant::Dir { root, .. }) = self.grants.get(&directory) else {
            return Err(FileError::Fault(self.config.invalid_path_code));
        };
        let local = if root.is_empty() {
            path
        } else {
            path.strip_prefix(&format!("{root}/"))
                .ok_or(FileError::Fault(self.config.invalid_path_code))?
        };
        self.resolve_in(tree, directory, root, local, follow_leaf)
    }
    pub(crate) fn walk_entries(
        &self,
        tree: u64,
        directory: u64,
        root: &str,
        local: &str,
        after: &str,
        recursive: bool,
    ) -> Result<Vec<hirsel_kernels::walk::Entry>, FileError> {
        let physical = self.resolve_in(tree, directory, root, local, true)?;
        if self.detached_directories.contains(&directory) {
            return Ok(vec![]);
        }
        if self.metadata(tree, &physical)?.kind != 1 {
            return Err(FileError::NotDirectory);
        }
        if tree == 0
            && let Some(error) = self.config.directory_errors.get(&physical)
        {
            return Err(error.clone());
        }
        let prefix = if physical.is_empty() {
            String::new()
        } else {
            format!("{physical}/")
        };
        let mut page = hirsel_kernels::walk::PageSelector::default();
        let (dirs, files) = &self.trees[&tree];
        let links = (tree == 0).then_some(&self.config.links);
        let metadata = (tree == 0).then_some(&self.config.metadata);
        let mut offer = |p: String| {
            let name = p.strip_prefix(&prefix).expect("direct child");
            page.offer(
                hirsel_kernels::walk::Entry {
                    path: name.into(),
                    kind: self
                        .metadata(tree, &p)
                        .expect("indexed directory entry")
                        .kind,
                },
                after,
                recursive,
            );
        };
        hirsel_kernels::walk::visit_children(
            &physical,
            |bound| dirs.range(bound.to_owned()..).next().cloned(),
            &mut offer,
        );
        hirsel_kernels::walk::visit_children(
            &physical,
            |bound| {
                files
                    .range(bound.to_owned()..)
                    .next()
                    .map(|(p, _)| p.clone())
            },
            &mut offer,
        );
        if let Some(links) = links {
            hirsel_kernels::walk::visit_children(
                &physical,
                |bound| {
                    links
                        .range(bound.to_owned()..)
                        .next()
                        .map(|(p, _)| p.clone())
                },
                &mut offer,
            );
        }
        if let Some(metadata) = metadata {
            hirsel_kernels::walk::visit_children(
                &physical,
                |bound| {
                    metadata
                        .range(bound.to_owned()..)
                        .next()
                        .map(|(p, _)| p.clone())
                },
                &mut offer,
            );
        }
        Ok(page
            .finish()
            .into_iter()
            .map(|mut e| {
                if !local.is_empty() {
                    e.path = format!("{local}/{}", e.path);
                }
                e
            })
            .collect())
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
                Some(Grant::Half { write, .. }) => {
                    kind == if *write { "WriteHalf" } else { "ReadHalf" }
                }
                Some(Grant::Proc { .. }) => kind == "Proc",
                Some(Grant::Child { group, .. }) => {
                    kind == if *group { "ChildGroup" } else { "Child" }
                }
                Some(Grant::UdpSocket { .. }) => kind == "UdpSocket",
                Some(Grant::Listener { .. }) => kind == "Listener",
                Some(Grant::Root(name)) => name == kind,
                Some(Grant::Env(_)) => kind == "Env",
                Some(Grant::Dir { .. }) => kind == "Dir",
                Some(Grant::File { .. }) => kind == "File",
                Some(Grant::Atomic { .. }) => kind == "AtomicFile",
                Some(Grant::Runner(_)) => kind == "Runner",
                Some(Grant::Mapped { .. }) => kind == "Mapped",
                Some(Grant::Watcher { .. }) => kind == "Watcher",
                Some(Grant::Span { .. }) => kind == "Span",
                None => false,
            }
        {
            Ok(*id)
        } else {
            Err(invalid(format!("invalid {kind} grant")))
        }
    }
    /// Whether Env `env` grants `name`: the root grants every configured name.
    pub(crate) fn env_granted(&self, env: u64, name: &str) -> bool {
        match self.grants.get(&env) {
            Some(Grant::Env(names)) => names.contains(name),
            _ => self.config.env.contains_key(name),
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
    /// The virtual monotonic reading: the configured value, advanced only by timer waits.
    pub(crate) fn monotonic_now(&self) -> u64 {
        self.config.monotonic
    }
    pub(crate) fn advance_monotonic(&mut self, ticks: u64) {
        self.config.monotonic = self.config.monotonic.max(ticks);
    }
    pub(crate) fn file_fault(
        &mut self,
        name: &str,
        requested: Option<u64>,
    ) -> Result<Option<Fault>, Error> {
        use hirsel_run_schema::FileFaultError as E;
        let op = self
            .config
            .file_faults
            .keys()
            .find(|op| op.name() == name)
            .copied();
        let Some(op) = op else {
            return Ok(self.fault(name));
        };
        let step = hirsel_run_schema::take_file_fault(&mut self.config.file_faults, op, requested)
            .map_err(invalid)?;
        Ok(step.and_then(|step| {
            step.error.map(|error| match error {
                E::Canceled(cause) => Fault::Canceled(cause),
                E::Fault(code) => Fault::Fault {
                    code,
                    partial: step.partial as usize,
                },
                error => Fault::File {
                    error: match error {
                        E::NotFound => FileError::NotFound,
                        E::PermissionDenied => FileError::PermissionDenied,
                        E::AlreadyExists => FileError::AlreadyExists,
                        E::IsDirectory => FileError::IsDirectory,
                        E::NotDirectory => FileError::NotDirectory,
                        E::DirectoryNotEmpty => FileError::DirectoryNotEmpty,
                        E::CrossDevice => FileError::CrossDevice,
                        _ => unreachable!(),
                    },
                    partial: step.partial as usize,
                },
            })
        }))
    }
    pub(crate) fn fault(&mut self, op: &str) -> Option<Fault> {
        self.config.faults.get_mut(op).and_then(VecDeque::pop_front)
    }
    /// Peek a recorded input only after the operation has checked its current authority.
    /// record() accepts and advances it after the completion shape has been validated.
    pub(crate) fn replay_output(
        &self,
        capability: &str,
        operation: &str,
        input: &[u8],
    ) -> Result<Option<&[u8]>, Error> {
        if self.replay.is_empty() {
            return Ok(None);
        }
        let event = self
            .replay
            .get(self.replay_index)
            .ok_or_else(|| invalid("missing capability replay event"))?;
        if event.capability != DefPath::core(capability)
            || event.operation != operation
            || event.input != input
        {
            return Err(invalid("capability replay request mismatch"));
        }
        Ok(Some(&event.output))
    }
    /// Event payloads are length-framed text, raw bytes or bits, with no spans/host identities.
    pub(crate) fn record(
        &mut self,
        capability: &str,
        operation: &str,
        input: Vec<u8>,
        output: Vec<u8>,
        context: &mut OperationContext<'_>,
    ) -> Result<(), Error> {
        let event = CapabilityEvent {
            capability: hirsel_syntax::DefPath::core(capability),
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
    pub(super) fn next_replay(&self) -> Option<&hirsel_tir::CapabilityEvent> {
        self.replay.get(self.replay_index)
    }
    pub fn finish_replay(&self) -> Result<(), Error> {
        if !self.replay.is_empty() && self.replay_index != self.replay.len() {
            Err(invalid("unconsumed capability replay events"))
        } else {
            Ok(())
        }
    }
    /// `random.splitmix64@1`: successive SplitMix64 words of the run's Random seed, each
    /// little-endian, the last cut short; the state advances one word per started eight
    /// bytes. A host adapter supplies its provider's draw instead (`supply_random`).
    pub(crate) fn random_bytes(&mut self, count: u64) -> Result<Vec<u8>, ()> {
        if let Some(draw) = self.supplied_random.take() {
            return draw;
        }
        let mut bytes = Vec::new();
        while (bytes.len() as u64) < count {
            self.state = self.state.wrapping_add(0x9e3779b97f4a7c15);
            let mut z = self.state;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
            let word = (z ^ (z >> 31)).to_le_bytes();
            let take = (count - bytes.len() as u64).min(8) as usize;
            bytes.extend_from_slice(&word[..take]);
        }
        Ok(bytes)
    }
    /// The next `random.bytes` outcome, from a host provider; `Err` is `Unavailable()`.
    /// A begin's identities: a host provider's supplied ones, else the null provider's
    /// stream, retrying zero and colliding draws.
    pub(crate) fn telemetry_ids(
        &mut self,
        parent: Option<[u8; 16]>,
    ) -> Result<([u8; 16], [u8; 8]), Error> {
        let (trace, span) = match self.supplied_telemetry.take() {
            Some(ids) => ids,
            None => {
                let ids = &mut self.telemetry_ids;
                self.telemetry_issued.choose(parent, |n| ids.draw(n))
            }
        };
        if !self.telemetry_issued.admits(parent, &trace, &span) {
            return Err(invalid("inadmissible span identities"));
        }
        self.telemetry_issued.accept(parent, trace, span);
        Ok((trace, span))
    }
    /// A host provider's identities for the next `capture.begin`.
    pub fn supply_telemetry(&mut self, trace: [u8; 16], span: [u8; 8]) {
        self.supplied_telemetry = Some((trace, span));
    }
    pub fn supply_random(&mut self, draw: Result<Vec<u8>, ()>) {
        self.supplied_random = Some(draw);
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

#[cfg(test)]
mod semantic_events {
    use super::*;
    use crate::observation::{Anchor, SemanticRole};
    use hirsel_span::{SourceMap, Span};
    use hirsel_tir::BodyId;
    #[test]
    fn accepted_capability_events_have_source_anchors_and_rejected_replay_has_no_effect() {
        let run = RunConfiguration {
            entry: hirsel_syntax::DefPath::core("main"),
            hash_seed: 0,
            inputs: vec![],
        };
        let mut sim = Simulation::new(&run, Default::default()).unwrap();
        let mut sources = SourceMap::default();
        let file = sources.add("effect.hir", "operation\n".into()).unwrap();
        let site = Span::new(sources.file_anchor(file), 0, 9).unwrap();
        let anchor = Anchor {
            definition: (run.entry.clone()).into(),
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

#[cfg(test)]
mod directory_reads {
    use super::*;

    #[test]
    fn errors_follow_resolved_directories_without_faulting_parent_pages_or_test_trees() {
        let run = RunConfiguration {
            entry: hirsel_syntax::DefPath::core("main"),
            hash_seed: 0,
            inputs: vec![],
        };
        let config = SimulationConfig {
            directories: ["fixtures".into(), "fixtures/zzbad".into()].into(),
            files: [("fixtures/a".into(), vec![])].into(),
            links: [("fixtures/alias".into(), "zzbad".into())].into(),
            directory_errors: [("fixtures/zzbad".into(), FileError::Fault(7))].into(),
            ..Default::default()
        };
        let mut sim = Simulation::new(&run, config).unwrap();
        assert!(sim.walk_entries(0, 0, "fixtures", "", "", true).is_ok());
        assert!(sim.walk_entries(0, 0, "fixtures", "", "a", true).is_ok());
        for (root, path) in [
            ("fixtures", "zzbad"),
            ("fixtures", "alias"),
            ("fixtures/zzbad", ""),
        ] {
            for _ in 0..2 {
                assert_eq!(
                    sim.walk_entries(0, 0, root, path, "", false),
                    Err(FileError::Fault(7))
                );
            }
        }
        assert_eq!(
            sim.walk_entries(0, 0, "fixtures/zzbad", "..", "", false),
            Err(FileError::Fault(1))
        );
        sim.trees.insert(1, sim.trees[&0].clone());
        assert!(
            sim.walk_entries(1, 0, "fixtures", "zzbad", "", false)
                .unwrap()
                .is_empty()
        );
    }
}
