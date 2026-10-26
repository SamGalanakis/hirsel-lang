//! The checked heap executor over a Ready program: the entry runs
//! with the launch handles and a model of the Io operations native code has,
//! and the terminal audit must pass on success and on every failure path.
use super::observe::{Observation, Sites};
use super::stages::{NativeError, ReadyProgram, StageName};
use hirsel_core_contract::OperationId;
use hirsel_heapcheck::{Arg, Config, Counters, Observed, OpOutcome, OpValue, Operations};
use hirsel_lir::ir::SiteRange;
use hirsel_lir::layout_types::ScalarKind;
use hirsel_rt::io::UnitResult;
use hirsel_rt::launch::{ARGS_HANDLE, IO_HANDLE};
use hirsel_run_schema::Access;
use hirsel_syntax::DefPath;
use std::collections::{BTreeMap, BTreeSet};

mod durability;
mod env;
mod file_faults;
mod filesystem;
mod input;
mod net;
mod numeric;
mod process;
mod signals;

/// The Io operations with a native entry (`hirsel_rt_operations`), over
/// logical handles numbered as the runtime's adapter numbers them.
#[derive(Clone)]
struct Io {
    stdin: std::io::Cursor<Vec<u8>>,
    args: Vec<String>,
    file_reads: hirsel_run_schema::FileReadiness,
    file_faults: hirsel_run_schema::FileFaults,
    deny_output: bool,
    next: u64,
    test_seed: u64,
    stdout: Vec<u64>,
    paths: Vec<String>,
    mounts: BTreeMap<String, Access>,
    grants: BTreeMap<u64, IoGrant>,
    trees: BTreeMap<u64, (BTreeSet<String>, BTreeMap<String, Vec<u8>>)>,
    files: BTreeMap<(u64, String), std::sync::Arc<()>>,
    permissions: BTreeMap<(u64, String), u32>,
    network: hirsel_rt::io_net_script::ScriptIo,
    network_handles: BTreeMap<u64, u64>,
    /// `random.splitmix64@1` from the launch's Random seed, as the reference draws.
    random: hirsel_rt::io::RandomSource,
    /// `telemetry.null@1` with seed 0, as the harness runtime draws span identities.
    telemetry_ids: hirsel_rt::telemetry::NullIds,
    telemetry_issued: hirsel_rt::telemetry::Issued,
    /// The last call was a capability operation (a scheduling point).
    yielded: bool,
    /// The virtual monotonic reading when the launch grants a monotonic clock. H owns what
    /// waits for it and when it advances (`hirsel-heapcheck/src/exec/clock.rs`).
    monotonic: Option<u64>,
    /// The network model's handle for each monotonic clock a `proc.run` deadline named.
    script_clocks: BTreeMap<u64, u64>,
    detached_directories: BTreeSet<u64>,
    resolved_directories: BTreeMap<u64, String>,
    /// What a crash of each `test.dir` tree keeps (core-library.md, Durability).
    durable: BTreeMap<u64, durability::Durable>,
    /// File and AtomicFile handles opened before their tree's last crash.
    stale: BTreeSet<u64>,
    serials: durability::Serials,
    /// Each failure site's line and column, for a program that isolates (FAIL-2).
    positions: Positions,
    signals: signals::Signals,
}

type Positions = std::sync::Arc<BTreeMap<(DefPath, SiteRange), (u64, u64)>>;

/// The positions `task.isolate` reports, of every site a failure can start at.
fn positions(program: &ReadyProgram, sites: &Sites<'_>) -> Positions {
    if !super::observe::isolates(program) {
        return Positions::default();
    }
    std::sync::Arc::new(
        program
            .summaries
            .iter()
            .flat_map(|summary| {
                summary.sites.iter().filter_map(|&range| {
                    let owner = super::observe::source_definition(program, &summary.definition);
                    let (line, column) = sites.position(owner, range)?;
                    Some((
                        (summary.definition.clone(), range),
                        (u64::from(line), u64::from(column)),
                    ))
                })
            })
            .collect(),
    )
}

impl Io {
    fn for_launch(
        kind: &super::link::LaunchKind,
        test_seed: u64,
        random_seed: u64,
        world: &HeapWorld,
    ) -> Result<Self, NativeError> {
        hirsel_run_schema::validate_file_faults(&world.file_faults)
            .map_err(|e| NativeError::new(StageName::Heap, e))?;
        let mut io = Self::new();
        io.args = world.args.clone();
        io.stdin = std::io::Cursor::new(world.stdin.clone());
        io.mounts = world.mounts.clone();
        let tree = hirsel_rt::io::Tree::fixture(world.files.clone(), world.mounts.clone())
            .map_err(|e| NativeError::new(StageName::Launch, e.to_string()))?;
        for path in tree.files.keys() {
            io.files.insert((0, path.clone()), std::sync::Arc::new(()));
        }
        io.trees.insert(0, (tree.directories, tree.files));
        io.file_reads = hirsel_run_schema::FileReadiness::new(world.file_reads.clone());
        io.file_faults = world.file_faults.clone();
        io.deny_output = world.deny_output;
        io.test_seed = test_seed;
        io.monotonic = world.monotonic;
        io.random = hirsel_rt::io::RandomSource::SplitMix64(random_seed);
        io.network = hirsel_rt::io_net_script::ScriptIo::new(
            matches!(kind, super::link::LaunchKind::Test).then_some(test_seed),
        );
        // Peers the runtime's script refuses refuse the launch, as a host launch refuses them.
        if let Some(grant) = &world.net {
            io.network.set_net_grant(grant).map_err(|_| {
                NativeError::new(StageName::Launch, "io.net.peers is not a valid script")
            })?;
        }
        if let Some(grant) = &world.process {
            io.network.set_process_grant(grant);
        }
        if let Some(env) = &world.env {
            io.network.set_env(env.clone());
        }
        io.signals = signals::Signals::new(world.signals.clone());
        match kind {
            super::link::LaunchKind::Zero => io.grants.clear(),
            super::link::LaunchKind::Test => {
                io.grants.insert(IO_HANDLE, IoGrant::Test);
            }
            super::link::LaunchKind::Main(_) | super::link::LaunchKind::Call { .. } => {}
        }
        Ok(io)
    }
    fn new() -> Self {
        Self {
            stdin: std::io::Cursor::new(vec![]),
            args: vec![],
            file_reads: Default::default(),
            file_faults: Default::default(),
            next: IO_HANDLE + 1,
            test_seed: 0,
            stdout: Vec::new(),
            paths: Vec::new(),
            mounts: BTreeMap::from([(String::new(), Access::ReadWrite)]),
            grants: BTreeMap::from([(IO_HANDLE, IoGrant::Root)]),
            trees: BTreeMap::from([(0, (BTreeSet::from([String::new()]), BTreeMap::new()))]),
            files: BTreeMap::new(),
            network: Default::default(),
            network_handles: BTreeMap::new(),
            random: hirsel_rt::io::RandomSource::SplitMix64(0),
            telemetry_ids: hirsel_rt::telemetry::NullIds::new(0),
            telemetry_issued: Default::default(),
            yielded: false,
            deny_output: false,
            monotonic: None,
            script_clocks: BTreeMap::new(),
            permissions: BTreeMap::new(),
            detached_directories: BTreeSet::new(),
            resolved_directories: BTreeMap::new(),
            durable: BTreeMap::new(),
            stale: BTreeSet::new(),
            serials: Default::default(),
            positions: Positions::default(),
            signals: Default::default(),
        }
    }
}

impl Operations for Io {
    fn domain(&self) -> hirsel_heapcheck::OperationDomain {
        hirsel_heapcheck::OperationDomain::Simulation
    }
    fn fork_simulation(&self) -> Option<Box<dyn Operations>> {
        Some(Box::new(self.clone()))
    }
    fn failure_source_position(&self, definition: &DefPath, site: SiteRange) -> Option<(u64, u64)> {
        self.positions.get(&(definition.clone(), site)).copied()
    }
    fn path_text(&self, id: u64) -> Option<&str> {
        self.paths
            .get(id.checked_sub(1)? as usize)
            .map(String::as_str)
    }
    fn intern_path(&mut self, text: &str) -> Option<u64> {
        Some(self.path(text))
    }
    fn close_handle(
        &mut self,
        handle: u64,
        output: &mut Vec<u8>,
    ) -> Result<Option<String>, String> {
        if let Some(id) = self.network_handles.remove(&handle) {
            let completion = self
                .network
                .call(hirsel_rt::io::Request::Close { handle: id });
            output.extend(self.network.take_report());
            return match completion {
                // A dropped accepted stream or listener left a scripted client's Expect unmet.
                Ok(hirsel_rt::io::Completion::Checked(false)) => Ok(Some("LANG-9".into())),
                Ok(_) => Ok(None),
                Err(e) => Err(format!("network close: {e:?}")),
            };
        }
        match self.grants.remove(&handle) {
            Some(
                IoGrant::File { .. }
                | IoGrant::Mapped { .. }
                | IoGrant::Watcher { .. }
                | IoGrant::Span { .. },
            ) => Ok(None),
            Some(IoGrant::Atomic {
                tree,
                temporary,
                directory,
                ..
            }) => {
                if let Some(temporary) = self.physical_path(directory, &temporary) {
                    self.trees.get_mut(&tree).unwrap().1.remove(&temporary);
                    self.files.remove(&(tree, temporary));
                }
                Ok(None)
            }
            _ => Err(format!("model close of unknown handle {handle}")),
        }
    }
    fn yielded(&mut self) -> bool {
        std::mem::take(&mut self.yielded)
    }
    fn monotonic(&self, clock: u64) -> Option<u64> {
        matches!(self.grants.get(&clock), Some(IoGrant::Monotonic))
            .then_some(self.monotonic)
            .flatten()
    }
    fn deliver_signal(&mut self) -> hirsel_heapcheck::SignalDelivery {
        self.signals.deliver()
    }
    fn advance_monotonic(&mut self, ticks: u64) {
        if let Some(now) = &mut self.monotonic {
            *now = (*now).max(ticks);
        }
    }
    /// The runtime's cancellation points (`hirsel-rt/src/host_waits.rs`): a canceled stdin
    /// read, print, file or waiting network/process request checks program validity before
    /// completing `Canceled(cause)` with no effect, and is still a capability operation.
    fn canceled(
        &mut self,
        op: &OperationId,
        args: &[OpValue],
        cause: u32,
        output: &mut Vec<u8>,
    ) -> Option<OpOutcome> {
        let path = &op.definition.0;
        if !path.package.name_is("core") {
            return None;
        }
        let module = path.modules.first().map(String::as_str);
        let value = match (module, path.name.as_str(), args) {
            // A zero-byte read is invalid before it is a cancellation point.
            (Some("stdin"), "read", [_, _, OpValue::Bits { bits: 0, .. }]) => return None,
            (Some("stdin"), "read", _) => OpValue::Variant {
                tag: 1,
                value: Box::new(OpValue::Variant {
                    tag: 1,
                    value: Box::new(OpValue::Variant {
                        tag: cause,
                        value: Box::new(OpValue::Unit),
                    }),
                }),
            },
            (Some("out"), "print" | "write", _) => {
                let canceled = UnitResult::of(Err(hirsel_rt::io::IoError::Canceled(cause)));
                let mut bytes = Vec::with_capacity(12);
                for word in [canceled.tag, canceled.error, canceled.code] {
                    bytes.extend_from_slice(&word.to_le_bytes());
                }
                OpValue::Plain(bytes)
            }
            (Some("err"), "print" | "write", _) => {
                let mut bytes = 1u32.to_le_bytes().to_vec();
                bytes.extend_from_slice(&cause.to_le_bytes());
                OpValue::Variant {
                    tag: 1,
                    value: Box::new(OpValue::Plain(bytes)),
                }
            }
            // WaitError::Canceled(cause).
            (Some("clock"), "sleep_until", _) => OpValue::Variant {
                tag: 1,
                value: Box::new(OpValue::Variant {
                    tag: 1,
                    value: Box::new(OpValue::Variant {
                        tag: cause,
                        value: Box::new(OpValue::Unit),
                    }),
                }),
            },
            // `mapped.with`'s entries are no cancellation points.
            (Some("$intrinsic"), "MappedView" | "MappedCheck", _) => return None,
            (Some("watcher"), "next", [OpValue::Handle(directory), OpValue::Handle(watcher)]) => {
                if !matches!(self.grants.get(watcher), Some(IoGrant::Watcher { directory: owner, .. }) if owner == directory)
                {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                }
                let mut bytes = 1u32.to_le_bytes().to_vec();
                bytes.extend_from_slice(&cause.to_le_bytes());
                OpValue::Variant {
                    tag: 1,
                    value: Box::new(OpValue::Plain(bytes)),
                }
            }
            (Some("net" | "udp" | "tls" | "read_half" | "write_half" | "$intrinsic"), _, _) => {
                self.yielded = true;
                let outcome = self.net(module, &path.name, args, Some(cause));
                output.extend(self.network.take_report());
                return outcome;
            }
            (Some("proc" | "child" | "group"), _, _) => {
                self.yielded = true;
                let outcome = self.process(module, &path.name, args, Some(cause));
                output.extend(self.network.take_report());
                return outcome;
            }
            (Some("signal"), "wait", _) => {
                self.yielded = true;
                return self.signals(module, &path.name, args, Some(cause));
            }
            (Some("file"), _, _) | (Some("dir"), "remove_all", _) => {
                self.yielded = true;
                return self.cancel_file(&path.name, args, cause);
            }
            _ => return None,
        };
        self.yielded = true;
        Some(OpOutcome::Value(value))
    }
    fn iterator_next(
        &mut self,
        state: &mut OpValue,
        canceled: Option<u32>,
    ) -> Result<Option<OpValue>, String> {
        self.directory_next(state, canceled)
    }
    fn call(&mut self, op: &OperationId, args: &[OpValue], output: &mut Vec<u8>) -> OpOutcome {
        let path = &op.definition.0;
        if !path.package.name_is("core") {
            return OpOutcome::Unsupported;
        }
        let module = path.modules.first().map(String::as_str);
        if module == Some("$intrinsic") {
            if let Some(outcome) = views(&path.name, args) {
                return outcome;
            }
        }
        if let Some(outcome) = text(module, &path.name, args) {
            return outcome;
        }
        if let Some(value) = instant(module, &path.name, args) {
            return OpOutcome::Value(value);
        }
        if module == Some("capture") || (module == Some("io") && path.name == "telemetry") {
            return self.telemetry(&path.name, args);
        }
        // Everything below is a capability operation, as the runtime's Io entries are.
        self.yielded = true;
        if let Some(outcome) = self.net(module, &path.name, args, None) {
            output.extend(self.network.take_report());
            return outcome;
        }
        if let Some(outcome) = self.process(module, &path.name, args, None) {
            output.extend(self.network.take_report());
            return outcome;
        }
        if let Some(outcome) = self.signals(module, &path.name, args, None) {
            return outcome;
        }
        if let Some(outcome) = self.detached_filesystem(module, &path.name, args) {
            return outcome;
        }
        if let Some(outcome) = self.filesystem(module, &path.name, args) {
            return outcome;
        }
        if let Some(outcome) = self.io(module, &path.name, args) {
            return outcome;
        }
        match (module, path.name.as_str(), args) {
            (
                Some("test"),
                "check_case",
                [
                    OpValue::Handle(test),
                    OpValue::Bits {
                        scalar: ScalarKind::Bool,
                        bits,
                    },
                    OpValue::Bits {
                        scalar: ScalarKind::U64,
                        bits: case,
                    },
                ],
            ) if matches!(self.grants.get(test), Some(IoGrant::Test)) => {
                if *bits == 0 {
                    output.extend_from_slice(
                        format!("test failure: seed={} case={case}\n", self.test_seed).as_bytes(),
                    );
                    OpOutcome::Fail("LANG-9".into())
                } else {
                    OpOutcome::Value(OpValue::Unit)
                }
            }
            (Some("test"), "seed", [OpValue::Handle(test)])
                if matches!(self.grants.get(test), Some(IoGrant::Test)) =>
            {
                OpOutcome::Value(OpValue::Bits {
                    scalar: ScalarKind::U64,
                    bits: self.test_seed.into(),
                })
            }
            (Some("test"), "tasks", [OpValue::Handle(test)])
                if matches!(self.grants.get(test), Some(IoGrant::Test)) =>
            {
                OpOutcome::Value(OpValue::Handle(self.grant(IoGrant::Tasks)))
            }
            (Some("test"), "log", [OpValue::Handle(test), OpValue::Bytes(text)])
                if matches!(self.grants.get(test), Some(IoGrant::Test)) =>
            {
                let Ok(text) = std::str::from_utf8(text) else {
                    return OpOutcome::Unsupported;
                };
                output.extend_from_slice(&hirsel_rt::kernels::report::log(text));
                OpOutcome::Value(OpValue::Unit)
            }
            (
                Some("test"),
                "expect_snapshot",
                [
                    OpValue::Handle(test),
                    OpValue::Bytes(name),
                    OpValue::Bytes(text),
                ],
            ) if matches!(self.grants.get(test), Some(IoGrant::Test)) => {
                let (Ok(name), Ok(text)) = (std::str::from_utf8(name), std::str::from_utf8(text))
                else {
                    return OpOutcome::Unsupported;
                };
                if !hirsel_rt::kernels::report::snapshot_name(name) {
                    return OpOutcome::Fail("LANG-9".into());
                }
                output.extend_from_slice(&hirsel_rt::kernels::report::snapshot(name, text));
                OpOutcome::Value(OpValue::Unit)
            }
            (Some("io"), "stdout", [OpValue::Handle(IO_HANDLE)]) => {
                if self.deny_output {
                    return OpOutcome::Value(grant_not_granted());
                }
                let handle = self.next;
                self.next += 1;
                self.stdout.push(handle);
                OpOutcome::Value(io_ok(OpValue::Handle(handle)))
            }
            (Some("out"), "print" | "write", [OpValue::Handle(s), OpValue::Bytes(text)])
                if self.stdout.contains(s) =>
            {
                output.extend_from_slice(text);
                let ok = UnitResult::of(Ok(()));
                let mut bytes = Vec::with_capacity(12);
                for word in [ok.tag, ok.error, ok.code] {
                    bytes.extend_from_slice(&word.to_le_bytes());
                }
                OpOutcome::Value(OpValue::Plain(bytes))
            }
            _ => OpOutcome::Unsupported,
        }
    }
}

#[derive(Clone)]
enum IoGrant {
    Telemetry,
    /// A live span of Telemetry `telemetry` and its context.
    Span {
        telemetry: u64,
        context: Vec<u8>,
    },
    Monotonic,
    Signals,
    Stdin,
    Stderr,
    Root,
    Random,
    Tasks,
    Test,
    Dir {
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
        contents: Vec<u8>,
        identity: std::sync::Arc<()>,
    },
    Atomic {
        tree: u64,
        directory: u64,
        temporary: String,
        destination: String,
        identity: std::sync::Arc<()>,
    },
    /// A `file.map` mapping: its identity's current contents, the mapped length, and
    /// whether a check found the identity shorter.
    Mapped {
        directory: u64,
        contents: Vec<u8>,
        length: u64,
        truncated: bool,
        identity: std::sync::Arc<()>,
    },
    /// A `file.watch` watcher over tree path `root`; its queue holds the events
    /// `test.dir_event` delivered (`hirsel_rt::watch`).
    Watcher {
        tree: u64,
        directory: u64,
        root: String,
        recursive: bool,
        queue: hirsel_rt::watch::WatchQueue,
    },
}
/// The pure Instant operations: no capability, no scheduling point.
fn instant(module: Option<&str>, name: &str, args: &[OpValue]) -> Option<OpValue> {
    Some(match (module, name, args) {
        (Some("instant"), "ticks", [OpValue::Handle(ticks)]) => OpValue::Bits {
            scalar: ScalarKind::U64,
            bits: u128::from(*ticks),
        },
        (
            Some("instant"),
            "checked_add",
            [
                OpValue::Handle(_),
                OpValue::Handle(ticks),
                OpValue::Bits {
                    scalar: ScalarKind::U64,
                    bits,
                },
            ],
        ) => match ticks.checked_add(*bits as u64) {
            Some(ticks) => OpValue::Variant {
                tag: 1,
                value: Box::new(OpValue::Handle(ticks)),
            },
            None => OpValue::Variant {
                tag: 0,
                value: Box::new(OpValue::Unit),
            },
        },
        _ => return None,
    })
}

fn io_ok(value: OpValue) -> OpValue {
    OpValue::Variant {
        tag: 0,
        value: Box::new(value),
    }
}
fn grant_not_granted() -> OpValue {
    OpValue::Variant {
        tag: 1,
        value: Box::new(OpValue::Variant {
            tag: 0,
            value: Box::new(OpValue::Unit),
        }),
    }
}
fn io_error(code: u32) -> OpValue {
    let mut bytes = vec![0; 8];
    bytes[4..].copy_from_slice(&code.to_le_bytes());
    OpValue::Variant {
        tag: 1,
        value: Box::new(OpValue::Plain(bytes)),
    }
}
fn io_portable(ordinal: u32) -> OpValue {
    let mut bytes = vec![0; 8];
    bytes[..4].copy_from_slice(&ordinal.to_le_bytes());
    OpValue::Variant {
        tag: 1,
        value: Box::new(OpValue::Plain(bytes)),
    }
}
fn io_not_granted() -> OpValue {
    let mut bytes = vec![0; 8];
    bytes[..4].copy_from_slice(&2u32.to_le_bytes());
    OpValue::Variant {
        tag: 1,
        value: Box::new(OpValue::Plain(bytes)),
    }
}
fn io_relative(root: &str, path: &str) -> Option<String> {
    if path.starts_with('/') || path.contains(['\0', '\\']) {
        return None;
    }
    let mut parts: Vec<_> = root.split('/').filter(|p| !p.is_empty()).collect();
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
impl Io {
    /// `request`'s completion from the runtime's scripted model. A source that cannot answer
    /// yet (a stalled peer, a child at an `Expect`) leaves the model as it was and is
    /// `Pending`: H suspends the task and re-issues the request at each poll.
    fn script(
        &mut self,
        request: hirsel_rt::io::Request,
        cancellation: Option<u32>,
    ) -> Result<hirsel_rt::io::Completion, OpOutcome> {
        let before = self.network.clone();
        let completion = match cancellation {
            Some(cause) => hirsel_rt::io_checks::cancel(&mut self.network, &request, cause)
                .and_then(|completion| match completion {
                    Some(completion) => Ok(completion),
                    None => self.network.call(request),
                }),
            None => self.network.call(request),
        };
        match completion {
            Ok(completion) => Ok(completion),
            Err(hirsel_rt::io::AdapterError::Blocked) => {
                self.network = before;
                Err(OpOutcome::Pending)
            }
            Err(_) => Err(OpOutcome::Unsupported),
        }
    }
    /// Telemetry under the harness's `telemetry.null@1` (seed 0): a refused payload and a
    /// context read are no scheduling points; every accepted observation is one.
    fn telemetry(&mut self, name: &str, args: &[OpValue]) -> OpOutcome {
        use hirsel_rt::telemetry::kernel;
        let refusal = |refusal: kernel::Refusal| OpValue::Variant {
            tag: 1,
            value: Box::new(OpValue::Plain(refusal.ordinal().to_le_bytes().to_vec())),
        };
        let refused = |r: kernel::Refusal| OpOutcome::Value(refusal(r));
        let span_of = |io: &Self, telemetry: &u64, span: &u64| match io.grants.get(span) {
            Some(IoGrant::Span {
                telemetry: owner,
                context,
            }) if owner == telemetry
                && matches!(io.grants.get(telemetry), Some(IoGrant::Telemetry)) =>
            {
                Some(context.clone())
            }
            _ => None,
        };
        let value = match (name, args) {
            ("telemetry", [OpValue::Handle(IO_HANDLE)]) => {
                io_ok(OpValue::Handle(self.grant(IoGrant::Telemetry)))
            }
            ("context", [OpValue::Handle(telemetry), OpValue::Handle(span)]) => {
                return match span_of(self, telemetry, span) {
                    Some(context) => OpOutcome::Value(OpValue::ByteSequence(context)),
                    None => OpOutcome::Fail("CAP-2".into()),
                };
            }
            (
                "begin",
                [
                    OpValue::Handle(telemetry),
                    OpValue::Bytes(parent),
                    OpValue::Bytes(spec),
                ],
            ) => {
                if !matches!(self.grants.get(telemetry), Some(IoGrant::Telemetry)) {
                    return OpOutcome::Fail("CAP-2".into());
                }
                let trace = match kernel::bounded(&[spec]).and_then(|()| kernel::parent(parent)) {
                    Ok(trace) => trace,
                    Err(refusal) => return refused(refusal),
                };
                let ids = &mut self.telemetry_ids;
                let (id, span) = self.telemetry_issued.choose(trace, |n| ids.draw(n));
                self.telemetry_issued.accept(trace, id, span);
                let telemetry = *telemetry;
                io_ok(OpValue::OpenHandle(self.grant(IoGrant::Span {
                    telemetry,
                    context: kernel::context(&id, &span, parent),
                })))
            }
            (
                "event" | "finish",
                [
                    OpValue::Handle(telemetry),
                    OpValue::Handle(span),
                    OpValue::Bytes(payload),
                ],
            ) => {
                let owned = span_of(self, telemetry, span).is_some();
                if name == "event" {
                    if !owned {
                        return OpOutcome::Fail("CAP-2".into());
                    }
                    if let Err(refusal) = kernel::bounded(&[payload]) {
                        return refused(refusal);
                    }
                    io_ok(OpValue::Unit)
                } else {
                    // The span is taken on every outcome: finished, abandoned by a refused
                    // status, or failed as another Telemetry's.
                    let result = if !owned {
                        Err("CAP-2".into())
                    } else {
                        self.grants.remove(span);
                        match kernel::bounded(&[payload]) {
                            Err(r) => Ok(refusal(r)),
                            Ok(()) => {
                                self.yielded = true;
                                Ok(io_ok(OpValue::Unit))
                            }
                        }
                    };
                    return OpOutcome::Complete {
                        result,
                        consumed: vec![1],
                    };
                }
            }
            ("log" | "measure", [OpValue::Handle(telemetry), OpValue::Bytes(payload)]) => {
                if !matches!(self.grants.get(telemetry), Some(IoGrant::Telemetry)) {
                    return OpOutcome::Fail("CAP-2".into());
                }
                if let Err(refusal) = kernel::bounded(&[payload]) {
                    return refused(refusal);
                }
                io_ok(OpValue::Unit)
            }
            _ => return OpOutcome::Unsupported,
        };
        self.yielded = true;
        OpOutcome::Value(value)
    }
    fn grant(&mut self, grant: IoGrant) -> u64 {
        let id = self.next;
        self.next += 1;
        self.grants.insert(id, grant);
        id
    }
    fn update_files(&mut self, identity: &std::sync::Arc<()>, bytes: &[u8], append: bool) {
        for grant in self.grants.values_mut() {
            if let IoGrant::File {
                identity: opened,
                contents,
                ..
            }
            | IoGrant::Mapped {
                identity: opened,
                contents,
                ..
            } = grant
                && std::sync::Arc::ptr_eq(identity, opened)
            {
                if !append {
                    contents.clear();
                }
                contents.extend_from_slice(bytes);
            }
        }
    }
    fn path(&mut self, path: &str) -> u64 {
        if let Some(i) = self.paths.iter().position(|p| p == path) {
            i as u64 + 1
        } else {
            self.paths.push(path.into());
            self.paths.len() as u64
        }
    }
    fn io(&mut self, module: Option<&str>, name: &str, args: &[OpValue]) -> Option<OpOutcome> {
        if module == Some("file")
            && let Some(outcome) = self.scripted_file(name, args)
        {
            return Some(outcome);
        }
        self.io_ordinary(module, name, args)
    }
    fn io_ordinary(
        &mut self,
        module: Option<&str>,
        name: &str,
        args: &[OpValue],
    ) -> Option<OpOutcome> {
        let value = match (module, name, args) {
            (Some("path"), "text", [OpValue::Handle(path)]) => OpValue::ByteSequence(
                self.paths
                    .get(path.checked_sub(1)? as usize)?
                    .as_bytes()
                    .to_vec(),
            ),
            (Some("path"), "of", [OpValue::Bytes(bytes)]) => {
                OpValue::Handle(self.path(std::str::from_utf8(bytes).ok()?))
            }
            (
                Some("arg"),
                "path" | "text",
                [
                    OpValue::Handle(ARGS_HANDLE),
                    OpValue::Bits {
                        scalar: ScalarKind::U64,
                        bits: index,
                    },
                ],
            ) => {
                match usize::try_from(*index)
                    .ok()
                    .and_then(|i| self.args.get(i))
                    .cloned()
                {
                    Some(text) => io_ok(if name == "path" {
                        OpValue::Handle(self.path(&text))
                    } else {
                        OpValue::ByteSequence(text.into_bytes())
                    }),
                    None => OpValue::Variant {
                        tag: 1,
                        value: Box::new(OpValue::Variant {
                            tag: 0,
                            value: Box::new(OpValue::Unit),
                        }),
                    },
                }
            }
            (Some("io"), kind @ ("stdin" | "stderr"), [OpValue::Handle(IO_HANDLE)]) => {
                if kind == "stderr" && self.deny_output {
                    return Some(OpOutcome::Value(grant_not_granted()));
                }
                let grant = if kind == "stdin" {
                    IoGrant::Stdin
                } else {
                    IoGrant::Stderr
                };
                io_ok(OpValue::Handle(self.grant(grant)))
            }
            (
                Some("stdin"),
                "read",
                [
                    OpValue::Handle(stream),
                    buffer,
                    OpValue::Bits {
                        scalar: ScalarKind::U64,
                        bits: max,
                    },
                ],
            ) => {
                if !matches!(self.grants.get(stream), Some(IoGrant::Stdin)) {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                }
                if *max == 0 {
                    OpValue::Variant {
                        tag: 1,
                        value: Box::new(OpValue::Variant {
                            tag: 2,
                            value: Box::new(OpValue::Unit),
                        }),
                    }
                } else {
                    return Some(self.read_input(buffer, *max));
                }
            }
            (Some("err"), "print" | "write", [OpValue::Handle(stream), OpValue::Bytes(_)]) => {
                if !matches!(self.grants.get(stream), Some(IoGrant::Stderr)) {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                }
                io_ok(OpValue::Unit)
            }
            (Some("env"), name, _) => return self.env(name, args),
            (Some("arg"), "len", [OpValue::Handle(ARGS_HANDLE)]) => OpValue::Bits {
                scalar: ScalarKind::U64,
                bits: self.args.len() as u128,
            },
            (Some("io"), "monotonic_clock", [OpValue::Handle(IO_HANDLE)])
                if self.monotonic.is_some() =>
            {
                io_ok(OpValue::Handle(self.grant(IoGrant::Monotonic)))
            }
            (Some("io"), "monotonic_clock" | "wall_clock", [OpValue::Handle(IO_HANDLE)]) => {
                OpValue::Variant {
                    tag: 1,
                    value: Box::new(OpValue::Variant {
                        tag: 0,
                        value: Box::new(OpValue::Unit),
                    }),
                }
            }
            (Some("test"), "clock", [OpValue::Handle(test)])
                if matches!(self.grants.get(test), Some(IoGrant::Test)) =>
            {
                self.monotonic.get_or_insert(0);
                OpValue::Handle(self.grant(IoGrant::Monotonic))
            }
            (Some("clock"), "monotonic", [OpValue::Handle(clock)]) => {
                match self.monotonic(*clock) {
                    Some(now) => io_ok(OpValue::Handle(now)),
                    None => return Some(OpOutcome::Fail("CAP-2".into())),
                }
            }
            // A deadline not yet reached suspends the task: H owns the wait.
            (Some("clock"), "sleep_until", [OpValue::Handle(clock), OpValue::Handle(until)]) => {
                match self.monotonic(*clock) {
                    Some(now) if now >= *until => io_ok(OpValue::Unit),
                    Some(_) => {
                        return Some(OpOutcome::Wait {
                            clock: *clock,
                            until: *until,
                        });
                    }
                    None => return Some(OpOutcome::Fail("CAP-2".into())),
                }
            }
            (Some("io"), name @ ("random" | "tasks"), [OpValue::Handle(IO_HANDLE)]) => {
                let grant = match name {
                    "random" => IoGrant::Random,
                    _ => IoGrant::Tasks,
                };
                io_ok(OpValue::Handle(self.grant(grant)))
            }
            (
                Some("random"),
                "bytes",
                [
                    OpValue::Handle(random),
                    OpValue::Bits {
                        scalar: ScalarKind::U64,
                        bits: count,
                    },
                ],
            ) => {
                if !matches!(self.grants.get(random), Some(IoGrant::Random)) {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                }
                match self.random.draw(*count as u64) {
                    Ok(bytes) => io_ok(OpValue::ByteSequence(bytes)),
                    Err(_) => OpValue::Variant {
                        tag: 1,
                        value: Box::new(OpValue::Unit),
                    },
                }
            }
            // The directory itself, read only: its pinned resolution and detachment carry over.
            (Some("dir"), "read_only", [OpValue::Handle(parent)]) => {
                let Some(IoGrant::Dir { tree, root, .. }) = self.grants.get(parent).cloned() else {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                };
                let id = self.grant(IoGrant::Dir {
                    tree,
                    root,
                    access: Access::Read,
                });
                if let Some(resolved) = self.resolved_directories.get(parent).cloned() {
                    self.resolved_directories.insert(id, resolved);
                }
                if self.detached_directories.contains(parent) {
                    self.detached_directories.insert(id);
                }
                io_ok(OpValue::Handle(id))
            }
            (
                Some("io" | "dir"),
                kind @ ("dir" | "narrow"),
                [OpValue::Handle(parent), OpValue::Handle(path)],
            ) => {
                let requested = self.paths.get(path.checked_sub(1)? as usize)?;
                let selected = if kind == "dir" && *parent == IO_HANDLE {
                    self.mounts.iter().find_map(|(selector, access)| {
                        let local = if selector.is_empty() {
                            Some(requested.as_str())
                        } else if requested == selector {
                            Some("")
                        } else {
                            requested.strip_prefix(selector)?.strip_prefix('/')
                        }?;
                        Some((0, selector.clone(), *access, local.to_owned()))
                    })
                } else {
                    match self.grants.get(parent) {
                        Some(IoGrant::Dir { tree, root, access }) => {
                            Some((*tree, root.clone(), *access, requested.clone()))
                        }
                        _ => return Some(OpOutcome::Fail("CAP-2".into())),
                    }
                };
                if let Some((tree, root, access, path)) = selected {
                    let logical = io_relative(&root, &path);
                    let physical = logical.as_ref().and_then(|p| {
                        if kind == "narrow" {
                            self.physical_path(*parent, p)
                        } else {
                            Some(p.clone())
                        }
                    });
                    match physical {
                        Some(root) if self.trees[&tree].0.contains(&root) => {
                            let id = self.grant(IoGrant::Dir {
                                tree,
                                root: logical.unwrap(),
                                access,
                            });
                            self.resolved_directories.insert(id, root);
                            io_ok(OpValue::Handle(id))
                        }
                        Some(root) => io_portable(if self.trees[&tree].1.contains_key(&root) {
                            7
                        } else {
                            3
                        }),
                        None => io_error(1),
                    }
                } else {
                    io_not_granted()
                }
            }
            (
                Some("file"),
                "read",
                [
                    OpValue::Handle(directory),
                    OpValue::Handle(file),
                    buffer,
                    OpValue::Bits {
                        scalar: ScalarKind::U64,
                        bits: max,
                    },
                ],
            ) => {
                if !matches!(self.grants.get(directory), Some(IoGrant::Dir { .. })) {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                }
                let Some(IoGrant::File {
                    directory: owner,
                    position,
                    writable,
                    contents,
                    tree,
                    path,
                    ..
                }) = self.grants.get_mut(file)
                else {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                };
                if owner != directory {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                }
                if *writable || self.stale.contains(file) {
                    return Some(OpOutcome::Value(io_error(1)));
                }
                if self.file_reads.blocked(*tree, path) {
                    return Some(OpOutcome::Pending);
                }
                let mut bytes = match buffer {
                    OpValue::Elements {
                        scalar: ScalarKind::U8,
                        bits,
                    } => bits.clone(),
                    OpValue::Bytes(bytes) => bytes.iter().copied().map(u128::from).collect(),
                    _ => return Some(OpOutcome::Unsupported),
                };
                let start = (*position).min(contents.len() as u64) as usize;
                let n = (*max).min(65536).min((contents.len() - start) as u128) as usize;
                bytes.extend(contents[start..start + n].iter().copied().map(u128::from));
                *position += n as u64;
                return Some(OpOutcome::Updated {
                    value: io_ok(OpValue::Bits {
                        scalar: ScalarKind::U64,
                        bits: n as u128,
                    }),
                    arguments: vec![(
                        2,
                        OpValue::Elements {
                            scalar: ScalarKind::U8,
                            bits: bytes,
                        },
                    )],
                });
            }
            (
                Some("file"),
                "seek",
                [
                    OpValue::Handle(directory),
                    OpValue::Handle(file),
                    OpValue::Bits {
                        scalar: ScalarKind::U64,
                        bits: offset,
                    },
                ],
            ) => {
                if !matches!(self.grants.get(directory), Some(IoGrant::Dir { .. })) {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                }
                let Some(IoGrant::File {
                    directory: owner,
                    position,
                    ..
                }) = self.grants.get_mut(file)
                else {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                };
                if owner != directory {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                }
                if *offset > i64::MAX as u128 {
                    io_error(1)
                } else {
                    *position = *offset as u64;
                    io_ok(OpValue::Unit)
                }
            }
            (
                Some("file"),
                name @ ("write" | "append"),
                [
                    OpValue::Handle(directory),
                    OpValue::Handle(file),
                    OpValue::Bytes(bytes),
                ],
            ) => {
                let Some(IoGrant::Dir { tree, access, .. }) = self.grants.get(directory).cloned()
                else {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                };
                let Some(IoGrant::File {
                    tree: owner_tree,
                    directory: owner,
                    writable,
                    identity,
                    contents,
                    ..
                }) = self.grants.get(file).cloned()
                else {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                };
                if tree != owner_tree || owner != *directory {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                }
                if access == Access::Read {
                    io_not_granted()
                } else if !writable || self.stale.contains(file) {
                    io_error(1)
                } else {
                    self.append_file(tree, &identity, &contents, bytes);
                    io_ok(if name == "write" {
                        OpValue::Bits {
                            scalar: ScalarKind::U64,
                            bits: bytes.len() as u128,
                        }
                    } else {
                        OpValue::Unit
                    })
                }
            }
            (Some("file"), "narrow", [OpValue::Handle(directory), OpValue::Handle(file)]) => {
                let old = self.grants.remove(file);
                let result = match (self.grants.get(directory).cloned(), old) {
                    (
                        Some(IoGrant::Dir { tree, root, .. }),
                        Some(IoGrant::File {
                            tree: old,
                            path,
                            identity,
                            position,
                            writable,
                            contents,
                            ..
                        }),
                    ) => {
                        let fits = tree == old
                            && !self.stale.contains(file)
                            && !self.detached_directories.contains(directory)
                            && (root.is_empty() || path.starts_with(&format!("{root}/")))
                            && self
                                .physical_path(*directory, &path)
                                .and_then(|p| self.files.get(&(tree, p)))
                                .is_some_and(|current| std::sync::Arc::ptr_eq(current, &identity));
                        if fits {
                            OpValue::Variant {
                                tag: 1,
                                value: Box::new(OpValue::OpenHandle(self.grant(IoGrant::File {
                                    tree,
                                    path,
                                    identity,
                                    directory: *directory,
                                    position,
                                    writable,
                                    contents,
                                }))),
                            }
                        } else {
                            OpValue::Variant {
                                tag: 0,
                                value: Box::new(OpValue::Unit),
                            }
                        }
                    }
                    _ => {
                        return Some(OpOutcome::Complete {
                            result: Err("CAP-2".into()),
                            consumed: vec![1],
                        });
                    }
                };
                return Some(OpOutcome::Complete {
                    result: Ok(result),
                    consumed: vec![1],
                });
            }
            (
                Some("file"),
                "commit" | "write_all",
                [OpValue::Handle(directory), OpValue::Handle(file), rest @ ..],
            ) => {
                let Some(IoGrant::Dir { tree, access, .. }) = self.grants.get(directory) else {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                };
                if *access == Access::Read {
                    return Some(OpOutcome::Value(io_not_granted()));
                }
                let tree = *tree;
                let Some(IoGrant::Atomic {
                    tree: owner,
                    directory: grant,
                    temporary,
                    destination,
                    identity,
                }) = self.grants.get(file).cloned()
                else {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                };
                if tree != owner || *directory != grant {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                }
                if self.stale.contains(file) {
                    if name == "commit" {
                        self.grants.remove(file);
                    }
                    return Some(OpOutcome::Complete {
                        result: Ok(io_error(1)),
                        consumed: if name == "commit" { vec![1] } else { vec![] },
                    });
                }
                let Some(temporary) = self.physical_path(*directory, &temporary) else {
                    if name == "commit" {
                        self.grants.remove(file);
                    }
                    return Some(OpOutcome::Complete {
                        result: Ok(io_portable(3)),
                        consumed: if name == "commit" { vec![1] } else { vec![] },
                    });
                };
                let Some(destination) = self.physical_path(*directory, &destination) else {
                    return Some(OpOutcome::Unsupported);
                };
                if name == "commit" {
                    if !self
                        .files
                        .get(&(tree, temporary.clone()))
                        .is_some_and(|current| std::sync::Arc::ptr_eq(current, &identity))
                    {
                        self.grants.remove(file);
                        self.trees.get_mut(&tree).unwrap().1.remove(&temporary);
                        let parent = temporary.rsplit_once('/').map_or("", |(p, _)| p);
                        let error = if self.trees[&tree].0.contains(parent) {
                            io_error(1)
                        } else {
                            io_portable(3)
                        };
                        self.files.remove(&(tree, temporary));
                        return Some(OpOutcome::Complete {
                            result: Ok(error),
                            consumed: vec![1],
                        });
                    }
                    let bytes = self.trees.get_mut(&tree).unwrap().1.remove(&temporary)?;
                    self.trees
                        .get_mut(&tree)
                        .unwrap()
                        .1
                        .insert(destination.clone(), bytes);
                    let identity = self
                        .files
                        .remove(&(tree, temporary))
                        .expect("atomic file identity");
                    self.permissions.remove(&(tree, destination.clone()));
                    self.files.insert((tree, destination), identity);
                    self.grants.remove(file);
                    return Some(OpOutcome::Complete {
                        result: Ok(io_ok(OpValue::Unit)),
                        consumed: vec![1],
                    });
                }
                let [OpValue::Bytes(bytes)] = rest else {
                    return Some(OpOutcome::Unsupported);
                };
                let start = self.trees[&tree].1.get(&temporary)?.len();
                self.durable_write(tree, &identity, start, bytes);
                self.trees
                    .get_mut(&tree)
                    .unwrap()
                    .1
                    .get_mut(&temporary)?
                    .extend_from_slice(bytes);
                self.update_files(&identity, bytes, true);
                io_ok(OpValue::Unit)
            }
            // The next delivered event, its paths relative to the watcher's Dir; with none
            // the task suspends and polls again (CONC-8 when nothing can deliver one).
            (Some("watcher"), "next", [OpValue::Handle(directory), OpValue::Handle(watcher)]) => {
                let Some(IoGrant::Dir { root, .. }) = self.grants.get(directory).cloned() else {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                };
                let Some(IoGrant::Watcher {
                    directory: owner,
                    queue,
                    ..
                }) = self.grants.get_mut(watcher)
                else {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                };
                if owner != directory {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                }
                let Some(event) = queue.pop() else {
                    return Some(OpOutcome::Pending);
                };
                use hirsel_rt::watch::WatchEvent::*;
                // The event inline: its tag, then its paths' identities.
                let words = match hirsel_rt::watch::relative_to(&root, event) {
                    Created(p) => [0, self.path(&p), 0],
                    Modified(p) => [1, self.path(&p), 0],
                    Removed(p) => [2, self.path(&p), 0],
                    Renamed(a, b) => [3, self.path(&a), self.path(&b)],
                    Overflow => [4, 0, 0],
                };
                io_ok(OpValue::Plain(
                    words.iter().flat_map(|w| w.to_le_bytes()).collect(),
                ))
            }
            // Delivered to the watchers of a `test.dir` tree that see it; an event whose
            // path leaves the Dir reaches none, nor does one on another tree.
            (
                Some("test"),
                "dir_event",
                [
                    OpValue::Handle(test),
                    OpValue::Handle(directory),
                    OpValue::Variant { tag, value },
                ],
            ) if matches!(self.grants.get(test), Some(IoGrant::Test)) => {
                let Some(IoGrant::Dir { tree, root, .. }) = self.grants.get(directory).cloned()
                else {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                };
                let path = |v: &OpValue| -> Option<String> {
                    let OpValue::Handle(p) = v else { return None };
                    self.paths.get(p.checked_sub(1)? as usize).cloned()
                };
                use hirsel_rt::watch::WatchEvent::*;
                let event = match (tag, value.as_ref()) {
                    (0, p) => Created(path(p)?),
                    (1, p) => Modified(path(p)?),
                    (2, p) => Removed(path(p)?),
                    (3, OpValue::Aggregate(pair)) if pair.len() == 2 => {
                        Renamed(path(&pair[0])?, path(&pair[1])?)
                    }
                    (4, _) => Overflow,
                    _ => return Some(OpOutcome::Unsupported),
                };
                if tree != 0
                    && let Some(event) =
                        hirsel_rt::watch::map_paths(&event, |p| io_relative(&root, p))
                {
                    for grant in self.grants.values_mut() {
                        if let IoGrant::Watcher {
                            tree: watched,
                            root,
                            recursive,
                            queue,
                            ..
                        } = grant
                            && *watched == tree
                            && let Some(seen) = hirsel_rt::watch::seen(root, *recursive, &event)
                        {
                            queue.push(seen);
                        }
                    }
                }
                OpValue::Unit
            }
            // `mapped.with`'s view: the identity's current bytes, zero past their end;
            // its check: 1 once the identity is found shorter than the mapping. A mapping
            // from before its tree's crash views zeros and checks 2 (FileFault(1)).
            (
                Some("$intrinsic"),
                view @ ("MappedView" | "MappedCheck"),
                [OpValue::Handle(directory), OpValue::Handle(mapped)],
            ) => {
                let Some(IoGrant::Mapped {
                    directory: owner,
                    contents,
                    length,
                    truncated,
                    ..
                }) = self.grants.get_mut(mapped)
                else {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                };
                if owner != directory {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                }
                let stale = self.stale.contains(mapped);
                if view == "MappedView" {
                    let mut bytes = if stale {
                        vec![]
                    } else {
                        contents[..contents.len().min(*length as usize)].to_vec()
                    };
                    bytes.resize(*length as usize, 0);
                    OpValue::ByteSequence(bytes)
                } else if stale {
                    OpValue::Bits {
                        scalar: ScalarKind::U32,
                        bits: 2,
                    }
                } else {
                    *truncated |= (contents.len() as u64) < *length;
                    OpValue::Bits {
                        scalar: ScalarKind::U32,
                        bits: u128::from(*truncated),
                    }
                }
            }
            (Some("file"), "walk", [OpValue::Handle(directory)]) => {
                io_ok(self.directory_iterator(*directory, "", true))
            }
            (
                Some("file"),
                kind @ ("open" | "create" | "open_append" | "size" | "read_bytes" | "write_bytes"
                | "remove" | "create_atomic" | "put_new" | "map" | "watch"),
                [OpValue::Handle(directory), OpValue::Handle(path), rest @ ..],
            ) => {
                let Some(IoGrant::Dir { tree, root, access }) = self.grants.get(directory).cloned()
                else {
                    return Some(OpOutcome::Fail("CAP-2".into()));
                };
                if access == Access::Read
                    && matches!(
                        kind,
                        "create"
                            | "open_append"
                            | "write_bytes"
                            | "remove"
                            | "create_atomic"
                            | "put_new"
                    )
                {
                    return Some(OpOutcome::Value(io_not_granted()));
                }
                let text = self.paths.get(path.checked_sub(1)? as usize)?;
                let Some(logical) = io_relative(&root, text) else {
                    return Some(OpOutcome::Value(io_error(1)));
                };
                let Some(path) = self.physical_path(*directory, &logical) else {
                    return Some(OpOutcome::Value(io_portable(3)));
                };
                let mut prefix = String::new();
                let parts: Vec<_> = path.split('/').filter(|p| !p.is_empty()).collect();
                for part in parts.iter().take(parts.len().saturating_sub(1)) {
                    if !prefix.is_empty() {
                        prefix.push('/');
                    }
                    prefix.push_str(part);
                    if !self.trees[&tree].0.contains(&prefix) {
                        return Some(OpOutcome::Value(io_portable(
                            if self.trees[&tree].1.contains_key(&prefix) {
                                7
                            } else {
                                3
                            },
                        )));
                    }
                }
                match kind {
                    "put_new" => {
                        let [OpValue::Bytes(bytes), OpValue::Variant { tag, .. }] = rest else {
                            return Some(OpOutcome::Unsupported);
                        };
                        let (bytes, durable) = (bytes.clone(), *tag == 0);
                        self.put_new(tree, &path, &bytes, durable)
                    }
                    "open" if self.trees[&tree].1.contains_key(&path) => {
                        io_ok(OpValue::OpenHandle(self.grant(IoGrant::File {
                            identity: self.files[&(tree, path.clone())].clone(),
                            directory: *directory,
                            position: 0,
                            writable: false,
                            contents: self.trees[&tree].1[&path].clone(),
                            tree,
                            path: logical,
                        })))
                    }
                    "watch"
                        if self.trees[&tree].1.contains_key(&path)
                            || self.trees[&tree].0.contains(&path) =>
                    {
                        let [
                            OpValue::Bits {
                                scalar: ScalarKind::Bool,
                                bits,
                            },
                        ] = rest
                        else {
                            return Some(OpOutcome::Unsupported);
                        };
                        io_ok(OpValue::OpenHandle(self.grant(IoGrant::Watcher {
                            tree,
                            directory: *directory,
                            root: logical,
                            recursive: *bits != 0,
                            queue: Default::default(),
                        })))
                    }
                    "map" if self.trees[&tree].1.contains_key(&path) => {
                        let contents = self.trees[&tree].1[&path].clone();
                        io_ok(OpValue::OpenHandle(self.grant(IoGrant::Mapped {
                            identity: self.files[&(tree, path.clone())].clone(),
                            directory: *directory,
                            length: contents.len() as u64,
                            truncated: false,
                            contents,
                        })))
                    }
                    "create" | "open_append" => {
                        let parent = path.rsplit_once('/').map_or("", |(p, _)| p);
                        if self.trees[&tree].0.contains(&path)
                            || !self.trees[&tree].0.contains(parent)
                        {
                            io_portable(if self.trees[&tree].0.contains(&path) {
                                6
                            } else {
                                3
                            })
                        } else {
                            let identity = self
                                .files
                                .entry((tree, path.clone()))
                                .or_insert_with(|| std::sync::Arc::new(()))
                                .clone();
                            let contents = if kind == "open_append" {
                                self.trees[&tree].1.get(&path).cloned().unwrap_or_default()
                            } else {
                                vec![]
                            };
                            self.trees
                                .get_mut(&tree)
                                .unwrap()
                                .1
                                .insert(path.clone(), contents.clone());
                            if kind == "create" {
                                self.update_files(&identity, &[], false);
                            }
                            io_ok(OpValue::OpenHandle(self.grant(IoGrant::File {
                                tree,
                                path: logical,
                                directory: *directory,
                                position: 0,
                                writable: true,
                                contents,
                                identity,
                            })))
                        }
                    }
                    "size" => self.trees[&tree]
                        .1
                        .get(&path)
                        .map(|bytes| {
                            io_ok(OpValue::Bits {
                                scalar: ScalarKind::U64,
                                bits: bytes.len() as u128,
                            })
                        })
                        .unwrap_or_else(|| {
                            io_portable(if self.trees[&tree].0.contains(&path) {
                                6
                            } else {
                                3
                            })
                        }),
                    "read_bytes" => self.trees[&tree]
                        .1
                        .get(&path)
                        .map(|bytes| io_ok(OpValue::ByteSequence(bytes.clone())))
                        .unwrap_or_else(|| {
                            io_portable(if self.trees[&tree].0.contains(&path) {
                                6
                            } else {
                                3
                            })
                        }),
                    "remove" => {
                        if self.trees.get_mut(&tree).unwrap().1.remove(&path).is_some() {
                            self.permissions.remove(&(tree, path.clone()));
                            self.files.remove(&(tree, path));
                            io_ok(OpValue::Unit)
                        } else {
                            io_portable(if self.trees[&tree].0.contains(&path) {
                                6
                            } else {
                                3
                            })
                        }
                    }
                    "write_bytes" | "create_atomic" => {
                        let parent = path.rsplit_once('/').map_or("", |(p, _)| p);
                        if self.trees[&tree].0.contains(&path)
                            || !self.trees[&tree].0.contains(parent)
                        {
                            io_portable(if self.trees[&tree].0.contains(&path) {
                                6
                            } else {
                                3
                            })
                        } else if kind == "write_bytes" {
                            let [OpValue::Bytes(bytes)] = rest else {
                                return Some(OpOutcome::Unsupported);
                            };
                            self.trees
                                .get_mut(&tree)
                                .unwrap()
                                .1
                                .insert(path.clone(), bytes.clone());
                            self.permissions.remove(&(tree, path.clone()));
                            self.file_reads.wrote(tree, &path);
                            let identity = std::sync::Arc::new(());
                            self.files.insert((tree, path), identity.clone());
                            self.durable_write(tree, &identity, 0, bytes);
                            io_ok(OpValue::Unit)
                        } else {
                            let mut serial = self.next;
                            let temporary = loop {
                                let p = format!("{path}.hirsel-tmp-{serial}");
                                if !self.trees[&tree].1.contains_key(&p)
                                    && !self.trees[&tree].0.contains(&p)
                                {
                                    break p;
                                }
                                serial += 1;
                            };
                            self.trees
                                .get_mut(&tree)
                                .unwrap()
                                .1
                                .insert(temporary.clone(), vec![]);
                            self.files
                                .insert((tree, temporary.clone()), std::sync::Arc::new(()));
                            io_ok(OpValue::OpenHandle(self.grant(IoGrant::Atomic {
                                tree,
                                directory: *directory,
                                temporary: format!("{logical}.hirsel-tmp-{serial}"),
                                destination: logical,
                                identity: self.files[&(tree, temporary.clone())].clone(),
                            })))
                        }
                    }
                    _ => io_portable(if self.trees[&tree].0.contains(&path) {
                        6
                    } else {
                        3
                    }),
                }
            }
            (Some("test"), "dir", [OpValue::Handle(test), OpValue::Sequence(entries)])
                if matches!(self.grants.get(test), Some(IoGrant::Test)) =>
            {
                let mut files = BTreeMap::new();
                let mut directories = BTreeSet::from([String::new()]);
                for entry in entries {
                    let OpValue::Aggregate(fields) = entry else {
                        return None;
                    };
                    let [OpValue::Handle(path), OpValue::Bytes(bytes)] = fields.as_slice() else {
                        return None;
                    };
                    let path = io_relative("", self.paths.get(path.checked_sub(1)? as usize)?)?;
                    let mut parent = path.as_str();
                    while let Some((p, _)) = parent.rsplit_once('/') {
                        directories.insert(p.into());
                        parent = p;
                    }
                    files.insert(path, bytes.clone());
                }
                if files.keys().any(|p| directories.contains(p)) {
                    return Some(OpOutcome::Unsupported);
                }
                let tree = self.trees.keys().next_back().copied().unwrap() + 1;
                for path in files.keys() {
                    self.files
                        .insert((tree, path.clone()), std::sync::Arc::new(()));
                }
                self.trees.insert(tree, (directories, files));
                self.durable_start(tree);
                OpValue::Handle(self.grant(IoGrant::Dir {
                    tree,
                    root: String::new(),
                    access: Access::ReadWrite,
                }))
            }
            (
                Some("test"),
                "crash",
                [
                    OpValue::Handle(test),
                    OpValue::Handle(directory),
                    OpValue::Bits {
                        scalar: ScalarKind::U64,
                        bits: seed,
                    },
                ],
            ) if matches!(self.grants.get(test), Some(IoGrant::Test)) => {
                return self.crash(*directory, *seed as u64);
            }
            (Some("file"), "sync", [OpValue::Handle(directory), OpValue::Handle(file)]) => {
                return self.sync(*directory, *file);
            }
            (
                Some("test"),
                "check",
                [
                    OpValue::Handle(test),
                    OpValue::Bits {
                        scalar: ScalarKind::Bool,
                        bits,
                    },
                ],
            ) if matches!(self.grants.get(test), Some(IoGrant::Test)) => {
                if *bits == 0 {
                    return Some(OpOutcome::Fail("LANG-9".into()));
                }
                OpValue::Unit
            }
            (Some("test"), "fail", [OpValue::Handle(test)])
                if matches!(self.grants.get(test), Some(IoGrant::Test)) =>
            {
                return Some(OpOutcome::Fail("LANG-9".into()));
            }
            _ => return None,
        };
        Some(OpOutcome::Value(value))
    }
}

/// Semantic text kernels return bytes or input-relative ranges; H owns every block.
fn text(module: Option<&str>, name: &str, args: &[OpValue]) -> Option<OpOutcome> {
    if module == Some("bytes")
        && let Some(outcome) = numeric::call(name, args)
    {
        return Some(outcome);
    }
    use hirsel_lir::layout_types::ScalarKind;
    let value = match (module, name, args) {
        (
            Some("bytes"),
            "replace_range",
            [
                OpValue::Elements {
                    scalar: ScalarKind::U8,
                    bits,
                },
                OpValue::Bits {
                    scalar: ScalarKind::U64,
                    bits: start,
                },
                OpValue::Bytes(bytes),
            ],
        ) => {
            let len = bits.len() as u128;
            if *start > len || bytes.len() as u128 > len - start {
                return Some(OpOutcome::Fail("MEM-14".into()));
            }
            if bytes.is_empty() {
                return Some(OpOutcome::Value(OpValue::Unit));
            }
            let mut bits = bits.clone();
            for (slot, byte) in bits[*start as usize..].iter_mut().zip(bytes) {
                *slot = u128::from(*byte);
            }
            return Some(OpOutcome::Updated {
                value: OpValue::Unit,
                arguments: vec![(
                    0,
                    OpValue::Elements {
                        scalar: ScalarKind::U8,
                        bits,
                    },
                )],
            });
        }
        (
            Some("bytes"),
            "append",
            [
                OpValue::Elements {
                    scalar: ScalarKind::U8,
                    bits,
                },
                OpValue::Bytes(bytes),
            ],
        ) => {
            if bytes.is_empty() {
                return Some(OpOutcome::Value(OpValue::Unit));
            }
            let mut bits = bits.clone();
            bits.extend(bytes.iter().map(|&byte| u128::from(byte)));
            return Some(OpOutcome::Updated {
                value: OpValue::Unit,
                arguments: vec![(
                    0,
                    OpValue::Elements {
                        scalar: ScalarKind::U8,
                        bits,
                    },
                )],
            });
        }
        (
            Some("bytes"),
            "append_from_history",
            [
                OpValue::Elements {
                    scalar: ScalarKind::U8,
                    bits,
                },
                OpValue::Bits {
                    scalar: ScalarKind::U64,
                    bits: distance,
                },
                OpValue::Bits {
                    scalar: ScalarKind::U64,
                    bits: count,
                },
            ],
        ) => {
            if *distance == 0
                || *distance > bits.len() as u128
                || *count > ((1u128 << 62) - 17).saturating_sub(bits.len() as u128)
            {
                return Some(OpOutcome::Fail("SEM-3".into()));
            }
            if *count == 0 {
                return Some(OpOutcome::Value(OpValue::Unit));
            }
            // Independent forward LZ model; H owns growth, COW and arena storage.
            let mut bits = bits.clone();
            for _ in 0..*count {
                let byte = bits[bits.len() - *distance as usize];
                bits.push(byte);
            }
            return Some(OpOutcome::Updated {
                value: OpValue::Unit,
                arguments: vec![(
                    0,
                    OpValue::Elements {
                        scalar: ScalarKind::U8,
                        bits,
                    },
                )],
            });
        }
        (
            Some("bytes"),
            "from_list",
            [
                OpValue::Elements {
                    scalar: ScalarKind::U8,
                    bits,
                },
            ],
        ) => OpValue::ByteSequence(bits.iter().map(|&b| b as u8).collect()),
        (
            Some("bytes"),
            "slice",
            [
                OpValue::Bytes(bytes),
                OpValue::Bits {
                    scalar: ScalarKind::U64,
                    bits: start,
                },
                OpValue::Bits {
                    scalar: ScalarKind::U64,
                    bits: end,
                },
            ],
        ) => {
            if start > end || *end > bytes.len() as u128 {
                return Some(OpOutcome::Fail("SEM-3".into()));
            }
            OpValue::Slice {
                argument: 0,
                start: *start as u64,
                length: (*end - *start) as u64,
            }
        }
        (
            Some("string"),
            "position",
            [
                OpValue::Bytes(bytes),
                OpValue::Bits {
                    scalar: ScalarKind::U64,
                    bits: offset,
                },
            ],
        ) => {
            let text = std::str::from_utf8(bytes).expect("valid string");
            let valid = usize::try_from(*offset)
                .ok()
                .is_some_and(|offset| text.is_char_boundary(offset));
            OpValue::Variant {
                tag: u32::from(valid),
                value: Box::new(if valid {
                    OpValue::Plain((*offset as u64).to_le_bytes().to_vec())
                } else {
                    OpValue::Unit
                }),
            }
        }
        (
            Some("string"),
            "byte_offset",
            [
                OpValue::Bytes(_),
                OpValue::Bits {
                    scalar: ScalarKind::U64,
                    bits,
                },
            ],
        ) => OpValue::Bits {
            scalar: ScalarKind::U64,
            bits: *bits,
        },
        (
            Some("bytes"),
            "find_any",
            [
                OpValue::Bytes(source),
                OpValue::Bytes(needles),
                OpValue::Bits {
                    scalar: ScalarKind::U64,
                    bits: start,
                },
            ],
        ) => {
            // Independent scalar oracle for the native byte-set scan.
            let offset = usize::try_from(*start).ok().and_then(|start| {
                source
                    .get(start..)?
                    .iter()
                    .position(|byte| needles.contains(byte))
                    .map(|at| start + at)
            });
            OpValue::Bits {
                scalar: ScalarKind::U64,
                bits: offset.unwrap_or(source.len()) as u128,
            }
        }
        (
            Some("bytes"),
            name @ ("base64_encode" | "base64_decode"),
            [
                OpValue::Bytes(bytes),
                OpValue::Variant { tag: alphabet, .. },
                OpValue::Variant { tag: padding, .. },
            ],
        ) => {
            use base64::Engine;
            let engine = match (alphabet, padding) {
                (0, 0) => base64::engine::general_purpose::STANDARD,
                (0, 1) => base64::engine::general_purpose::STANDARD_NO_PAD,
                (1, 0) => base64::engine::general_purpose::URL_SAFE,
                (1, 1) => base64::engine::general_purpose::URL_SAFE_NO_PAD,
                _ => return Some(OpOutcome::Unsupported),
            };
            if name == "base64_encode" {
                OpValue::ByteSequence(engine.encode(bytes).into_bytes())
            } else {
                match engine.decode(bytes) {
                    Ok(bytes) => OpValue::Variant {
                        tag: 1,
                        value: Box::new(OpValue::ByteSequence(bytes)),
                    },
                    Err(_) => OpValue::Variant {
                        tag: 0,
                        value: Box::new(OpValue::Unit),
                    },
                }
            }
        }
        (Some("bytes"), name, [OpValue::Bytes(bytes)])
            if hirsel_rt::kernels::sha::Algorithm::from_operation(name).is_some() =>
        {
            let algorithm = hirsel_rt::kernels::sha::Algorithm::from_operation(name).unwrap();
            let digest = hirsel_rt::kernels::sha::digest(algorithm, bytes, |_| {
                Ok::<_, std::convert::Infallible>(())
            })
            .unwrap();
            OpValue::Plain(digest[..algorithm.output_size()].to_vec())
        }
        (Some("string"), "from_bytes", [OpValue::Bytes(bytes)]) => OpValue::Variant {
            tag: u32::from(hirsel_rt::kernels::string_from_bytes(bytes.clone()).is_err()),
            value: Box::new(OpValue::Slice {
                argument: 0,
                start: 0,
                length: bytes.len() as u64,
            }),
        },
        (
            Some("string"),
            "slice",
            [
                OpValue::Bytes(bytes),
                OpValue::Bits {
                    scalar: ScalarKind::U64,
                    bits: from,
                },
                OpValue::Bits {
                    scalar: ScalarKind::U64,
                    bits: to,
                },
            ],
        ) => {
            let text = std::str::from_utf8(bytes).expect("SEM-3: String is valid UTF-8");
            let (Ok(from), Ok(to)) = (u64::try_from(*from), u64::try_from(*to)) else {
                return Some(OpOutcome::Fail("SEM-3".into()));
            };
            let Some(part) = hirsel_rt::kernels::string_slice(text, from, to) else {
                return Some(OpOutcome::Fail("SEM-3".into()));
            };
            OpValue::Slice {
                argument: 0,
                start: from,
                length: part.len() as u64,
            }
        }
        _ => return None,
    };
    Some(OpOutcome::Value(value))
}

/// Independent byte-level model; no production cursor/allocator/count helper runs here.
fn views(name: &str, args: &[OpValue]) -> Option<OpOutcome> {
    let (protocol, step) = name.rsplit_once('_')?;
    if !matches!(protocol, "UnicodeScalars" | "ScalarPositions") {
        return None;
    }
    if step == "open" {
        let [OpValue::Bytes(bytes)] = args else {
            return Some(OpOutcome::Unsupported);
        };
        return Some(OpOutcome::Value(OpValue::ViewCursor {
            bytes: bytes.clone(),
            position: 0,
        }));
    }
    let [OpValue::ViewCursor { bytes, position }] = args else {
        return Some(OpOutcome::Unsupported);
    };
    let text = std::str::from_utf8(bytes).expect("SEM-3: String is valid UTF-8");
    let mut next = *position;
    let value = match protocol {
        "UnicodeScalars" => {
            let mut words = vec![0u8; 8];
            if let Some(c) = text
                .get(*position as usize..)
                .and_then(|s| s.chars().next())
            {
                next += c.len_utf8() as u64;
                words[..4].copy_from_slice(&1u32.to_le_bytes());
                words[4..].copy_from_slice(&u32::from(c).to_le_bytes());
            }
            OpValue::Plain(words)
        }
        "ScalarPositions" => {
            let mut words = vec![0u8; 16];
            if *position <= bytes.len() as u64 {
                words[..4].copy_from_slice(&1u32.to_le_bytes());
                words[8..].copy_from_slice(&position.to_le_bytes());
                next += text[*position as usize..]
                    .chars()
                    .next()
                    .map_or(1, |c| c.len_utf8() as u64);
            }
            OpValue::Plain(words)
        }
        _ => unreachable!(),
    };
    Some(OpOutcome::Updated {
        value,
        arguments: vec![(
            0,
            OpValue::ViewCursor {
                bytes: bytes.clone(),
                position: next,
            },
        )],
    })
}

/// One audited heap-executor run.
pub struct HeapRun {
    /// Model filesystem after a single run; campaigns audit independent variants
    /// and do not retain their provider state. This is not physical heap evidence.
    pub files: Option<BTreeMap<String, Vec<u8>>>,
    /// Live model File/AtomicFile grants after a single run (None for campaigns).
    pub file_handles: Option<Vec<u64>>,
    pub observation: Observation,
    /// Failure-capable sites the run executed: the fault campaign's domain.
    pub sites: u64,
    /// Timer choice points the run executed: the early-firing domain.
    pub choices: u64,
    /// Instrumentation only; no check reads them.
    pub counters: Counters,
}

fn config(inject: Option<u64>, kind: &super::link::LaunchKind, world: &HeapWorld) -> Config {
    let default = Config::default();
    // A raised interpreter budget raises H's instruction limit by the same factor.
    let interpreter = hirsel_interp::Limits::default()
        .steps
        .expect("the default interpreter budget is bounded");
    let step_limit = world.steps.map_or(default.step_limit, |steps| {
        (default.step_limit.saturating_mul(steps) / interpreter).max(default.step_limit)
    });
    Config {
        inject,
        step_limit,
        context_handles: match kind {
            super::link::LaunchKind::Zero => vec![],
            super::link::LaunchKind::Test => vec![(IO_HANDLE, "Test".into())],
            super::link::LaunchKind::Main(roots) | super::link::LaunchKind::Call { roots, .. } => {
                roots
                    .iter()
                    .map(|root| (super::link::root_handle(*root), format!("{root:?}")))
                    .collect()
            }
        },
        ..default
    }
}

fn violation(
    entry: &DefPath,
    v: &hirsel_heapcheck::Violation,
    run: hirsel_heapcheck::Perturbation,
) -> NativeError {
    NativeError {
        definition: Some(entry.clone()),
        ..NativeError::new(StageName::Heap, format!("{:?}: {v}{run}", v.kind))
    }
}

fn observation(
    program: &ReadyProgram,
    audit: &hirsel_heapcheck::Audit,
    output: &[u8],
    sites: &Sites<'_>,
) -> Result<Observation, NativeError> {
    let o = &audit.observation;
    let exit_code = if !o.failures.is_empty() {
        None
    } else {
        fn code(value: &Observed) -> Option<i32> {
            match value {
                Observed::Unit => Some(0),
                Observed::Integer {
                    scalar: ScalarKind::U8,
                    bits,
                } => i32::try_from(*bits).ok(),
                Observed::Aggregate(fields) if fields.len() == 1 => code(fields[0].as_ref()?),
                Observed::Variant { tag: 0, fields } if fields.len() == 1 => {
                    code(fields[0].as_ref()?)
                }
                Observed::Variant { tag: 1, .. } => Some(1),
                _ => None,
            }
        }
        o.returned
            .as_ref()
            .map(|v| {
                code(v).ok_or_else(|| {
                    NativeError::new(StageName::Heap, format!("invalid entry return value {v:?}"))
                })
            })
            .transpose()?
    };
    Ok(Observation {
        exit_code,
        blocked: o.blocked,
        budget: None,
        signaled: o
            .signaled
            .map(|ordinal| {
                hirsel_run_schema::SignalKind::from_ordinal(ordinal).ok_or_else(|| {
                    NativeError::new(StageName::Heap, "a signal of no SignalKind ended the run")
                })
            })
            .transpose()?,
        output: output.to_vec(),
        failures: o
            .failures
            .iter()
            .map(|f| {
                let site = SiteRange {
                    source: f.source,
                    start: f.site.0,
                    end: f.site.1,
                };
                let mut failure = sites.failure(
                    &f.rule,
                    super::observe::source_definition(program, &f.definition),
                    site,
                )?;
                if let Some((definition, (start, end))) = &f.operation {
                    let operation = sites.failure(
                        &f.rule,
                        super::observe::source_definition(program, definition),
                        SiteRange {
                            source: f.operation_source,
                            start: *start,
                            end: *end,
                        },
                    )?;
                    failure.operation = Some(hirsel_interp::observation::SourceSite {
                        definition: operation.definition,
                        file: operation.file,
                        line: operation.line,
                        column: operation.column,
                    });
                }
                Ok(failure)
            })
            .collect::<Result<_, _>>()?,
        roots: Vec::new(),
    })
}

/// What a launch grants H's model beyond its entry's handles.
#[derive(Clone, Debug)]
pub struct HeapWorld {
    /// Shared bounded input stream, followed by EOF, as in SimulationConfig.
    pub stdin: Vec<u8>,
    pub args: Vec<String>,
    pub files: BTreeMap<String, Vec<u8>>,
    pub mounts: BTreeMap<String, Access>,
    pub file_reads: BTreeMap<String, hirsel_run_schema::FileReadScript>,
    /// Typed snapshot queues, cloned afresh for each run and campaign variant.
    pub file_faults: hirsel_run_schema::FileFaults,
    /// Deny stdout and stderr projection for a negative grant fixture.
    pub deny_output: bool,
    /// A monotonic clock (`io.monotonic_clock`), whose virtual reading starts here, as the
    /// deterministic simulation's does.
    pub monotonic: Option<u64>,
    /// A Main launch's `io.process` grant; only a scripted one (process.script@1) runs.
    pub process: Option<hirsel_run_schema::ProcessGrant>,
    /// A Main launch's `io.net` grant; only a scripted one (net.script@2, with peers) runs.
    pub net: Option<hirsel_run_schema::NetGrant>,
    /// A Main launch's `io.signals` grant; only a scripted one (signals.script@1) delivers.
    pub signals: Option<hirsel_run_schema::SignalsGrant>,
    /// A Main launch's Env snapshot; absent, the root Env grants no name.
    pub env: Option<std::collections::BTreeMap<String, Option<String>>>,
    /// A raised interpreter step budget; H's instruction limit scales with it.
    pub steps: Option<u64>,
}

impl Default for HeapWorld {
    fn default() -> Self {
        Self {
            stdin: vec![],
            args: vec![],
            files: Default::default(),
            mounts: hirsel_engine::SimulationConfig::default().mounts,
            file_reads: Default::default(),
            file_faults: Default::default(),
            deny_output: false,
            monotonic: None,
            process: None,
            net: None,
            signals: None,
            env: None,
            steps: None,
        }
    }
}

/// Run `entry` once and audit it.
pub fn run_heap(
    program: &ReadyProgram,
    entry: &DefPath,
    sites: &Sites<'_>,
) -> Result<HeapRun, NativeError> {
    run_heap_in(program, entry, sites, &HeapWorld::default())
}

/// Run `entry` once in `world` and audit it.
pub fn run_heap_in(
    program: &ReadyProgram,
    entry: &DefPath,
    sites: &Sites<'_>,
    world: &HeapWorld,
) -> Result<HeapRun, NativeError> {
    if matches!(
        program.launches.get(entry),
        Some(super::link::LaunchKind::Call { .. })
    ) {
        return Err(NativeError::new(
            StageName::Launch,
            "callee replay requires its canonical runner argument and reply protocol",
        ));
    }
    let heap = program.heap_program();
    let kind = program
        .launches
        .get(entry)
        .ok_or_else(|| NativeError::new(StageName::Launch, "no typed launch kind"))?;
    let args = match kind {
        super::link::LaunchKind::Zero => vec![],
        super::link::LaunchKind::Test => vec![Arg::Handle(IO_HANDLE)],
        super::link::LaunchKind::Main(roots) | super::link::LaunchKind::Call { roots, .. } => roots
            .iter()
            .map(|root| Arg::Handle(super::link::root_handle(*root)))
            .collect(),
    };
    let mut io = Io::for_launch(
        kind,
        program.launch.test_seed,
        program.launch.random_seed,
        world,
    )?;
    io.positions = positions(program, sites);
    let run = hirsel_heapcheck::run_ready(&heap, entry, &args, &mut io, &config(None, kind, world));
    let audit = run
        .result
        .as_ref()
        .map_err(|v| violation(entry, v, hirsel_heapcheck::Perturbation::Clean))?;
    Ok(HeapRun {
        files: Some(io.trees[&0].1.clone()),
        file_handles: Some(
            io.grants
                .iter()
                .filter_map(|(id, grant)| {
                    matches!(grant, IoGrant::File { .. } | IoGrant::Atomic { .. }).then_some(*id)
                })
                .collect(),
        ),
        observation: observation(program, audit, &run.output, sites)?,
        sites: run.sites,
        choices: run.choices,
        counters: run.counters,
    })
}

/// A clean run, then one run per failure-capable site with that site failed and one per
/// timer choice point with a deadline fired there: every one must pass the terminal audit.
/// Returns the clean run.
pub fn heap_campaign(
    program: &ReadyProgram,
    entry: &DefPath,
    sites: &Sites<'_>,
) -> Result<HeapRun, NativeError> {
    heap_campaign_in(program, entry, sites, &HeapWorld::default())
}

/// [`heap_campaign`] in `world`.
pub fn heap_campaign_in(
    program: &ReadyProgram,
    entry: &DefPath,
    sites: &Sites<'_>,
    world: &HeapWorld,
) -> Result<HeapRun, NativeError> {
    if matches!(
        program.launches.get(entry),
        Some(super::link::LaunchKind::Call { .. })
    ) {
        return Err(NativeError::new(
            StageName::Launch,
            "callee replay requires its canonical runner argument and reply protocol",
        ));
    }
    let heap = program.heap_program();
    let kind = program
        .launches
        .get(entry)
        .ok_or_else(|| NativeError::new(StageName::Launch, "no typed launch kind"))?;
    let args = match kind {
        super::link::LaunchKind::Zero => vec![],
        super::link::LaunchKind::Test => vec![Arg::Handle(IO_HANDLE)],
        super::link::LaunchKind::Main(roots) | super::link::LaunchKind::Call { roots, .. } => roots
            .iter()
            .map(|root| Arg::Handle(super::link::root_handle(*root)))
            .collect(),
    };
    let mut io = Io::for_launch(
        kind,
        program.launch.test_seed,
        program.launch.random_seed,
        world,
    )?;
    io.positions = positions(program, sites);
    let campaign = hirsel_heapcheck::audit_campaign_ready(
        &heap,
        entry,
        &args,
        &mut || Box::new(io.clone()),
        &config(None, kind, world),
    );
    if let Some((site, v)) = campaign.first_violation() {
        return Err(violation(entry, v, site));
    }
    let audit = campaign.clean.result.as_ref().expect("checked above");
    Ok(HeapRun {
        files: None,
        file_handles: None,
        observation: observation(program, audit, &campaign.clean.output, sites)?,
        sites: campaign.clean.sites,
        choices: campaign.clean.choices,
        counters: campaign.clean.counters,
    })
}

#[cfg(test)]
mod mount_rights {
    use super::*;

    fn value(io: &mut Io, module: &str, operation: &str, args: &[OpValue]) -> OpValue {
        match io
            .io(Some(module), operation, args)
            .expect("supported mount operation")
        {
            OpOutcome::Value(value) => value,
            _ => panic!("expected a Result value"),
        }
    }
    fn mounted(access: Access) -> (Io, u64) {
        let mut io = Io::new();
        io.mounts = BTreeMap::from([("work".into(), access)]);
        io.trees
            .get_mut(&0)
            .unwrap()
            .0
            .extend(["work".into(), "work/sub".into()]);
        let path = io.path("work");
        let OpValue::Variant {
            tag: 0,
            value: handle,
        } = value(
            &mut io,
            "io",
            "dir",
            &[OpValue::Handle(IO_HANDLE), OpValue::Handle(path)],
        )
        else {
            panic!("mount acquisition");
        };
        let OpValue::Handle(directory) = *handle else {
            panic!("directory grant");
        };
        (io, directory)
    }

    #[test]
    fn heap_mount_selection_denies_absent_selectors_with_an_authorized_twin() {
        let (mut io, directory) = mounted(Access::Read);
        for denied in ["", "other", "working"] {
            let path = io.path(denied);
            let before = io.next;
            assert_eq!(
                value(
                    &mut io,
                    "io",
                    "dir",
                    &[OpValue::Handle(IO_HANDLE), OpValue::Handle(path)]
                ),
                io_not_granted()
            );
            assert_eq!(io.next, before);
        }
        assert!(matches!(
            io.grants[&directory],
            IoGrant::Dir {
                access: Access::Read,
                ..
            }
        ));
        io.mounts = BTreeMap::from([("other".into(), Access::ReadWrite)]);
        let work = io.path("work");
        assert_eq!(
            value(
                &mut io,
                "io",
                "dir",
                &[OpValue::Handle(IO_HANDLE), OpValue::Handle(work)]
            ),
            io_not_granted()
        );
    }

    #[test]
    fn heap_narrow_inherits_read_rights_and_denies_mutation_before_effects() {
        for access in [Access::Read, Access::ReadWrite] {
            let (mut io, directory) = mounted(access);
            let sub = io.path("sub");
            let OpValue::Variant {
                tag: 0,
                value: handle,
            } = value(
                &mut io,
                "dir",
                "narrow",
                &[OpValue::Handle(directory), OpValue::Handle(sub)],
            )
            else {
                panic!();
            };
            let OpValue::Handle(narrowed) = *handle else {
                panic!();
            };
            let a = io.path("a");
            let outcome = value(
                &mut io,
                "file",
                "write_bytes",
                &[
                    OpValue::Handle(narrowed),
                    OpValue::Handle(a),
                    OpValue::Bytes(b"written".to_vec()),
                ],
            );
            assert_eq!(
                outcome,
                if access == Access::Read {
                    io_not_granted()
                } else {
                    io_ok(OpValue::Unit)
                }
            );
            assert_eq!(
                io.trees[&0].1.contains_key("work/sub/a"),
                access == Access::ReadWrite
            );
            for operation in ["remove", "create_atomic"] {
                let outcome = value(
                    &mut io,
                    "file",
                    operation,
                    &[OpValue::Handle(narrowed), OpValue::Handle(a)],
                );
                if access == Access::Read {
                    assert_eq!(outcome, io_not_granted());
                    assert!(io.trees[&0].1.is_empty());
                } else if let OpValue::Variant {
                    tag: 0,
                    value: file,
                } = outcome
                {
                    if let OpValue::OpenHandle(id) = *file {
                        io.close_handle(id, &mut vec![]).unwrap();
                    }
                } else {
                    panic!("writable twin");
                }
            }
        }
    }

    #[test]
    fn heap_file_narrow_requires_virtual_containment_even_for_shared_storage() {
        let (mut io, parent) = mounted(Access::ReadWrite);
        io.mounts.insert("child".into(), Access::Read);
        io.trees.get_mut(&0).unwrap().0.insert("child".into());
        let child = io.grant(IoGrant::Dir {
            tree: 0,
            root: "child".into(),
            access: Access::Read,
        });
        let sub = io.grant(IoGrant::Dir {
            tree: 0,
            root: "work/sub".into(),
            access: Access::ReadWrite,
        });
        // One identity accessible under physically overlapping mounts still has
        // distinct virtual names. The target selector must contain the old name.
        let identity = std::sync::Arc::new(());
        io.files.insert((0, "work/sub/a".into()), identity.clone());
        io.files.insert((0, "child/a".into()), identity.clone());
        for (target, expected_tag) in [(child, 0), (sub, 1), (parent, 1)] {
            let file = io.grant(IoGrant::File {
                tree: 0,
                path: "work/sub/a".into(),
                directory: parent,
                position: 0,
                writable: false,
                contents: vec![],
                identity: identity.clone(),
            });
            let Some(OpOutcome::Complete {
                result: Ok(OpValue::Variant { tag, value }),
                consumed,
            }) = io.io(
                Some("file"),
                "narrow",
                &[OpValue::Handle(target), OpValue::Handle(file)],
            )
            else {
                panic!("narrow completion");
            };
            assert_eq!(tag, expected_tag);
            assert_eq!(consumed, [1]);
            if let OpValue::OpenHandle(file) = *value {
                io.close_handle(file, &mut vec![]).unwrap();
            }
        }
    }

    #[test]
    fn heap_atomic_updates_deny_read_rights_with_a_writable_twin() {
        for access in [Access::Read, Access::ReadWrite] {
            let (mut io, directory) = mounted(access);
            let identity = std::sync::Arc::new(());
            io.trees
                .get_mut(&0)
                .unwrap()
                .1
                .insert("work/tmp".into(), vec![]);
            io.files.insert((0, "work/tmp".into()), identity.clone());
            let atomic = io.grant(IoGrant::Atomic {
                tree: 0,
                directory,
                temporary: "work/tmp".into(),
                destination: "work/done".into(),
                identity,
            });
            let outcome = value(
                &mut io,
                "file",
                "write_all",
                &[
                    OpValue::Handle(directory),
                    OpValue::Handle(atomic),
                    OpValue::Bytes(b"written".to_vec()),
                ],
            );
            assert_eq!(
                outcome,
                if access == Access::Read {
                    io_not_granted()
                } else {
                    io_ok(OpValue::Unit)
                }
            );
            assert_eq!(
                io.trees[&0].1["work/tmp"].as_slice(),
                if access == Access::Read {
                    b"".as_slice()
                } else {
                    b"written".as_slice()
                }
            );
            if access == Access::Read {
                assert_eq!(
                    value(
                        &mut io,
                        "file",
                        "commit",
                        &[OpValue::Handle(directory), OpValue::Handle(atomic)]
                    ),
                    io_not_granted()
                );
                assert!(io.grants.contains_key(&atomic));
            } else {
                assert!(matches!(
                    io.io(
                        Some("file"),
                        "commit",
                        &[OpValue::Handle(directory), OpValue::Handle(atomic)]
                    ),
                    Some(OpOutcome::Complete { result: Ok(_), .. })
                ));
                assert_eq!(io.trees[&0].1["work/done"], b"written");
            }
        }
    }
}

#[cfg(test)]
mod scoped_document;
