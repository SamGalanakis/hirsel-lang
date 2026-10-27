//! Logical host resource keys and their mode-aware epochs (design §6 Io).
//!
//! A [`Key`] names a host resource, never a host number. A [`Registry`] assigns one when a
//! resource first appears (a handle opens, a sink is bound, a grant is admitted) and uses
//! host data only for live alias discovery: an open file or sink whose `(dev, ino)` matches
//! an existing resource gets that resource's key. Replay reads keys from the tape and never
//! asks the registry.
//!
//! - **One namespace domain** [`NAMESPACE`] covers every mounted filesystem grant and every
//!   file-backed sink. Namespace mutations take it exclusively; every other filesystem request
//!   and every write to a file-backed sink takes it shared, plus its own resource key. So a
//!   truncation through one mount is ordered against a write through another mount to a hard
//!   link of the same inode, or against output to a redirected stdout file, with nothing
//!   resolved.
//! - **Sockets** take their own key; listen, bind and connect take their Net's port key.
//!   Split halves take distinct direction keys and the stream key shared; whole-stream
//!   operations take its key exclusively, including close and join.
//! - **Clocks, random, telemetry, test worlds:** one key per grant. **Env reads:** none.
//! - **Processes:** a spawn takes the process grant's key, the namespace exclusively and the
//!   run's sink keys, for the spawn call only; child stream ops and waits take the child's key.
//! - **Runner calls:** the place's key. **An adapter without keys:** [`KeySet::coupled`].
//!
//! [`KeyTable`] holds each key's epochs live. An exclusive transition is an epoch by itself;
//! consecutive shared transitions form one epoch, and a shared transition joins only a shared
//! latest epoch, so a queued exclusive transition is never overtaken. A transition of epoch e
//! makes its bounded host call once epoch e - 1 has ended, and ends its positions when the call
//! returns or registers its wait, so no key is held across a wait. The numbering is
//! `hirsel_free::keys::KeyEpochs`'s (the recorder's model); the live table keeps only the
//! epochs not yet ended, so a long run's memory does not grow with its request count.

use crate::io::Request;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex};

/// A logical resource identity.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Key(pub u32);

/// The run-wide namespace domain N.
pub const NAMESPACE: Key = Key(0);
/// The key of every request of an adapter that declares none, and of a handle no resource
/// was registered for: one conservative resource.
pub const COUPLED: Key = Key(1);
/// The run's root `Io` grant, which grant admissions read.
pub const ROOT: Key = Key(2);
/// The in-process place runner calls go to.
pub const PLACE: Key = Key(3);
pub(crate) const FIRST_RESOURCE: u32 = 4;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Mode {
    Shared,
    Exclusive,
}

/// The positions a transition takes, in key order; a key named twice keeps its strongest mode.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KeySet(Vec<(Key, Mode)>);

impl KeySet {
    pub fn new(mut keys: Vec<(Key, Mode)>) -> KeySet {
        keys.sort();
        // Exclusive sorts after Shared: keep the last entry of each key.
        let mut out: Vec<(Key, Mode)> = Vec::with_capacity(keys.len());
        for (key, mode) in keys {
            match out.last_mut() {
                Some(last) if last.0 == key => last.1 = last.1.max(mode),
                _ => out.push((key, mode)),
            }
        }
        KeySet(out)
    }
    /// An adapter without keys: one key for all its requests.
    pub fn coupled() -> KeySet {
        KeySet(vec![(COUPLED, Mode::Exclusive)])
    }
    pub fn keys(&self) -> &[(Key, Mode)] {
        &self.0
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn contains(&self, key: Key, mode: Mode) -> bool {
        self.0.contains(&(key, mode))
    }
}

/// What a registered handle names, for the requests that reach it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    /// An open file, mapping or watcher: its requests also take the namespace shared.
    File,
    /// A sink: with `file_backed` (a regular file, or storage of unknown aliasing) its writes
    /// also take the namespace shared.
    Sink { file_backed: bool },
    /// Any other resource: a stream, socket, listener, child, clock, random or test world.
    Other,
    /// A split direction: its own key serializes that direction, while shared access
    /// to the parent orders it against whole-stream close without coupling read/write.
    Half { stream: Key },
}

/// What a keyed recorder declared for one request event, so replay serves the
/// same resource graph the live run observed (determinism.md, Resource footprints):
/// the request's keys and auxiliary domains, and every handle identity established
/// since the previous request event, initial grants included, in handle order.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Footprint {
    pub keys: KeySet,
    pub domains: Vec<(Key, u64, String)>,
    pub identities: Vec<(u64, Key, Kind)>,
}

/// Assigns logical keys as resources appear, with live alias discovery.
#[derive(Debug)]
pub struct Registry {
    next: u32,
    handles: HashMap<u64, (Key, Kind)>,
    inodes: HashMap<(u64, u64), Key>,
    storages: HashMap<usize, Key>,
    /// Each Net's port key.
    ports: HashMap<Key, Key>,
    sinks: Vec<Key>,
    unknown_storage: bool,
    /// Handles whose identity was set since the last [`Registry::established`],
    /// while a recorder asks for them.
    log: Option<Vec<u64>>,
}

impl Default for Registry {
    fn default() -> Self {
        Registry {
            next: FIRST_RESOURCE,
            handles: HashMap::new(),
            inodes: HashMap::new(),
            storages: HashMap::new(),
            ports: HashMap::new(),
            sinks: Vec::new(),
            unknown_storage: false,
            log: None,
        }
    }
}

impl Registry {
    pub(crate) fn namespace_coupled(&self) -> bool {
        self.unknown_storage
    }
    /// Log identity changes for [`Self::established`] from now on.
    pub(crate) fn record(&mut self) {
        self.log.get_or_insert_with(Vec::new);
    }
    /// The identities set since the last call, in handle order, omitting handles
    /// since forgotten: a recorder's footprint identities.
    pub(crate) fn established(&mut self) -> Vec<(u64, Key, Kind)> {
        let mut handles = self.log.as_mut().map(std::mem::take).unwrap_or_default();
        handles.sort_unstable();
        handles.dedup();
        handles
            .into_iter()
            .filter_map(|handle| {
                let (key, kind) = self.identity(handle)?;
                Some((handle, key, kind))
            })
            .collect()
    }
    fn set(&mut self, handle: u64, row: (Key, Kind)) {
        self.handles.insert(handle, row);
        if let Some(log) = &mut self.log {
            log.push(handle);
        }
    }
    fn fresh(&mut self) -> Key {
        let key = Key(self.next);
        self.next = self
            .next
            .checked_add(1)
            .expect("logical key space exhausted");
        key
    }
    /// A new resource for `handle`.
    pub fn resource(&mut self, handle: u64, kind: Kind) -> Key {
        let key = self.fresh();
        self.set(handle, (key, kind));
        key
    }
    /// `handle` names the host object `(dev, ino)`: the key of the resource already holding
    /// that object, else a new one.
    pub fn inode(&mut self, handle: u64, kind: Kind, dev: u64, ino: u64) -> Key {
        let key = match self.inodes.get(&(dev, ino)) {
            Some(&key) => key,
            None => {
                let key = self.fresh();
                self.inodes.insert((dev, ino), key);
                key
            }
        };
        self.set(handle, (key, kind));
        key
    }
    /// Sink `handle` writes to `storage`: clones and wrappers of one storage share its key.
    pub fn sink(&mut self, handle: u64, storage: Storage) -> Key {
        let (key, file_backed) = match storage {
            Storage::Shared(id) => match self.storages.get(&id) {
                Some(&key) => (key, false),
                None => {
                    let key = self.fresh();
                    self.storages.insert(id, key);
                    (key, false)
                }
            },
            Storage::Descriptor { dev, ino, regular } => {
                let key = match self.inodes.get(&(dev, ino)) {
                    Some(&key) => key,
                    None => {
                        let key = self.fresh();
                        self.inodes.insert((dev, ino), key);
                        key
                    }
                };
                (key, regular)
            }
            Storage::Unknown => {
                self.unknown_storage = true;
                (COUPLED, true)
            }
        };
        self.set(handle, (key, Kind::Sink { file_backed }));
        if !self.sinks.contains(&key) {
            self.sinks.push(key);
        }
        key
    }
    /// Restore an already established logical identity, e.g. a joined stream.
    pub fn bind(&mut self, handle: u64, key: Key, kind: Kind) {
        self.next = self
            .next
            .max(key.0.checked_add(1).expect("logical key space exhausted"));
        self.set(handle, (key, kind));
    }
    /// `to` names the same resource as `from` (a narrowed file, an accepted stream's
    /// listener is not one): it keeps `from`'s key.
    pub fn alias(&mut self, from: u64, to: u64) {
        if let Some(&row) = self.handles.get(&from) {
            self.set(to, row);
        }
    }
    /// Register the two handles returned by a successful `NetSplit`. Like `resource`
    /// and `alias`, the adapter calls this when the handles appear, before publishing
    /// their completion. The original stream identity remains valid for close/join.
    pub fn split(&mut self, stream: u64, read: u64, write: u64) {
        let (parent, _) = self.handle(stream);
        self.resource(read, Kind::Half { stream: parent });
        self.resource(write, Kind::Half { stream: parent });
    }
    /// `handle` was closed. Its key stays valid for any alias.
    pub fn forget(&mut self, handle: u64) {
        self.handles.remove(&handle);
    }
    /// Checked live discovery for a created handle; unknown resources fail closed.
    pub fn identity(&self, handle: u64) -> Option<(Key, Kind)> {
        self.handles.get(&handle).copied()
    }
    /// A process issue touches the grant, namespace and every inherited stream.
    /// The caller has checked that `process` is a live process grant.
    pub fn process_keys(&self, process: u64) -> Option<KeySet> {
        let (key, Kind::Other) = self.identity(process)? else {
            return None;
        };
        let mut keys = vec![(key, Mode::Exclusive), (NAMESPACE, Mode::Exclusive)];
        keys.extend(self.sinks.iter().map(|&key| (key, Mode::Exclusive)));
        if self.unknown_storage {
            keys.push((COUPLED, Mode::Exclusive));
        }
        Some(KeySet::new(keys))
    }
    /// Stable admission order of inherited sink domains, independent of keys.
    pub(crate) fn inherited_domains(&self) -> Vec<Key> {
        self.sinks.clone()
    }
    fn handle(&self, handle: u64) -> (Key, Kind) {
        self.handles
            .get(&handle)
            .copied()
            .unwrap_or((COUPLED, Kind::Other))
    }
    /// Persist the existing world's port domain before its first effect.
    pub fn net_ports(&mut self, net: u64) -> Option<Key> {
        self.identity(net)?;
        Some(self.ports(net))
    }
    fn ports(&mut self, net: u64) -> Key {
        let net = self.handle(net).0;
        if let Some(&key) = self.ports.get(&net) {
            return key;
        }
        let key = self.fresh();
        self.ports.insert(net, key);
        key
    }

    /// The keys of `request`'s issue transition.
    pub fn keys(&mut self, request: &Request) -> KeySet {
        use Mode::{Exclusive as X, Shared as S};
        use Request as R;
        let mut keys = Vec::with_capacity(3);
        let on = |registry: &Registry, keys: &mut Vec<(Key, Mode)>, handle: u64, mode: Mode| {
            let (key, kind) = registry.handle(handle);
            keys.push((key, mode));
            if let Kind::Half { stream } = kind {
                keys.push((stream, S));
            }
            if matches!(kind, Kind::File | Kind::Sink { file_backed: true }) {
                keys.push((NAMESPACE, S));
            }
        };
        match request {
            R::Stdin { .. }
            | R::Stderr { .. }
            | R::Stdout { .. }
            | R::MonotonicClock { .. }
            | R::WallClock { .. }
            | R::Random { .. }
            | R::Tasks { .. }
            | R::Net { .. }
            | R::Process { .. }
            | R::Signals { .. }
            | R::Telemetry { .. } => keys.push((ROOT, S)),
            R::EnvGet { .. }
            | R::EnvNarrow { .. }
            | R::Ready { .. }
            | R::Schedule { .. }
            | R::CpuControl { .. }
            | R::ProcessTime { .. }
            | R::ProcessSettle { .. } => {}
            R::Print { stdout: h, .. } | R::PrintStderr { stderr: h, .. } => {
                on(self, &mut keys, *h, X)
            }
            R::ReadStdin { stdin, .. } => on(self, &mut keys, *stdin, X),
            R::Monotonic { clock } | R::Wall { clock } | R::SleepUntil { clock, .. } => {
                on(self, &mut keys, *clock, X)
            }
            R::RandomBytes { random, .. } => on(self, &mut keys, *random, X),
            // A wait consumes the pending arrival it returns from its Signals.
            R::SignalWait { signals, .. } => on(self, &mut keys, *signals, X),
            // A Telemetry's observations are ordered on it; its spans are its own state.
            R::TelemetryBegin { telemetry, .. }
            | R::TelemetryEvent { telemetry, .. }
            | R::TelemetryFinish { telemetry, .. }
            | R::TelemetryLog { telemetry, .. }
            | R::TelemetryMeasure { telemetry, .. } => on(self, &mut keys, *telemetry, X),
            R::TestProcess { test, .. }
            | R::TestCheck { test, .. }
            | R::TestCheckCase { test, .. }
            | R::TestFail { test }
            | R::TestClock { test }
            | R::TestSeed { test }
            | R::TestTasks { test }
            | R::TestLog { test, .. }
            | R::TestSnapshot { test, .. }
            | R::TestNet { test, .. } => on(self, &mut keys, *test, X),
            R::TestDir { test, .. } | R::TestCrash { test, .. } | R::TestDirEvent { test, .. } => {
                on(self, &mut keys, *test, X);
                keys.push((NAMESPACE, X));
            }
            R::ProcessNarrow { process, .. } => on(self, &mut keys, *process, S),
            R::ProcessRun { process, .. }
            | R::ProcessPipeline { process, .. }
            | R::ProcessSpawn { process, .. }
            | R::ProcessSpawnPipeline { process, .. } => {
                on(self, &mut keys, *process, X);
                keys.push((NAMESPACE, X));
                keys.extend(self.sinks.iter().map(|&key| (key, X)));
            }
            R::ChildWrite { child, .. }
            | R::ChildCloseStdin { child, .. }
            | R::ChildRead { child, .. }
            | R::ChildWait { child, .. }
            | R::ChildKill { child, .. } => on(self, &mut keys, *child, X),
            R::GroupWait { group, .. } => on(self, &mut keys, *group, X),
            R::Directory { .. }
            | R::NarrowDir { .. }
            | R::ReadOnlyDir { .. }
            | R::Open { .. }
            | R::ReadDirPage { .. }
            | R::Metadata { .. }
            | R::Size { .. }
            | R::Read { .. }
            | R::Map { .. }
            | R::Watch { .. } => keys.push((NAMESPACE, S)),
            R::OpenAppend { .. }
            | R::Create { .. }
            | R::Mkdir { .. }
            | R::Rename { .. }
            | R::RemoveDir { .. }
            | R::SetPermissions { .. }
            | R::RemoveAll { .. }
            | R::Write { .. }
            | R::Remove { .. }
            | R::CreateAtomic { .. }
            | R::PutNew { .. } => keys.push((NAMESPACE, X)),
            R::Commit { file, .. } => {
                on(self, &mut keys, *file, X);
                keys.push((NAMESPACE, X));
            }
            R::FileRead { file, .. }
            | R::FileSeek { file, .. }
            | R::FileWrite { file, .. }
            | R::FileAppend { file, .. }
            | R::NarrowFile { file, .. }
            | R::WriteAll { file, .. }
            | R::Sync { file, .. } => {
                on(self, &mut keys, *file, X);
                keys.push((NAMESPACE, S));
            }
            R::MapView { mapped, .. } | R::MapCheck { mapped, .. } => {
                on(self, &mut keys, *mapped, X);
                keys.push((NAMESPACE, S));
            }
            R::WatchNext { watcher, .. } => {
                on(self, &mut keys, *watcher, X);
                keys.push((NAMESPACE, S));
            }
            R::Close { handle } => on(self, &mut keys, *handle, X),
            R::NetNarrow { net, .. } | R::Resolve { net, .. } => on(self, &mut keys, *net, S),
            R::Connect { net, .. }
            | R::ConnectAddr { net, .. }
            | R::Listen { net, .. }
            | R::UdpBind { net, .. } => keys.push((self.ports(*net), X)),
            R::UnixConnect { net, .. } => {
                keys.push((self.ports(*net), X));
                keys.push((NAMESPACE, S));
            }
            R::UnixListen { net, .. } => {
                keys.push((self.ports(*net), X));
                keys.push((NAMESPACE, X));
            }
            R::NetRead { stream, .. }
            | R::NetWrite { stream, .. }
            | R::NetWriteAll { stream, .. }
            | R::Shutdown { stream, .. }
            | R::SetTimeout { stream, .. }
            | R::PeerAddr { stream, .. }
            | R::TlsConnect { stream, .. }
            | R::TlsAccept { stream, .. }
            | R::TlsPeer { stream, .. }
            | R::TlsProtocol { stream, .. }
            | R::TlsClose { stream, .. }
            | R::NetSplit { stream, .. } => on(self, &mut keys, *stream, X),
            R::NetJoin { read, write, .. } => {
                for half in [read, write] {
                    on(self, &mut keys, *half, X);
                    if let (_, Kind::Half { stream }) = self.handle(*half) {
                        keys.push((stream, X));
                    }
                }
            }
            R::UdpSend { socket, .. } | R::UdpRecv { socket, .. } => {
                on(self, &mut keys, *socket, X)
            }
            R::Accept { listener, .. } | R::LocalAddr { listener, .. } => {
                on(self, &mut keys, *listener, X)
            }
            R::RunnerCall(_)
            | R::RunnerPoll { .. }
            | R::RunnerCancel { .. }
            | R::CallReturn(_)
            | R::Stopped => keys.push((PLACE, X)),
        }
        if self.unknown_storage && keys.iter().any(|(key, _)| *key == NAMESPACE) {
            keys.push((COUPLED, X));
        }
        KeySet::new(keys)
    }
}

/// A sink's storage, for its key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Storage {
    /// In-memory storage shared by clones and wrappers: its identity.
    Shared(usize),
    /// A descriptor's host object; `regular` when it is a regular file.
    Descriptor { dev: u64, ino: u64, regular: bool },
    /// A custom writer whose aliasing is unknown: the conservative key, file-backed.
    Unknown,
}

impl Storage {
    /// The storage behind descriptor `fd`, or `Unknown` when it cannot be inspected.
    pub fn of_descriptor(fd: std::os::fd::RawFd) -> Storage {
        // SAFETY: fstat writes only the stat buffer; a closed fd is an error.
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(fd, &mut stat) } != 0 {
            return Storage::Unknown;
        }
        Storage::Descriptor {
            dev: stat.st_dev,
            ino: stat.st_ino,
            regular: stat.st_mode & libc::S_IFMT == libc::S_IFREG,
        }
    }
}

/// One live epoch of a key.
#[derive(Clone, Copy, Debug)]
struct Epoch {
    shared: bool,
    members: u32,
    ended: u32,
}

/// One key's unended epochs: `epochs[0]` is epoch `base`. The latest epoch is always kept,
/// so a shared transition can join it as `KeyEpochs::take` does.
#[derive(Debug, Default)]
struct Epochs {
    base: u64,
    epochs: VecDeque<Epoch>,
    /// Prepared transitions that have not taken their position yet; the queue keeps room
    /// for each, so a take inside a section never grows it.
    reserved: usize,
    /// [`KeyTable::wait`]ers blocked on this cell's condvar. An end that sees none
    /// skips the wake: a later waiter re-checks readiness under the same lock.
    waiters: usize,
}

impl Epochs {
    fn reserve(&mut self) {
        self.reserved += 1;
        self.epochs.reserve(self.reserved);
    }
    fn unreserve(&mut self) {
        self.reserved -= 1;
    }
    fn take(&mut self, shared: bool) -> u64 {
        self.unreserve();
        if self.base == 0 {
            self.base = 1;
        }
        match self.epochs.back_mut() {
            Some(last) if shared && last.shared => last.members += 1,
            _ => {
                assert!(
                    self.epochs.len() < self.epochs.capacity(),
                    "a key's epoch was reserved before its take"
                );
                self.epochs.push_back(Epoch {
                    shared,
                    members: 1,
                    ended: 0,
                })
            }
        }
        self.base + self.epochs.len() as u64 - 1
    }
    fn ready(&self, epoch: u64) -> bool {
        if epoch <= self.base {
            return true;
        }
        let previous = &self.epochs[(epoch - 1 - self.base) as usize];
        previous.ended == previous.members
    }
    fn end(&mut self, epoch: u64) {
        let e = &mut self.epochs[(epoch - self.base) as usize];
        assert!(
            e.ended < e.members,
            "an epoch ended more often than it has members"
        );
        e.ended += 1;
        while self.epochs.len() > 1 && self.epochs[0].ended == self.epochs[0].members {
            self.epochs.pop_front();
            self.base += 1;
        }
    }
}

#[derive(Debug, Default)]
struct Cell {
    epochs: Mutex<Epochs>,
    ended: Condvar,
}

/// Every key's live epochs.
#[derive(Debug, Default)]
pub struct KeyTable {
    cells: Mutex<HashMap<Key, Arc<Cell>>>,
    /// Serialises takes, so a multi-key footprint is taken atomically. Only bounded takes
    /// hold it, never an allocating [`KeyTable::prepare`].
    order: Mutex<()>,
    metrics: Option<Arc<crate::io_metrics::IoMetrics>>,
}

/// A transition's positions: one epoch per key of its [`KeySet`], in key order.
#[derive(Debug)]
pub struct Positions {
    held: Vec<(Key, Mode, u64, Arc<Cell>)>,
    state: State,
    /// Some cell had a blocked waiter when these positions ended.
    wake: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    /// Cells resolved and room reserved; no epoch taken yet.
    Prepared,
    Taken,
    /// Every epoch ended; waiters are woken when the positions drop.
    Ended,
}

impl KeyTable {
    pub fn with_metrics(metrics: Option<Arc<crate::io_metrics::IoMetrics>>) -> KeyTable {
        KeyTable {
            metrics,
            ..KeyTable::default()
        }
    }
    /// Resolve `keys`' cells and reserve a position on each, outside any section: every
    /// allocation [`take_prepared`](Self::take_prepared) would need happens here.
    pub fn prepare(&self, keys: &KeySet) -> Positions {
        let mut cells = self.cells.lock().unwrap();
        let held = keys
            .keys()
            .iter()
            .map(|&(key, mode)| {
                let cell = cells.entry(key).or_default().clone();
                cell.epochs.lock().unwrap().reserve();
                (key, mode, 0, cell)
            })
            .collect();
        Positions {
            held,
            state: State::Prepared,
            wake: false,
        }
    }
    /// Take prepared positions, in key order (a request's entered phase). It allocates
    /// nothing, so it may run inside a section.
    pub fn take_prepared(&self, positions: &mut Positions) {
        assert_eq!(positions.state, State::Prepared, "positions are taken once");
        // Taking a multi-key footprint must be atomic. Otherwise a shared namespace
        // member can acquire its resource epoch after a namespace writer that waits
        // for that same member, forming a dependency cycle.
        let _order = self.order.lock().unwrap();
        for (_, mode, epoch, cell) in &mut positions.held {
            *epoch = cell.epochs.lock().unwrap().take(*mode == Mode::Shared);
        }
        positions.state = State::Taken;
    }
    /// Prepare and take `keys`' positions, outside sections.
    pub fn take(&self, keys: &KeySet) -> Positions {
        let mut positions = self.prepare(keys);
        self.take_prepared(&mut positions);
        positions
    }
    /// Wait until every epoch `positions` depend on has ended (outside sections; a task
    /// executor parks instead and asks [`Positions::ready`]).
    pub fn wait(&self, positions: &Positions) {
        for (key, _, epoch, cell) in &positions.held {
            let mut epochs = cell.epochs.lock().unwrap();
            if epochs.ready(*epoch) {
                continue;
            }
            let start = self.metrics.as_ref().map(|_| std::time::Instant::now());
            epochs.waiters += 1;
            while !epochs.ready(*epoch) {
                epochs = cell.ended.wait(epochs).unwrap();
            }
            epochs.waiters -= 1;
            if let (Some(metrics), Some(start)) = (&self.metrics, start) {
                metrics.key_wait(*key == NAMESPACE, start.elapsed());
            }
        }
    }
}

impl Positions {
    /// Whether the transition may make its host call now.
    pub fn ready(&self) -> bool {
        self.held
            .iter()
            .all(|(_, _, epoch, cell)| cell.epochs.lock().unwrap().ready(*epoch))
    }
    /// Each position as (key, mode, epoch), for the tape.
    pub fn epochs(&self) -> impl Iterator<Item = (Key, Mode, u64)> + '_ {
        self.held
            .iter()
            .map(|(key, mode, epoch, _)| (*key, *mode, *epoch))
    }
    /// The bounded call returned or registered its wait: end every position and wake
    /// blocked waiters.
    pub fn end(mut self) {
        self.end_epochs();
    }
    /// End every taken position without waking anyone or freeing anything, so it may run
    /// inside a section; dropping the positions afterwards wakes [`KeyTable::wait`]ers.
    pub fn end_epochs(&mut self) {
        assert_eq!(self.state, State::Taken, "taken positions end once");
        for (_, _, epoch, cell) in &self.held {
            let mut epochs = cell.epochs.lock().unwrap();
            epochs.end(*epoch);
            self.wake |= epochs.waiters != 0;
        }
        self.state = State::Ended;
    }
}

impl Drop for Positions {
    /// A transition abandoned before its call (withdrawn, or its task stopped) still ends its
    /// positions, so no later transition waits for it; one abandoned before its take
    /// returns its reserved room.
    fn drop(&mut self) {
        match self.state {
            State::Prepared => {
                for (_, _, _, cell) in &self.held {
                    cell.epochs.lock().unwrap().unreserve();
                }
                return;
            }
            State::Taken => self.end_epochs(),
            State::Ended => {}
        }
        if !self.wake {
            return;
        }
        for (_, _, _, cell) in &self.held {
            cell.ended.notify_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::Request as R;
    use hirsel_free::keys::KeyEpochs;

    #[test]
    fn live_epochs_number_and_order_as_the_recorder_model() {
        // A seeded walk of takes and ends: the compacting table answers every readiness
        // question as hirsel-free's full model does.
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..200 {
            let mut live = Epochs::default();
            let mut model = KeyEpochs::default();
            let mut open: Vec<u64> = Vec::new();
            for _ in 0..60 {
                if open.is_empty() || next() % 3 != 0 {
                    let shared = next() % 2 == 0;
                    live.reserve();
                    let epoch = live.take(shared);
                    assert_eq!(epoch, model.take(shared));
                    open.push(epoch);
                } else {
                    // End a ready transition, as only a ready one makes its call.
                    let ready: Vec<usize> =
                        (0..open.len()).filter(|&i| model.ready(open[i])).collect();
                    let i = ready[(next() % ready.len() as u64) as usize];
                    let epoch = open.swap_remove(i);
                    live.end(epoch);
                    model.end(epoch);
                }
                for &epoch in &open {
                    assert_eq!(live.ready(epoch), model.ready(epoch), "epoch {epoch}");
                }
            }
            assert!(live.epochs.len() <= open.len().max(1) + 1);
        }
    }

    #[test]
    fn an_abandoned_transition_ends_its_positions() {
        let table = KeyTable::default();
        let keys = KeySet::new(vec![(Key(9), Mode::Exclusive)]);
        let first = table.take(&keys);
        let second = table.take(&keys);
        drop(first);
        assert!(second.ready());
    }

    fn registry() -> Registry {
        Registry::default()
    }

    #[test]
    fn requests_on_two_open_files_of_one_mount_share_only_the_namespace_epoch() {
        let mut keys = registry();
        let a = keys.inode(10, Kind::File, 1, 100);
        let b = keys.inode(11, Kind::File, 1, 101);
        assert_ne!(a, b);
        let write = |file| R::FileWrite {
            directory: 5,
            file,
            bytes: vec![1],
        };
        let table = KeyTable::default();
        let first = table.take(&keys.keys(&write(10)));
        let second = table.take(&keys.keys(&write(11)));
        assert!(
            first.ready() && second.ready(),
            "independent files run concurrently"
        );
        let same = table.take(&keys.keys(&write(10)));
        assert!(!same.ready(), "one file's writes stay ordered");
    }

    #[test]
    fn create_or_truncate_waits_for_an_open_writer_and_identical_roots_share_a_resource() {
        let mut keys = registry();
        // Two virtual mounts behind one physical directory: the same inode, one resource.
        let through_a = keys.inode(10, Kind::File, 7, 42);
        let through_b = keys.inode(11, Kind::File, 7, 42);
        assert_eq!(through_a, through_b);
        let table = KeyTable::default();
        let write = table.take(&keys.keys(&R::FileWrite {
            directory: 5,
            file: 10,
            bytes: vec![1],
        }));
        let truncate = table.take(&keys.keys(&R::Create {
            directory: 6,
            path: "f".into(),
        }));
        assert!(write.ready() && !truncate.ready());
        write.end();
        assert!(truncate.ready());
    }

    #[test]
    fn a_hard_link_across_mount_roots_is_ordered_by_the_namespace_domain() {
        // A truncation through mount A by path (no handle, nothing resolved) against a write
        // through mount B to a hard link of the same inode: ordered by N alone.
        let mut keys = registry();
        keys.inode(20, Kind::File, 3, 9);
        let truncate = keys.keys(&R::Write {
            directory: 1,
            path: "link".into(),
            bytes: vec![],
        });
        let write = keys.keys(&R::FileAppend {
            directory: 2,
            file: 20,
            bytes: vec![0],
        });
        assert!(truncate.contains(NAMESPACE, Mode::Exclusive));
        assert!(write.contains(NAMESPACE, Mode::Shared));
    }

    #[test]
    fn redirected_stdout_to_a_file_is_ordered_against_its_truncation() {
        let dir_scratch = tempfile::Builder::new()
            .prefix("hirsel-keys-")
            .tempdir()
            .unwrap();
        let dir = dir_scratch.path().to_path_buf();
        let file = std::fs::File::create(dir.join("out")).unwrap();
        use std::os::fd::AsRawFd;
        // fds 1 and 2 redirected to one file share its key (fd-aliased redirection).
        let other = file.try_clone().unwrap();
        let mut keys = registry();
        let stdout = keys.sink(1, Storage::of_descriptor(file.as_raw_fd()));
        let stderr = keys.sink(2, Storage::of_descriptor(other.as_raw_fd()));
        assert_eq!(stdout, stderr);
        let print = keys.keys(&R::Print {
            stdout: 1,
            bytes: b"x".to_vec(),
        });
        assert!(print.contains(stdout, Mode::Exclusive));
        assert!(print.contains(NAMESPACE, Mode::Shared));
        let table = KeyTable::default();
        let written = table.take(&print);
        let truncate = table.take(&keys.keys(&R::Create {
            directory: 3,
            path: "out".into(),
        }));
        assert!(!truncate.ready());
        written.end();
        assert!(truncate.ready());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn in_memory_sinks_share_a_key_only_with_their_own_storage() {
        let mut keys = registry();
        let one = crate::host::SendSink::capture().0;
        let wrapper = one.clone();
        let two = crate::host::SendSink::capture().0;
        let a = keys.sink(1, Storage::Shared(one.storage()));
        let b = keys.sink(2, Storage::Shared(wrapper.storage()));
        let c = keys.sink(3, Storage::Shared(two.storage()));
        assert_eq!(a, b);
        assert_ne!(a, c);
        // In-memory storage is not file-backed: no namespace position.
        let print = keys.keys(&R::Print {
            stdout: 1,
            bytes: vec![],
        });
        assert_eq!(print.keys(), &[(a, Mode::Exclusive)]);
        // A custom writer of unknown aliasing is conservative and file-backed.
        let custom = keys.sink(4, Storage::Unknown);
        assert_eq!(custom, COUPLED);
        assert!(
            keys.keys(&R::PrintStderr {
                stderr: 4,
                bytes: vec![]
            })
            .contains(NAMESPACE, Mode::Shared)
        );
    }

    #[test]
    fn a_spawn_takes_the_namespace_and_sinks_but_a_child_wait_holds_only_its_child() {
        let mut keys = registry();
        let stdout = keys.sink(1, Storage::Shared(1));
        let process = keys.resource(7, Kind::Other);
        let child = keys.resource(8, Kind::Other);
        let spawn = keys.keys(&R::ProcessSpawn {
            process: 7,
            command: hirsel_run_schema::Command {
                program: "p".into(),
                args: vec![],
                directory: None,
                env: None,
                own_group: false,
            },
        });
        assert!(spawn.contains(process, Mode::Exclusive));
        assert!(spawn.contains(NAMESPACE, Mode::Exclusive));
        assert!(spawn.contains(stdout, Mode::Exclusive));
        let wait = keys.keys(&R::ChildWait {
            process: 7,
            child: 8,
            deadline: None,
        });
        assert_eq!(wait.keys(), &[(child, Mode::Exclusive)]);
    }

    #[test]
    fn split_halves_have_independent_keys_and_whole_stream_close_waits_for_both() {
        let mut keys = registry();
        let parent = keys.resource(10, Kind::Other);
        // The split's handles must be registered as directions of their stream.
        keys.split(10, 11, 12);
        let read_keys = keys.keys(&R::NetRead {
            net: 4,
            stream: 11,
            max: 1,
        });
        let write_keys = keys.keys(&R::NetWriteAll {
            net: 4,
            stream: 12,
            bytes: vec![1],
        });
        assert_ne!(
            read_keys, write_keys,
            "opposite directions must not serialize"
        );
        assert!(!read_keys.contains(COUPLED, Mode::Exclusive));
        assert!(read_keys.contains(parent, Mode::Shared));
        let table = KeyTable::default();
        let read = table.take(&read_keys);
        let write = table.take(&write_keys);
        assert!(read.ready() && write.ready());
        let another_read = table.take(&read_keys);
        assert!(!another_read.ready(), "one direction stays ordered");
        read.end();
        assert!(another_read.ready());
        let close = table.take(&keys.keys(&R::Close { handle: 10 }));
        assert!(!close.ready());
        another_read.end();
        assert!(!close.ready(), "the writer still uses the stream");
        write.end();
        assert!(close.ready());
    }

    #[test]
    fn split_join_excludes_both_directions_and_half_aliases_keep_the_parent() {
        let mut keys = registry();
        let parent = keys.resource(10, Kind::Other);
        keys.split(10, 11, 12);
        keys.alias(11, 13);
        let read = keys.keys(&R::NetRead {
            net: 4,
            stream: 11,
            max: 1,
        });
        let alias = keys.keys(&R::NetRead {
            net: 4,
            stream: 13,
            max: 1,
        });
        assert_eq!(read, alias);
        let join_keys = keys.keys(&R::NetJoin {
            net: 4,
            read: 11,
            write: 12,
        });
        assert!(join_keys.contains(parent, Mode::Exclusive));
        let table = KeyTable::default();
        let first = table.take(&read);
        let join = table.take(&join_keys);
        let later_write = table.take(&keys.keys(&R::NetWriteAll {
            net: 4,
            stream: 12,
            bytes: vec![],
        }));
        assert!(first.ready());
        assert!(!join.ready() && !later_write.ready());
        first.end();
        assert!(join.ready() && !later_write.ready());
        join.end();
        assert!(later_write.ready());
        // Forgetting a half does not remove its live alias's close dependency.
        keys.forget(11);
        assert_eq!(
            keys.keys(&R::NetRead {
                net: 4,
                stream: 13,
                max: 1
            }),
            read
        );
    }

    #[test]
    fn listen_bind_and_connect_take_their_nets_port_key() {
        let mut keys = registry();
        let addr = crate::net::SocketAddr {
            ip: std::net::Ipv4Addr::LOCALHOST.into(),
            port: 80,
        };
        let listen = keys.keys(&R::Listen { net: 4, addr });
        let bind = keys.keys(&R::UdpBind { net: 4, addr });
        assert_eq!(listen, bind);
        assert_eq!(listen.keys().len(), 1);
        assert_eq!(
            keys.keys(&R::EnvGet {
                env: 1,
                name: "X".into()
            }),
            KeySet::default()
        );
    }
}
