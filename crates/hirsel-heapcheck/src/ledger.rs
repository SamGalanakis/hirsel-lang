//! The checked ledger: abstract allocations keyed by ID and generation, their
//! byte extents with poisoning and checked handle cells, resources, raw tokens,
//! compiler-local regions, run-context handles and named exemptions.
//!
//! Nothing here calls the production runtime. A pointer is a `Ptr` (allocation,
//! generation, offset); in an extent it occupies pointer-sized bytes whose
//! content is a `Cell`, never an address.
use crate::snapshot::{Map, Snapshot};
use crate::violation::{Checked, ViolationKind as V, fail};
use hirsel_lir::ir::{ProjectionStep, RegionInstanceId, SourceAnchorId};
use hirsel_lir::layout_types::{LayoutId, TagForm};
use hirsel_lir::runtime_abi::{FREE_POISON, UNINIT_POISON};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, OnceLock};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct AllocId(pub u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct ResourceId(pub u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct TokenId(pub u32);

/// A checked handle: never a real address.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct Ptr {
    pub alloc: AllocId,
    pub generation: u32,
    pub offset: u64,
    /// Interned logical projection from this allocation extent's root.
    pub(crate) path: u32,
}

impl Ptr {
    pub(crate) fn at(self, offset: u64) -> Self {
        Self {
            offset: self.offset + offset,
            ..self
        }
    }
}

/// What a pointer-sized word of an extent holds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Cell {
    /// A data handle. `counted` cells carry one holder credit of their target.
    /// A borrowed reference kept in frame storage keeps its borrow tag.
    Data {
        ptr: Ptr,
        counted: bool,
        borrow: Option<crate::exec::Borrow>,
    },
    /// A code symbol: the program unit it names, or none for a generic
    /// nested function's value, called only through `Callee::Bound`.
    Code(Option<usize>),
    /// An arena token: the ledger region of the arena it names.
    Arena(Option<usize>),
}

/// Bytes with an initialized map and checked handle cells.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Extent {
    pub bytes: Arc<[u8]>,
    pub init: Arc<[bool]>,
    pub cells: BTreeMap<u64, Cell>,
    pub resources: BTreeMap<Vec<ProjectionStep<u64>>, ResourceId>,
    /// Words whose one data cell a move took out (`Exec::take_part`). The
    /// native move leaves the bits in place, so a null-pointer niche there
    /// still reads as its dataful variant.
    pub vacated: BTreeSet<u64>,
    /// The bits a move left behind (`Exec::take_part`), by offset: native
    /// memory keeps them until the next write there, so a reuse that skips a
    /// store (`Reuse::kept`) relies on them (LIR schema revision 5).
    pub ghosts: BTreeMap<u64, Vec<u8>>,
    /// Allocation extents journal physical holder writes; detached images do not.
    pub(crate) holder_edits: Option<Vec<CellEdit>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CellEdit {
    pub offset: u64,
    pub old: Option<Ptr>,
    pub new: Option<Ptr>,
}

fn holder(cell: &Cell) -> Option<Ptr> {
    match cell {
        Cell::Data {
            ptr, counted: true, ..
        } => Some(*ptr),
        _ => None,
    }
}

impl Extent {
    pub fn new(size: u64) -> Self {
        Self {
            bytes: vec![UNINIT_POISON; size as usize].into(),
            init: vec![false; size as usize].into(),
            cells: BTreeMap::new(),
            resources: BTreeMap::new(),
            vacated: BTreeSet::new(),
            ghosts: BTreeMap::new(),
            holder_edits: None,
        }
    }
    pub fn len(&self) -> u64 {
        self.bytes.len() as u64
    }
    pub fn check_range(&self, offset: u64, size: u64) -> Checked<()> {
        if offset.checked_add(size).is_none_or(|end| end > self.len()) {
            return fail(
                V::OutOfExtent,
                format!(
                    "{size} bytes at offset {offset} of a {}-byte extent",
                    self.len()
                ),
            );
        }
        Ok(())
    }
    fn range(offset: u64, size: u64) -> std::ops::Range<usize> {
        offset as usize..(offset + size) as usize
    }
    pub fn all_init(&self, offset: u64, size: u64) -> bool {
        self.init[Self::range(offset, size)].iter().all(|b| *b)
    }
    pub fn any_init(&self, offset: u64, size: u64) -> bool {
        self.init[Self::range(offset, size)].iter().any(|b| *b)
    }
    pub fn write_bytes(&mut self, offset: u64, data: &[u8]) {
        self.unvacate(offset, data.len() as u64);
        let range = Self::range(offset, data.len() as u64);
        Arc::make_mut(&mut self.bytes)[range.clone()].copy_from_slice(data);
        Arc::make_mut(&mut self.init)[range]
            .iter_mut()
            .for_each(|b| *b = true);
    }
    pub fn read_bytes(&self, offset: u64, size: u64) -> &[u8] {
        &self.bytes[Self::range(offset, size)]
    }
    /// Install a cell; the word's bytes become initialized.
    pub fn put_cell(&mut self, offset: u64, word: u64, cell: Cell) {
        let marker = match &cell {
            Cell::Data { ptr, .. } => u64::from(ptr.alloc.0) << 32 | ptr.offset,
            Cell::Code(unit) => unit.map_or(0, |u| u as u64 + 1),
            Cell::Arena(region) => region.map_or(0, |r| r as u64 + 1),
        };
        let bytes = marker.to_le_bytes();
        self.write_bytes(offset, &bytes[..word as usize]);
        self.set_cell(offset, cell);
    }
    /// The single mutation seam for cells in allocation storage, including
    /// rewrites that preserve bytes (deep-copy relinking).
    pub fn set_cell(&mut self, offset: u64, cell: Cell) {
        let new = holder(&cell);
        let old = self.cells.insert(offset, cell).as_ref().and_then(holder);
        if let Some(edits) = &mut self.holder_edits {
            edits.push(CellEdit { offset, old, new });
        }
    }

    fn remove_cell(&mut self, offset: u64) -> Cell {
        let cell = self.cells.remove(&offset).expect("listed cell");
        if let Some(edits) = &mut self.holder_edits {
            edits.push(CellEdit {
                offset,
                old: holder(&cell),
                new: None,
            });
        }
        cell
    }

    pub fn allocation(size: u64) -> Self {
        Self {
            holder_edits: Some(Vec::new()),
            ..Self::new(size)
        }
    }

    /// Move bytes and exactly the logical resource occurrences selected by
    /// `(prefix, walk)`, re-rooting them in the detached image. None selects the
    /// whole extent; byte ranges never select resources, including empty ones.
    pub fn take(
        &mut self,
        offset: u64,
        size: u64,
        poison: u8,
        owned: Option<(&[ProjectionStep<u64>], &[Vec<ProjectionStep<u64>>])>,
    ) -> Extent {
        self.unvacate(offset, size);
        let mut moved = Extent::new(size);
        let range = Self::range(offset, size);
        Arc::make_mut(&mut moved.bytes).copy_from_slice(&self.bytes[range.clone()]);
        Arc::make_mut(&mut moved.init).copy_from_slice(&self.init[range.clone()]);
        let cells: Vec<u64> = self
            .cells
            .range(offset..offset + size)
            .map(|(o, _)| *o)
            .collect();
        for o in cells {
            let cell = self.remove_cell(o);
            moved.cells.insert(o - offset, cell);
        }
        let resources = self.selected_resources(owned);
        for (source, destination) in resources {
            if let Some(resource) = self.resources.remove(&source) {
                moved.resources.insert(destination, resource);
            }
        }
        Arc::make_mut(&mut self.bytes)[range.clone()]
            .iter_mut()
            .for_each(|b| *b = poison);
        Arc::make_mut(&mut self.init)[range]
            .iter_mut()
            .for_each(|b| *b = false);
        moved
    }
    /// A copy of `[offset, offset + size)` with offsets rebased to zero, with
    /// the resource tags at `owned` as in `take`.
    pub fn copy(
        &self,
        offset: u64,
        size: u64,
        owned: Option<(&[ProjectionStep<u64>], &[Vec<ProjectionStep<u64>>])>,
    ) -> Extent {
        let range = Self::range(offset, size);
        Extent {
            bytes: self.bytes[range.clone()].to_vec().into(),
            init: self.init[range].to_vec().into(),
            cells: self
                .cells
                .range(offset..offset + size)
                .map(|(o, c)| (o - offset, c.clone()))
                .collect(),
            resources: self
                .selected_resources(owned)
                .into_iter()
                .filter_map(|(source, destination)| {
                    self.resources.get(&source).map(|r| (destination, *r))
                })
                .collect(),
            vacated: BTreeSet::new(),
            ghosts: BTreeMap::new(),
            holder_edits: None,
        }
    }
    fn selected_resources(
        &self,
        owned: Option<(&[ProjectionStep<u64>], &[Vec<ProjectionStep<u64>>])>,
    ) -> Vec<(Vec<ProjectionStep<u64>>, Vec<ProjectionStep<u64>>)> {
        match owned {
            None => self
                .resources
                .keys()
                .map(|p| (p.clone(), p.clone()))
                .collect(),
            Some((prefix, walk)) => walk
                .iter()
                .map(|relative| {
                    let mut source = prefix.to_vec();
                    source.extend(relative);
                    (source, relative.clone())
                })
                .collect(),
        }
    }
    fn unvacate(&mut self, offset: u64, size: u64) {
        let inside: Vec<u64> = self.vacated.range(offset..offset + size).copied().collect();
        for o in inside {
            self.vacated.remove(&o);
        }
        let end = offset + size;
        self.ghosts
            .retain(|&o, bits| o + bits.len() as u64 <= offset || o >= end);
    }
    /// The bits native memory holds at each byte: the initialized bytes, and
    /// where a move left uninitialized bytes, the bits it left behind.
    pub fn native(&self) -> Vec<Option<u8>> {
        let mut out: Vec<Option<u8>> = self
            .bytes
            .iter()
            .zip(self.init.iter())
            .map(|(b, i)| i.then_some(*b))
            .collect();
        for (&o, bits) in &self.ghosts {
            for (i, b) in bits.iter().enumerate() {
                let at = o as usize + i;
                if at < out.len() && out[at].is_none() {
                    out[at] = Some(*b);
                }
            }
        }
        out
    }
    /// The active ordinal of a variant whose tag, read as `form`, lies at
    /// `offset`; `word` is the pointer size. A niche word is the dataful
    /// variant's while it holds (or a move vacated) a data cell, and the
    /// fieldless variant's when its bytes are zero.
    pub fn active(&self, offset: u64, form: TagForm, word: u64) -> Checked<u32> {
        if matches!(form, TagForm::LiveBit { bit } if bit >= 64) {
            return fail(V::Malformed, "live bit outside its word");
        }
        let width = form.width(word as u32);
        self.check_range(offset, width)?;
        if let TagForm::Niche { dataful, .. } = form
            && (self.vacated.contains(&offset)
                || matches!(self.cells.get(&offset), Some(Cell::Data { .. })))
        {
            return Ok(dataful);
        }
        if !self.all_init(offset, width) {
            return fail(
                V::UninitializedRead,
                format!("the variant tag at offset {offset} is not initialized"),
            );
        }
        let mut bytes = [0u8; 8];
        bytes[..width as usize].copy_from_slice(self.read_bytes(offset, width));
        let value = u64::from_le_bytes(bytes);
        if matches!(form, TagForm::Niche { .. }) && value != 0 {
            return fail(
                V::TypeConfusion,
                format!("a non-null niche word at offset {offset} holds no block reference"),
            );
        }
        Ok(form.active(value))
    }
    /// Place `other` at `offset`; only its initialized bytes are written.
    pub fn place(&mut self, offset: u64, other: Extent) {
        self.place_at(offset, &[], other);
    }
    /// Install an image below its destination's logical projection.
    pub fn place_at(&mut self, offset: u64, path: &[ProjectionStep<u64>], other: Extent) {
        self.unvacate(offset, other.len());
        let bytes = Arc::make_mut(&mut self.bytes);
        let init = Arc::make_mut(&mut self.init);
        for (i, (byte, initialized)) in other.bytes.iter().zip(other.init.iter()).enumerate() {
            if *initialized {
                bytes[offset as usize + i] = *byte;
                init[offset as usize + i] = true;
            }
        }
        for (o, c) in other.cells {
            self.set_cell(offset + o, c);
        }
        for (relative, resource) in other.resources {
            let mut destination = path.to_vec();
            destination.extend(relative);
            self.resources.insert(destination, resource);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Count {
    Finite(u64),
    /// Sticky: overflow saturation or static data. Never decremented.
    Immortal,
    /// A `Shared` root block: the number of live roots (MEM-11), kept apart
    /// from ordinary credits. Only roots dup and release it.
    Root(u64),
    /// A block of a frozen graph while a root of the graph lives: its parent
    /// holds it, never written, and `copies` more holders hold it uncredited,
    /// each a copy read out of the frozen value (`shared.with`). A copy's dup
    /// and release change only `copies`, and none may outlive the roots.
    Frozen {
        copies: u64,
    },
    /// A tracked arena List block (MEM-6, runtime ABI 11): never released,
    /// and shared, stickily, from its first dup or `Share`.
    Arena {
        shared: bool,
    },
}

impl Count {
    /// The number of holder cells the count stands for, when it is exact.
    pub fn holders(self) -> Option<u64> {
        match self {
            Count::Finite(n) | Count::Root(n) => Some(n),
            Count::Frozen { copies } => Some(1 + copies),
            Count::Immortal | Count::Arena { .. } => None,
        }
    }
}

/// Which storage owns an allocation (the ledger's view of
/// `StorageOwner`, plus executor-internal frame and temporary storage).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Owner {
    /// A counted block in the task heap.
    Heap,
    /// Uncounted storage of a live compiler-local region.
    Region(usize),
    /// A frame slot's storage.
    Frame,
    /// Storage the executor materializes for a value being dropped.
    Temporary,
    /// Program-lifetime static data.
    Static,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AllocState {
    Live,
    /// Count reached zero; its payload is being drained.
    Dying,
    /// Raw storage owned by a reuse token.
    Raw(TokenId),
    Dead,
}

#[derive(Clone, Debug)]
pub(crate) struct Allocation {
    pub generation: u32,
    /// H-derived element boundaries, discarded before any mutable access.
    pub flat_boundaries: std::cell::RefCell<BTreeMap<u64, Vec<u64>>>,
    pub owner: Owner,
    /// The payload layout while Live; None for raw and dead storage.
    pub layout: Option<LayoutId>,
    pub size: u64,
    /// Usable bytes: the size class for heap blocks.
    pub capacity: u64,
    pub align: u32,
    pub state: AllocState,
    /// Some for counted heap blocks and static blocks.
    pub count: Option<Count>,
    pub published: bool,
    pub extent: Extent,
    /// Queued once until the next holder audit; avoids per-write tree allocation.
    pub(crate) audit_dirty: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceState {
    Live,
    Finalizing,
    FieldsPending,
    Dead,
}

#[derive(Clone, Debug)]
pub(crate) struct Resource {
    pub layout: LayoutId,
    /// Whether the layout names a user finalizer that must run before death.
    pub finalizer: bool,
    pub state: ResourceState,
    pub forgotten: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct Token {
    pub alloc: AllocId,
    pub generation: u32,
    pub consumed: bool,
    /// The payload bits native memory held when the donor was reset
    /// (`Extent::native`); a kept field must find its operand's bits here.
    pub native: Arc<[Option<u8>]>,
}

#[derive(Clone, Debug)]
pub(crate) struct Region {
    pub instance: RegionInstanceId,
    pub live: bool,
    pub allocations: Snapshot<AllocId>,
    /// A bounded arena's budget (MEM-6).
    pub budget: Option<Budget>,
}

/// What remains of a bounded arena's capacity, the number of failure records
/// when it opened, and the record of its own `OutOfSpace`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Budget {
    pub remaining: u64,
    pub start: usize,
    pub exhausted: Option<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HandleState {
    Open,
    Closed,
}

/// Why an allocation, resource or handle may outlive the terminal audit. Each
/// is a named record with its justification; there is no blanket exemption.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Exemption {
    /// Static data, immortal for the program's lifetime.
    Static { alloc: AllocId, what: String },
    /// A counted block whose count saturated (sticky overflow immortality).
    Overflow { alloc: AllocId },
    /// Explicit `forget`: the value and everything it owns.
    Forget {
        anchor: Option<SourceAnchorId>,
        allocations: Arc<[AllocId]>,
        resources: Arc<[ResourceId]>,
    },
    /// Harness-owned run-context backing.
    Context { handle: u64, what: String },
}

/// Allocation counters: instrumentation only. No check reads them.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Counters {
    pub allocations: u64,
    pub frees: u64,
    pub dups: u64,
    pub releases: u64,
    pub resets: u64,
    pub reuses: u64,
    pub disposes: u64,
    pub finalizers: u64,
    pub peak_live_heap: u64,
    /// Executed scalar byte reads through an index projection; not reserve traffic.
    pub indexed_byte_reads: u64,
    /// Payload capacity in live counted blocks (excludes frame/static storage).
    pub live_heap_bytes: u64,
    pub peak_live_heap_bytes: u64,
    /// Actual variable-element length-prefix reads, and sequential Packed
    /// element extent reads (MEM-13), while finding boundaries.
    pub flat_prefix_reads: u64,
}

/// One ownership transition, in execution order, for trace comparison.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    Alloc {
        alloc: AllocId,
        owner: &'static str,
        capacity: u64,
    },
    Dup {
        alloc: AllocId,
        count: Option<u64>,
    },
    Release {
        alloc: AllocId,
        count: Option<u64>,
    },
    Saturate {
        alloc: AllocId,
    },
    Free {
        alloc: AllocId,
    },
    Reset {
        alloc: AllocId,
        token: Option<TokenId>,
    },
    Reuse {
        alloc: AllocId,
        generation: u32,
    },
    Dispose {
        alloc: AllocId,
    },
    Finalize {
        resource: ResourceId,
    },
    RegionEnter {
        region: usize,
    },
    RegionExit {
        region: usize,
    },
    Forget,
    Failure {
        rule: String,
    },
}

/// Logical handles retain closed entries for repeated-close diagnostics, while
/// the independent terminal audit keeps a sparse index of remaining obligations.
#[derive(Clone, Default)]
pub(crate) struct Handles {
    pub(crate) entries: Snapshot<(HandleState, bool), 15>,
}
impl Handles {
    pub(crate) fn get(&self, h: &u64) -> Option<&(HandleState, bool)> {
        self.entries.get(*h as usize)
    }
    pub(crate) fn get_mut(&mut self, h: &u64) -> Option<&mut (HandleState, bool)> {
        self.entries.get_mut(*h as usize)
    }
    pub(crate) fn contains_key(&self, h: &u64) -> bool {
        self.get(h).is_some()
    }
    pub(crate) fn insert(&mut self, h: u64, state: (HandleState, bool)) {
        self.entries.set(h as usize, state);
    }
}
impl<'a> IntoIterator for &'a Handles {
    type Item = (u64, &'a (HandleState, bool));
    type IntoIter = std::iter::Map<
        crate::snapshot::Entries<'a, (HandleState, bool)>,
        fn((usize, &'a (HandleState, bool))) -> (u64, &'a (HandleState, bool)),
    >;
    fn into_iter(self) -> Self::IntoIter {
        self.entries.indexed().map(|(h, v)| (h as u64, v))
    }
}

/// A shared append-only audit history. Forks and reports retain its prefix
/// in constant time. Iteration streams; slice consumers materialize lazily.
pub struct History<T> {
    entries: Snapshot<T>,
    contiguous: OnceLock<Vec<T>>,
}
impl<T> Default for History<T> {
    fn default() -> Self {
        Self {
            entries: Snapshot::default(),
            contiguous: OnceLock::new(),
        }
    }
}
impl<T: Clone> Clone for History<T> {
    fn clone(&self) -> Self {
        Self {
            entries: self.entries.clone(),
            contiguous: OnceLock::new(),
        }
    }
}
impl<T> History<T> {
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.entries.iter()
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn get(&self, index: usize) -> Option<&T> {
        self.entries.get(index)
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
impl<T: Clone> History<T> {
    pub(crate) fn push(&mut self, value: T) {
        self.contiguous.take();
        self.entries.push(value);
    }
}
impl<T: Clone> History<T> {
    pub fn as_slice(&self) -> &[T] {
        self.contiguous
            .get_or_init(|| self.iter().cloned().collect())
    }
}
impl<T: std::fmt::Debug> std::fmt::Debug for History<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}
impl<T: PartialEq> PartialEq for History<T> {
    fn eq(&self, other: &Self) -> bool {
        self.entries == other.entries
    }
}
impl<T: Eq> Eq for History<T> {}
/// Ownership transitions in execution order.
pub type Trace = History<Event>;

#[derive(Clone, Default)]
pub(crate) struct Ledger {
    pub allocations: Snapshot<Arc<Allocation>>,
    retired: Retired,
    paths: RefCell<Snapshot<Vec<ProjectionStep<u64>>>>,
    path_ids: RefCell<Map<Vec<ProjectionStep<u64>>, u32>>,
    pub resources: Snapshot<Resource>,
    pub tokens: Snapshot<Token>,
    pub regions: Snapshot<Region>,
    pub handles: Handles,
    pub exemptions: History<Exemption>,
    pub counters: Counters,
    /// Heap blocks not yet freed, for `Counters::peak_live_heap`.
    live_heap: u64,
    pub trace: Trace,
    /// Every allocation whose count, lifetime, generation or cells may have changed.
    pub(crate) changed: Vec<AllocId>,
}

/// Sparse allocation IDs are never recycled. A hole below the ID high-water
/// mark is retired generation-zero frame storage, with one bit distinguishing
/// temporary storage. Keep this detection state indefinitely: an arbitrarily
/// old handle must still produce its original lifetime diagnostic.
#[derive(Clone)]
struct Retired {
    temporary: Snapshot<u64>,
    frame: Arc<Allocation>,
    temp: Arc<Allocation>,
}

impl Default for Retired {
    fn default() -> Self {
        let tombstone = |owner| {
            Arc::new(Allocation {
                generation: 0,
                flat_boundaries: Default::default(),
                owner,
                layout: None,
                size: 0,
                capacity: 0,
                align: 1,
                state: AllocState::Dead,
                count: None,
                published: false,
                extent: Extent::allocation(0),
                audit_dirty: false,
            })
        };
        Self {
            temporary: Snapshot::default(),
            frame: tombstone(Owner::Frame),
            temp: tombstone(Owner::Temporary),
        }
    }
}

impl Ledger {
    pub(crate) fn path(&self, ptr: Ptr) -> Vec<ProjectionStep<u64>> {
        if ptr.path == 0 {
            Vec::new()
        } else {
            self.paths.borrow()[ptr.path as usize - 1].clone()
        }
    }
    pub(crate) fn project(&self, ptr: Ptr, offset: u64, step: ProjectionStep<u64>) -> Ptr {
        let mut path = self.path(ptr);
        // Slot and FlatAt identify the same logical element as Index; their
        // capacity check and byte cursor affect resolution, not identity.
        path.push(match step {
            ProjectionStep::Slot(index) | ProjectionStep::FlatAt { index, .. } => {
                ProjectionStep::Index(index)
            }
            step => step,
        });
        let found = self.path_ids.borrow().get(&path).copied();
        let id = if let Some(id) = found {
            id
        } else {
            let id = self.paths.borrow().len() as u32 + 1;
            self.paths.borrow_mut().push(path.clone());
            self.path_ids.borrow_mut().insert(path, id);
            id
        };
        Ptr {
            offset: ptr.offset + offset,
            path: id,
            ..ptr
        }
    }

    pub fn alloc(
        &mut self,
        owner: Owner,
        layout: Option<LayoutId>,
        size: u64,
        capacity: u64,
        align: u32,
        count: Option<Count>,
    ) -> Ptr {
        let id = AllocId(self.allocations.len() as u32);
        self.changed.push(id);
        self.allocations.push(Arc::new(Allocation {
            generation: 0,
            flat_boundaries: Default::default(),
            owner,
            layout,
            size,
            capacity,
            align,
            state: AllocState::Live,
            count,
            published: false,
            extent: Extent::allocation(capacity.max(size)),
            audit_dirty: true,
        }));
        if owner == Owner::Heap {
            self.counters.allocations += 1;
            self.live_heap += 1;
            self.counters.live_heap_bytes += capacity;
            self.counters.peak_live_heap_bytes = self
                .counters
                .peak_live_heap_bytes
                .max(self.counters.live_heap_bytes);
            self.counters.peak_live_heap = self.counters.peak_live_heap.max(self.live_heap);
        }
        if matches!(owner, Owner::Heap | Owner::Region(_) | Owner::Static) {
            self.trace.push(Event::Alloc {
                alloc: id,
                owner: owner_name(owner),
                capacity,
            });
        }
        Ptr {
            alloc: id,
            generation: 0,
            offset: 0,
            path: 0,
        }
    }

    pub fn get(&self, id: AllocId) -> &Allocation {
        self.lookup(id).expect("allocated ID")
    }
    fn lookup(&self, id: AllocId) -> Option<&Allocation> {
        let index = id.0 as usize;
        if let Some(a) = self.allocations.get(index) {
            return Some(a);
        }
        if index >= self.allocations.len() {
            return None;
        }
        let temporary = self.retired.temporary.get(index / 64).copied().unwrap_or(0);
        Some(if temporary & (1 << (index % 64)) != 0 {
            &self.retired.temp
        } else {
            &self.retired.frame
        })
    }
    pub fn get_mut(&mut self, id: AllocId) -> &mut Allocation {
        // A malformed command may revisit dead storage. Materialize only that
        // tombstone rather than changing its diagnostic to "unknown allocation".
        if self.allocations.get(id.0 as usize).is_none() {
            let a = Arc::new(self.get(id).clone());
            self.allocations.set(id.0 as usize, a);
        }
        let allocation = Arc::make_mut(&mut self.allocations[id.0 as usize]);
        if !allocation.audit_dirty {
            allocation.audit_dirty = true;
            self.changed.push(id);
        }
        allocation.flat_boundaries.get_mut().clear();
        allocation
    }

    /// Called only after the holder journal has been consumed. Physical audits
    /// compare their own retained cells with the now absent storage; they do
    /// not rely on either this retirement decision or the write journal.
    pub(crate) fn retire_dead(&mut self, id: AllocId) -> bool {
        let a = self.get(id);
        if a.state != AllocState::Dead
            || a.generation != 0
            || a.count.is_some()
            || !matches!(a.owner, Owner::Frame | Owner::Temporary)
        {
            return false;
        }
        if a.owner == Owner::Temporary {
            let index = id.0 as usize;
            let bits = self.retired.temporary.get(index / 64).copied().unwrap_or(0);
            self.retired
                .temporary
                .set(index / 64, bits | (1 << (index % 64)));
        }
        assert!(
            self.get(id)
                .extent
                .holder_edits
                .as_ref()
                .is_some_and(Vec::is_empty),
            "retirement must follow holder journal consumption"
        );
        self.allocations.remove(id.0 as usize);
        true
    }

    /// The allocation a handle names, checked live at the handle's generation.
    pub fn check(&self, ptr: Ptr) -> Checked<&Allocation> {
        let Some(a) = self.lookup(ptr.alloc) else {
            return fail(
                V::ForgedPointer,
                format!("handle to unknown allocation {:?}", ptr.alloc),
            );
        };
        if a.generation != ptr.generation {
            return fail(
                V::StaleGeneration,
                format!(
                    "handle to {:?} generation {} but its storage is generation {} (reused)",
                    ptr.alloc, ptr.generation, a.generation
                ),
            );
        }
        match a.state {
            AllocState::Live | AllocState::Dying => Ok(a),
            AllocState::Raw(_) => fail(
                V::UseAfterFree,
                format!("access to {:?}, which is raw reuse storage", ptr.alloc),
            ),
            AllocState::Dead => match a.owner {
                Owner::Region(_) => fail(
                    V::RegionEscape,
                    format!("access to {:?} after its region exited", ptr.alloc),
                ),
                Owner::Frame => fail(
                    V::FrameStorageEscape,
                    format!("access to {:?}, a slot of an exited frame", ptr.alloc),
                ),
                _ => fail(V::UseAfterFree, format!("access to freed {:?}", ptr.alloc)),
            },
        }
    }

    /// Free storage: every remaining holder or live resource is a leak.
    pub fn free(&mut self, id: AllocId) -> Checked<()> {
        let size = self.get(id).extent.len();
        let rest = self.get_mut(id).extent.take(0, size, FREE_POISON, None);
        if let Some((offset, _)) = rest
            .cells
            .iter()
            .find(|(_, c)| matches!(c, Cell::Data { counted: true, .. }))
        {
            return fail(
                V::LeakedCredit,
                format!("{id:?} freed while it still holds a holder credit at offset {offset}"),
            );
        }
        self.end_resources(&rest)?;
        let a = self.get_mut(id);
        a.state = AllocState::Dead;
        a.layout = None;
        let owner = a.owner;
        let capacity = a.capacity;
        if owner == Owner::Heap {
            self.counters.live_heap_bytes -= capacity;
            self.counters.frees += 1;
            self.live_heap -= 1;
        }
        if matches!(owner, Owner::Heap | Owner::Region(_)) {
            self.trace.push(Event::Free { alloc: id });
        }
        Ok(())
    }

    /// Resources whose storage is ending: FieldsPending ones are done; a live
    /// resource with a user finalizer was never finalized.
    pub fn end_resources(&mut self, extent: &Extent) -> Checked<()> {
        for r in extent.resources.values() {
            let resource = &mut self.resources[r.0 as usize];
            match resource.state {
                ResourceState::FieldsPending | ResourceState::Dead => {
                    resource.state = ResourceState::Dead
                }
                ResourceState::Live if resource.forgotten => {}
                ResourceState::Live if !resource.finalizer => resource.state = ResourceState::Dead,
                ResourceState::Live | ResourceState::Finalizing => {
                    return fail(
                        V::ResourceNotFinalized,
                        format!("storage of {r:?} ended while it was {:?}", resource.state),
                    );
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn owner_name(owner: Owner) -> &'static str {
    match owner {
        Owner::Heap => "heap",
        Owner::Region(_) => "region",
        Owner::Frame => "frame",
        Owner::Temporary => "temporary",
        Owner::Static => "static",
    }
}

#[cfg(test)]
mod fork_tests {
    use super::*;
    #[test]
    fn retired_owner_bits_preserve_interleaved_ids_and_snapshot_isolation() {
        let mut ledger = Ledger::default();
        let pointers: Vec<_> = (0..257)
            .map(|i| {
                ledger.alloc(
                    if i % 3 == 0 {
                        Owner::Temporary
                    } else {
                        Owner::Frame
                    },
                    None,
                    8,
                    8,
                    8,
                    None,
                )
            })
            .collect();
        let original = ledger.clone();
        for &p in pointers.iter().rev().step_by(2) {
            ledger.free(p.alloc).unwrap();
            let diagnostic = ledger.check(p).unwrap_err();
            assert!(ledger.retire_dead(p.alloc));
            assert_eq!(ledger.check(p).unwrap_err(), diagnostic);
        }
        for (i, &p) in pointers.iter().enumerate() {
            assert_eq!(ledger.check(p).is_ok(), i % 2 == 1);
            assert!(original.check(p).is_ok());
        }
        let retired = ledger.clone();
        // Even malformed repeated free preserves the old zero-sized/dead state.
        ledger.free(pointers[0].alloc).unwrap();
        assert!(ledger.retire_dead(pointers[0].alloc));
        assert_eq!(
            ledger.check(pointers[0]).unwrap_err(),
            retired.check(pointers[0]).unwrap_err()
        );
    }

    #[test]
    fn retired_frame_and_statement_temporary_keep_exact_lifetime_diagnostics() {
        for (owner, kind, message) in [
            (
                Owner::Frame,
                V::FrameStorageEscape,
                "access to AllocId(0), a slot of an exited frame",
            ),
            (
                Owner::Temporary,
                V::UseAfterFree,
                "access to freed AllocId(0)",
            ),
        ] {
            let mut ledger = Ledger::default();
            let old = ledger.alloc(owner, None, 8, 8, 8, None);
            let live_fork = ledger.clone();
            ledger.free(old.alloc).unwrap();
            // Ledger unit control: consume the empty holder journal as sync does.
            ledger
                .get_mut(old.alloc)
                .extent
                .holder_edits
                .as_mut()
                .unwrap()
                .clear();
            let dead_fork = ledger.clone();
            assert!(ledger.retire_dead(old.alloc));
            assert!(ledger.allocations.is_empty());
            for _ in 0..1024 {
                let next = ledger.alloc(owner, None, 8, 8, 8, None);
                ledger.free(next.alloc).unwrap();
                assert!(ledger.retire_dead(next.alloc));
            }
            let violation = ledger.check(old).unwrap_err();
            assert_eq!(violation.kind, kind);
            assert_eq!(violation.message, message);
            assert_eq!(violation, dead_fork.check(old).unwrap_err());
            assert!(live_fork.check(old).is_ok(), "retirement is fork-local");
            let forged = Ptr {
                alloc: AllocId(ledger.allocations.len() as u32),
                ..old
            };
            assert_eq!(ledger.check(forged).unwrap_err().kind, V::ForgedPointer);
            let stale = Ptr {
                generation: 1,
                ..old
            };
            assert_eq!(
                ledger.check(stale).unwrap_err().message,
                "handle to AllocId(0) generation 1 but its storage is generation 0 (reused)"
            );
        }
    }

    #[test]
    fn logical_handle_snapshots_cover_the_full_u64_key_space() {
        let mut ledger = Ledger::default();
        let handles = [0, u64::from(u32::MAX) + 1, u64::MAX - 1, u64::MAX];
        for h in handles {
            ledger.handles.insert(h, (HandleState::Open, false));
        }
        let snapshot = ledger.clone();
        ledger.handles.get_mut(&u64::MAX).unwrap().0 = HandleState::Closed;
        assert_eq!(
            snapshot.handles.get(&u64::MAX),
            Some(&(HandleState::Open, false))
        );
        assert_eq!(
            ledger.handles.get(&u64::MAX),
            Some(&(HandleState::Closed, false))
        );
        assert_eq!(
            (&ledger.handles)
                .into_iter()
                .map(|(h, _)| h)
                .collect::<Vec<_>>(),
            handles
        );
    }

    #[test]
    fn allocation_snapshots_isolate_headers_and_physical_storage() {
        let mut ledger = Ledger::default();
        let ptr = ledger.alloc(Owner::Heap, None, 8, 8, 8, Some(Count::Finite(1)));
        ledger.get_mut(ptr.alloc).extent.write_bytes(0, &[1]);
        let snapshot = ledger.clone();
        assert!(Arc::ptr_eq(
            &ledger.allocations[0],
            &snapshot.allocations[0]
        ));
        let allocation = ledger.get_mut(ptr.alloc);
        allocation.generation = 7;
        allocation.count = Some(Count::Finite(2));
        allocation.extent.write_bytes(0, &[9]);
        assert_eq!(snapshot.get(ptr.alloc).generation, 0);
        assert_eq!(snapshot.get(ptr.alloc).count, Some(Count::Finite(1)));
        assert_eq!(snapshot.get(ptr.alloc).extent.read_bytes(0, 1), &[1]);
        assert_eq!(ledger.get(ptr.alloc).extent.read_bytes(0, 1), &[9]);
        assert!(!Arc::ptr_eq(
            &ledger.allocations[0],
            &snapshot.allocations[0]
        ));
    }
    #[test]
    fn extent_snapshots_isolate_bytes_initialization_cells_and_moves() {
        let mut extent = Extent::allocation(16);
        extent.write_bytes(0, &[1, 2, 3, 4]);
        let ptr = Ptr {
            alloc: AllocId(7),
            generation: 3,
            offset: 0,
            path: 0,
        };
        extent.set_cell(
            8,
            Cell::Data {
                ptr,
                counted: true,
                borrow: None,
            },
        );
        let snapshot = extent.clone();
        assert!(Arc::ptr_eq(&extent.bytes, &snapshot.bytes));
        assert!(Arc::ptr_eq(&extent.init, &snapshot.init));
        extent.write_bytes(0, &[9]);
        let taken = extent.take(8, 8, FREE_POISON, None);
        assert_eq!(snapshot.read_bytes(0, 4), &[1, 2, 3, 4]);
        assert!(snapshot.cells.contains_key(&8));
        assert!(!extent.cells.contains_key(&8));
        assert_eq!(taken.cells.get(&0), snapshot.cells.get(&8));
        assert_eq!(snapshot.holder_edits.as_ref().unwrap().len(), 1);
        assert_eq!(extent.holder_edits.as_ref().unwrap().len(), 2);
        assert!(snapshot.all_init(0, 4));
        assert!(!extent.any_init(8, 8));
        let mut restored = snapshot.clone();
        restored.place(8, taken);
        assert_eq!(snapshot.read_bytes(0, 4), &[1, 2, 3, 4]);
        assert_eq!(restored.cells.get(&8), snapshot.cells.get(&8));
    }
}
