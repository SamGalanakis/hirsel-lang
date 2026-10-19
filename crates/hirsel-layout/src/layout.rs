//! Layout derivation: the single physical layout authority.
//!
//! A layout depends only on type structure, size actuals and owner
//! representation records. The key encoding never reads a relation, brand,
//! uniqueness or sendability mode, so instances differing only there share one
//! `LayoutId`; a callable's multiplicity is kept because it selects the copy
//! recipe. Owner-specific allocation facts (block capacity, size class, storage
//! domain) stay in the header and never enter a per-type layout.
//!
//! Unresolved attribute combinations are native-unavailable with their issue
//! (#47 SoA/attribute composition, #6 C interop), never guessed and never a
//! source rejection. Private FlatFormatV0 is independent of SER-2 wire bytes.
use hirsel_core_contract::Passing;
use hirsel_lir::layout_types::*;
use hirsel_lir::runtime_abi::{HEADER_SIZE, HEADER_V2};
use hirsel_syntax::{ContentHash, DefPath};
use hirsel_tir::RepresentationRecord;
use hirsel_types::{
    BinderRef, BuiltinType, GenericArgument, GenericParameter, Layout, Multiplicity, Primitive,
    Size, SizeParameterRef, TypeId, TypeKind, TypeStore,
};
use std::collections::{BTreeMap, HashMap};

use hirsel_lir::layout::*;

/// Tooling limits on instance expansion; reaching one is operational, never a
/// source rejection (generic code).
pub const MAX_LAYOUTS: usize = 1 << 20;
/// Largest value whose scalar leaves are listed for C classification. Every
/// executed C convention passes a larger aggregate in memory.
pub const ABI_LEAF_BYTES: u64 = 32;

fn unavailable<T>(issue: Option<u32>, what: impl Into<String>) -> Result<T, LayoutError> {
    Err(LayoutError::Unavailable {
        issue,
        what: what.into(),
    })
}
fn invalid<T>(what: impl Into<String>) -> Result<T, LayoutError> {
    Err(LayoutError::Invalid(what.into()))
}

/// A nominal owner's representation input.
#[derive(Clone, Debug)]
pub enum OwnerRepresentation {
    /// A source-defined type: the owner-only record.
    Defined(RepresentationRecord),
    /// A capability, Path or foreign-handle type: a compact run-context ID,
    /// never an address. A resource handle closes on drop.
    Handle {
        revision: ContentHash,
        resource: bool,
    },
    /// The borrowed collection cursor: opaque block source, position and end.
    /// Its source is scanned as uncounted and never dropped.
    Cursor { revision: ContentHash },
    /// Unicode borrowed cursor: `{ view: String slice, position: u64 }`.
    ViewCursor { revision: ContentHash },
    /// A core string view has exactly String's layout, including its copy recipe.
    StringView,
}

/// The library-owned opaque containers, whose private shapes come from owners.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Container {
    Map,
    Set,
    SortedMap,
    Slots,
    SlotId,
}

/// Every representation record layout derivation may read.
#[derive(Clone, Debug, Default)]
pub struct Owners {
    pub nominals: BTreeMap<DefPath, OwnerRepresentation>,
    /// Map, Set, SortedMap and Slots are counted blocks whose payload is the
    /// owner's private shape; SlotId is that shape inline.
    pub containers: BTreeMap<Container, RepresentationRecord>,
}

/// A concrete closure or `Iter` environment (Closures).
#[derive(Clone, Copy, Debug)]
pub struct EnvironmentShape<'s> {
    /// The closure body: its definition and TIR body ordinal.
    pub definition: &'s DefPath,
    pub body: u32,
    pub store: &'s TypeStore,
    /// Captures in declaration order. Take captures are owned inline values;
    /// Borrow captures are the borrowed views themselves, their holders
    /// scanned as uncounted and never dropped; Mut captures are addresses.
    pub captures: &'s [(TypeId, Passing)],
    /// Captured outer size substitutions, folded to constants.
    pub sizes: &'s [u64],
    /// Captures that the once body moves out, in increasing ordinal order.
    pub movable: &'s [u32],
}

/// One generic frame: the store a type row lives in, and the actuals its
/// depth-zero parameters resolve to, which live in frame `parent`.
#[derive(Clone, Copy)]
struct Frame<'s> {
    store: &'s TypeStore,
    actuals: &'s [GenericArgument],
    parent: usize,
}

struct Walk<'s> {
    frames: Vec<Frame<'s>>,
    /// Block payloads reserved during this derivation and derived once the
    /// inline layouts they read are complete (a `List[Tree]` inside `Tree`).
    pending: Vec<Pending<'s>>,
    /// Box payloads reserved but not yet derived, by id. An inline use or a
    /// sequence of one derives it on the spot; `drain` derives the rest.
    deferred: BTreeMap<LayoutId, Deferred<'s>>,
}

/// A reserved Box payload: its memory key, type and generic frames.
struct Deferred<'s> {
    key: Vec<u8>,
    ty: TypeId,
    frames: Vec<Frame<'s>>,
    frame: usize,
}

/// A reserved block payload awaiting derivation.
enum Pending<'s> {
    Sequence {
        id: LayoutId,
        element: LayoutId,
    },
    Soa {
        id: LayoutId,
        element: LayoutId,
    },
    /// A `Shared` root block's payload (MEM-11).
    Frozen {
        id: LayoutId,
        value: LayoutId,
    },
    Container {
        id: LayoutId,
        frames: Vec<Frame<'s>>,
        frame: usize,
        owner: &'s RepresentationRecord,
    },
}

impl<'s> Walk<'s> {
    fn new(store: &'s TypeStore) -> Self {
        Self {
            frames: vec![Frame {
                store,
                actuals: &[],
                parent: 0,
            }],
            pending: Vec::new(),
            deferred: BTreeMap::new(),
        }
    }
    fn row(&self, ty: TypeId, f: usize) -> Result<&'s hirsel_types::Type, LayoutError> {
        let store = self.frames[f].store;
        // Compare row identity first: `TypeStore::get` asserts on foreign IDs.
        if store.id(ty.index()) != Some(ty) {
            return Err(LayoutError::MissingType);
        }
        store.get(ty).ok_or(LayoutError::MissingType)
    }
    /// Follow parameters to the actual they denote, across frames.
    fn resolve(&self, mut ty: TypeId, mut f: usize) -> Result<(TypeId, usize), LayoutError> {
        for _ in 0..=self.frames.len() {
            let TypeKind::Parameter(reference) = &self.row(ty, f)?.kind else {
                return Ok((ty, f));
            };
            let BinderRef::Bound {
                depth: 0,
                parameter,
            } = *reference
            else {
                return Err(LayoutError::Incomplete);
            };
            if f == 0 {
                return Err(LayoutError::Incomplete);
            }
            let frame = self.frames[f];
            match frame.actuals.get(parameter.0 as usize) {
                Some(GenericArgument::Type(actual)) => (ty, f) = (*actual, frame.parent),
                _ => return invalid("type parameter has no type actual"),
            }
        }
        invalid("parameter resolution does not terminate")
    }
    fn size(&self, size: &Size, f: usize) -> Result<u64, LayoutError> {
        match size {
            Size::Constant(n) => Ok(*n),
            Size::Parameter(reference) => self.size_parameter(*reference, f),
        }
    }
    fn size_parameter(&self, reference: SizeParameterRef, f: usize) -> Result<u64, LayoutError> {
        let BinderRef::Bound {
            depth: 0,
            parameter,
        } = reference
        else {
            return Err(LayoutError::Incomplete);
        };
        if f == 0 {
            return Err(LayoutError::Incomplete);
        }
        let frame = self.frames[f];
        match frame.actuals.get(parameter.0 as usize) {
            Some(GenericArgument::Size(size)) => self.size(size, frame.parent),
            _ => invalid("size parameter has no size actual"),
        }
    }
}

/// Length-framed canonical bytes of the layout-relevant structure of a type.
struct Encoder {
    bytes: Vec<u8>,
    /// Fold each nominal's private representation revision in (layout keys);
    /// without it the encoding is the public identity (ABI keys).
    revisions: bool,
}

impl Encoder {
    fn atom(&mut self, b: &[u8]) {
        self.bytes
            .extend_from_slice(&(b.len() as u64).to_le_bytes());
        self.bytes.extend_from_slice(b);
    }
    fn tag(&mut self, s: &str) {
        self.atom(s.as_bytes());
    }
    fn number(&mut self, n: u64) {
        self.atom(&n.to_le_bytes());
    }
    fn path(&mut self, p: &DefPath) {
        self.tag(&p.0.package.name);
        self.tag(&p.0.package.version);
        self.number(p.0.modules.len() as u64);
        for m in &p.0.modules {
            self.tag(m);
        }
        self.tag(&p.0.name);
    }
}

fn describe(p: &DefPath) -> String {
    let mut s = p.0.modules.join(".");
    if !s.is_empty() {
        s.push('.');
    }
    s.push_str(&p.0.name);
    s
}

fn primitive_name(p: Primitive) -> &'static str {
    match p {
        Primitive::Bool => "bool",
        Primitive::U8 => "u8",
        Primitive::U16 => "u16",
        Primitive::U32 => "u32",
        Primitive::U64 => "u64",
        Primitive::U128 => "u128",
        Primitive::I8 => "i8",
        Primitive::I16 => "i16",
        Primitive::I32 => "i32",
        Primitive::I64 => "i64",
        Primitive::I128 => "i128",
        Primitive::F32 => "f32",
        Primitive::F64 => "f64",
        Primitive::Char => "char",
        Primitive::String => "String",
        Primitive::Bytes => "Bytes",
        Primitive::Never => "never",
    }
}

fn primitive_scalar(p: Primitive) -> Option<ScalarKind> {
    Some(match p {
        Primitive::Bool => ScalarKind::Bool,
        Primitive::U8 => ScalarKind::U8,
        Primitive::U16 => ScalarKind::U16,
        // A char is its scalar value's u32 word (0..=0x10FFFF, no surrogates).
        Primitive::U32 | Primitive::Char => ScalarKind::U32,
        Primitive::U64 => ScalarKind::U64,
        Primitive::U128 => ScalarKind::U128,
        Primitive::I8 => ScalarKind::I8,
        Primitive::I16 => ScalarKind::I16,
        Primitive::I32 => ScalarKind::I32,
        Primitive::I64 => ScalarKind::I64,
        Primitive::I128 => ScalarKind::I128,
        Primitive::F32 => ScalarKind::F32,
        Primitive::F64 => ScalarKind::F64,
        Primitive::String | Primitive::Bytes | Primitive::Never => return None,
    })
}

fn builtin_name(kind: BuiltinType) -> &'static str {
    match kind {
        BuiltinType::Array => "Array",
        BuiltinType::List => "List",
        BuiltinType::Map => "Map",
        BuiltinType::Set => "Set",
        BuiltinType::SortedMap => "SortedMap",
        BuiltinType::Inline => "Inline",
        BuiltinType::Simd => "Simd",
        BuiltinType::Packed => "Packed",
        BuiltinType::Shared => "Shared",
        BuiltinType::Slots => "Slots",
        BuiltinType::SlotId => "SlotId",
        BuiltinType::Region => "Region",
        BuiltinType::Iter => "Iter",
        BuiltinType::Task => "Task",
        BuiltinType::Sender => "Sender",
        BuiltinType::Receiver => "Receiver",
        BuiltinType::Mutex => "Mutex",
        BuiltinType::Atomic => "Atomic",
        BuiltinType::Ptr => "Ptr",
        BuiltinType::FnPtr => "FnPtr",
        BuiltinType::TypeShape => "TypeShape",
    }
}

fn container(kind: BuiltinType) -> Option<Container> {
    Some(match kind {
        BuiltinType::Map => Container::Map,
        BuiltinType::Set => Container::Set,
        BuiltinType::SortedMap => Container::SortedMap,
        BuiltinType::Slots => Container::Slots,
        BuiltinType::SlotId => Container::SlotId,
        _ => return None,
    })
}

fn multiplicity(m: Multiplicity) -> hirsel_lir::abi_types::CallMultiplicity {
    match m {
        Multiplicity::Many => hirsel_lir::abi_types::CallMultiplicity::Many,
        Multiplicity::Once => hirsel_lir::abi_types::CallMultiplicity::Once,
    }
}

/// A field list's steps and facts lifted to its parent's offsets.
struct Lifted {
    scan: Vec<ScanStep>,
    drop: Vec<DropStep>,
    copy: CopyRecipe,
    leaves: Vec<(u64, ScalarKind)>,
    unaligned: bool,
}

/// How a field participates in the inline layout of its parent.
struct Piece {
    layout: LayoutId,
    size: u64,
    align: u32,
}

/// Builds one `LayoutTable` for one target. Derivation is deterministic: the
/// same requests in the same order give the same table and digest.
pub struct LayoutBuilder<'o> {
    target: TargetSpec,
    owners: &'o Owners,
    table: LayoutTable,
    memo: HashMap<Vec<u8>, LayoutId>,
    done: Vec<bool>,
    pub(crate) abi_memo:
        HashMap<(ContentHash, hirsel_lir::abi_types::CallableShape), hirsel_lir::FnAbiId>,
    depth: u32,
}

impl<'o> LayoutBuilder<'o> {
    pub fn new(target: &TargetSpec, owners: &'o Owners) -> Self {
        Self {
            target: target.clone(),
            owners,
            table: LayoutTable::new(target),
            memo: HashMap::new(),
            done: Vec::new(),
            abi_memo: HashMap::new(),
            depth: 0,
        }
    }
    pub fn target(&self) -> &TargetSpec {
        &self.target
    }
    pub fn owners(&self) -> &'o Owners {
        self.owners
    }
    pub fn table(&self) -> &LayoutTable {
        &self.table
    }
    /// Seal the table. It always holds `Scalar(Bool)`, which a stage may need
    /// after sealing (a uniqueness test's result) whether or not the source
    /// names a bool; appended last, so no existing id moves.
    pub fn finish(mut self) -> LayoutTable {
        self.scalar(ScalarKind::Bool);
        self.table
    }
    pub(crate) fn table_mut_fn_abis(&mut self) -> &mut Vec<hirsel_lir::abi_types::FnAbi> {
        &mut self.table.fn_abis
    }
    pub fn record(&self, id: LayoutId) -> &LayoutRecord {
        &self.table.layouts[id.0 as usize]
    }

    /// The in-memory layout of a complete type.
    pub fn layout(&mut self, store: &TypeStore, ty: TypeId) -> Result<LayoutId, LayoutError> {
        self.transaction(|b| {
            let mut walk = Walk::new(store);
            let id = b.memory(&mut walk, ty, 0, false)?;
            b.drain(&mut walk)?;
            Ok(id)
        })
    }

    /// The FlatFormatV0 byte extent of a flat-compatible type, as its
    /// out-of-line storage holds it (private native format).
    pub fn flat(&mut self, store: &TypeStore, ty: TypeId) -> Result<LayoutId, LayoutError> {
        self.transaction(|b| {
            let mut walk = Walk::new(store);
            let id = b.flat_body(&mut walk, ty, 0)?;
            b.drain(&mut walk)?;
            Ok(id)
        })
    }

    /// The counted block header v2, transcribed from `runtime_abi::HEADER_V2`.
    pub fn header(&mut self) -> LayoutId {
        let mut key = Encoder {
            bytes: Vec::new(),
            revisions: true,
        };
        key.tag("header");
        key.number(u64::from(hirsel_lir::RUNTIME_ABI_VERSION));
        if let Some(&id) = self.memo.get(&key.bytes) {
            return id;
        }
        let fields = HEADER_V2
            .iter()
            .enumerate()
            .map(|(ordinal, &(_, scalar, offset))| FieldLayout {
                ordinal: ordinal as u32,
                offset,
                layout: self.scalar(scalar),
                unaligned: false,
            })
            .collect();
        let mut record = self.blank(
            &key.bytes,
            LayoutKind::Header,
            SizeSpec::Fixed(HEADER_SIZE),
            8,
        );
        record.fields = fields;
        record.copy = CopyRecipe::NotCopyable;
        record.storage = vec![StorageKind::Heap, StorageKind::Static];
        record.abi.scalars = Vec::new();
        self.insert(key.bytes, record)
    }

    /// The environment record of a concrete closure or `Iter` state: captures
    /// in declaration order, its drop program its finalizer instance, and the
    /// captured size substitutions part of its key.
    pub fn environment(&mut self, env: &EnvironmentShape<'_>) -> Result<LayoutId, LayoutError> {
        self.transaction(|b| {
            let mut walk = Walk::new(env.store);
            let mut key = Encoder {
                bytes: Vec::new(),
                revisions: true,
            };
            key.tag("environment");
            key.number(env.movable.len() as u64);
            for &capture in env.movable {
                key.number(u64::from(capture));
            }
            if env.movable.len() > hirsel_lir::ir::LIVE_MASK_WORD_BITS as usize {
                return unavailable(
                    None,
                    hirsel_lir::ir::live_mask_word_limit(
                        &format!(
                            "closure {} body {} with {} movable captures",
                            env.definition.0.name,
                            env.body,
                            env.movable.len()
                        ),
                        env.movable.len(),
                    ),
                );
            }
            if env.movable.windows(2).any(|w| w[0] >= w[1])
                || env.movable.iter().any(|&i| {
                    env.captures
                        .get(i as usize)
                        .is_none_or(|(_, p)| *p != Passing::Take)
                })
            {
                return invalid("invalid movable environment captures");
            }
            key.path(env.definition);
            key.number(u64::from(env.body));
            key.number(env.sizes.len() as u64);
            env.sizes.iter().for_each(|&s| key.number(s));
            key.number(env.captures.len() as u64);
            for (ty, passing) in env.captures {
                key.tag(match passing {
                    Passing::Borrow => "borrow",
                    Passing::Take => "take",
                    Passing::Mut => "mut",
                });
                b.encode(&walk, *ty, 0, &mut key)?;
            }
            if let Some(&id) = b.memo.get(&key.bytes) {
                return Ok(id);
            }
            let mut pieces = Vec::new();
            for (ty, passing) in env.captures {
                let piece = match passing {
                    // A borrowed capture is the captured view itself.
                    Passing::Take | Passing::Borrow => b.piece(&mut walk, *ty, 0)?,
                    Passing::Mut => {
                        let layout = b.scalar(ScalarKind::Pointer);
                        Piece {
                            layout,
                            size: b.target.scalar_size(ScalarKind::Pointer),
                            align: b.target.scalar_align(ScalarKind::Pointer),
                        }
                    }
                };
                pieces.push(piece);
            }
            if !env.movable.is_empty() {
                let layout = b.scalar(ScalarKind::U64);
                pieces.push(Piece {
                    layout,
                    size: 8,
                    align: b.target.scalar_align(ScalarKind::U64),
                });
            }
            let passing: Vec<_> = env.captures.iter().map(|(_, p)| *p).collect();
            let id = b.environment_record(&key.bytes, pieces, &passing, env.movable)?;
            b.drain(&mut walk)?;
            Ok(id)
        })
    }

    /// An ordinary environment record from already concrete data layouts.
    /// This carries no code pointer or callable type. Borrowed fields retain
    /// their uncounted scan and absent drop, just as closure captures do.
    pub fn environment_data(
        &mut self,
        definition: &DefPath,
        body: u32,
        captures: &[(LayoutId, Passing)],
        movable: &[u32],
    ) -> Result<LayoutId, LayoutError> {
        self.transaction(|b| {
            let mut key = Encoder {
                bytes: Vec::new(),
                revisions: true,
            };
            key.tag("environment-data");
            key.path(definition);
            key.number(u64::from(body));
            key.number(captures.len() as u64);
            for &(layout, passing) in captures {
                key.atom(hirsel_codec::to_text(&b.record(layout).key).as_bytes());
                key.tag(match passing {
                    Passing::Borrow => "borrow",
                    Passing::Take => "take",
                    Passing::Mut => "mut",
                });
            }
            key.number(movable.len() as u64);
            for &ordinal in movable {
                key.number(u64::from(ordinal));
            }
            if movable.len() > hirsel_lir::ir::LIVE_MASK_WORD_BITS as usize {
                return unavailable(
                    None,
                    hirsel_lir::ir::live_mask_word_limit(
                        &format!(
                            "closure {} body {} with {} movable captures",
                            definition.0.name,
                            body,
                            movable.len()
                        ),
                        movable.len(),
                    ),
                );
            }
            if movable.windows(2).any(|w| w[0] >= w[1])
                || movable.iter().any(|&i| {
                    captures
                        .get(i as usize)
                        .is_none_or(|(_, p)| *p != Passing::Take)
                })
            {
                return invalid("invalid movable environment captures");
            }
            if let Some(&id) = b.memo.get(&key.bytes) {
                return Ok(id);
            }
            let mut pieces = Vec::new();
            for &(layout, passing) in captures {
                if passing == Passing::Mut {
                    return invalid("an environment data field cannot be a mutable address");
                }
                let record = b.record(layout);
                pieces.push(Piece {
                    layout,
                    size: record
                        .size
                        .storage()
                        .ok_or_else(|| LayoutError::Invalid("unbounded dynamic capture".into()))?,
                    align: record.align,
                });
            }
            if !movable.is_empty() {
                let layout = b.scalar(ScalarKind::U64);
                pieces.push(Piece {
                    layout,
                    size: 8,
                    align: b.target.scalar_align(ScalarKind::U64),
                });
            }
            let passing: Vec<_> = captures.iter().map(|(_, p)| *p).collect();
            b.environment_record(&key.bytes, pieces, &passing, movable)
        })
    }

    fn environment_record(
        &mut self,
        key: &[u8],
        pieces: Vec<Piece>,
        passing: &[Passing],
        movable: &[u32],
    ) -> Result<LayoutId, LayoutError> {
        let mut record = self.aggregate(key, pieces, false, None)?;
        // A borrowed view and a mut capture's address point into the
        // creator's storage: their holders are scanned as uncounted, and
        // the environment never releases nor copies them (LIR schema
        // revision 3).
        let mut scan = Vec::new();
        let mut drop = Vec::new();
        let mut copy = CopyRecipe::Bits;
        for (field, passing) in record.fields.iter().zip(passing) {
            let child = self.record(field.layout);
            match passing {
                Passing::Take => {
                    scan.extend(child.scan.iter().map(|s| shift_scan(s, field.offset)));
                    copy = join_copy(copy, child.copy);
                }
                Passing::Borrow => {
                    match borrowed_scan(child, field.offset, &|id| self.record(id)) {
                        Some(steps) => {
                            // A borrow with no scan/drop distinction is an
                            // ordinary inline field to the layout checker.
                            if steps
                                == child
                                    .scan
                                    .iter()
                                    .map(|s| shift_scan(s, field.offset))
                                    .collect::<Vec<_>>()
                                && child.drop.is_empty()
                                && child.finalizer.is_none()
                            {
                                copy = join_copy(copy, child.copy);
                            }
                            scan.extend(steps);
                        }
                        None => {
                            return unavailable(
                                None,
                                "a borrowed capture of a value holding a sequence inline",
                            );
                        }
                    }
                }
                Passing::Mut => scan.push(ScanStep::Uncounted {
                    offset: field.offset,
                }),
            }
        }
        for (field, passing) in record.fields.iter().zip(passing).rev() {
            let child = self.record(field.layout);
            if *passing != Passing::Take {
                continue;
            }
            if child.finalizer.is_some() {
                drop.push(DropStep::Child {
                    offset: field.offset,
                    layout: field.layout,
                });
            } else {
                drop.extend(child.drop.iter().map(|d| shift_drop(d, field.offset)));
            }
        }
        if !movable.is_empty() {
            let offset = record.fields.last().expect("live word").offset;
            for (bit, &ordinal) in movable.iter().enumerate() {
                let field = &record.fields[ordinal as usize];
                let end =
                    field.offset
                        + self.record(field.layout).size.storage().ok_or_else(|| {
                            LayoutError::Invalid("unbounded dynamic capture".into())
                        })?;
                let belongs = |at: u64| (field.offset..end).contains(&at);
                let guarded_scan: Vec<_> = scan
                    .iter()
                    .filter(|s| belongs(scan_order(s)))
                    .cloned()
                    .collect();
                scan.retain(|s| !belongs(scan_order(s)));
                if !guarded_scan.is_empty() {
                    scan.push(ScanStep::ActiveVariant {
                        tag_offset: offset,
                        form: TagForm::LiveBit { bit: bit as u32 },
                        variants: vec![(1, guarded_scan)],
                    });
                }
                // Keep declaration order in the drop program.
                let start = drop
                    .iter()
                    .position(|d| drop_offset(d).is_some_and(belongs));
                if let Some(start) = start {
                    let mut end = start;
                    while end < drop.len() && drop_offset(&drop[end]).is_some_and(belongs) {
                        end += 1;
                    }
                    let steps: Vec<_> = drop.drain(start..end).collect();
                    drop.insert(
                        start,
                        DropStep::ActiveVariant {
                            tag_offset: offset,
                            form: TagForm::LiveBit { bit: bit as u32 },
                            variants: vec![(1, steps)],
                        },
                    );
                }
            }
            record.kind = LayoutKind::Environment {
                movable: movable.to_vec(),
            };
        }
        record.scan = scan;
        record.drop = drop;
        record.copy = copy;
        record.scan.sort_by_key(scan_order);
        let id = self.insert(key.to_vec(), record);
        Ok(id)
    }

    /// A callable value whose environment layout is known (a concrete closure
    /// specialization): code symbol plus environment address.
    pub fn closure(
        &mut self,
        environment: LayoutId,
        multiplicity: hirsel_lir::abi_types::CallMultiplicity,
    ) -> LayoutId {
        let mut key = Encoder {
            bytes: Vec::new(),
            revisions: true,
        };
        key.tag("closure");
        key.atom(
            &self
                .record(environment)
                .key
                .instance
                .map_or([0; 32], |h| h.0),
        );
        key.tag(match multiplicity {
            hirsel_lir::abi_types::CallMultiplicity::Many => "many",
            hirsel_lir::abi_types::CallMultiplicity::Once => "once",
        });
        if let Some(&id) = self.memo.get(&key.bytes) {
            return id;
        }
        let record = self.callable_record(&key.bytes, Some(environment), multiplicity);
        self.insert(key.bytes, record)
    }

    fn transaction<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<T, LayoutError>,
    ) -> Result<T, LayoutError> {
        let mark = self.table.layouts.len();
        let result = f(self);
        if result.is_err() {
            self.table.layouts.truncate(mark);
            self.done.truncate(mark);
            self.memo.retain(|_, id| (id.0 as usize) < mark);
            self.depth = 0;
        }
        result
    }

    fn key(&self, bytes: &[u8]) -> LayoutKey {
        LayoutKey {
            owner_revision: None,
            instance: Some(ContentHash::of_parts("hirsel.layout-key.v1", [bytes])),
            target: self.table.target,
            layout_version: hirsel_lir::LAYOUT_VERSION,
        }
    }

    fn blank(&self, key: &[u8], kind: LayoutKind, size: SizeSpec, align: u32) -> LayoutRecord {
        LayoutRecord {
            key: self.key(key),
            kind,
            size,
            align,
            fields: Vec::new(),
            tag: None,
            initialized: InitializedBytes::Fields,
            scan: Vec::new(),
            drop: Vec::new(),
            copy: CopyRecipe::Bits,
            finalizer: None,
            storage: ALL_STORAGE.to_vec(),
            abi: AbiInputs {
                scalars: Vec::new(),
                unaligned: false,
            },
        }
    }

    fn insert(&mut self, key: Vec<u8>, record: LayoutRecord) -> LayoutId {
        let id = LayoutId(self.table.layouts.len() as u32);
        self.table.layouts.push(record);
        self.done.push(true);
        self.memo.insert(key, id);
        id
    }

    /// Memoized derivation under `key`, reserving the id first so a block
    /// payload may refer back to a layout still being derived.
    fn memoized(
        &mut self,
        key: Vec<u8>,
        through_block: bool,
        derive: impl FnOnce(&mut Self, &[u8]) -> Result<LayoutRecord, LayoutError>,
    ) -> Result<LayoutId, LayoutError> {
        let (id, fresh) = self.reserve(&key, through_block)?;
        if !fresh {
            return Ok(id);
        }
        self.depth += 1;
        let record = derive(self, &key);
        self.depth -= 1;
        self.complete(id, record?);
        Ok(id)
    }

    /// The id memoized under `key`, or a fresh placeholder (`true`). A layout
    /// still being derived is returned only through a block reference.
    fn reserve(
        &mut self,
        key: &[u8],
        through_block: bool,
    ) -> Result<(LayoutId, bool), LayoutError> {
        if let Some(&id) = self.memo.get(key) {
            if self.done[id.0 as usize] || through_block {
                return Ok((id, false));
            }
            return Err(LayoutError::InfiniteSize(String::new()));
        }
        if self.table.layouts.len() >= MAX_LAYOUTS || self.depth >= MAX_DEPTH {
            return Err(LayoutError::InstanceLimit);
        }
        let id = LayoutId(self.table.layouts.len() as u32);
        let placeholder = self.blank(key, LayoutKind::Unit, SizeSpec::Fixed(0), 1);
        self.table.layouts.push(placeholder);
        self.done.push(false);
        self.memo.insert(key.to_vec(), id);
        Ok((id, true))
    }

    /// Install a derived record, keeping the key its reservation assigned.
    fn complete(&mut self, id: LayoutId, mut record: LayoutRecord) {
        let slot = &mut self.table.layouts[id.0 as usize];
        record.key.instance = slot.key.instance;
        *slot = record;
        self.done[id.0 as usize] = true;
    }

    /// Derive every reserved payload, in a deterministic order: pending
    /// payloads last reserved first, then deferred Box payloads by id.
    fn drain<'s>(&mut self, walk: &mut Walk<'s>) -> Result<(), LayoutError>
    where
        'o: 's,
    {
        loop {
            if let Some(item) = walk.pending.pop() {
                match item {
                    Pending::Sequence { id, element } => {
                        self.settle(walk, element)?;
                        let record = self.sequence_record(element)?;
                        self.complete(id, record);
                    }
                    Pending::Soa { id, element } => {
                        self.settle(walk, element)?;
                        let record = self.soa_record(element)?;
                        self.complete(id, record);
                    }
                    Pending::Frozen { id, value } => {
                        self.settle(walk, value)?;
                        let record = self.frozen_record(value)?;
                        self.complete(id, record);
                    }
                    Pending::Container {
                        id,
                        frames,
                        frame,
                        owner,
                    } => {
                        let outer = std::mem::replace(&mut walk.frames, frames);
                        let record = self.representation(walk, &[], owner, frame);
                        walk.frames = outer;
                        let mut record = record?;
                        record.kind = LayoutKind::Opaque {
                            owner: owner.definition.clone(),
                        };
                        record.key.owner_revision = Some(owner.representation_revision);
                        self.complete(id, record);
                    }
                }
            } else if let Some((id, payload)) = walk.deferred.pop_first() {
                self.derive_deferred(walk, id, payload)?;
            } else {
                return Ok(());
            }
        }
    }

    /// Derive `id` now if it is a deferred Box payload.
    fn settle<'s>(&mut self, walk: &mut Walk<'s>, id: LayoutId) -> Result<(), LayoutError>
    where
        'o: 's,
    {
        match walk.deferred.remove(&id) {
            Some(payload) => self.derive_deferred(walk, id, payload),
            None => Ok(()),
        }
    }

    fn derive_deferred<'s>(
        &mut self,
        walk: &mut Walk<'s>,
        id: LayoutId,
        payload: Deferred<'s>,
    ) -> Result<(), LayoutError>
    where
        'o: 's,
    {
        let outer = std::mem::replace(&mut walk.frames, payload.frames);
        let result = self.derive_reserved(walk, id, &payload.key, payload.ty, payload.frame);
        walk.frames = outer;
        result
    }

    /// The layout of a byte in String and Bytes storage, views and flat
    /// encodings: the source `u8`'s own, so indexing any of them reads a
    /// place of that type's layout.
    fn byte(&mut self) -> LayoutId {
        let mut key = Encoder {
            bytes: Vec::new(),
            revisions: true,
        };
        key.tag("memory");
        key.tag("primitive");
        key.tag(primitive_name(Primitive::U8));
        if let Some(&id) = self.memo.get(&key.bytes) {
            return id;
        }
        let record = self.scalar_record(&key.bytes, ScalarKind::U8);
        self.insert(key.bytes, record)
    }

    fn scalar(&mut self, scalar: ScalarKind) -> LayoutId {
        let mut key = Encoder {
            bytes: Vec::new(),
            revisions: true,
        };
        key.tag("scalar");
        key.tag(scalar_name(scalar));
        if let Some(&id) = self.memo.get(&key.bytes) {
            return id;
        }
        let record = self.scalar_record(&key.bytes, scalar);
        self.insert(key.bytes, record)
    }

    fn scalar_record(&self, key: &[u8], scalar: ScalarKind) -> LayoutRecord {
        let mut record = self.blank(
            key,
            LayoutKind::Scalar(scalar),
            SizeSpec::Fixed(self.target.scalar_size(scalar)),
            self.target.scalar_align(scalar),
        );
        record.abi.scalars = vec![(0, scalar)];
        record
    }

    // ----- key encoding --------------------------------------------------

    fn encode(
        &self,
        walk: &Walk<'_>,
        ty: TypeId,
        f: usize,
        e: &mut Encoder,
    ) -> Result<(), LayoutError> {
        self.encode_at(walk, ty, f, e, 0)
    }

    fn encode_at(
        &self,
        walk: &Walk<'_>,
        ty: TypeId,
        f: usize,
        e: &mut Encoder,
        nesting: u32,
    ) -> Result<(), LayoutError> {
        if nesting > MAX_DEPTH {
            return Err(LayoutError::InstanceLimit);
        }
        let (ty, f) = walk.resolve(ty, f)?;
        let row = walk.row(ty, f)?;
        let n = nesting + 1;
        match &row.kind {
            TypeKind::Primitive(p) => {
                e.tag("primitive");
                e.tag(primitive_name(*p));
            }
            TypeKind::Tuple(items) => {
                e.tag("tuple");
                e.number(items.len() as u64);
                for item in items.iter() {
                    self.encode_at(walk, *item, f, e, n)?;
                }
            }
            TypeKind::Parameter(_) => unreachable!("resolved above"),
            TypeKind::Named {
                definition,
                arguments,
            } => {
                if matches!(
                    self.owners.nominals.get(definition),
                    Some(OwnerRepresentation::StringView)
                ) {
                    e.tag("primitive");
                    e.tag("String");
                    return Ok(());
                }
                e.tag("named");
                e.path(definition);
                if e.revisions {
                    match self.owners.nominals.get(definition) {
                        Some(OwnerRepresentation::Defined(record)) => {
                            e.atom(&record.representation_revision.0);
                            e.atom(&[u8::from(record.resource)]);
                        }
                        Some(OwnerRepresentation::Handle { revision, resource }) => {
                            e.atom(&revision.0);
                            e.atom(&[u8::from(*resource), 2]);
                        }
                        Some(OwnerRepresentation::Cursor { revision }) => {
                            e.atom(&revision.0);
                            e.atom(&[0, 3]);
                        }
                        Some(OwnerRepresentation::ViewCursor { revision }) => {
                            e.atom(&revision.0);
                            e.atom(&[0, 4]);
                        }
                        Some(OwnerRepresentation::StringView) => unreachable!("handled above"),
                        None => return Err(LayoutError::MissingOwner(describe(definition))),
                    }
                }
                self.encode_arguments(walk, arguments, f, e, n)?;
            }
            TypeKind::Record(fields) => {
                e.tag("record");
                e.number(fields.len() as u64);
                for field in fields.iter() {
                    e.atom(&[u8::from(field.growable)]);
                    self.encode_at(walk, field.ty, f, e, n)?;
                }
            }
            TypeKind::Variant(variants) => {
                e.tag("variant");
                e.number(variants.len() as u64);
                for variant in variants.iter() {
                    e.number(variant.fields.len() as u64);
                    for (_, field) in variant.fields.iter() {
                        self.encode_at(walk, *field, f, e, n)?;
                    }
                }
            }
            TypeKind::Function(_) => {
                // Every callable value is a code/environment pair; only the
                // multiplicity changes its copy recipe.
                e.tag("function");
                e.tag(match row.modes.multiplicity {
                    Multiplicity::Many => "many",
                    Multiplicity::Once => "once",
                });
            }
            TypeKind::Box(inner) => {
                e.tag("box");
                self.encode_at(walk, *inner, f, e, n)?;
            }
            TypeKind::Builtin { kind, arguments } => {
                e.tag("builtin");
                e.tag(builtin_name(*kind));
                if let Some(c) = container(*kind)
                    && e.revisions
                {
                    match self.owners.containers.get(&c) {
                        Some(record) => e.atom(&record.representation_revision.0),
                        None => e.tag("no-owner"),
                    }
                }
                self.encode_arguments(walk, arguments, f, e, n)?;
            }
            TypeKind::Error => return invalid("recovery type in a native layout"),
        }
        Ok(())
    }

    fn encode_arguments(
        &self,
        walk: &Walk<'_>,
        arguments: &[GenericArgument],
        f: usize,
        e: &mut Encoder,
        nesting: u32,
    ) -> Result<(), LayoutError> {
        e.number(arguments.len() as u64);
        for argument in arguments {
            match argument {
                GenericArgument::Type(ty) => {
                    e.tag("type");
                    self.encode_at(walk, *ty, f, e, nesting)?;
                }
                GenericArgument::Size(size) => {
                    e.tag("size");
                    e.number(walk.size(size, f)?);
                }
            }
        }
        Ok(())
    }

    /// The public, revision-free identity of a type (ABI keys).
    pub(crate) fn public_identity(
        &self,
        store: &TypeStore,
        ty: TypeId,
        e: &mut Vec<u8>,
    ) -> Result<(), LayoutError> {
        let walk = Walk::new(store);
        let mut encoder = Encoder {
            bytes: std::mem::take(e),
            revisions: false,
        };
        let result = self.encode(&walk, ty, 0, &mut encoder);
        *e = encoder.bytes;
        result
    }

    // ----- in-memory layouts ---------------------------------------------

    fn memory<'s>(
        &mut self,
        walk: &mut Walk<'s>,
        ty: TypeId,
        f: usize,
        through_block: bool,
    ) -> Result<LayoutId, LayoutError>
    where
        'o: 's,
    {
        let (key, ty, f) = self.memory_key(walk, ty, f)?;
        let row = walk.row(ty, f)?;
        let deferred = self
            .memo
            .get(&key)
            .copied()
            .filter(|id| walk.deferred.contains_key(id));
        let result = match deferred {
            // A deferred Box payload is derived now when held inline.
            Some(id) if !through_block => self.settle(walk, id).map(|()| id),
            Some(id) => Ok(id),
            // A block reference reaches its payload only once the inline
            // layouts the payload may read are complete, as a Box does:
            // `Value = Items(List[Member])` with `Member { value: Value }`.
            None => self.reserve(&key, through_block).and_then(|(id, fresh)| {
                if fresh && through_block {
                    let frames = walk.frames.clone();
                    walk.deferred.insert(
                        id,
                        Deferred {
                            key: key.clone(),
                            ty,
                            frames,
                            frame: f,
                        },
                    );
                } else if fresh {
                    self.derive_reserved(walk, id, &key, ty, f)?;
                }
                Ok(id)
            }),
        };
        result.map_err(|e| match e {
            LayoutError::InfiniteSize(s) if s.is_empty() => {
                LayoutError::InfiniteSize(match &row.kind {
                    TypeKind::Named { definition, .. } => describe(definition),
                    _ => "a structural type".into(),
                })
            }
            e => e,
        })
    }

    /// The in-memory key of `ty`, and the type and frame it resolves to.
    fn memory_key<'s>(
        &mut self,
        walk: &mut Walk<'s>,
        ty: TypeId,
        f: usize,
    ) -> Result<(Vec<u8>, TypeId, usize), LayoutError>
    where
        'o: 's,
    {
        let (resolved, frame) = walk.resolve(ty, f)?;
        let fixed_flat = if let TypeKind::Named { definition, .. } =
            &walk.row(resolved, frame)?.kind
            && self.flat_owner(definition)
        {
            let body = self.flat_body(walk, ty, f)?;
            matches!(self.record(body).size, SizeSpec::Fixed(_))
        } else {
            false
        };
        let mut key = Encoder {
            bytes: Vec::new(),
            revisions: true,
        };
        // A fixed flat value is exactly its flat body, including when requested
        // as a deferred Box payload. One key prevents identical representations
        // acquiring incompatible identities in construction and projections.
        key.tag(if fixed_flat { "flat" } else { "memory" });
        self.encode(walk, ty, f, &mut key)?;
        Ok((key.bytes, resolved, frame))
    }

    /// Derive the in-memory record reserved as `id`. A block reference is one
    /// pointer whatever its payload, so a Box completes at once and defers its
    /// payload until the inline layouts the payload reads are complete: the
    /// payload may hold this very reference inline (`Box[Cons]` with
    /// `Cons = variant { Nil, Cons(Box[Cons]) }`), or hold a type whose
    /// derivation is under way (`Even = variant { Zero, Next(Box[Odd]) }`,
    /// `Odd = record { Even }`), and the result must not depend on which type
    /// is requested first.
    fn derive_reserved<'s>(
        &mut self,
        walk: &mut Walk<'s>,
        id: LayoutId,
        key: &[u8],
        ty: TypeId,
        f: usize,
    ) -> Result<(), LayoutError>
    where
        'o: 's,
    {
        if let TypeKind::Box(inner) = walk.row(ty, f)?.kind {
            let (payload_key, inner, inner_frame) = self.memory_key(walk, inner, f)?;
            let (payload, fresh) = self.reserve(&payload_key, true)?;
            let mut record = self.block_record(key, payload);
            record.kind = LayoutKind::Box { payload };
            self.complete(id, record);
            if fresh {
                let frames = walk.frames.clone();
                walk.deferred.insert(
                    payload,
                    Deferred {
                        key: payload_key,
                        ty: inner,
                        frames,
                        frame: inner_frame,
                    },
                );
            }
            return Ok(());
        }
        self.depth += 1;
        let record = self.derive_memory(walk, ty, f, key);
        self.depth -= 1;
        self.complete(id, record?);
        Ok(())
    }

    /// An ordinary field reserves its bounded storage, including the largest
    /// Packed variant. A variable flat extent has no such bound.
    fn piece<'s>(&mut self, walk: &mut Walk<'s>, ty: TypeId, f: usize) -> Result<Piece, LayoutError>
    where
        'o: 's,
    {
        let layout = self.memory(walk, ty, f, false)?;
        let record = self.record(layout);
        let Some(size) = record.size.storage() else {
            return unavailable(
                None,
                "an unbounded dynamic-extent value inline in an ordinary aggregate, variant or sequence",
            );
        };
        Ok(Piece {
            layout,
            size,
            align: record.align,
        })
    }

    fn derive_memory<'s>(
        &mut self,
        walk: &mut Walk<'s>,
        ty: TypeId,
        f: usize,
        key: &[u8],
    ) -> Result<LayoutRecord, LayoutError>
    where
        'o: 's,
    {
        let row = walk.row(ty, f)?;
        match &row.kind {
            TypeKind::Primitive(Primitive::String | Primitive::Bytes) => {
                let bytes = self.byte();
                self.sequence(walk, bytes, false)?;
                Ok(self.slice_record(key, bytes))
            }
            TypeKind::Primitive(Primitive::Never) => Ok(self.unit_record(key)),
            TypeKind::Primitive(p) => {
                Ok(self.scalar_record(key, primitive_scalar(*p).expect("sized scalar")))
            }
            TypeKind::Tuple(items) if items.is_empty() => Ok(self.unit_record(key)),
            TypeKind::Tuple(items) => {
                let items = items.to_vec();
                let mut pieces = Vec::new();
                for item in items {
                    pieces.push(self.piece(walk, item, f)?);
                }
                self.aggregate(key, pieces, false, None)
            }
            TypeKind::Record(fields) => {
                if fields.iter().any(|field| field.growable) {
                    return invalid("a growable field outside a flat owner");
                }
                let fields = fields.to_vec();
                let mut pieces = Vec::new();
                for field in fields {
                    pieces.push(self.piece(walk, field.ty, f)?);
                }
                self.aggregate(key, pieces, false, None)
            }
            TypeKind::Variant(variants) => {
                let variants = variant_fields(variants);
                self.variant(walk, key, &variants, f, &Layout::default())
            }
            TypeKind::Named { definition, .. } if self.flat_owner(definition) => {
                // The same FlatFormatV0 body `flat` returns: inline when fixed,
                // behind a counted block of its bytes when variable.
                let body = self.flat_body(walk, ty, f)?;
                let body_record = self.record(body).clone();
                if matches!(body_record.size, SizeSpec::Fixed(_)) {
                    return invalid("a fixed flat body bypassed its canonical memory key");
                }
                let mut wrapper = self.block_record(key, body);
                wrapper.copy = CopyRecipe::FlatBytes;
                wrapper.key.owner_revision = body_record.key.owner_revision;
                Ok(wrapper)
            }
            TypeKind::Named {
                definition,
                arguments,
            } => self.nominal(walk, key, definition, arguments, f),
            TypeKind::Function(_) => {
                Ok(self.callable_record(key, None, multiplicity(row.modes.multiplicity)))
            }
            TypeKind::Box(_) => unreachable!("derive_reserved completes a box before its payload"),
            TypeKind::Builtin { kind, arguments } => self.builtin(walk, key, *kind, arguments, f),
            TypeKind::Parameter(_) => Err(LayoutError::Incomplete),
            TypeKind::Error => invalid("recovery type in a native layout"),
        }
    }

    fn flat_owner(&self, definition: &DefPath) -> bool {
        matches!(
            self.owners.nominals.get(definition),
            Some(OwnerRepresentation::Defined(record)) if record.layout_promises.flat
        )
    }

    fn unit_record(&self, key: &[u8]) -> LayoutRecord {
        self.blank(key, LayoutKind::Unit, SizeSpec::Fixed(0), 1)
    }

    fn pointer(&self) -> (u64, u32) {
        (
            self.target.scalar_size(ScalarKind::Pointer),
            self.target.scalar_align(ScalarKind::Pointer),
        )
    }

    /// Words at consecutive pointer-aligned offsets.
    fn words(&mut self, record: &mut LayoutRecord, scalars: &[ScalarKind]) {
        let mut offset = 0;
        for (ordinal, &scalar) in scalars.iter().enumerate() {
            let layout = self.scalar(scalar);
            record.fields.push(FieldLayout {
                ordinal: ordinal as u32,
                offset,
                layout,
                unaligned: false,
            });
            record.abi.scalars.push((offset, scalar));
            offset += self.target.scalar_size(scalar);
        }
    }

    /// A counted block reference: Box, List, Array, Map, Set, SortedMap,
    /// Slots and dynamic flat values. The pointer is the wrapper; the payload
    /// layout describes the bytes after the header.
    fn block_record(&mut self, key: &[u8], payload: LayoutId) -> LayoutRecord {
        let (size, align) = self.pointer();
        let mut record = self.blank(
            key,
            LayoutKind::Block { payload },
            SizeSpec::Fixed(size),
            align,
        );
        self.words(&mut record, &[ScalarKind::Pointer]);
        record.scan = vec![ScanStep::Counted { offset: 0 }];
        record.drop = vec![DropStep::Release { offset: 0 }];
        record.copy = CopyRecipe::Dup;
        record
    }

    /// String and Bytes values: backing owner, start and length, so a slice
    /// never loses its release root. The bytes live in a `Sequence` payload.
    fn slice_record(&mut self, key: &[u8], element: LayoutId) -> LayoutRecord {
        let (size, align) = self.pointer();
        let mut record = self.blank(
            key,
            LayoutKind::Slice { element },
            SizeSpec::Fixed(size + 16),
            align.max(self.target.scalar_align(ScalarKind::U64)),
        );
        self.words(
            &mut record,
            &[ScalarKind::Pointer, ScalarKind::U64, ScalarKind::U64],
        );
        record.scan = vec![ScanStep::SliceOwner { offset: 0 }];
        record.drop = vec![DropStep::Release { offset: 0 }];
        record.copy = CopyRecipe::Dup;
        record
    }

    fn callable_record(
        &mut self,
        key: &[u8],
        environment: Option<LayoutId>,
        multiplicity: hirsel_lir::abi_types::CallMultiplicity,
    ) -> LayoutRecord {
        let (size, align) = self.pointer();
        let mut record = self.blank(
            key,
            LayoutKind::Callable { environment },
            SizeSpec::Fixed(2 * size),
            align,
        );
        self.words(&mut record, &[ScalarKind::Pointer, ScalarKind::Pointer]);
        record.scan = vec![ScanStep::Environment { offset: size }];
        record.drop = vec![DropStep::Environment { offset: size }];
        record.copy = match multiplicity {
            hirsel_lir::abi_types::CallMultiplicity::Many => CopyRecipe::Dup,
            hirsel_lir::abi_types::CallMultiplicity::Once => CopyRecipe::NotCopyable,
        };
        record
    }

    fn handle_record(&mut self, key: &[u8], resource: bool) -> LayoutRecord {
        let mut record = self.blank(
            key,
            LayoutKind::Handle,
            SizeSpec::Fixed(self.target.scalar_size(ScalarKind::U64)),
            self.target.scalar_align(ScalarKind::U64),
        );
        self.words(&mut record, &[ScalarKind::U64]);
        record.scan = vec![ScanStep::Handle { offset: 0 }];
        if resource {
            record.drop = vec![DropStep::CloseHandle { offset: 0 }];
            record.copy = CopyRecipe::NotCopyable;
            record.storage = RESOURCE_STORAGE.to_vec();
        }
        record
    }

    /// The cursor's opaque source (LIR schema revision 7): a block reference
    /// over no payload. `Inst::Recast` turns a borrowed container block into
    /// it and back.
    pub fn cursor_source(&mut self) -> LayoutId {
        let key = b"cursor-source".to_vec();
        if let Some(&id) = self.memo.get(&key) {
            return id;
        }
        let payload_key = b"cursor-source-payload".to_vec();
        let payload = self.unit_record(&payload_key);
        let payload = self.insert(payload_key, payload);
        let record = self.block_record(&key, payload);
        self.insert(key, record)
    }

    /// A flat list builder (LIR schema revision 22): a counted block of a
    /// sequential sequence whose elements are the prefixed variable flat
    /// records `element`, appended in their encoding by `Inst::FlatAppend`.
    /// The count and used bytes are a flat list field's count and content.
    pub fn flat_builder(&mut self, element: LayoutId) -> Result<LayoutId, LayoutError> {
        if !self.flat_prefixed(element) {
            return invalid("a flat builder of an element that is not a prefixed flat record");
        }
        let instance = self.record(element).key.instance.map_or([0; 32], |h| h.0);
        let mut key = Encoder {
            bytes: Vec::new(),
            revisions: true,
        };
        key.tag("flat-builder");
        key.atom(&instance);
        if let Some(&id) = self.memo.get(&key.bytes) {
            return Ok(id);
        }
        let mut payload_key = Encoder {
            bytes: Vec::new(),
            revisions: true,
        };
        payload_key.tag("flat-builder-payload");
        payload_key.atom(&instance);
        let payload = self.memoized(payload_key.bytes, false, |b, _| {
            b.sequential_record(element)
        })?;
        let record = self.block_record(&key.bytes, payload);
        Ok(self.insert(key.bytes, record))
    }

    /// `{ source, position, end }` of `OwnerRepresentation::Cursor`.
    fn cursor_record(&mut self, key: &[u8]) -> Result<LayoutRecord, LayoutError> {
        let source = self.cursor_source();
        let word = self.scalar(ScalarKind::U64);
        let (size, align) = self.pointer();
        let word = || Piece {
            layout: word,
            size: self.target.scalar_size(ScalarKind::U64),
            align: self.target.scalar_align(ScalarKind::U64),
        };
        let pieces = vec![
            Piece {
                layout: source,
                size,
                align,
            },
            word(),
            word(),
        ];
        let mut record = self.aggregate(key, pieces, false, None)?;
        record.scan = vec![ScanStep::Uncounted { offset: 0 }];
        record.drop = Vec::new();
        record.copy = CopyRecipe::NotCopyable;
        Ok(record)
    }

    /// Reserve the payload of a counted sequence of `element`, or of a
    /// structure-of-arrays sequence when `soa` (#47 assumption). Derived once
    /// the element is complete.
    fn sequence<'s>(
        &mut self,
        walk: &mut Walk<'s>,
        element: LayoutId,
        soa: bool,
    ) -> Result<LayoutId, LayoutError> {
        let mut key = Encoder {
            bytes: Vec::new(),
            revisions: true,
        };
        key.tag(if soa { "soa" } else { "sequence" });
        key.atom(&self.record(element).key.instance.map_or([0; 32], |h| h.0));
        let (id, fresh) = self.reserve(&key.bytes, true)?;
        if fresh {
            walk.pending.push(if soa {
                Pending::Soa { id, element }
            } else {
                Pending::Sequence { id, element }
            });
        }
        Ok(id)
    }

    /// The payload of a `Shared[T]` root block, reserved now and derived once
    /// `value`, the inline layout of `T`, is complete.
    fn frozen<'s>(
        &mut self,
        walk: &mut Walk<'s>,
        value: LayoutId,
    ) -> Result<LayoutId, LayoutError> {
        let mut key = Encoder {
            bytes: Vec::new(),
            revisions: true,
        };
        key.tag("frozen");
        key.atom(&self.record(value).key.instance.map_or([0; 32], |h| h.0));
        let (id, fresh) = self.reserve(&key.bytes, true)?;
        if fresh {
            walk.pending.push(Pending::Frozen { id, value });
        }
        Ok(id)
    }

    /// The value inline at offset 0. Its program thaws the value, restoring
    /// ordinary counts, before the value's own drop releases it; a value
    /// that holds nothing has nothing to freeze or thaw.
    fn frozen_record(&mut self, value: LayoutId) -> Result<LayoutRecord, LayoutError> {
        let v = self.record(value).clone();
        if !matches!(v.size, SizeSpec::Fixed(_)) {
            return unavailable(None, "a Shared value of dynamic extent");
        }
        let mut record = self.blank(&[], LayoutKind::Frozen { value }, v.size.clone(), v.align);
        record.fields = vec![FieldLayout {
            ordinal: 0,
            offset: 0,
            layout: value,
            unaligned: false,
        }];
        record.initialized = v.initialized.clone();
        record.abi = v.abi.clone();
        if !v.scan.is_empty() {
            record.scan = vec![ScanStep::Inline {
                offset: 0,
                layout: value,
            }];
        }
        if !v.drop.is_empty() {
            record.drop = vec![
                DropStep::Thaw {
                    offset: 0,
                    layout: value,
                },
                DropStep::Child {
                    offset: 0,
                    layout: value,
                },
            ];
        }
        record.copy = CopyRecipe::NotCopyable;
        Ok(record)
    }

    fn sequence_record(&mut self, element: LayoutId) -> Result<LayoutRecord, LayoutError> {
        let e = self.record(element).clone();
        if e.kind == LayoutKind::Packed && matches!(e.size, SizeSpec::Dynamic(_)) {
            return self.sequential_record(element);
        }
        let SizeSpec::Fixed(stride) = e.size else {
            return unavailable(
                None,
                "a sequence of dynamic-extent elements outside a flat value",
            );
        };
        let count = self.scalar(ScalarKind::U64);
        let elements = sequence_elements_offset(&e)?;
        let mut record = self.blank(
            &[],
            LayoutKind::Sequence { element },
            SizeSpec::Dynamic(vec![
                SizeTerm::Constant(elements),
                SizeTerm::ReadLength {
                    offset: 0,
                    scalar: ScalarKind::U64,
                },
                SizeTerm::Constant(stride),
                SizeTerm::Multiply,
                SizeTerm::Add,
            ]),
            e.align.max(8),
        );
        record.fields = vec![
            FieldLayout {
                ordinal: 0,
                offset: 0,
                layout: count,
                unaligned: false,
            },
            FieldLayout {
                ordinal: 1,
                offset: SEQUENCE_CAPACITY_OFFSET,
                layout: count,
                unaligned: false,
            },
        ];
        if !e.scan.is_empty() {
            record.scan = vec![ScanStep::Sequence {
                count_offset: 0,
                offset: elements,
                stride,
                element,
            }];
        }
        if !e.drop.is_empty() {
            record.drop = vec![DropStep::ElementsReverse {
                count_offset: 0,
                offset: elements,
                stride,
                element,
            }];
        }
        record.copy = CopyRecipe::NotCopyable;
        record.storage = e.storage.clone();
        Ok(record)
    }

    /// A sequence of `Packed[T]` is sequential (MEM-13): a u64 count, a u64
    /// count of used content bytes, then the elements back to back from the
    /// first offset aligned to the element, each at its active variant's
    /// extent. Every extent is a multiple of the element alignment, so each
    /// element stays aligned. Nothing reserves the largest variant.
    fn sequential_record(&mut self, element: LayoutId) -> Result<LayoutRecord, LayoutError> {
        let e = self.record(element).clone();
        if e.size.storage().is_none() && !self.flat_prefixed(element) {
            return unavailable(None, "a sequential Packed element without bounded extents");
        }
        // Packed[T] is Flat (MEM-12), so its elements hold no reference or resource.
        if !e.scan.is_empty() || !e.drop.is_empty() || e.finalizer.is_some() {
            return unavailable(
                None,
                "a sequential Packed element with references or resources",
            );
        }
        let count = self.scalar(ScalarKind::U64);
        let elements = sequential_elements_offset(&e)?;
        let mut record = self.blank(
            &[],
            LayoutKind::Sequential { element },
            SizeSpec::Dynamic(vec![
                SizeTerm::Constant(elements),
                SizeTerm::ReadLength {
                    offset: 8,
                    scalar: ScalarKind::U64,
                },
                SizeTerm::Add,
            ]),
            e.align.max(8),
        );
        record.fields = vec![
            FieldLayout {
                ordinal: 0,
                offset: 0,
                layout: count,
                unaligned: false,
            },
            FieldLayout {
                ordinal: 1,
                offset: 8,
                layout: count,
                unaligned: false,
            },
        ];
        record.copy = CopyRecipe::NotCopyable;
        record.storage = e.storage.clone();
        Ok(record)
    }

    /// Declaration-order field columns in one allocation: a u64 length, a u64
    /// element capacity (a multiple of the largest field alignment), then one
    /// column per field at `data_offset + capacity * prefix`.
    fn soa_record(&mut self, element: LayoutId) -> Result<LayoutRecord, LayoutError> {
        let e = self.record(element).clone();
        if e.finalizer.is_some() {
            return unavailable(
                Some(47),
                "@soa sequence of a resource with a drop body (the finalizer would need a projection bundle)",
            );
        }
        if e.kind != LayoutKind::Aggregate && e.kind != LayoutKind::Resource || e.tag.is_some() {
            return unavailable(Some(47), "@soa sequence of a non-record element");
        }
        let count = self.scalar(ScalarKind::U64);
        let quantum = e
            .fields
            .iter()
            .map(|field| u64::from(self.record(field.layout).align))
            .max()
            .unwrap_or(1);
        let data_offset = soa_data_offset(quantum)?;
        let mut columns = Vec::new();
        let mut prefix = 0u64;
        for field in &e.fields {
            let SizeSpec::Fixed(stride) = self.record(field.layout).size else {
                return unavailable(Some(47), "@soa sequence of a variable-size record");
            };
            columns.push(SoaColumn {
                field: field.ordinal,
                prefix,
                stride,
                layout: field.layout,
            });
            prefix = prefix.checked_add(stride).ok_or(LayoutError::Overflow)?;
        }
        let placement = |pick: &dyn Fn(&LayoutRecord) -> bool| SoaColumns {
            count_offset: 0,
            capacity_offset: 8,
            data_offset,
            capacity_quantum: quantum,
            columns: columns
                .iter()
                .filter(|c| pick(self.record(c.layout)))
                .cloned()
                .collect(),
        };
        let scanned = placement(&|r| !r.scan.is_empty());
        let dropped = placement(&|r| !r.drop.is_empty());
        let mut record = self.blank(
            &[],
            LayoutKind::SoaColumns { element },
            SizeSpec::Dynamic(vec![
                SizeTerm::Constant(data_offset),
                SizeTerm::ReadLength {
                    offset: 8,
                    scalar: ScalarKind::U64,
                },
                SizeTerm::Constant(prefix),
                SizeTerm::Multiply,
                SizeTerm::Add,
            ]),
            u32::try_from(quantum.max(8)).map_err(|_| LayoutError::Overflow)?,
        );
        record.fields = vec![
            FieldLayout {
                ordinal: 0,
                offset: 0,
                layout: count,
                unaligned: false,
            },
            FieldLayout {
                ordinal: 1,
                offset: 8,
                layout: count,
                unaligned: false,
            },
        ];
        if !scanned.columns.is_empty() {
            record.scan = vec![ScanStep::Columns(scanned)];
        }
        if !dropped.columns.is_empty() {
            record.drop = vec![DropStep::ColumnsReverse(dropped)];
        }
        record.copy = CopyRecipe::NotCopyable;
        record.storage = e.storage.clone();
        Ok(record)
    }

    /// Inline declaration-order fields with explicit padding; `packed` places
    /// every field unaligned with no padding, `raise` raises the alignment.
    fn aggregate(
        &mut self,
        key: &[u8],
        pieces: Vec<Piece>,
        packed: bool,
        raise: Option<u32>,
    ) -> Result<LayoutRecord, LayoutError> {
        let (fields, end, natural) = self.place(&pieces, packed, 0)?;
        let align = raise.map_or(natural, |r| natural.max(r));
        let size = align_up(end, u64::from(align))?;
        let mut record = self.blank(key, LayoutKind::Aggregate, SizeSpec::Fixed(size), align);
        self.adopt_fields(&mut record, fields);
        Ok(record)
    }

    /// Field offsets from `start`: (fields, end, natural alignment).
    fn place(
        &self,
        pieces: &[Piece],
        packed: bool,
        start: u64,
    ) -> Result<(Vec<FieldLayout>, u64, u32), LayoutError> {
        let mut offset = start;
        let mut align = 1u32;
        let mut fields = Vec::new();
        for (ordinal, piece) in pieces.iter().enumerate() {
            let a = if packed { 1 } else { piece.align };
            offset = align_up(offset, u64::from(a))?;
            fields.push(FieldLayout {
                ordinal: ordinal as u32,
                offset,
                layout: piece.layout,
                unaligned: packed && piece.align > 1,
            });
            offset = offset
                .checked_add(piece.size)
                .ok_or(LayoutError::Overflow)?;
            align = align.max(a);
        }
        Ok((fields, offset, align))
    }

    /// Scan and drop steps of a field list, lifted to the parent's offsets:
    /// scans in declaration order, drops in reverse declaration order. A child
    /// with its own finalizer is dropped through its layout so the finalizer
    /// runs before its fields.
    fn lift(&self, fields: &[FieldLayout]) -> Lifted {
        let mut scan = Vec::new();
        let mut drop = Vec::new();
        let mut copy = CopyRecipe::Bits;
        let mut leaves = Vec::new();
        let mut unaligned = false;
        for field in fields {
            let child = self.record(field.layout);
            scan.extend(child.scan.iter().map(|s| shift_scan(s, field.offset)));
            leaves.extend(
                child
                    .abi
                    .scalars
                    .iter()
                    .map(|&(o, s)| (o + field.offset, s)),
            );
            unaligned |= field.unaligned || child.abi.unaligned;
            copy = join_copy(copy, child.copy);
        }
        for field in fields.iter().rev() {
            let child = self.record(field.layout);
            if child.finalizer.is_some() {
                drop.push(DropStep::Child {
                    offset: field.offset,
                    layout: field.layout,
                });
            } else {
                drop.extend(child.drop.iter().map(|d| shift_drop(d, field.offset)));
            }
        }
        Lifted {
            scan,
            drop,
            copy,
            leaves,
            unaligned,
        }
    }

    fn adopt_fields(&self, record: &mut LayoutRecord, fields: Vec<FieldLayout>) {
        let Lifted {
            scan,
            drop,
            copy,
            leaves,
            unaligned,
        } = self.lift(&fields);
        record.scan = scan;
        record.drop = drop;
        record.copy = copy;
        record.abi = abi_inputs(&record.size, leaves, unaligned);
        record.storage = self.storage_of(&fields);
        record.fields = fields;
    }

    fn storage_of(&self, fields: &[FieldLayout]) -> Vec<StorageKind> {
        let mut storage = ALL_STORAGE.to_vec();
        for field in fields {
            let child = &self.record(field.layout).storage;
            storage.retain(|s| child.contains(s));
        }
        storage
    }

    /// Ordinary variants: an explicit u32 ordinal tag at offset 0, then the
    /// payload union at the largest variant alignment. The one exception is a
    /// null-pointer niche (layout version 2): see `niche`.
    fn variant<'s>(
        &mut self,
        walk: &mut Walk<'s>,
        key: &[u8],
        variants: &[Vec<TypeId>],
        f: usize,
        promises: &Layout,
    ) -> Result<LayoutRecord, LayoutError>
    where
        'o: 's,
    {
        let packed = promises.packed;
        let mut layouts = Vec::new();
        for fields in variants {
            let mut pieces = Vec::new();
            for &field in fields {
                pieces.push(self.piece(walk, field, f)?);
            }
            layouts.push(pieces);
        }
        if let Some((nullary, dataful, field)) = self.niche(&layouts, packed) {
            let (fields, end, natural) = self.place(&layouts[dataful as usize], false, 0)?;
            let tag = TagEncoding {
                offset: fields[field].offset,
                form: TagForm::Niche { nullary, dataful },
                variants: (0..2)
                    .map(|ordinal| VariantLayout {
                        ordinal,
                        fields: if ordinal == dataful {
                            fields.clone()
                        } else {
                            Vec::new()
                        },
                    })
                    .collect(),
            };
            let align = promises.align.map_or(natural, |r| natural.max(r));
            let size = align_up(end, u64::from(align))?;
            let mut record = self.blank(key, LayoutKind::Variant, SizeSpec::Fixed(size), align);
            self.adopt_variants(&mut record, tag, false);
            return Ok(record);
        }
        let mut payload_align = 1u32;
        for pieces in &layouts {
            let (_, _, a) = self.place(pieces, packed, 0)?;
            payload_align = payload_align.max(a);
        }
        let tag_align = if packed { 1 } else { 4 };
        let start = align_up(4, u64::from(payload_align))?;
        let mut end = 4u64;
        let mut tag = TagEncoding {
            offset: 0,
            form: TagForm::Ordinal,
            variants: Vec::new(),
        };
        for (ordinal, pieces) in layouts.iter().enumerate() {
            let (fields, e, _) = self.place(pieces, packed, start)?;
            end = end.max(e);
            tag.variants.push(VariantLayout {
                ordinal: ordinal as u32,
                fields,
            });
        }
        let natural = payload_align.max(tag_align);
        let align = promises.align.map_or(natural, |r| natural.max(r));
        let size = align_up(end, u64::from(align))?;
        let mut record = self.blank(key, LayoutKind::Variant, SizeSpec::Fixed(size), align);
        self.adopt_variants(&mut record, tag, packed);
        Ok(record)
    }

    /// The null-pointer niche (layout version 2): a non-`@packed` variant of
    /// exactly two variants, one without fields and one with a non-null pointer
    /// field (a counted block reference, Ptr or FnPtr). It has no tag word: the first
    /// such field's word is null exactly when the fieldless variant is active,
    /// so `Leaf | Node(Box[T], Box[T])` is two words. Returns the fieldless
    /// ordinal, the other ordinal and the index of its niche field.
    fn niche(&self, variants: &[Vec<Piece>], packed: bool) -> Option<(u32, u32, usize)> {
        let [first, second] = variants else {
            return None;
        };
        let (nullary, dataful, pieces) = match (first.is_empty(), second.is_empty()) {
            (true, false) => (0, 1, second),
            (false, true) => (1, 0, first),
            _ => return None,
        };
        if packed {
            return None;
        }
        let field = pieces.iter().position(|p| {
            matches!(
                self.record(p.layout).kind,
                LayoutKind::Block { .. }
                    | LayoutKind::Box { .. }
                    | LayoutKind::Scalar(ScalarKind::Pointer)
            )
        })?;
        Some((nullary, dataful, field))
    }

    fn adopt_variants(&self, record: &mut LayoutRecord, tag: TagEncoding, packed: bool) {
        let mut scans = Vec::new();
        let mut drops = Vec::new();
        let mut copy = CopyRecipe::Bits;
        // A niche's word is the dataful variant's pointer leaf.
        let mut leaves = match tag.form {
            TagForm::Ordinal => vec![(tag.offset, ScalarKind::U32)],
            TagForm::Niche { .. } => Vec::new(),
            TagForm::LiveBit { .. } => {
                unreachable!("derivation creates only ordinal and niche variant tags")
            }
        };
        let mut unaligned = packed;
        let mut storage = ALL_STORAGE.to_vec();
        for variant in &tag.variants {
            let Lifted {
                scan,
                drop,
                copy: c,
                leaves: l,
                unaligned: u,
            } = self.lift(&variant.fields);
            if !scan.is_empty() {
                scans.push((variant.ordinal, scan));
            }
            if !drop.is_empty() {
                drops.push((variant.ordinal, drop));
            }
            copy = join_copy(copy, c);
            leaves.extend(l);
            unaligned |= u;
            storage.retain(|s| self.storage_of(&variant.fields).contains(s));
        }
        if !scans.is_empty() {
            record.scan = vec![ScanStep::ActiveVariant {
                tag_offset: tag.offset,
                form: tag.form,
                variants: scans,
            }];
        }
        if !drops.is_empty() {
            record.drop = vec![DropStep::ActiveVariant {
                tag_offset: tag.offset,
                form: tag.form,
                variants: drops,
            }];
        }
        record.copy = copy;
        leaves.sort();
        leaves.dedup();
        record.abi = abi_inputs(&record.size, leaves, unaligned);
        record.storage = storage;
        record.tag = Some(tag);
    }

    /// `Packed[T]`: the active ordinal plus only that variant's payload. Each
    /// variant's extent is rounded to the whole alignment, so consecutive
    /// Packed values stay aligned. Distinct from `@packed`.
    fn packed_variant<'s>(
        &mut self,
        walk: &mut Walk<'s>,
        key: &[u8],
        inner: TypeId,
        f: usize,
    ) -> Result<LayoutRecord, LayoutError>
    where
        'o: 's,
    {
        let (variants, nf, pushed) = self.variant_shape(walk, inner, f)?;
        let result = (|| {
            let mut layouts = Vec::new();
            for fields in &variants {
                let mut pieces = Vec::new();
                for &field in fields {
                    pieces.push(self.piece(walk, field, nf)?);
                }
                layouts.push(pieces);
            }
            let mut payload_align = 1u32;
            for pieces in &layouts {
                payload_align = payload_align.max(self.place(pieces, false, 0)?.2);
            }
            let align = payload_align.max(4);
            let start = align_up(4, u64::from(payload_align))?;
            let mut tag = TagEncoding {
                offset: 0,
                form: TagForm::Ordinal,
                variants: Vec::new(),
            };
            let mut sizes = Vec::new();
            for (ordinal, pieces) in layouts.iter().enumerate() {
                let (fields, end, _) = self.place(pieces, false, start)?;
                sizes.push((ordinal as u32, align_up(end, u64::from(align))?));
                tag.variants.push(VariantLayout {
                    ordinal: ordinal as u32,
                    fields,
                });
            }
            let size = packed_size(&sizes);
            let mut record = self.blank(key, LayoutKind::Packed, size, align);
            self.adopt_variants(&mut record, tag, false);
            Ok(record)
        })();
        if pushed {
            walk.frames.pop();
        }
        result
    }

    /// The variants of `ty`, which must be a variant, directly or as a plain
    /// nominal's representation. Returns the frame its fields live in.
    fn variant_shape<'s>(
        &mut self,
        walk: &mut Walk<'s>,
        ty: TypeId,
        f: usize,
    ) -> Result<(Vec<Vec<TypeId>>, usize, bool), LayoutError>
    where
        'o: 's,
    {
        let (ty, f) = walk.resolve(ty, f)?;
        match &walk.row(ty, f)?.kind {
            TypeKind::Variant(variants) => Ok((variant_fields(variants), f, false)),
            TypeKind::Named {
                definition,
                arguments,
            } => {
                let owner = self
                    .owners
                    .nominals
                    .get(definition)
                    .ok_or_else(|| LayoutError::MissingOwner(describe(definition)))?;
                let OwnerRepresentation::Defined(record) = owner else {
                    return invalid("Packed of a handle or cursor type");
                };
                if record.resource {
                    return unavailable(None, "Packed of a resource variant");
                }
                if record.layout_promises != Layout::default() {
                    return unavailable(Some(47), "Packed of a variant with layout attributes");
                }
                let nf = self.enter(walk, record, arguments, f)?;
                match &walk.row(record.private_shape, nf)?.kind {
                    TypeKind::Variant(variants) => Ok((variant_fields(variants), nf, true)),
                    _ => {
                        walk.frames.pop();
                        invalid("Packed of a non-variant type")
                    }
                }
            }
            _ => invalid("Packed of a non-variant type"),
        }
    }

    /// Push the frame of a nominal's private representation.
    fn enter<'s>(
        &self,
        walk: &mut Walk<'s>,
        record: &'s RepresentationRecord,
        arguments: &'s [GenericArgument],
        f: usize,
    ) -> Result<usize, LayoutError>
    where
        'o: 's,
    {
        if record.generics.len() != arguments.len()
            || record.generics.iter().zip(arguments).any(|(g, a)| {
                matches!(
                    (g, a),
                    (GenericParameter::Type { .. }, GenericArgument::Size(_))
                        | (GenericParameter::Size { .. }, GenericArgument::Type(_))
                )
            })
        {
            return invalid(format!(
                "{} takes {} generic actuals of matching kinds",
                describe(&record.definition),
                record.generics.len()
            ));
        }
        walk.frames.push(Frame {
            store: &record.types,
            actuals: arguments,
            parent: f,
        });
        Ok(walk.frames.len() - 1)
    }

    fn nominal<'s>(
        &mut self,
        walk: &mut Walk<'s>,
        key: &[u8],
        definition: &DefPath,
        arguments: &'s [GenericArgument],
        f: usize,
    ) -> Result<LayoutRecord, LayoutError>
    where
        'o: 's,
    {
        let owners = self.owners;
        let owner = owners
            .nominals
            .get(definition)
            .ok_or_else(|| LayoutError::MissingOwner(describe(definition)))?;
        let record = match owner {
            OwnerRepresentation::Handle { revision, resource } => {
                let mut handle = self.handle_record(key, *resource);
                if !resource && definition.0.package.name_is("core") && definition.0.name == "Path"
                {
                    handle.kind = LayoutKind::Path;
                    handle.scan.clear();
                }
                // An arena token is a record address, never a run-context ID.
                if !resource && definition.0.package.name_is("core") && definition.0.name == "Arena"
                {
                    let (size, align) = self.pointer();
                    handle = self.blank(key, LayoutKind::Arena, SizeSpec::Fixed(size), align);
                    self.words(&mut handle, &[ScalarKind::Pointer]);
                }
                handle.key.owner_revision = Some(*revision);
                return Ok(handle);
            }
            OwnerRepresentation::Cursor { revision } => {
                let mut cursor = self.cursor_record(key)?;
                cursor.key.owner_revision = Some(*revision);
                return Ok(cursor);
            }
            OwnerRepresentation::ViewCursor { revision } => {
                let byte = self.byte();
                self.sequence(walk, byte, false)?;
                let slice_key = b"view-cursor-slice".to_vec();
                let slice_record = self.slice_record(&slice_key, byte);
                let slice = self.insert(slice_key, slice_record);
                let word = self.scalar(ScalarKind::U64);
                let pieces = [slice, word]
                    .into_iter()
                    .map(|layout| {
                        let r = self.record(layout);
                        let SizeSpec::Fixed(size) = r.size else {
                            unreachable!()
                        };
                        Piece {
                            layout,
                            size,
                            align: r.align,
                        }
                    })
                    .collect();
                let mut cursor = self.aggregate(key, pieces, false, None)?;
                cursor.scan = vec![ScanStep::Uncounted { offset: 0 }];
                cursor.drop.clear();
                cursor.copy = CopyRecipe::Bits;
                cursor.key.owner_revision = Some(*revision);
                return Ok(cursor);
            }
            OwnerRepresentation::StringView => {
                let byte = self.byte();
                self.sequence(walk, byte, false)?;
                return Ok(self.slice_record(key, byte));
            }
            OwnerRepresentation::Defined(record) => record,
        };
        let nf = self.enter(walk, record, arguments, f)?;
        let result = self.representation(walk, key, record, nf);
        walk.frames.pop();
        let mut layout = result?;
        layout.key.owner_revision = Some(record.representation_revision);
        if record.resource {
            make_resource(&mut layout, record);
        }
        Ok(layout)
    }

    /// A nominal's private shape with its layout promises applied.
    fn representation<'s>(
        &mut self,
        walk: &mut Walk<'s>,
        key: &[u8],
        record: &RepresentationRecord,
        f: usize,
    ) -> Result<LayoutRecord, LayoutError>
    where
        'o: 's,
    {
        let p = &record.layout_promises;
        let name = describe(&record.definition);
        if p.flat && (p.c || p.packed || p.align.is_some() || p.soa) {
            return unavailable(
                Some(47),
                format!("{name}: flat combined with @layout(c), @packed, @align or @soa"),
            );
        }
        if p.packed && p.align.is_some() {
            return unavailable(Some(47), format!("{name}: @packed combined with @align"));
        }
        if p.soa && (p.c || p.packed || p.align.is_some()) {
            return unavailable(
                Some(47),
                format!("{name}: @soa combined with @layout(c), @packed or @align"),
            );
        }
        if let Some(n) = p.align
            && (n == 0 || !n.is_power_of_two())
        {
            return invalid(format!("{name}: @align({n}) is not a power of two"));
        }
        let shape = record.private_shape;
        let (shape, sf) = walk.resolve(shape, f)?;
        if p.flat {
            // Flat nominals are laid out from their FlatFormatV0 body before
            // reaching here; an owner-supplied container shape is never flat.
            return invalid(format!("{name}: a flat container representation"));
        }
        match &walk.row(shape, sf)?.kind {
            TypeKind::Record(fields) => {
                if fields.iter().any(|field| field.growable) {
                    return invalid(format!("{name}: a growable field outside a flat owner"));
                }
                let mut pieces = Vec::new();
                for field in fields.iter() {
                    if p.c {
                        self.require_c(walk, field.ty, sf, &name)?;
                    }
                    pieces.push(self.piece(walk, field.ty, sf)?);
                }
                let mut layout = self.aggregate(key, pieces, p.packed, p.align)?;
                if p.c {
                    layout.initialized = InitializedBytes::ZeroPadding;
                }
                Ok(layout)
            }
            TypeKind::Tuple(items) if !items.is_empty() => {
                let mut pieces = Vec::new();
                for item in items.iter() {
                    if p.c {
                        self.require_c(walk, *item, sf, &name)?;
                    }
                    pieces.push(self.piece(walk, *item, sf)?);
                }
                let mut layout = self.aggregate(key, pieces, p.packed, p.align)?;
                if p.c {
                    layout.initialized = InitializedBytes::ZeroPadding;
                }
                Ok(layout)
            }
            TypeKind::Variant(variants) => {
                if p.soa {
                    return unavailable(Some(47), format!("{name}: @soa on a variant"));
                }
                if p.c {
                    return unavailable(Some(6), format!("{name}: @layout(c) on a variant"));
                }
                let variants = variant_fields(variants);
                self.variant(walk, key, &variants, sf, p)
            }
            _ => {
                if p.c || p.packed || p.align.is_some() || p.soa {
                    return invalid(format!(
                        "{name}: a layout attribute on a non-aggregate representation"
                    ));
                }
                // A nominal over a non-aggregate representation (a distinct
                // scalar, a box, a handle-backed resource) holds it as its one
                // field. Through the memo, so `type A = A` is an infinite size.
                let piece = self.piece(walk, shape, sf)?;
                self.aggregate(key, vec![piece], false, None)
            }
        }
    }

    /// `@layout(c)` fields: sized scalars, nested C records and Inline arrays
    /// of them. Anything else needs the #6 interop rules.
    fn require_c(
        &self,
        walk: &Walk<'_>,
        ty: TypeId,
        f: usize,
        owner: &str,
    ) -> Result<(), LayoutError> {
        let (ty, f) = walk.resolve(ty, f)?;
        match &walk.row(ty, f)?.kind {
            TypeKind::Primitive(p) if primitive_scalar(*p).is_some() => Ok(()),
            TypeKind::Named { definition, .. } => match self.owners.nominals.get(definition) {
                Some(OwnerRepresentation::Defined(record))
                    if record.layout_promises.c && !record.resource =>
                {
                    Ok(())
                }
                _ => unavailable(
                    Some(6),
                    format!(
                        "{owner}: a @layout(c) field of non-C type {}",
                        describe(definition)
                    ),
                ),
            },
            TypeKind::Builtin {
                kind: BuiltinType::Inline,
                arguments,
            } => match arguments.first() {
                Some(GenericArgument::Type(element)) => self.require_c(walk, *element, f, owner),
                _ => invalid("Inline without an element type"),
            },
            _ => unavailable(
                Some(6),
                format!(
                    "{owner}: a @layout(c) field outside sized scalars, C records and Inline arrays"
                ),
            ),
        }
    }

    fn builtin<'s>(
        &mut self,
        walk: &mut Walk<'s>,
        key: &[u8],
        kind: BuiltinType,
        arguments: &'s [GenericArgument],
        f: usize,
    ) -> Result<LayoutRecord, LayoutError>
    where
        'o: 's,
    {
        let ty_at = |i: usize| match arguments.get(i) {
            Some(GenericArgument::Type(t)) => Ok(*t),
            _ => invalid(format!("{} needs a type actual", builtin_name(kind))),
        };
        let size_at = |walk: &Walk<'_>, i: usize| match arguments.get(i) {
            Some(GenericArgument::Size(s)) => walk.size(s, f),
            _ => invalid(format!("{} needs a size actual", builtin_name(kind))),
        };
        match kind {
            BuiltinType::Array | BuiltinType::List => {
                let element_ty = ty_at(0)?;
                let element = self.memory(walk, element_ty, f, true)?;
                let soa = self.is_soa(walk, element_ty, f)?;
                let payload = self.sequence(walk, element, soa)?;
                Ok(self.block_record(key, payload))
            }
            // MEM-11: a root reference whose header count is the root count.
            BuiltinType::Shared => {
                let value = self.memory(walk, ty_at(0)?, f, true)?;
                let payload = self.frozen(walk, value)?;
                let mut record = self.block_record(key, payload);
                record.kind = LayoutKind::Shared { payload };
                Ok(record)
            }
            BuiltinType::Inline => {
                let element_ty = ty_at(0)?;
                let count = size_at(walk, 1)?;
                if self.is_soa(walk, element_ty, f)? {
                    return unavailable(Some(47), "Inline of an @soa record");
                }
                let piece = self.piece(walk, element_ty, f)?;
                if !matches!(self.record(piece.layout).size, SizeSpec::Fixed(_)) {
                    return unavailable(None, "a sequential Packed Inline value");
                }
                let size = piece.size.checked_mul(count).ok_or(LayoutError::Overflow)?;
                let mut record = self.blank(
                    key,
                    LayoutKind::Inline {
                        element: piece.layout,
                        count,
                    },
                    SizeSpec::Fixed(size),
                    piece.align,
                );
                let e = self.record(piece.layout).clone();
                if count > 0 && !e.scan.is_empty() {
                    record.scan = vec![ScanStep::Repeat {
                        offset: 0,
                        count,
                        stride: piece.size,
                        element: piece.layout,
                    }];
                }
                if count > 0 && !e.drop.is_empty() {
                    record.drop = vec![DropStep::RepeatReverse {
                        offset: 0,
                        count,
                        stride: piece.size,
                        element: piece.layout,
                    }];
                }
                record.copy = e.copy;
                record.storage = e.storage.clone();
                let mut leaves = Vec::new();
                if size <= ABI_LEAF_BYTES {
                    for i in 0..count {
                        leaves.extend(e.abi.scalars.iter().map(|&(o, s)| (o + i * piece.size, s)));
                    }
                }
                record.abi = abi_inputs(&record.size, leaves, e.abi.unaligned);
                Ok(record)
            }
            BuiltinType::Simd => {
                let element_ty = ty_at(0)?;
                let lanes = size_at(walk, 1)?;
                let (et, ef) = walk.resolve(element_ty, f)?;
                // `simd.splat[A, n]` admits any scalar lane, bool included
                // (v22 MEM-8); `simd.add` alone needs integer lanes.
                let scalar = match &walk.row(et, ef)?.kind {
                    TypeKind::Primitive(p) => primitive_scalar(*p),
                    _ => None,
                };
                let Some(scalar) = scalar else {
                    return invalid("Simd lanes must be scalars");
                };
                if lanes == 0 {
                    return invalid("Simd requires a positive lane count");
                }
                let lanes32 = u32::try_from(lanes).map_err(|_| LayoutError::Overflow)?;
                let lane = self.target.scalar_size(scalar);
                let bytes = lane.checked_mul(lanes).ok_or(LayoutError::Overflow)?;
                let natural = bytes
                    .checked_next_power_of_two()
                    .ok_or(LayoutError::Overflow)?
                    .min(u64::from(self.target.alignments.vector))
                    .max(u64::from(self.target.scalar_align(scalar)));
                let align = u32::try_from(natural).map_err(|_| LayoutError::Overflow)?;
                let element = self.scalar(scalar);
                let mut record = self.blank(
                    key,
                    LayoutKind::Simd {
                        element,
                        lanes: lanes32,
                    },
                    SizeSpec::Fixed(align_up(bytes, natural)?),
                    align,
                );
                record.abi = abi_inputs(
                    &record.size,
                    (0..lanes).map(|i| (i * lane, scalar)).collect(),
                    false,
                );
                Ok(record)
            }
            BuiltinType::Packed => {
                let inner = ty_at(0)?;
                self.packed_variant(walk, key, inner, f)
            }
            BuiltinType::Iter => {
                let element_ty = ty_at(0)?;
                let element = self.memory(walk, element_ty, f, true)?;
                let (size, align) = self.pointer();
                let mut record = self.blank(
                    key,
                    LayoutKind::Iterator { element },
                    SizeSpec::Fixed(3 * size),
                    align,
                );
                // Environment, next thunk, drop thunk.
                self.words(
                    &mut record,
                    &[
                        ScalarKind::Pointer,
                        ScalarKind::Pointer,
                        ScalarKind::Pointer,
                    ],
                );
                record.scan = vec![ScanStep::Environment { offset: 0 }];
                record.drop = vec![DropStep::Environment { offset: 0 }];
                record.copy = CopyRecipe::NotCopyable;
                Ok(record)
            }
            BuiltinType::Map
            | BuiltinType::Set
            | BuiltinType::SortedMap
            | BuiltinType::Slots
            | BuiltinType::SlotId => {
                let c = container(kind).expect("container");
                let owners = self.owners;
                let Some(owner) = owners.containers.get(&c) else {
                    return Err(LayoutError::MissingOwner(builtin_name(kind).into()));
                };
                let nf = self.enter(walk, owner, arguments, f)?;
                if c == Container::SlotId {
                    let shape = self.representation(walk, key, owner, nf);
                    walk.frames.pop();
                    let mut record = shape?;
                    record.key.owner_revision = Some(owner.representation_revision);
                    return Ok(record);
                }
                let mut payload_key = Encoder {
                    bytes: Vec::new(),
                    revisions: true,
                };
                payload_key.tag("opaque");
                payload_key.atom(key);
                let reserved = self.reserve(&payload_key.bytes, true);
                let frames = walk.frames.clone();
                walk.frames.pop();
                let (payload, fresh) = reserved?;
                if fresh {
                    walk.pending.push(Pending::Container {
                        id: payload,
                        frames,
                        frame: nf,
                        owner,
                    });
                }
                let mut record = self.block_record(key, payload);
                record.key.owner_revision = Some(owner.representation_revision);
                Ok(record)
            }
            // Raw and callback pointers are plain machine addresses (PROG-4).
            BuiltinType::Ptr | BuiltinType::FnPtr => {
                Ok(self.scalar_record(key, ScalarKind::Pointer))
            }
            BuiltinType::TypeShape => unavailable(None, "TypeShape materialization as static data"),
            // A run-local task-table word (CONC-2); a handle nobody awaited needs no drop:
            // its scope's exit joins the child.
            BuiltinType::Task => Ok(self.handle_record(key, false)),
            // A run-local channel endpoint word (CONC-6), closed when dropped.
            BuiltinType::Sender | BuiltinType::Receiver => Ok(self.handle_record(key, true)),
            // Run-local handle words (CONC-4, CONC-5): the runtime holds an atomic's scalar
            // image and a mutex's payload block, which it releases when the run ends. That is
            // unobservable only for a copyable payload, whose Mutex is copyable too; a mutex
            // over a resource must release its payload with its last handle.
            BuiltinType::Atomic => Ok(self.handle_record(key, false)),
            BuiltinType::Mutex => {
                let payload = self.memory(walk, ty_at(0)?, f, false)?;
                if self.record(payload).copy == CopyRecipe::NotCopyable {
                    return unavailable(None, "a Mutex of a noncopyable payload is M2");
                }
                Ok(self.handle_record(key, false))
            }
            BuiltinType::Region => unavailable(None, format!("{} is M2", builtin_name(kind))),
        }
    }

    fn is_soa(&self, walk: &Walk<'_>, ty: TypeId, f: usize) -> Result<bool, LayoutError> {
        let (ty, f) = walk.resolve(ty, f)?;
        Ok(match &walk.row(ty, f)?.kind {
            TypeKind::Named { definition, .. } => matches!(
                self.owners.nominals.get(definition),
                Some(OwnerRepresentation::Defined(record)) if record.layout_promises.soa
            ),
            _ => false,
        })
    }

    // ----- FlatFormatV0 --------------------------------------------------

    /// The FlatFormatV0 body of a flat-compatible type: little-endian
    /// fixed-width scalars, bool 0/1, u32 ordinals, declaration-order
    /// concatenation, no padding. String, Bytes and List bodies carry their
    /// own u64 length prefix; records and Packed values are self-delimiting.
    fn flat_body<'s>(
        &mut self,
        walk: &mut Walk<'s>,
        ty: TypeId,
        f: usize,
    ) -> Result<LayoutId, LayoutError>
    where
        'o: 's,
    {
        let mut key = Encoder {
            bytes: Vec::new(),
            revisions: true,
        };
        key.tag("flat");
        self.encode(walk, ty, f, &mut key)?;
        let (resolved, rf) = walk.resolve(ty, f)?;
        if let TypeKind::Primitive(p) = &walk.row(resolved, rf)?.kind
            && primitive_scalar(*p).is_some()
        {
            // A fixed-width scalar is its own type's layout, so a place of a
            // flat record's scalar field has its type's layout (LIR schema
            // revision 3); the record marks the field unaligned.
            return self.memory(walk, ty, f, false);
        }
        let (ty, f) = (resolved, rf);
        let body = self.memoized(key.bytes, false, |b, key| b.derive_flat(walk, ty, f, key))?;
        // A zero-size value has no packing differences. Keep its ordinary
        // layout so element reads retain their source type, including Unit.
        let flat_nominal = matches!(&walk.row(ty, f)?.kind,
            TypeKind::Named { definition, .. } if self.flat_owner(definition));
        if self.record(body).size == SizeSpec::Fixed(0) && !flat_nominal {
            self.memory(walk, ty, f, false)
        } else {
            Ok(body)
        }
    }

    /// A field of a flat record: its body, behind a u64 length prefix when
    /// the body is variable and not already prefixed.
    fn flat_field<'s>(
        &mut self,
        walk: &mut Walk<'s>,
        ty: TypeId,
        f: usize,
    ) -> Result<Piece, LayoutError>
    where
        'o: 's,
    {
        let body = self.flat_body(walk, ty, f)?;
        let record = self.record(body);
        match record.size {
            SizeSpec::Fixed(size) => Ok(Piece {
                layout: body,
                size,
                align: 1,
            }),
            SizeSpec::Dynamic(_) if self.flat_prefixed(body) => Ok(Piece {
                layout: body,
                size: 0,
                align: 0,
            }),
            SizeSpec::Dynamic(_) => {
                let mut key = Encoder {
                    bytes: Vec::new(),
                    revisions: true,
                };
                key.tag("flat-field");
                key.atom(&record.key.instance.map_or([0; 32], |h| h.0));
                let layout = self.memoized(key.bytes, false, |b, key| {
                    Ok(b.prefixed(key, body, FLAT_PREFIX))
                })?;
                Ok(Piece {
                    layout,
                    size: 0,
                    align: 0,
                })
            }
        }
    }

    fn flat_prefixed(&self, id: LayoutId) -> bool {
        let r = self.record(id);
        r.kind == LayoutKind::Flat && r.size == prefixed_size()
    }

    /// A length-prefixed variable field: `8 + prefix` bytes, content at `at`.
    fn prefixed(&self, key: &[u8], content: LayoutId, at: u64) -> LayoutRecord {
        let mut record = self.blank(key, LayoutKind::Flat, prefixed_size(), 1);
        record.fields = vec![FieldLayout {
            ordinal: 0,
            offset: at,
            layout: content,
            unaligned: self.record(content).align > 1,
        }];
        record.initialized = InitializedBytes::ZeroPadding;
        record.copy = CopyRecipe::FlatBytes;
        record
    }

    fn derive_flat<'s>(
        &mut self,
        walk: &mut Walk<'s>,
        ty: TypeId,
        f: usize,
        key: &[u8],
    ) -> Result<LayoutRecord, LayoutError>
    where
        'o: 's,
    {
        match &walk.row(ty, f)?.kind {
            TypeKind::Primitive(Primitive::String | Primitive::Bytes) => {
                let byte = self.byte();
                Ok(self.prefixed(key, byte, FLAT_PREFIX))
            }
            TypeKind::Primitive(Primitive::Never) => invalid("never in a flat value"),
            TypeKind::Primitive(p) => {
                let scalar = primitive_scalar(*p).expect("sized scalar");
                Ok(self.scalar_record(key, scalar))
            }
            TypeKind::Tuple(items) => {
                let mut pieces = Vec::new();
                for item in items.iter() {
                    pieces.push(self.flat_field(walk, *item, f)?);
                }
                self.flat_record(key, pieces)
            }
            TypeKind::Record(fields) => {
                let last = fields.len().saturating_sub(1);
                let mut pieces = Vec::new();
                for (i, field) in fields.iter().enumerate() {
                    if field.growable && (i != last || !self.is_list(walk, field.ty, f)?) {
                        return invalid("the growable field must be the last field and a List");
                    }
                    pieces.push(self.flat_field(walk, field.ty, f)?);
                }
                self.flat_record(key, pieces)
            }
            TypeKind::Named {
                definition,
                arguments,
            } => {
                let owners = self.owners;
                let Some(OwnerRepresentation::Defined(record)) = owners.nominals.get(definition)
                else {
                    return invalid(format!("{} is not flat-compatible", describe(definition)));
                };
                if record.resource {
                    return invalid(format!(
                        "a resource ({}) is not flat-compatible",
                        describe(definition)
                    ));
                }
                let p = &record.layout_promises;
                if p.c || p.packed || p.align.is_some() || p.soa {
                    return unavailable(
                        Some(47),
                        format!(
                            "{}: a flat field with layout attributes",
                            describe(definition)
                        ),
                    );
                }
                let nf = self.enter(walk, record, arguments, f)?;
                let shape = record.private_shape;
                let body = self.derive_flat_in(walk, shape, nf, key);
                walk.frames.pop();
                let mut body = body?;
                body.key.owner_revision = Some(record.representation_revision);
                Ok(body)
            }
            TypeKind::Builtin {
                kind: BuiltinType::List | BuiltinType::Array,
                arguments,
            } => {
                let Some(GenericArgument::Type(element)) = arguments.first() else {
                    return invalid("List needs a type actual");
                };
                let element = self.flat_field(walk, *element, f)?;
                if element.align == 0 || self.record(element.layout).size == SizeSpec::Fixed(0) {
                    // Variable and zero-size elements need an explicit count.
                    Ok(self.prefixed(key, element.layout, 2 * FLAT_PREFIX))
                } else {
                    Ok(self.prefixed(key, element.layout, FLAT_PREFIX))
                }
            }
            TypeKind::Builtin {
                kind: BuiltinType::Inline,
                arguments,
            } => {
                let (Some(GenericArgument::Type(element)), Some(GenericArgument::Size(n))) =
                    (arguments.first(), arguments.get(1))
                else {
                    return invalid("Inline needs a type and a size actual");
                };
                let count = walk.size(n, f)?;
                let element = self.flat_field(walk, *element, f)?;
                if element.align == 0 {
                    return invalid("Inline of a variable flat element");
                }
                let size = element
                    .size
                    .checked_mul(count)
                    .ok_or(LayoutError::Overflow)?;
                let mut record = self.blank(
                    key,
                    LayoutKind::Inline {
                        element: element.layout,
                        count,
                    },
                    SizeSpec::Fixed(size),
                    1,
                );
                record.initialized = InitializedBytes::ZeroPadding;
                record.copy = CopyRecipe::FlatBytes;
                record.abi.unaligned = true;
                Ok(record)
            }
            TypeKind::Builtin {
                kind: BuiltinType::Packed,
                arguments,
            } => {
                let Some(GenericArgument::Type(inner)) = arguments.first() else {
                    return invalid("Packed needs a type actual");
                };
                let (variants, nf, pushed) = self.variant_shape(walk, *inner, f)?;
                let result = (|| {
                    let mut tag = TagEncoding {
                        offset: 0,
                        form: TagForm::Ordinal,
                        variants: Vec::new(),
                    };
                    let mut programs = Vec::new();
                    for (ordinal, fields) in variants.iter().enumerate() {
                        let mut pieces = Vec::new();
                        for &field in fields {
                            pieces.push(self.flat_field(walk, field, nf)?);
                        }
                        let (fields, program) = self.flat_sequence(&pieces, 4)?;
                        programs.push((ordinal as u32, program));
                        tag.variants.push(VariantLayout {
                            ordinal: ordinal as u32,
                            fields,
                        });
                    }
                    let size = flat_tag_size(programs)?;
                    let mut record = self.blank(key, LayoutKind::Packed, size, 1);
                    record.tag = Some(tag);
                    record.initialized = InitializedBytes::ZeroPadding;
                    record.copy = CopyRecipe::FlatBytes;
                    record.abi.unaligned = true;
                    Ok(record)
                })();
                if pushed {
                    walk.frames.pop();
                }
                result
            }
            _ => invalid(
                "a pointer-bearing, capability, function or handle type is not flat-compatible",
            ),
        }
    }

    fn derive_flat_in<'s>(
        &mut self,
        walk: &mut Walk<'s>,
        ty: TypeId,
        f: usize,
        key: &[u8],
    ) -> Result<LayoutRecord, LayoutError>
    where
        'o: 's,
    {
        let (ty, f) = walk.resolve(ty, f)?;
        self.derive_flat(walk, ty, f, key)
    }

    fn is_list(&self, walk: &Walk<'_>, ty: TypeId, f: usize) -> Result<bool, LayoutError> {
        let (ty, f) = walk.resolve(ty, f)?;
        Ok(matches!(
            walk.row(ty, f)?.kind,
            TypeKind::Builtin {
                kind: BuiltinType::List,
                ..
            }
        ))
    }

    fn flat_record(&self, key: &[u8], pieces: Vec<Piece>) -> Result<LayoutRecord, LayoutError> {
        let (fields, program) = self.flat_sequence(&pieces, 0)?;
        let size = fold_program(program)?;
        let mut record = self.blank(key, LayoutKind::Flat, size, 1);
        record.fields = fields;
        record.initialized = InitializedBytes::ZeroPadding;
        record.copy = CopyRecipe::FlatBytes;
        record.abi.unaligned = true;
        Ok(record)
    }

    /// Static field offsets from `start` and the cursor program summing the
    /// extents: a variable piece (align 0) counts 8 statically and its prefix
    /// value dynamically.
    fn flat_sequence(
        &self,
        pieces: &[Piece],
        start: u64,
    ) -> Result<(Vec<FieldLayout>, Vec<SizeTerm>), LayoutError> {
        let mut fields = Vec::new();
        let mut offset = start;
        let mut program = vec![SizeTerm::Constant(start)];
        for (ordinal, piece) in pieces.iter().enumerate() {
            fields.push(FieldLayout {
                ordinal: ordinal as u32,
                offset,
                layout: piece.layout,
                unaligned: self.record(piece.layout).align > 1,
            });
            if piece.align == 0 {
                offset = offset
                    .checked_add(FLAT_PREFIX)
                    .ok_or(LayoutError::Overflow)?;
                program.extend([
                    SizeTerm::Dup,
                    SizeTerm::At(prefixed_program()),
                    SizeTerm::Add,
                ]);
            } else {
                offset = offset
                    .checked_add(piece.size)
                    .ok_or(LayoutError::Overflow)?;
                program.extend([SizeTerm::Constant(piece.size), SizeTerm::Add]);
            }
        }
        Ok((fields, program))
    }
}

const ALL_STORAGE: [StorageKind; 5] = [
    StorageKind::Heap,
    StorageKind::LocalRegion,
    StorageKind::Arena,
    StorageKind::Frozen,
    StorageKind::Static,
];
/// A value with destruction state never lives in frozen or static storage.
const RESOURCE_STORAGE: [StorageKind; 3] = [
    StorageKind::Heap,
    StorageKind::LocalRegion,
    StorageKind::Arena,
];

fn scalar_name(s: ScalarKind) -> &'static str {
    match s {
        ScalarKind::Bool => "bool",
        ScalarKind::U8 => "u8",
        ScalarKind::U16 => "u16",
        ScalarKind::U32 => "u32",
        ScalarKind::U64 => "u64",
        ScalarKind::U128 => "u128",
        ScalarKind::I8 => "i8",
        ScalarKind::I16 => "i16",
        ScalarKind::I32 => "i32",
        ScalarKind::I64 => "i64",
        ScalarKind::I128 => "i128",
        ScalarKind::F32 => "f32",
        ScalarKind::F64 => "f64",
        ScalarKind::Pointer => "pointer",
    }
}

fn variant_fields(variants: &[hirsel_types::Variant]) -> Vec<Vec<TypeId>> {
    variants
        .iter()
        .map(|v| v.fields.iter().map(|(_, ty)| *ty).collect())
        .collect()
}

/// A Resource keeps its representation's fields; the user finalizer runs
/// (borrowed, shielded) before the implicit fields drop high to low.
fn make_resource(layout: &mut LayoutRecord, record: &RepresentationRecord) {
    layout.kind = LayoutKind::Resource;
    layout.copy = CopyRecipe::NotCopyable;
    layout.storage.retain(|s| RESOURCE_STORAGE.contains(s));
    if let Some((definition, body)) = &record.finalizer {
        layout.finalizer = Some(FinalizerRef {
            definition: definition.clone(),
            body: body.0,
            unsafe_ffi_package: record.unsafe_ffi_package,
        });
        layout.drop.insert(0, DropStep::Finalize);
    }
}

fn join_copy(a: CopyRecipe, b: CopyRecipe) -> CopyRecipe {
    use CopyRecipe::*;
    match (a, b) {
        (NotCopyable, _) | (_, NotCopyable) => NotCopyable,
        (Bits, Bits) => Bits,
        _ => Fields,
    }
}

fn abi_inputs(size: &SizeSpec, leaves: Vec<(u64, ScalarKind)>, unaligned: bool) -> AbiInputs {
    let small = matches!(size, SizeSpec::Fixed(n) if *n <= ABI_LEAF_BYTES);
    AbiInputs {
        scalars: if small { leaves } else { Vec::new() },
        unaligned,
    }
}

fn prefixed_program() -> Vec<SizeTerm> {
    vec![
        SizeTerm::Constant(FLAT_PREFIX),
        SizeTerm::ReadLength {
            offset: 0,
            scalar: ScalarKind::U64,
        },
        SizeTerm::Add,
    ]
}

fn prefixed_size() -> SizeSpec {
    SizeSpec::Dynamic(prefixed_program())
}

/// Collapse a cursor program with no dynamic step to its checked constant.
fn fold_program(program: Vec<SizeTerm>) -> Result<SizeSpec, LayoutError> {
    let mut total = 0u64;
    for term in &program {
        match term {
            SizeTerm::Constant(n) => total = total.checked_add(*n).ok_or(LayoutError::Overflow)?,
            SizeTerm::Add => {}
            _ => return Ok(SizeSpec::Dynamic(program)),
        }
    }
    Ok(SizeSpec::Fixed(total))
}

fn packed_size(sizes: &[(u32, u64)]) -> SizeSpec {
    match sizes {
        [] => SizeSpec::Fixed(4),
        [(_, first), rest @ ..] if rest.iter().all(|(_, s)| s == first) => SizeSpec::Fixed(*first),
        _ => SizeSpec::Dynamic(vec![SizeTerm::ByTag {
            tag_offset: 0,
            variants: sizes
                .iter()
                .map(|&(o, s)| (o, vec![SizeTerm::Constant(s)]))
                .collect(),
        }]),
    }
}

fn flat_tag_size(programs: Vec<(u32, Vec<SizeTerm>)>) -> Result<SizeSpec, LayoutError> {
    let mut folded = Vec::new();
    for (o, p) in &programs {
        folded.push((*o, fold_program(p.clone())?));
    }
    Ok(match folded.as_slice() {
        [] => SizeSpec::Fixed(4),
        [(_, SizeSpec::Fixed(first)), rest @ ..]
            if rest.iter().all(|(_, s)| *s == SizeSpec::Fixed(*first)) =>
        {
            SizeSpec::Fixed(*first)
        }
        _ => SizeSpec::Dynamic(vec![SizeTerm::ByTag {
            tag_offset: 0,
            variants: programs,
        }]),
    })
}

fn drop_offset(step: &DropStep) -> Option<u64> {
    match step {
        DropStep::Release { offset }
        | DropStep::Child { offset, .. }
        | DropStep::Thaw { offset, .. }
        | DropStep::Environment { offset }
        | DropStep::CloseHandle { offset }
        | DropStep::ElementsReverse { offset, .. }
        | DropStep::RepeatReverse { offset, .. } => Some(*offset),
        DropStep::ActiveVariant { tag_offset, .. } => Some(*tag_offset),
        DropStep::Finalize | DropStep::ColumnsReverse(_) => None,
    }
}
