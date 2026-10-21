//! The native object cache's key and store seam (design §1e, §3).
//!
//! A `UnitKey` hashes everything a unit object's bytes depend on: compiler, LIR,
//! validator, backend and ISA identity, target and opt level, the code-affecting native
//! options, the unit's instance, role, origin and linkage, its TIR digest and
//! implementation revision, the `LayoutKey`s and FnAbi keys it reaches, each callee's
//! symbol and ABI digest, its `CodecKey`s, `debug` and `measurement` (with the resolved
//! source locations under measurement), the manifest rows it uses and the finalizer
//! facts it reads. Units never contain a number the program assigns (`crate::glue`), so
//! one key names the same bytes in every program.
//!
//! An `ObjectStore` maps keys to stored unit fragments. Implementations (`none@1`,
//! `dir@1`, `sim@1`; `objects_by_name`) are selected by the host's run config; none
//! changes program meaning, only what is recompiled. The heap checker and dump tooling
//! use `none@1`.
use crate::abi_types::FnAbiId;
use crate::emission::EmitInputs;
use crate::ir::{Inst, LirUnit};
use crate::layout_types::LayoutId;
use crate::stage::Ready;
use hirsel_syntax::ContentHash;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

/// The content key of one planned unit's object.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct UnitKey(pub ContentHash);

impl hirsel_codec::Snapshot for UnitKey {
    fn write(&self, w: &mut hirsel_codec::Writer) {
        self.0.write(w);
    }
    fn read(r: &mut hirsel_codec::Reader<'_>) -> Result<Self, hirsel_codec::DecodeError> {
        Ok(Self(ContentHash::read(r)?))
    }
}

impl UnitKey {
    /// The key's lowercase hex digits, as stores name entries.
    pub fn hex(&self) -> String {
        self.0.0.iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// Stored unit fragments by `UnitKey`. A `get` that cannot produce intact bytes is a
/// miss; a `put` that cannot store is dropped. Neither is ever an error the build sees.
pub trait ObjectStore: Send + Sync {
    /// Whether writes can be retained; none avoids preparing import-cache entries.
    fn enabled(&self) -> bool {
        true
    }
    fn get(&self, key: &UnitKey) -> Option<Arc<[u8]>>;
    fn put(&self, key: &UnitKey, value: &[u8]);
}

/// A writable cache with a read-only toolchain seed as fallback. Hits never copy
/// seed entries into the writable store; misses are written only to `writable`.
pub struct SeededObjects {
    pub writable: Arc<dyn ObjectStore>,
    pub seed: Arc<dyn ObjectStore>,
}

impl ObjectStore for SeededObjects {
    fn enabled(&self) -> bool {
        self.writable.enabled() || self.seed.enabled()
    }
    fn get(&self, key: &UnitKey) -> Option<Arc<[u8]>> {
        self.writable.get(key).or_else(|| self.seed.get(key))
    }
    fn put(&self, key: &UnitKey, value: &[u8]) {
        self.writable.put(key, value);
    }
}

/// A toolchain object partition. It never refreshes mtimes, removes corrupt
/// entries or writes files, so a shared installation can be read-only.
pub struct ReadOnlyObjects(pub PathBuf);
impl ObjectStore for ReadOnlyObjects {
    fn get(&self, key: &UnitKey) -> Option<Arc<[u8]>> {
        let hex = key.hex();
        let bytes = fs::read(self.0.join(&hex[..2]).join(&hex)).ok()?;
        Some(unframe(&bytes)?.into())
    }
    fn put(&self, _: &UnitKey, _: &[u8]) {}
}

/// The store format version in `dir@1`'s directory name.
pub const FORMAT: u32 = 1;

/// The `dir@1` partition of one target, backend and opt level: objects of different
/// partitions never share a directory, so a key collision across them is impossible.
pub fn partition(target: &str, backend: &str, opt: &str) -> String {
    format!("{target}-{backend}-{opt}")
}

/// `none@1`: stores nothing.
#[derive(Debug, Default)]
pub struct NoObjects;
impl ObjectStore for NoObjects {
    fn enabled(&self) -> bool {
        false
    }
    fn get(&self, _: &UnitKey) -> Option<Arc<[u8]>> {
        None
    }
    fn put(&self, _: &UnitKey, _: &[u8]) {}
}

/// The default size cap of `dir@1`.
pub const DEFAULT_CAP: u64 = 4 << 30;
/// After this many bytes written in a process, `dir@1` enforces its cap.
const GC_EVERY: u64 = 64 << 20;

/// `dir@1`: one checksummed file per unit under
/// `$XDG_CACHE_HOME/hirsel/objects-v<N>/<partition>`. Writes go to a temp file renamed
/// into place, so readers never see a partial object and no lock is taken. A size cap
/// is enforced by evicting the least recently used objects by mtime; a hit refreshes
/// its object's mtime.
#[derive(Debug)]
pub struct DirObjects {
    root: PathBuf,
    cap: u64,
    written: AtomicU64,
    temp: AtomicU64,
}

/// What a collection found and removed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GcReport {
    pub entries: u64,
    pub bytes: u64,
    pub removed: u64,
    pub removed_bytes: u64,
}

impl DirObjects {
    /// The store at `root`, the directory of one partition.
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            cap: DEFAULT_CAP,
            written: AtomicU64::new(0),
            temp: AtomicU64::new(0),
        }
    }
    pub fn with_cap(mut self, cap: u64) -> Self {
        self.cap = cap;
        self
    }
    /// `$XDG_CACHE_HOME/hirsel/objects-v<N>/<partition>`, else under `$HOME/.cache`.
    pub fn default_location(partition: &str) -> Option<Self> {
        let base = std::env::var_os("XDG_CACHE_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
        Some(Self::new(
            base.join("hirsel")
                .join(format!("objects-v{FORMAT}"))
                .join(partition),
        ))
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    fn path(&self, key: &UnitKey) -> PathBuf {
        let hex = key.hex();
        self.root.join(&hex[..2]).join(&hex)
    }

    /// Evicts least recently used objects until the store is within `cap`, and removes
    /// temp files a crashed writer left behind.
    pub fn gc(&self, cap: u64) -> GcReport {
        let mut report = GcReport::default();
        let mut files: Vec<(SystemTime, u64, PathBuf)> = vec![];
        let Ok(shards) = fs::read_dir(&self.root) else {
            return report;
        };
        for shard in shards.flatten() {
            let shard = shard.path();
            if shard.file_name().is_some_and(|n| n == "tmp") {
                for stale in fs::read_dir(&shard).into_iter().flatten().flatten() {
                    let old = stale
                        .metadata()
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|age| age.as_secs() > 3600);
                    if old {
                        let _ = fs::remove_file(stale.path());
                    }
                }
                continue;
            }
            for file in fs::read_dir(&shard).into_iter().flatten().flatten() {
                if let Ok(meta) = file.metadata()
                    && meta.is_file()
                {
                    let time = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                    files.push((time, meta.len(), file.path()));
                }
            }
        }
        report.entries = files.len() as u64;
        report.bytes = files.iter().map(|f| f.1).sum();
        // Oldest first; ties by path, so the order does not depend on readdir.
        files.sort();
        let mut total = report.bytes;
        for (_, len, path) in files {
            if total <= cap {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total -= len;
                report.removed += 1;
                report.removed_bytes += len;
            }
        }
        report
    }
    pub fn cap(&self) -> u64 {
        self.cap
    }
}

impl ObjectStore for DirObjects {
    fn get(&self, key: &UnitKey) -> Option<Arc<[u8]>> {
        let path = self.path(key);
        let bytes = fs::read(&path).ok()?;
        match unframe(&bytes) {
            Some(payload) => {
                // Refresh the LRU clock; failure only ages the entry.
                if let Ok(file) = fs::File::options().append(true).open(&path) {
                    let _ = file.set_modified(SystemTime::now());
                }
                Some(payload.into())
            }
            None => {
                let _ = fs::remove_file(&path);
                None
            }
        }
    }
    fn put(&self, key: &UnitKey, value: &[u8]) {
        let path = self.path(key);
        let temp_dir = self.root.join("tmp");
        let framed = frame(value);
        let write = || -> std::io::Result<()> {
            fs::create_dir_all(path.parent().unwrap())?;
            fs::create_dir_all(&temp_dir)?;
            let temp = temp_dir.join(format!(
                "{}.{}.{}",
                key.hex(),
                std::process::id(),
                self.temp.fetch_add(1, Relaxed)
            ));
            fs::write(&temp, &framed)?;
            fs::rename(&temp, &path).inspect_err(|_| {
                let _ = fs::remove_file(&temp);
            })
        };
        // A failed write is a later miss, never a build failure.
        if write().is_ok() {
            let before = self.written.fetch_add(framed.len() as u64, Relaxed);
            if (before + framed.len() as u64) / GC_EVERY != before / GC_EVERY {
                self.gc(self.cap);
            }
        }
    }
}

/// Faults `sim@1` injects, each by operation ordinal (0-based, counted over gets and
/// puts separately), so a test names exactly which operation fails.
#[derive(Clone, Debug, Default)]
pub struct SimFaults {
    /// Puts that are silently lost (a crash before rename).
    pub lose_puts: Vec<u64>,
    /// Gets that return a corrupted payload, which the checksum rejects.
    pub corrupt_gets: Vec<u64>,
}

/// `sim@1`: a deterministic in-memory store with an operation log and injectable
/// faults. Payloads carry `dir@1`'s checksum framing, so corruption is detected the
/// same way.
#[derive(Debug, Default)]
pub struct SimObjects {
    state: Mutex<SimState>,
}
#[derive(Debug, Default)]
struct SimState {
    entries: HashMap<UnitKey, Vec<u8>>,
    faults: SimFaults,
    gets: u64,
    puts: u64,
    log: Vec<SimOp>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SimOp {
    Hit(UnitKey),
    Miss(UnitKey),
    Corrupt(UnitKey),
    Put(UnitKey),
    Lost(UnitKey),
}
impl SimObjects {
    pub fn with_faults(faults: SimFaults) -> Self {
        Self {
            state: Mutex::new(SimState {
                faults,
                ..Default::default()
            }),
        }
    }
    fn state(&self) -> std::sync::MutexGuard<'_, SimState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
    pub fn log(&self) -> Vec<SimOp> {
        self.state().log.clone()
    }
    pub fn len(&self) -> usize {
        self.state().entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// The bytes stored, framing included.
    pub fn bytes(&self) -> usize {
        self.state().entries.values().map(Vec::len).sum()
    }
}
impl ObjectStore for SimObjects {
    fn get(&self, key: &UnitKey) -> Option<Arc<[u8]>> {
        let mut state = self.state();
        let ordinal = state.gets;
        state.gets += 1;
        let Some(mut framed) = state.entries.get(key).cloned() else {
            state.log.push(SimOp::Miss(*key));
            return None;
        };
        if state.faults.corrupt_gets.contains(&ordinal)
            && let Some(last) = framed.last_mut()
        {
            *last ^= 0xff;
        }
        match unframe(&framed) {
            Some(payload) => {
                state.log.push(SimOp::Hit(*key));
                Some(payload.into())
            }
            None => {
                state.entries.remove(key);
                state.log.push(SimOp::Corrupt(*key));
                None
            }
        }
    }
    fn put(&self, key: &UnitKey, value: &[u8]) {
        let mut state = self.state();
        let ordinal = state.puts;
        state.puts += 1;
        if state.faults.lose_puts.contains(&ordinal) {
            state.log.push(SimOp::Lost(*key));
            return;
        }
        state.entries.insert(*key, frame(value));
        state.log.push(SimOp::Put(*key));
    }
}

const FRAME_MAGIC: &[u8; 8] = b"HIROBJ01";

/// A stored payload: magic, length, SHA-256 of the payload, payload.
fn frame(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 48);
    out.extend(FRAME_MAGIC);
    out.extend((payload.len() as u64).to_le_bytes());
    out.extend(ContentHash::of_parts("hirsel.object.payload.v1", [payload]).0);
    out.extend(payload);
    out
}

/// The payload of a framed record, or `None` on any mismatch.
fn unframe(bytes: &[u8]) -> Option<&[u8]> {
    let rest = bytes.strip_prefix(FRAME_MAGIC.as_slice())?;
    let (len, rest) = rest.split_at_checked(8)?;
    let (sum, payload) = rest.split_at_checked(32)?;
    (u64::from_le_bytes(len.try_into().ok()?) as usize == payload.len()
        && ContentHash::of_parts("hirsel.object.payload.v1", [payload]).0 == sum)
        .then_some(payload)
}

/// The store a run-config provider name selects for `partition`: `none@1`, `dir@1`
/// (at `root`, else the default cache directory) or `sim@1`.
pub fn objects_by_name(
    name: &str,
    root: Option<PathBuf>,
    partition: &str,
) -> Result<Arc<dyn ObjectStore>, String> {
    Ok(match name {
        "none@1" => Arc::new(NoObjects),
        "dir@1" => Arc::new(match root {
            Some(root) => DirObjects::new(root.join(partition)),
            None => DirObjects::default_location(partition)
                .ok_or("dir@1: no cache directory (set XDG_CACHE_HOME or HOME)")?,
        }),
        "sim@1" => Arc::new(SimObjects::default()),
        other => return Err(format!("unknown object store `{other}`")),
    })
}

/// A layout or FnAbi index a unit names.
enum TableId<'a> {
    Layout(&'a mut LayoutId),
    Abi(&'a mut FnAbiId),
}

/// Every `LayoutId` and `FnAbiId` `unit` names, mutably, in a fixed walk order: the
/// header's ABI, the types, the instructions, then the call, construct, closure, init
/// flag and codec tables. Terminators and pools name none.
fn for_each_table_id<S>(unit: &mut LirUnit<S>, f: &mut impl FnMut(TableId<'_>)) {
    f(TableId::Abi(&mut unit.header.abi));
    for ty in &mut unit.types {
        f(TableId::Layout(&mut ty.layout));
    }
    for inst in &mut unit.instructions {
        match inst {
            Inst::Compare { layout, .. }
            | Inst::FlatCopy { layout, .. }
            | Inst::Hash { layout, .. }
            | Inst::Release { layout, .. }
            | Inst::Reset { layout, .. }
            | Inst::RegionReserve { layout, .. } => f(TableId::Layout(layout)),
            _ => {}
        }
    }
    let tables = &mut unit.tables;
    for call in &mut tables.calls {
        f(TableId::Abi(&mut call.abi));
    }
    for site in &mut tables.constructs {
        f(TableId::Layout(&mut site.layout));
    }
    for site in &mut tables.closures {
        if let Some(code) = &mut site.code {
            f(TableId::Abi(&mut code.abi));
        }
        f(TableId::Layout(&mut site.environment));
    }
    for flag in &mut tables.init_flags {
        if let Some(layout) = &mut flag.environment {
            f(TableId::Layout(layout));
        }
    }
    for plan in &mut tables.codecs {
        for node in &mut plan.nodes {
            for layout in node.layouts_mut() {
                f(TableId::Layout(layout));
            }
        }
        for check in &mut plan.checks {
            f(TableId::Abi(&mut check.abi));
        }
    }
}

/// Every table row a unit directly names, in id order, without cloning the unit.
/// Keep this read walk paired with `for_each_table_id`; the erasure round-trip
/// test compares them so a new operand cannot silently escape receipt evidence.
pub fn references<S>(unit: &LirUnit<S>) -> (Vec<LayoutId>, Vec<FnAbiId>) {
    let mut layouts = std::collections::BTreeSet::new();
    let mut abis = std::collections::BTreeSet::from([unit.header.abi]);
    layouts.extend(unit.types.iter().map(|ty| ty.layout));
    for inst in &unit.instructions {
        match *inst {
            Inst::Compare { layout, .. }
            | Inst::FlatCopy { layout, .. }
            | Inst::Hash { layout, .. }
            | Inst::Release { layout, .. }
            | Inst::Reset { layout, .. }
            | Inst::RegionReserve { layout, .. } => {
                layouts.insert(layout);
            }
            _ => {}
        }
    }
    abis.extend(unit.tables.calls.iter().map(|call| call.abi));
    layouts.extend(unit.tables.constructs.iter().map(|site| site.layout));
    for site in &unit.tables.closures {
        abis.extend(site.code.iter().map(|code| code.abi));
        layouts.insert(site.environment);
    }
    layouts.extend(
        unit.tables
            .init_flags
            .iter()
            .filter_map(|flag| flag.environment),
    );
    for plan in &unit.tables.codecs {
        // Codec plans expose their reference traversal mutably; clone only the
        // node instead of the whole unit, including its CFG and source maps.
        for node in &plan.nodes {
            let mut node = node.clone();
            for layout in node.layouts_mut() {
                layouts.insert(*layout);
            }
        }
        abis.extend(plan.checks.iter().map(|check| check.abi));
    }
    (layouts.into_iter().collect(), abis.into_iter().collect())
}

/// `unit`, at any stage, with its key zeroed and each `LayoutId` and `FnAbiId`
/// renumbered by first use. Returns it
/// with the original ids in that order.
pub fn erased<S: Clone>(unit: &LirUnit<S>) -> (LirUnit<S>, Vec<LayoutId>, Vec<FnAbiId>) {
    let mut erased = unit.clone();
    erased.header.key = UnitKey(ContentHash([0; 32]));
    let (mut layouts, mut abis) = (Vec::new(), Vec::new());
    let (mut layout_index, mut abi_index) = (BTreeMap::new(), BTreeMap::new());
    for_each_table_id(&mut erased, &mut |id| match id {
        TableId::Layout(id) => {
            let next = layout_index.len() as u32;
            let index = *layout_index.entry(id.0).or_insert_with(|| {
                layouts.push(*id);
                next
            });
            *id = LayoutId(index);
        }
        TableId::Abi(id) => {
            let next = abi_index.len() as u32;
            let index = *abi_index.entry(id.0).or_insert_with(|| {
                abis.push(*id);
                next
            });
            *id = FnAbiId(index);
        }
    });
    (erased, layouts, abis)
}

/// An erased unit bound to a program again: each erased id `i` becomes `layouts[i]` or
/// `abis[i]`, the ids that program gives the keys `erased` returned. The error names an
/// erased id with no binding.
pub fn rebound<S>(
    mut unit: LirUnit<S>,
    layouts: &[LayoutId],
    abis: &[FnAbiId],
) -> Result<LirUnit<S>, String> {
    let mut missing = None;
    for_each_table_id(&mut unit, &mut |id| match id {
        TableId::Layout(id) => match layouts.get(id.0 as usize) {
            Some(bound) => *id = *bound,
            None => missing = Some(format!("erased layout {}", id.0)),
        },
        TableId::Abi(id) => match abis.get(id.0 as usize) {
            Some(bound) => *id = *bound,
            None => missing = Some(format!("erased FnAbi {}", id.0)),
        },
    });
    match missing {
        None => Ok(unit),
        Some(message) => Err(format!("{message} has no binding")),
    }
}

/// The digest of everything `unit`'s object bytes are a function of when `toolchain`
/// emits it with `inputs`, `debug` line tables and the small callee bodies `copies` (by
/// their digests) copied in: the erased unit (`erased`), the `LayoutKey`s and FnAbi keys
/// of the ids it names, the static data it reads and the manifest rows of the operations
/// it calls, the target, the options and every program-wide fact emission reads
/// (`EmitInputs`), the versions and the unit's linkage.
///
/// Unlike the `UnitKey`, which the plan computes before import from the unit's inputs,
/// this needs the Ready unit. Two units with one digest emit the same bytes in any
/// program; a store hit is checked against it under `VerifyStore::Recompute`. The
/// error names an id outside the table.
pub fn ready_digest(
    unit: &LirUnit<Ready>,
    inputs: &EmitInputs<'_>,
    toolchain: &str,
    debug: bool,
    copies: &[ContentHash],
) -> Result<ContentHash, String> {
    unit_digest(
        unit,
        inputs,
        toolchain,
        debug,
        copies,
        &[],
        "hirsel.ready-digest.v1",
    )
}

/// The object stage's key (design §1e, the two-stage cache in followups): the digest of
/// `unit` after the whole-program Value passes, as `ready_digest` digests a Ready unit,
/// with `facts`, the inputs the stages from placement to ready read besides the unit
/// (the native options, the instance's placement and proof facts, the finalizer facts of
/// the layouts it reaches), each as bytes. Two units with one key lower and emit to the
/// same object bytes in any program.
pub fn object_key<S: crate::stage::Stage + Clone>(
    unit: &LirUnit<S>,
    inputs: &EmitInputs<'_>,
    toolchain: &str,
    debug: bool,
    copies: &[ContentHash],
    facts: &[Vec<u8>],
) -> Result<UnitKey, String> {
    unit_digest(
        unit,
        inputs,
        toolchain,
        debug,
        copies,
        facts,
        "hirsel.object-key.v1",
    )
    .map(UnitKey)
}

fn unit_digest<S: crate::stage::Stage + Clone>(
    unit: &LirUnit<S>,
    inputs: &EmitInputs<'_>,
    toolchain: &str,
    debug: bool,
    copies: &[ContentHash],
    facts: &[Vec<u8>],
    domain: &str,
) -> Result<ContentHash, String> {
    let (erased, layouts, abis) = erased(unit);
    let mut parts: Vec<Vec<u8>> = vec![
        hirsel_codec::to_text(&erased).into_bytes(),
        toolchain.as_bytes().to_vec(),
        format!(
            "lir {} validator {} interface {} abi {} runtime {} layout {}",
            crate::LIR_VERSION,
            crate::VALIDATOR_VERSION,
            crate::INTERFACE_VERSION,
            crate::ABI_VERSION,
            crate::RUNTIME_ABI_VERSION,
            crate::LAYOUT_VERSION,
        )
        .into_bytes(),
        inputs.target.digest().0.to_vec(),
        format!(
            "{:?} callbacks {} tracking {} fuel {} measurement {} debug {} inline {} copy {}",
            inputs.opt,
            inputs.callbacks,
            inputs.tracking,
            inputs.fuel,
            inputs.measurement,
            debug,
            inputs.inline_instances.contains(&unit.header.instance),
            inputs.available_externally.contains(&unit.header.instance),
        )
        .into_bytes(),
    ];
    for id in &layouts {
        if inputs.layouts.layout(*id).is_none() {
            return Err(format!("the unit names layout {}, outside the table", id.0));
        }
        parts.push(inputs.keys.key(*id).0.0.to_vec());
    }
    for id in &abis {
        let key = inputs
            .keys
            .fn_abi_of(inputs.layouts, *id)
            .ok_or_else(|| format!("the unit names FnAbi {}, outside the table", id.0))?;
        parts.push(key.0.to_vec());
    }
    let owned;
    let index = match inputs.index.filter(|index| index.matches(inputs.layouts)) {
        Some(index) => index,
        None => {
            owned = crate::LayoutIndex::new(inputs.layouts);
            &owned
        }
    };
    let closure = crate::validate::UnitInputs {
        target: inputs.target,
        layouts: inputs.layouts,
        manifest: inputs.manifest,
    }
    .closure_with_index(unit, index)?;
    let mut dependencies: Vec<_> = closure
        .layouts
        .into_iter()
        .chain(
            index
                .introduced()
                .filter(|_| domain == "hirsel.object-key.v1"),
        )
        .map(|id| inputs.keys.key(id))
        .collect();
    dependencies.sort_unstable();
    dependencies.dedup();
    for key in dependencies {
        parts.push(key.0.0.to_vec());
    }
    // The static data the unit reads: glue holds the blocks, but the unit's view of them
    // carries their content.
    for inst in &unit.instructions {
        let Inst::ReadStatic { symbol, .. } = *inst else {
            continue;
        };
        let symbol = unit
            .tables
            .symbols
            .get(symbol.index())
            .ok_or("a static outside the symbol table")?;
        let data = inputs
            .statics
            .iter()
            .find(|d| d.symbol == *symbol)
            .ok_or("a static without its data")?;
        let mut data = data.clone();
        for block in &mut data.blocks {
            parts.push(inputs.keys.key(block.layout).0.0.to_vec());
            block.layout = LayoutId(0);
        }
        parts.push(hirsel_codec::to_text(&data).into_bytes());
    }
    for operation in &unit.tables.operations {
        match inputs.manifest.row(operation) {
            Some(row) => parts.push(hirsel_codec::to_text(row).into_bytes()),
            None => parts.push(b"no row".to_vec()),
        }
    }
    for copy in copies {
        parts.push(copy.0.to_vec());
    }
    parts.push(format!("facts {}", facts.len()).into_bytes());
    parts.extend(facts.iter().cloned());
    Ok(ContentHash::of_parts(
        domain,
        parts.iter().map(Vec::as_slice),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u8) -> UnitKey {
        UnitKey(ContentHash([n; 32]))
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "hirsel-objects-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn none_stores_nothing() {
        let store = objects_by_name("none@1", None, "p").unwrap();
        store.put(&key(1), b"object");
        assert!(store.get(&key(1)).is_none());
        assert!(objects_by_name("disk@9", None, "p").is_err());
    }

    #[test]
    fn dir_round_trips_rejects_damage_and_evicts_least_recently_used() {
        let root = scratch("dir");
        let store = DirObjects::new(root.join(partition("x86_64-unknown-linux-gnu", "llvm", "O1")));
        assert!(store.get(&key(1)).is_none());
        store.put(&key(1), b"first object");
        store.put(&key(2), b"second object");
        assert_eq!(&*store.get(&key(1)).unwrap(), b"first object");
        assert_eq!(&*store.get(&key(2)).unwrap(), b"second object");
        // A damaged file is a miss, and is removed.
        let path = store.path(&key(2));
        let mut bytes = fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        fs::write(&path, bytes).unwrap();
        assert!(store.get(&key(2)).is_none());
        assert!(!path.exists());
        // Eviction keeps the most recently used within the cap.
        store.put(&key(3), b"third object");
        let old = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
        fs::File::options()
            .append(true)
            .open(store.path(&key(1)))
            .unwrap()
            .set_modified(old)
            .unwrap();
        let one = fs::metadata(store.path(&key(3))).unwrap().len();
        let report = store.gc(one);
        assert_eq!((report.entries, report.removed), (2, 1));
        assert!(store.get(&key(1)).is_none());
        assert!(store.get(&key(3)).is_some());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn sim_logs_and_injects_faults() {
        let store = SimObjects::with_faults(SimFaults {
            lose_puts: vec![1],
            corrupt_gets: vec![1],
        });
        store.put(&key(1), b"a");
        store.put(&key(2), b"b");
        store.put(&key(3), b"c");
        assert!(store.get(&key(1)).is_some());
        assert!(store.get(&key(3)).is_none(), "the corrupted get");
        assert!(store.get(&key(2)).is_none(), "the lost put");
        assert_eq!(
            store.log(),
            [
                SimOp::Put(key(1)),
                SimOp::Lost(key(2)),
                SimOp::Put(key(3)),
                SimOp::Hit(key(1)),
                SimOp::Corrupt(key(3)),
                SimOp::Miss(key(2)),
            ]
        );
        assert_eq!(store.len(), 1);
    }
}
