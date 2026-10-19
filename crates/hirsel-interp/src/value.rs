//! Observable values contain bits and declaration ordinals, never host addresses.
use hirsel_syntax::DefPath;
use hirsel_tir::BodyId;
use hirsel_types::{BuiltinType, Primitive};

/// Concrete lexical size bindings, keyed by the declaration rather than its ordinal alone.
pub type SizeBindings =
    std::collections::BTreeMap<(hirsel_types::NameSite, hirsel_types::SizeParamId), u64>;

/// A named callable has no environment. Generic type rows retain their owner.
#[derive(Clone, Debug)]
pub struct NamedFunction {
    pub definition: DefPath,
    pub arguments: Vec<hirsel_types::GenericArgument>,
    pub types: std::sync::Arc<hirsel_types::TypeStore>,
    pub ty: hirsel_types::TypeId,
}
impl PartialEq for NamedFunction {
    fn eq(&self, other: &Self) -> bool {
        self.definition == other.definition
            && self.arguments.len() == other.arguments.len()
            && self.arguments.iter().zip(&other.arguments).all(|(a, b)| {
                match (
                    hirsel_types::encoding::canonical_argument_encoding(&self.types, a),
                    hirsel_types::encoding::canonical_argument_encoding(&other.types, b),
                ) {
                    (Ok(a), Ok(b)) => a == b,
                    _ => match (a, b) {
                        (
                            hirsel_types::GenericArgument::Type(a),
                            hirsel_types::GenericArgument::Type(b),
                        ) => hirsel_types::semantic_equal(&self.types, *a, &other.types, *b)
                            .unwrap_or(false),
                        _ => a == b,
                    },
                }
            })
    }
}
impl Eq for NamedFunction {}

/// A `Shared[T]` root (MEM-11). Snapshots of one root share its graph and its
/// explicit root count: only an owned copy adds a root ([`Value::copied`]) and
/// only a release or a successful thaw removes one. The graph is frozen: it is
/// never written while any root lives.
#[derive(Clone)]
pub struct SharedRoot {
    id: u64,
    graph: std::sync::Arc<std::sync::Mutex<SharedGraph>>,
}
struct SharedGraph {
    roots: u64,
    value: Option<Value>,
}
impl SharedRoot {
    /// The first root of a freshly frozen value.
    pub(crate) fn freeze(id: u64, value: Value) -> Self {
        Self {
            id,
            graph: std::sync::Arc::new(std::sync::Mutex::new(SharedGraph {
                roots: 1,
                value: Some(value),
            })),
        }
    }
    /// The logical identity of the frozen graph, shared by all its roots.
    pub fn id(&self) -> u64 {
        self.id
    }
    fn graph(&self) -> std::sync::MutexGuard<'_, SharedGraph> {
        self.graph.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn acquire(&self) {
        self.graph().roots += 1;
    }
    /// Give up this root: the last one hands back the graph for release.
    pub(crate) fn release(&self) -> Result<Option<Value>, crate::Error> {
        let mut graph = self.graph();
        graph.roots = graph
            .roots
            .checked_sub(1)
            .ok_or_else(|| crate::invalid("release of a dead Shared root"))?;
        Ok(if graph.roots == 0 {
            graph.value.take()
        } else {
            None
        })
    }
    /// A borrowed snapshot of the frozen value, for `shared.with`. It adds no root:
    /// the graph stays frozen while the caller's root lends it.
    pub(crate) fn view(&self) -> Result<Value, crate::Error> {
        self.graph()
            .value
            .clone()
            .ok_or_else(|| crate::invalid("read of a dead Shared root"))
    }
    /// The owned value when this is the last root, else the root unchanged.
    pub(crate) fn thaw(self) -> Result<Result<Value, SharedRoot>, crate::Error> {
        let mut graph = self.graph();
        if graph.roots != 1 {
            drop(graph);
            return Ok(Err(self));
        }
        graph.roots = 0;
        let value = graph
            .value
            .take()
            .ok_or_else(|| crate::invalid("thaw of a dead Shared root"))?;
        Ok(Ok(value))
    }
}
impl std::fmt::Debug for SharedRoot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Shared#{}", self.id)
    }
}
impl PartialEq for SharedRoot {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl Eq for SharedRoot {}

/// The storage of a List or Array block placed in an arena (MEM-6): the arena's
/// handle id, its element's logical size and whether the storage is shared. Every clone of one value shares the
/// cell, so a copy marks both holders. It is unobservable: equality ignores it.
#[derive(Clone, Debug)]
pub struct ArenaStorage {
    pub arena: u64,
    /// The logical bytes of one element: a push's charge (0 in an unbounded arena).
    pub element: u64,
    shared: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl ArenaStorage {
    /// Unshared storage built in `arena`.
    pub fn fresh(arena: u64, element: u64) -> Self {
        Self {
            arena,
            element,
            shared: Default::default(),
        }
    }
    pub fn shared(&self) -> bool {
        self.shared.load(std::sync::atomic::Ordering::Relaxed)
    }
    pub fn share(&self) {
        self.shared
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}
impl PartialEq for ArenaStorage {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}
impl Eq for ArenaStorage {}

/// The element storage of a List, Array, Map or Set block. A copy shares it and
/// the first write through a holder detaches that holder's own copy (Arc
/// make_mut), so snapshots, borrows and moves cost O(1) while every holder still
/// observes value semantics. Sharing is the reference model's own cost model,
/// never a semantic fact: no observation, equality or RC oracle reads it.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct Block(std::sync::Arc<Vec<Option<Value>>>);
impl Block {
    /// The elements, copied only when another holder shares them.
    pub fn into_vec(self) -> Vec<Option<Value>> {
        std::sync::Arc::try_unwrap(self.0).unwrap_or_else(|shared| (*shared).clone())
    }
    /// The elements when this is their only holder; a shared block yields
    /// nothing, since releasing this holder releases no element.
    fn take_unique(&mut self) -> Option<Vec<Option<Value>>> {
        std::sync::Arc::get_mut(&mut self.0).map(std::mem::take)
    }
}
impl std::ops::Deref for Block {
    type Target = Vec<Option<Value>>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for Block {
    fn deref_mut(&mut self) -> &mut Self::Target {
        if std::sync::Arc::get_mut(&mut self.0).is_none() {
            hirsel_intern::work::count_by(
                hirsel_intern::work::Counter::InterpCopiedValues,
                self.0.len() as u64,
            );
        }
        std::sync::Arc::make_mut(&mut self.0)
    }
}
impl From<Vec<Option<Value>>> for Block {
    fn from(elements: Vec<Option<Value>>) -> Self {
        Self(std::sync::Arc::new(elements))
    }
}
impl FromIterator<Option<Value>> for Block {
    fn from_iter<I: IntoIterator<Item = Option<Value>>>(elements: I) -> Self {
        Self::from(elements.into_iter().collect::<Vec<_>>())
    }
}
impl IntoIterator for Block {
    type Item = Option<Value>;
    type IntoIter = std::vec::IntoIter<Option<Value>>;
    fn into_iter(self) -> Self::IntoIter {
        self.into_vec().into_iter()
    }
}
impl<'a> IntoIterator for &'a Block {
    type Item = &'a Option<Value>;
    type IntoIter = std::slice::Iter<'a, Option<Value>>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}
impl std::fmt::Debug for Block {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum Value {
    FunctionReference(NamedFunction),
    Shared(SharedRoot),
    /// Opaque simulation identity. Tags come from the library registry, never addresses.
    Handle {
        kind: String,
        id: u64,
    },
    Unit,
    Bool(bool),
    Integer(Integer),
    F32(u32),
    F64(u64),
    /// A Unicode scalar value (`char`).
    Char(char),
    String(String),
    Bytes(Vec<u8>),
    /// A Map's entries in insertion order; `arena` as for a Sequence (MEM-6):
    /// a Map built in an arena, None elsewhere. Its element charge is one
    /// entry's.
    Map {
        entries: Block,
        arena: Option<ArenaStorage>,
    },
    Set {
        entries: Block,
        arena: Option<ArenaStorage>,
    },
    Tuple(Vec<Option<Value>>),
    Record {
        definition: DefPath,
        fields: Vec<Option<Value>>,
    },
    Sequence {
        kind: BuiltinType,
        elements: Block,
        /// A List or Array block placed in an arena (MEM-6); None elsewhere.
        arena: Option<ArenaStorage>,
    },
    Box(Vec<Option<Value>>),
    Distinct {
        definition: DefPath,
        fields: Vec<Option<Value>>,
    },
    ResourceVariant {
        sizes: Vec<Option<u64>>,
        definition: DefPath,
        variant: u32,
        fields: Vec<Option<Value>>,
    },
    Resource {
        /// Concrete type-size bindings retained until the finalizer runs.
        sizes: Vec<Option<u64>>,
        definition: DefPath,
        fields: Vec<Option<Value>>,
    },
    Closure {
        /// Once-environment snapshots share the fact that invocation moved its
        /// owners into a frame. Many-use environments have no invocation state.
        invoked: Option<std::sync::Arc<std::sync::OnceLock<()>>>,
        definition: DefPath,
        body: BodyId,
        captures: Vec<Option<Value>>,
        /// Borrow/mut captures are snapshots owned by the enclosing frame.
        owned_captures: Vec<bool>,
        /// Lexical size bindings travel with the environment, including callbacks.
        sizes: SizeBindings,
        type_arguments: crate::type_arguments::Bindings,
    },
    Variant {
        variant: u32,
        fields: Vec<Option<Value>>,
    },
}

impl Value {
    pub(crate) fn begin_call(&self) {
        if let Self::Closure {
            invoked: Some(invoked),
            ..
        } = self
        {
            let _ = invoked.set(());
        }
    }
    /// Writable fields. A shared block is detached first (copy on write).
    pub(crate) fn fields_mut(&mut self) -> Option<&mut Vec<Option<Value>>> {
        match self {
            Self::Map { entries: f, .. }
            | Self::Set { entries: f, .. }
            | Self::Sequence { elements: f, .. } => Some(f),
            Self::Tuple(f)
            | Self::Box(f)
            | Self::Record { fields: f, .. }
            | Self::Variant { fields: f, .. }
            | Self::Resource { fields: f, .. }
            | Self::ResourceVariant { fields: f, .. }
            | Self::Distinct { fields: f, .. }
            | Self::Closure { captures: f, .. } => Some(f),
            _ => None,
        }
    }
    /// The shared element storage of a List, Array, Map or Set.
    fn block(&self) -> Option<&Block> {
        match self {
            Self::Map { entries: b, .. }
            | Self::Set { entries: b, .. }
            | Self::Sequence { elements: b, .. } => Some(b),
            _ => None,
        }
    }

    /// Normalize external inputs and operation results as well as TIR literals.
    pub(crate) fn canonicalize(&mut self) {
        let mut pending = vec![self];
        while let Some(value) = pending.pop() {
            match value {
                Value::F32(bits) if f32::from_bits(*bits).is_nan() => *bits = 0x7fc0_0000,
                Value::F64(bits) if f64::from_bits(*bits).is_nan() => *bits = 0x7ff8_0000_0000_0000,
                value => {
                    let Some(fields) = value.fields_mut() else {
                        continue;
                    };
                    pending.extend(fields.iter_mut().filter_map(Option::as_mut));
                }
            }
        }
    }
}

/// A sized two's-complement bit vector; construction rejects out-of-width bits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Integer {
    kind: Primitive,
    bits: u128,
}
impl Integer {
    pub fn new(kind: Primitive, bits: u128) -> Option<Self> {
        let (width, _) = integer_layout(kind)?;
        (bits & !mask(width) == 0).then_some(Self { kind, bits })
    }
    pub fn kind(self) -> Primitive {
        self.kind
    }
    pub fn bits(self) -> u128 {
        self.bits
    }
    pub fn width(self) -> u32 {
        integer_layout(self.kind).unwrap().0
    }
    pub fn is_signed(self) -> bool {
        integer_layout(self.kind).unwrap().1
    }
    pub fn signed(self) -> i128 {
        let shift = 128 - self.width();
        ((self.bits << shift) as i128) >> shift
    }
    pub(crate) fn masked(self, bits: u128) -> Self {
        Self {
            bits: bits & mask(self.width()),
            ..self
        }
    }
}
pub fn integer_layout(kind: Primitive) -> Option<(u32, bool)> {
    use Primitive::*;
    Some(match kind {
        U8 => (8, false),
        U16 => (16, false),
        U32 => (32, false),
        U64 => (64, false),
        U128 => (128, false),
        I8 => (8, true),
        I16 => (16, true),
        I32 => (32, true),
        I64 => (64, true),
        I128 => (128, true),
        _ => return None,
    })
}
pub(crate) fn mask(width: u32) -> u128 {
    u128::MAX >> (128 - width)
}
pub(crate) fn signed_bounds(width: u32) -> (i128, i128) {
    let max = (mask(width) >> 1) as i128;
    (-max - 1, max)
}
/// SEM-2: canonical positive quiet NaN, including constants and unary results.
pub fn f32_value(value: f32) -> Value {
    Value::F32(if value.is_nan() {
        0x7fc0_0000
    } else {
        value.to_bits()
    })
}
pub fn f64_value(value: f64) -> Value {
    Value::F64(if value.is_nan() {
        0x7ff8_0000_0000_0000
    } else {
        value.to_bits()
    })
}

impl Clone for Value {
    fn clone(&self) -> Self {
        fn shallow(v: &Value) -> Value {
            match v {
                Value::FunctionReference(reference) => Value::FunctionReference(reference.clone()),
                Value::Unit => Value::Unit,
                Value::Bool(x) => Value::Bool(*x),
                Value::Integer(x) => Value::Integer(*x),
                Value::F32(x) => Value::F32(*x),
                Value::F64(x) => Value::F64(*x),
                Value::Char(x) => Value::Char(*x),
                Value::String(x) => Value::String(x.clone()),
                Value::Bytes(x) => Value::Bytes(x.clone()),
                Value::Handle { kind, id } => Value::Handle {
                    kind: kind.clone(),
                    id: *id,
                },
                Value::Shared(root) => Value::Shared(root.clone()),
                Value::Map { entries, arena } => Value::Map {
                    entries: entries.clone(),
                    arena: arena.clone(),
                },
                Value::Set { entries, arena } => Value::Set {
                    entries: entries.clone(),
                    arena: arena.clone(),
                },
                Value::Tuple(f) => Value::Tuple(vec![None; f.len()]),
                Value::Box(f) => Value::Box(vec![None; f.len()]),
                Value::Record { definition, fields } => Value::Record {
                    definition: definition.clone(),
                    fields: vec![None; fields.len()],
                },
                Value::Distinct { definition, fields } => Value::Distinct {
                    definition: definition.clone(),
                    fields: vec![None; fields.len()],
                },
                Value::ResourceVariant {
                    sizes,
                    definition,
                    variant,
                    fields,
                } => Value::ResourceVariant {
                    sizes: sizes.clone(),
                    definition: definition.clone(),
                    variant: *variant,
                    fields: vec![None; fields.len()],
                },
                Value::Resource {
                    definition,
                    fields,
                    sizes,
                } => Value::Resource {
                    sizes: sizes.clone(),
                    definition: definition.clone(),
                    fields: vec![None; fields.len()],
                },
                Value::Variant { variant, fields } => Value::Variant {
                    variant: *variant,
                    fields: vec![None; fields.len()],
                },
                Value::Sequence {
                    kind,
                    elements,
                    arena,
                } => Value::Sequence {
                    kind: *kind,
                    elements: elements.clone(),
                    arena: arena.clone(),
                },
                Value::Closure {
                    invoked,
                    definition,
                    body,
                    captures,
                    owned_captures,
                    sizes,
                    type_arguments,
                } => Value::Closure {
                    invoked: invoked.clone(),
                    definition: definition.clone(),
                    body: *body,
                    captures: vec![None; captures.len()],
                    owned_captures: owned_captures.clone(),
                    sizes: sizes.clone(),
                    type_arguments: type_arguments.clone(),
                },
            }
        }
        let mut result = shallow(self);
        let mut copied = 0;
        let mut work = vec![(self, &mut result)];
        while let Some((source, target)) = work.pop() {
            // A block's copy shares its elements already.
            if source.block().is_some() {
                continue;
            }
            if let (Some(src), Some(dst)) = (source.fields(), target.fields_mut()) {
                for (src, dst) in src.iter().zip(dst.iter_mut()) {
                    if let Some(src) = src {
                        *dst = Some(shallow(src));
                        copied += 1;
                        work.push((src, dst.as_mut().unwrap()));
                    }
                }
            }
        }
        hirsel_intern::work::count_by(hirsel_intern::work::Counter::InterpCopiedValues, copied);
        result
    }
}
impl Value {
    /// An owned semantic copy: the clone, with one more root for every
    /// `Shared` root it reaches (copying never traverses Shared contents).
    pub(crate) fn copied(&self) -> Value {
        self.acquire_roots();
        self.clone()
    }
    /// Add a root to every `Shared` root an owned copy of this value reaches.
    pub(crate) fn acquire_roots(&self) {
        let mut work = vec![self];
        while let Some(value) = work.pop() {
            match value {
                Value::Shared(root) => root.acquire(),
                Value::Closure {
                    captures,
                    owned_captures,
                    ..
                } => work.extend(
                    captures
                        .iter()
                        .zip(owned_captures)
                        .filter_map(|(v, owned)| owned.then(|| v.as_ref()).flatten()),
                ),
                _ => work.extend(value.fields().into_iter().flatten().flatten()),
            }
        }
    }
    pub(crate) fn fields(&self) -> Option<&[Option<Value>]> {
        match self {
            Self::Closure {
                invoked: Some(invoked),
                ..
            } if invoked.get().is_some() => Some(&[]),
            Self::Map { entries: f, .. }
            | Self::Set { entries: f, .. }
            | Self::Sequence { elements: f, .. } => Some(f),
            Self::Tuple(f)
            | Self::Box(f)
            | Self::Record { fields: f, .. }
            | Self::Distinct { fields: f, .. }
            | Self::Resource { fields: f, .. }
            | Self::ResourceVariant { fields: f, .. }
            | Self::Variant { fields: f, .. }
            | Self::Closure { captures: f, .. } => Some(f),
            _ => None,
        }
    }
    /// Free a simulation tree without running finalizers or recursing in Rust.
    /// Engines use the cleanup worklist; this is for consuming tooling snapshots.
    pub fn discard(self) {
        let mut work = vec![self];
        while let Some(mut value) = work.pop() {
            let fields = match &mut value {
                Value::Map { entries: b, .. }
                | Value::Set { entries: b, .. }
                | Value::Sequence { elements: b, .. } => b.take_unique(),
                value => value.fields_mut().map(std::mem::take),
            };
            work.extend(fields.into_iter().flatten().flatten());
        }
    }
}

/// The interpreter's memory model for `budgets.limits@1` (`memory_bytes`): every value
/// node is 16 bytes, and a String's or Bytes' contents add their length. A frozen
/// `Shared` root counts as its node only, so a value several roots share is never
/// counted twice.
pub(crate) const NODE_BYTES: u64 = 16;
impl Value {
    /// This value's logical bytes, over its whole tree.
    pub(crate) fn logical_bytes(&self) -> u64 {
        let mut total = 0u64;
        let mut work = vec![self];
        while let Some(value) = work.pop() {
            total = total.saturating_add(match value {
                Self::String(text) => NODE_BYTES.saturating_add(text.len() as u64),
                Self::Bytes(bytes) => NODE_BYTES.saturating_add(bytes.len() as u64),
                _ => NODE_BYTES,
            });
            if let Some(fields) = value.fields() {
                work.extend(fields.iter().flatten());
            }
        }
        total
    }
    /// An estimate, in constant time, of the logical bytes this value's own node and
    /// its direct fields hold: what a construction or a library result is charged
    /// before the model is next measured.
    pub(crate) fn shallow_bytes(&self) -> u64 {
        match self {
            Self::String(text) => NODE_BYTES.saturating_add(text.len() as u64),
            Self::Bytes(bytes) => NODE_BYTES.saturating_add(bytes.len() as u64),
            _ => NODE_BYTES.saturating_add(
                NODE_BYTES.saturating_mul(self.fields().map_or(0, |f| f.len()) as u64),
            ),
        }
    }
}

impl Value {
    /// A sequence value outside any arena.
    pub fn sequence(kind: BuiltinType, elements: Vec<Option<Value>>) -> Self {
        Self::Sequence {
            kind,
            elements: elements.into(),
            arena: None,
        }
    }
    /// A Map outside any arena.
    pub fn map(entries: Vec<Option<Value>>) -> Self {
        Self::Map {
            entries: entries.into(),
            arena: None,
        }
    }
    /// A Set outside any arena.
    pub fn set(entries: Vec<Option<Value>>) -> Self {
        Self::Set {
            entries: entries.into(),
            arena: None,
        }
    }
    /// The arena storage of a List, Array, Map or Set block placed in an arena
    /// (MEM-6).
    pub(crate) fn arena_storage(&self) -> Option<&ArenaStorage> {
        match self {
            Self::Sequence { arena, .. } | Self::Map { arena, .. } | Self::Set { arena, .. } => {
                arena.as_ref()
            }
            Self::Record { definition, fields } if definition.is_core("SortedMap") => {
                fields.get(1)?.as_ref()?.arena_storage()
            }
            _ => None,
        }
    }
    pub(crate) fn arena_storage_mut(&mut self) -> Option<&mut Option<ArenaStorage>> {
        match self {
            Self::Sequence { arena, .. } | Self::Map { arena, .. } | Self::Set { arena, .. } => {
                Some(arena)
            }
            Self::Record { definition, fields } if definition.is_core("SortedMap") => {
                fields.get_mut(1)?.as_mut()?.arena_storage_mut()
            }
            _ => None,
        }
    }
    /// MEM-6: a semantic copy of this value shares every arena List, Array, Map
    /// or Set of its tree. Native code marks only the blocks it holds at inline
    /// depth, but a block behind another one cannot be written or moved out
    /// until that one is, and copying that one on its write gives each child a
    /// second holder.
    pub(crate) fn share_arena_storage(&self) {
        let mut work = vec![self];
        while let Some(v) = work.pop() {
            if let Some(arena) = v.arena_storage() {
                arena.share();
            }
            if let Some(fields) = v.fields() {
                work.extend(fields.iter().flatten());
            }
        }
    }
    /// MEM-6: a deep copy (`unique x`) builds every block anew where its source
    /// lives, so each arena List, Array, Map or Set of it is fresh, unshared
    /// storage there.
    pub(crate) fn fresh_arena_storage(&mut self) {
        let mut work = vec![self];
        while let Some(v) = work.pop() {
            if let Some(Some(storage)) = v.arena_storage_mut() {
                *storage = ArenaStorage::fresh(storage.arena, storage.element);
            }
            if let Some(fields) = v.fields_mut() {
                work.extend(fields.iter_mut().flatten());
            }
        }
    }
    /// Whether this value is a block reference rather than inline storage.
    pub(crate) fn is_block(&self) -> bool {
        match self {
            Self::Sequence { kind, .. } => !matches!(kind, BuiltinType::Inline | BuiltinType::Simd),
            Self::Box(_)
            | Self::Map { .. }
            | Self::Set { .. }
            | Self::Closure { .. }
            | Self::String(_)
            | Self::Bytes(_) => true,
            _ => false,
        }
    }
    pub(crate) fn inline_bits(&self) -> bool {
        let mut work = vec![self];
        while let Some(v) = work.pop() {
            match v {
                Self::Unit
                | Self::Bool(_)
                | Self::Integer(_)
                | Self::F32(_)
                | Self::F64(_)
                | Self::Handle { .. } => (),
                Self::Tuple(f)
                | Self::Record { fields: f, .. }
                | Self::Distinct { fields: f, .. }
                | Self::Variant { fields: f, .. } => {
                    work.extend(f.iter().filter_map(Option::as_ref))
                }
                _ => return false,
            }
        }
        true
    }
}
