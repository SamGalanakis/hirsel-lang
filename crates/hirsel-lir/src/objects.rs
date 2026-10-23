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
use crate::ir::{ConstantValue, Inst, LirUnit};
use crate::layout_types::LayoutId;
use crate::stage::Ready;
use hirsel_artifact_store::{Action, Address, Domain, Store};
use hirsel_syntax::ContentHash;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

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
    /// Can retain approved local production associations.
    fn production_enabled(&self) -> bool {
        false
    }
    /// An association from an approved local provider. Unknown adapters and
    /// arbitrary read-only directories cannot turn coherent bytes into producer
    /// evidence. This retains the trusted-local-directory model; it is not a
    /// signature or proof of source-to-object correspondence.
    fn get_produced(&self, _key: &UnitKey) -> Option<ProducedBytes> {
        None
    }
    /// Whether writes can be retained; none avoids preparing import-cache entries.
    fn enabled(&self) -> bool {
        true
    }
    fn get(&self, key: &UnitKey) -> Option<Arc<[u8]>>;
    fn put(&self, key: &UnitKey, value: &[u8]);
}

/// Provider provenance for a local action association, separate from integrity.
/// Only the built-in providers can construct this token. Adapters may forward it.
pub struct ProducedBytes(Arc<[u8]>);
impl ProducedBytes {
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}

/// The store format version in `dir@1`'s directory name.
pub const FORMAT: u32 = 3;

// Native fragments have their own typed domain, independent of query/V records.
const OBJECT_DOMAIN: Domain = Domain {
    kind: 0x4400_0001,
    schema: 1,
};
fn address(key: &UnitKey) -> Address {
    Address(ContentHash::of_parts(
        "hirsel.object.address.v1",
        [key.0.0.as_slice()],
    ))
}
fn action(key: &UnitKey) -> Action {
    Action(ContentHash::of_parts(
        "hirsel.object.action.v1",
        [key.0.0.as_slice()],
    ))
}

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

/// `dir@1`: native action associations and checked immutable content in the
/// common artifact store. The v3 reset has no reader for old object files.
/// Only this configured local writable provider supplies production authority.
pub struct DirObjects {
    store: Store,
    cap: u64,
}
impl std::fmt::Debug for DirObjects {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirObjects")
            .field("root", &self.root())
            .field("cap", &self.cap)
            .finish_non_exhaustive()
    }
}

pub use hirsel_artifact_store::GcReport;

impl DirObjects {
    /// The store at `root`, the directory of one partition.
    pub fn new(root: PathBuf) -> Self {
        let store = Store::rebuildable(root);
        store.set_cache_cap(DEFAULT_CAP);
        Self {
            store,
            cap: DEFAULT_CAP,
        }
    }
    pub fn with_cap(mut self, cap: u64) -> Self {
        self.cap = cap;
        self.store.set_cache_cap(cap);
        self
    }
    /// `$XDG_CACHE_HOME/hirsel/objects-v<N>/<partition>`, else under `$HOME/.cache`.
    pub fn default_location(partition: &str) -> Option<Self> {
        let base = std::env::var_os("XDG_CACHE_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .filter(|v| !v.is_empty())
                    .map(|h| PathBuf::from(h).join(".cache"))
            })?;
        // Like Go's os.UserCacheDir: an absent or relative location is not
        // authority to write a cache into the current working directory.
        if !base.is_absolute() {
            return None;
        }
        Some(Self::new(
            base.join("hirsel")
                .join(format!("objects-v{FORMAT}"))
                .join(partition),
        ))
    }
    pub fn root(&self) -> &Path {
        self.store.root()
    }
    /// Collect the common store's action and content files within `cap`.
    pub fn gc(&self, cap: u64) -> GcReport {
        self.store.collect(cap)
    }
    pub fn cap(&self) -> u64 {
        self.cap
    }
}

impl ObjectStore for DirObjects {
    fn production_enabled(&self) -> bool {
        true
    }
    fn get_produced(&self, key: &UnitKey) -> Option<ProducedBytes> {
        self.get(key).map(ProducedBytes)
    }
    fn get(&self, key: &UnitKey) -> Option<Arc<[u8]>> {
        let entry = self.store.lookup(address(key))?;
        if entry.action != action(key) || entry.reference.domain != OBJECT_DOMAIN {
            return None;
        }
        let bytes = self.store.get(entry.reference)?;
        self.store.touch(address(key), entry.reference);
        Some(Arc::from(&*bytes))
    }
    fn put(&self, key: &UnitKey, value: &[u8]) {
        // Content alone is not a local production claim. Publish the association
        // only after the checked immutable output exists.
        let Ok(reference) = self.store.put(OBJECT_DOMAIN, value) else {
            return;
        };
        // The physical owner counts segments, index layers and maintenance,
        // including empty payloads, and collects at a safe publication boundary.
        let _ = self.store.associate(address(key), action(key), reference);
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
/// faults. Checksums model publication damage without a physical file codec.
#[derive(Debug, Default)]
pub struct SimObjects {
    state: Mutex<SimState>,
}
#[derive(Debug, Default)]
struct SimState {
    entries: HashMap<UnitKey, (ContentHash, Vec<u8>)>,
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
    /// Simulated record bytes, including checksum/header accounting.
    pub fn bytes(&self) -> usize {
        self.state()
            .entries
            .values()
            .map(|(_, bytes)| bytes.len() + 48)
            .sum()
    }
}
impl ObjectStore for SimObjects {
    fn production_enabled(&self) -> bool {
        true
    }
    fn get_produced(&self, key: &UnitKey) -> Option<ProducedBytes> {
        self.get(key).map(ProducedBytes)
    }
    fn get(&self, key: &UnitKey) -> Option<Arc<[u8]>> {
        let mut state = self.state();
        let ordinal = state.gets;
        state.gets += 1;
        let Some((mut checksum, mut payload)) = state.entries.get(key).cloned() else {
            state.log.push(SimOp::Miss(*key));
            return None;
        };
        if state.faults.corrupt_gets.contains(&ordinal) {
            if let Some(last) = payload.last_mut() {
                *last ^= 0xff;
            } else {
                checksum.0[0] ^= 0xff;
            }
        }
        if ContentHash::of_parts("hirsel.object.payload.v1", [payload.as_slice()]) == checksum {
            state.log.push(SimOp::Hit(*key));
            Some(payload.into())
        } else {
            state.entries.remove(key);
            state.log.push(SimOp::Corrupt(*key));
            None
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
        let checksum = ContentHash::of_parts("hirsel.object.payload.v1", [value]);
        state.entries.insert(*key, (checksum, value.to_vec()));
        state.log.push(SimOp::Put(*key));
    }
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
                .ok_or("dir@1: no cache directory (set an absolute XDG_CACHE_HOME or HOME, or select none@1)")?,
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
/// header's ABI, the types, the instructions, then the call, construct, closure,
/// constant, init flag and codec tables. Terminators and pools name none.
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
    for descriptor in &mut tables.seal_descriptors {
        f(TableId::Layout(&mut descriptor.root_layout));
        for node in &mut descriptor.projection.nodes {
            for layout in node.layouts_mut() {
                f(TableId::Layout(layout));
            }
        }
        for access in &mut descriptor.access {
            f(TableId::Layout(&mut access.layout));
            // Reuse the complete scan-reference remapper, including nested
            // variants/columns and typed handles, without inventing a subset.
            crate::layout_keys::scan_references_mut(&mut access.scan, &mut |layout| {
                f(TableId::Layout(layout))
            });
        }
    }
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
    // Descriptor constants carry a layout operand, not an ordinary u32 literal.
    // A cached import must bind it in the consumer's table just like a type.
    for constant in &mut tables.constants {
        if let ConstantValue::Descriptor(layout) = &mut constant.value {
            f(TableId::Layout(layout));
        }
    }
    for flag in &mut tables.init_flags {
        if let Some(layout) = &mut flag.environment {
            f(TableId::Layout(layout));
        }
    }
    for plan in &mut tables.structural {
        for node in &mut plan.nodes {
            for layout in node.layouts_mut() {
                f(TableId::Layout(layout));
            }
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
    for constant in &unit.tables.constants {
        if let ConstantValue::Descriptor(layout) = constant.value {
            layouts.insert(layout);
        }
    }
    layouts.extend(
        unit.tables
            .init_flags
            .iter()
            .filter_map(|flag| flag.environment),
    );
    for plan in &unit.tables.structural {
        layouts.extend(plan.layouts());
    }
    for descriptor in &unit.tables.seal_descriptors {
        layouts.extend(descriptor.layouts());
    }
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
    crate::construction::record(&unit.header, crate::construction::ValueWork::Rebind);
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
    use std::{fs, time::SystemTime};

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

    fn paths(store: &DirObjects, key: &UnitKey) -> (PathBuf, PathBuf) {
        let entry = store.store.lookup(address(key)).unwrap();
        // 25e2b384 stores associations in immutable packed segments.
        (
            store.store.association_path(address(key)).unwrap(),
            store.store.output_path(entry.reference),
        )
    }

    #[test]
    fn dir_round_trips_rejects_damage_and_evicts_least_recently_used() {
        let root = scratch("dir");
        let store = DirObjects::new(root.join(partition("x86_64-unknown-linux-gnu", "llvm", "O1")));
        assert!(store.get(&key(1)).is_none());
        store.put(&key(1), b"first object");
        store.put(&key(2), b"second object");
        let reopened = DirObjects::new(store.root().to_owned());
        assert_eq!(&*reopened.get(&key(1)).unwrap(), b"first object");
        assert_eq!(
            reopened.get_produced(&key(2)).unwrap().bytes(),
            b"second object"
        );
        let (_, path) = paths(&store, &key(2));
        let mut bytes = fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        fs::write(&path, bytes).unwrap();
        assert!(store.get(&key(2)).is_none());
        assert!(store.get_produced(&key(2)).is_none());
        store.gc(0);
        store.put(&key(1), b"first object");
        store.put(&key(3), b"third object");
        let old = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
        let (old_action, old_output) = paths(&store, &key(1));
        let old_bytes =
            fs::metadata(&old_action).unwrap().len() + fs::metadata(&old_output).unwrap().len();
        for path in [old_action, old_output] {
            fs::File::options()
                .append(true)
                .open(path)
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
        // The physical budget includes packed routing layers and their root.
        let before = store.gc(u64::MAX);
        let one = before.bytes - old_bytes;
        let report = store.gc(one);
        assert_eq!((report.entries, report.removed), (before.entries, 2));
        assert_eq!(report.bytes - report.removed_bytes, one);
        assert!(store.get(&key(1)).is_none());
        assert!(store.get(&key(3)).is_some());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn dir_requires_its_domain_action_and_complete_local_publication() {
        let root = scratch("identity");
        let store = DirObjects::new(root.clone());
        let physical = Store::new(root.clone());
        // Copying ordinary content into a local cache cannot mint an association.
        let reference = physical.put(OBJECT_DOMAIN, b"copied content").unwrap();
        assert!(store.get_produced(&key(1)).is_none());
        let wrong = physical
            .put(
                Domain {
                    kind: OBJECT_DOMAIN.kind + 1,
                    ..OBJECT_DOMAIN
                },
                b"wrong domain",
            )
            .unwrap();
        physical
            .associate(address(&key(1)), action(&key(1)), wrong)
            .unwrap();
        assert!(store.get_produced(&key(1)).is_none());
        let wrong = physical
            .put(
                Domain {
                    schema: OBJECT_DOMAIN.schema + 1,
                    ..OBJECT_DOMAIN
                },
                b"old schema",
            )
            .unwrap();
        physical
            .associate(address(&key(1)), action(&key(1)), wrong)
            .unwrap();
        assert!(store.get_produced(&key(1)).is_none());
        physical
            .associate(address(&key(1)), action(&key(2)), reference)
            .unwrap();
        assert!(store.get(&key(1)).is_none());
        store.put(&key(1), b"local production");
        assert_eq!(
            store.get_produced(&key(1)).unwrap().bytes(),
            b"local production"
        );
        // Unknown/read-only byte adapters never inherit the writable provider's
        // production capability, even when their bytes came from that provider.
        struct Imported(Arc<[u8]>);
        impl ObjectStore for Imported {
            fn get(&self, _: &UnitKey) -> Option<Arc<[u8]>> {
                Some(self.0.clone())
            }
            fn put(&self, _: &UnitKey, _: &[u8]) {}
        }
        let imported = Imported(store.get(&key(1)).unwrap());
        assert!(imported.get(&key(1)).is_some());
        assert!(!imported.production_enabled());
        assert!(imported.get_produced(&key(1)).is_none());
        let (_, output) = paths(&store, &key(1));
        fs::remove_file(output).unwrap();
        assert!(store.get_produced(&key(1)).is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dir_ignores_old_files_and_failed_or_oversized_writes() {
        let root = scratch("old");
        let hex = key(1).hex();
        fs::create_dir_all(root.join(&hex[..2])).unwrap();
        fs::write(
            root.join(&hex[..2]).join(&hex),
            b"HIROBJ01old object framing",
        )
        .unwrap();
        let store = DirObjects::new(root.clone());
        assert!(store.get(&key(1)).is_none());
        store.put(&key(1), &vec![0; hirsel_artifact_store::MAX_RECORD + 1]);
        assert!(store.get(&key(1)).is_none());
        let blocked = root.join("not-a-directory");
        fs::write(&blocked, b"blocked").unwrap();
        let blocked = DirObjects::new(blocked);
        blocked.put(&key(1), b"never published");
        assert!(blocked.get_produced(&key(1)).is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dir_rejects_every_truncation_and_corrupt_content_or_association() {
        let root = scratch("truncate");
        let store = DirObjects::new(root.clone());
        store.put(&key(1), b"complete");
        let (action, output) = paths(&store, &key(1));
        for path in [output, action] {
            let bytes = fs::read(&path).unwrap();
            for cut in 0..bytes.len() {
                fs::write(&path, &bytes[..cut]).unwrap();
                assert!(
                    store.get_produced(&key(1)).is_none(),
                    "{} cut {cut}",
                    path.display()
                );
            }
            for at in 0..bytes.len() {
                let mut bad = bytes.clone();
                bad[at] ^= 1;
                fs::write(&path, bad).unwrap();
                assert!(
                    store.get_produced(&key(1)).is_none(),
                    "{} byte {at}",
                    path.display()
                );
            }
            let mut trailing = bytes.clone();
            trailing.push(0);
            fs::write(&path, trailing).unwrap();
            assert!(store.get(&key(1)).is_none());
            fs::write(&path, bytes).unwrap();
        }
        assert_eq!(&*store.get(&key(1)).unwrap(), b"complete");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dir_empty_outputs_after_nonempty_publication_still_enforce_the_cap() {
        let root = scratch("empty-cap");
        let cap = 4096;
        let store = DirObjects::new(root.clone()).with_cap(cap);
        store.put(&key(0), b"first");
        for id in 1..100 {
            store.put(&key(id), b"");
        }
        let report = store.gc(u64::MAX);
        assert!(report.bytes <= cap, "{report:?}");
        fs::remove_dir_all(root).unwrap();
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

    #[test]
    fn dir_short_lived_writers_enforce_the_cap() {
        let root = scratch("short-writers");
        for n in 0..12 {
            let store = DirObjects::new(root.clone()).with_cap(4096);
            store.put(&key(n), &[n as u8; 2048]);
            let report = store.gc(u64::MAX);
            assert!(
                report.bytes <= 4096,
                "fresh writers must collect: {report:?}"
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dir_concurrent_handles_publish_complete_records_during_eviction() {
        let root = scratch("publish-evict");
        std::thread::scope(|scope| {
            for n in 0..8 {
                let root = &root;
                scope.spawn(move || {
                    // Independent handles must not reuse temporary files, even for
                    // simultaneous publication of different bytes at the same key.
                    let store = DirObjects::new(root.clone()).with_cap(512 * 1024);
                    let payload = vec![n as u8; 128 * 1024];
                    for _ in 0..16 {
                        store.put(&key(99), &payload);
                        if let Some(bytes) = store.get(&key(99)) {
                            assert_eq!(bytes.len(), payload.len());
                            assert!(bytes.iter().all(|b| *b == bytes[0]));
                            assert!(bytes[0] < 8);
                        }
                        store.gc(512 * 1024);
                    }
                });
            }
        });
        let store = DirObjects::new(root.clone());
        store.put(&key(99), b"complete");
        assert_eq!(&*store.get(&key(99)).unwrap(), b"complete");
        for kind in ["packs", "outputs"] {
            for shard in fs::read_dir(root.join(kind)).into_iter().flatten() {
                for file in fs::read_dir(shard.unwrap().path()).unwrap() {
                    assert!(!file.unwrap().file_name().to_string_lossy().starts_with('.'));
                }
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
