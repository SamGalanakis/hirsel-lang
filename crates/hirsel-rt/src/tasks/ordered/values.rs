//! Initialized-field observation, following SER-2's iterative native walk
//! (codec.rs:846,1008): fields in declaration order and only the active arm.
//! Unlike SER-2 this keeps exact floating-point bits and resolves handles to
//! logical object identities. There is no whole-carrier byte slice, user
//! callback, allocation in the run heap, or scheduling point in this decoder.
//! Programs use compiler-derived metadata under private trace admission.

use crate::ordered::footprint::ObjectId;
use crate::ordered::objects::Kind;
use hirsel_core_contract::handle_scan::*;
use std::collections::BTreeMap;

const WORK_MAX: usize = 65_536;
const BYTES_MAX: usize = 1 << 20;

#[derive(Clone, Debug)]
pub(super) struct Field {
    pub offset: usize,
    pub node: usize,
}

#[derive(Clone, Debug)]
pub(super) enum Tag {
    Ordinal { offset: usize },
    NonNull { offset: usize, nullary: u32 },
}

impl Tag {
    /// # Safety
    /// The validated tag bytes of the live value at `at` are initialized.
    unsafe fn ordinal(&self, at: *const u8) -> u32 {
        unsafe {
            match self {
                Self::Ordinal { offset } => at.add(*offset).cast::<u32>().read_unaligned(),
                Self::NonNull { offset, nullary } => {
                    if at.add(*offset).cast::<u64>().read_unaligned() == 0 {
                        *nullary
                    } else {
                        1 - nullary
                    }
                }
            }
        }
    }
}

/// Same live-count/capacity layout as alloc::allocate_sequence. Only the
/// initialized prefix is read; spare element storage remains untouched.
unsafe fn sequence_count(
    at: *const u8,
    capacity: usize,
    minimum: usize,
    offset: usize,
    stride: usize,
    budget: usize,
) -> Result<usize, Error> {
    if capacity < minimum {
        return Err(Error::Value);
    }
    let count =
        usize::try_from(unsafe { at.cast::<u64>().read_unaligned() }).map_err(|_| Error::Value)?;
    let reserved = usize::try_from(unsafe { at.add(8).cast::<u64>().read_unaligned() })
        .map_err(|_| Error::Value)?;
    if count > budget {
        return Err(Error::Limit);
    }
    let end = count
        .checked_mul(stride)
        .and_then(|bytes| offset.checked_add(bytes))
        .ok_or(Error::Value)?;
    let reserved_end = reserved
        .checked_mul(stride)
        .and_then(|bytes| offset.checked_add(bytes))
        .ok_or(Error::Value)?;
    if count > reserved || reserved_end > capacity || end > capacity || end > isize::MAX as usize {
        return Err(Error::Value);
    }
    Ok(count)
}

struct EntryWalk<'a> {
    descriptor: u32,
    minimum: usize,
    offset: usize,
    stride: usize,
    tag: &'a Tag,
    fields: &'a [Field],
}

fn entry_walk(nodes: &[Node], root: usize) -> Result<EntryWalk<'_>, Error> {
    let Some(Node::Indirect { node, descriptor }) = nodes.get(root) else {
        return Err(Error::Program);
    };
    let Some(Node::Sequence {
        minimum,
        offset,
        stride,
        node,
    }) = nodes.get(*node)
    else {
        return Err(Error::Program);
    };
    let Some(Node::Variant { tag, arms, .. }) = nodes.get(*node) else {
        return Err(Error::Program);
    };
    if arms.len() != 2 || !arms[0].is_empty() || arms[1].len() != 1 {
        return Err(Error::Program);
    }
    let pair = &arms[1][0];
    if !matches!(nodes.get(pair.node), Some(Node::Fields { fields, .. }) if fields.len() == 2) {
        return Err(Error::Program);
    }
    Ok(EntryWalk {
        descriptor: *descriptor,
        minimum: *minimum,
        offset: *offset,
        stride: *stride,
        tag,
        fields: &arms[1],
    })
}

#[derive(Clone, Debug)]
pub(super) enum Node {
    Bits(usize),
    Bool,
    Fields {
        extent: usize,
        fields: Vec<Field>,
    },
    Variant {
        extent: usize,
        tag: Tag,
        arms: Vec<Vec<Field>>,
    },
    Handle(Kind),
    Path,
    Indirect {
        node: usize,
        descriptor: u32,
    },
    Sequence {
        minimum: usize,
        offset: usize,
        stride: usize,
        node: usize,
    },
    Slice {
        byte: usize,
    },
    Entries {
        extent: usize,
        entries: Field,
        live: Field,
    },
    /// Missing metadata is never permission to inspect arbitrary bytes.
    Unsupported {
        extent: usize,
    },
}

impl Node {
    fn extent(&self) -> usize {
        match self {
            Self::Bits(bytes) => *bytes,
            Self::Bool => 1,
            Self::Slice { .. } => 24,
            Self::Handle(_) | Self::Path | Self::Indirect { .. } => 8,
            Self::Sequence { minimum, .. } => *minimum,
            Self::Entries { extent, .. }
            | Self::Fields { extent, .. }
            | Self::Variant { extent, .. }
            | Self::Unsupported { extent } => *extent,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Error {
    Program,
    Value,
    Unsupported,
    UnresolvedHandle,
    Limit,
}

#[derive(Clone)]
pub(super) struct Program {
    nodes: Vec<Node>,
    root: usize,
    /// Owner layouts may differ from the view layout (including flat views).
    /// Copy immutable extent facts only; no worker/heap borrow spans encoding.
    owners: Vec<OwnerBounds>,
    names: Option<std::sync::Arc<crate::paths::Names>>,
}

impl std::fmt::Debug for Program {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Program")
            .field("nodes", &self.nodes)
            .field("root", &self.root)
            .field("owners", &self.owners)
            .field("names_bound", &self.names.is_some())
            .finish()
    }
}

#[derive(Clone, Copy, Debug)]
struct OwnerBounds {
    size: u64,
    flags: u32,
    #[cfg(feature = "system-allocator")]
    align: u64,
}

impl OwnerBounds {
    /// Match alloc::Heap::usable without retaining its worker-local heap.
    /// # Safety
    /// The live owner has this linked layout and matching allocation metadata.
    unsafe fn capacity(self, owner: *const crate::Header) -> Result<usize, Error> {
        let header = unsafe { &*owner };
        let bytes = match header.kind() {
            #[cfg(feature = "system-allocator")]
            crate::alloc::KIND_SMALL => {
                if self.flags & (crate::LAYOUT_NO_EXTENT | crate::release::LAYOUT_WIDE_CURSOR)
                    != crate::LAYOUT_NO_EXTENT
                    || self.align > 8
                {
                    return Err(Error::Value);
                }
                crate::alloc::request_capacity(self.size, 8)
            }
            crate::alloc::KIND_REGION if self.flags & crate::LAYOUT_NO_EXTENT != 0 => self.size,
            _ => unsafe { header.capacity() },
        };
        usize::try_from(bytes).map_err(|_| Error::Value)
    }
}

impl Program {
    /// Decode compiler-emitted initialized-field metadata already linked in
    /// the native holder-scan stream. No compiler table index enters values.
    /// # Safety
    /// Matching immutable descriptors and programs live for this run.
    pub(super) unsafe fn linked(ctx: *mut crate::TaskCtx, root: u32) -> Result<Self, Error> {
        unsafe {
            let mut program = Self::from_layouts(root, (*ctx).heap_ref().layout_count(), |id| {
                (*ctx).descriptor(id)
            })?;
            if program.nodes.iter().any(|node| matches!(node, Node::Path)) {
                program.names = (*ctx).run_ref().names.clone();
            }
            Ok(program)
        }
    }

    unsafe fn from_layouts(
        root: u32,
        count: u32,
        descriptor: impl Fn(u32) -> *const crate::Layout,
    ) -> Result<Self, Error> {
        let mut ids = BTreeMap::from([(root, 0usize)]);
        let mut queue = vec![root];
        let mut nodes = Vec::new();
        let mut scanned = 0;
        while nodes.len() < queue.len() {
            let id = queue[nodes.len()];
            if id >= count || queue.len() > WORK_MAX {
                return Err(Error::Program);
            }
            let layout = unsafe { &*descriptor(id) };
            if layout.scan_words == 0
                || layout.scan_words == u64::MAX
                || !layout.scan_words.is_multiple_of(WORDS as u64)
                || layout.scan_words > (WORK_MAX * WORDS) as u64
                || layout.scan.is_null()
            {
                return Err(Error::Unsupported);
            }
            let words =
                unsafe { std::slice::from_raw_parts(layout.scan, layout.scan_words as usize) };
            let ops = words.as_chunks::<WORDS>().0;
            scanned += ops.len();
            if scanned > WORK_MAX {
                return Err(Error::Limit);
            }
            let mut headers = ops.iter().filter(|op| op[0] == VALUE);
            let header = headers.next().ok_or(Error::Unsupported)?;
            if headers.next().is_some() || header[2] != layout.size {
                return Err(Error::Program);
            }
            let extent = usize::try_from(layout.size).map_err(|_| Error::Program)?;
            let mut field = |op: &[u64; WORDS]| -> Result<Field, Error> {
                let child = u32::try_from(op[2]).map_err(|_| Error::Program)?;
                if child >= count {
                    return Err(Error::Program);
                }
                let index = *ids.entry(child).or_insert_with(|| {
                    let index = queue.len();
                    queue.push(child);
                    index
                });
                Ok(Field {
                    offset: usize::try_from(op[1]).map_err(|_| Error::Program)?,
                    node: index,
                })
            };
            let value_fields = ops.iter().filter(|op| op[0] == VALUE_FIELD);
            let node = match header[1] {
                VALUE_BITS => Node::Bits(extent),
                VALUE_BOOL if extent == 1 => Node::Bool,
                VALUE_FIELDS => Node::Fields {
                    extent,
                    fields: value_fields
                        .map(|op| {
                            if op[3] != u64::MAX {
                                return Err(Error::Program);
                            }
                            field(op)
                        })
                        .collect::<Result<_, _>>()?,
                },
                VALUE_VARIANT => {
                    let arms = usize::try_from(header[6]).map_err(|_| Error::Program)?;
                    if arms == 0 || arms > WORK_MAX {
                        return Err(Error::Program);
                    }
                    let mut arms = vec![Vec::new(); arms];
                    for op in value_fields {
                        let arm = usize::try_from(op[3]).map_err(|_| Error::Program)?;
                        arms.get_mut(arm).ok_or(Error::Program)?.push(field(op)?);
                    }
                    let offset = usize::try_from(header[3]).map_err(|_| Error::Program)?;
                    let tag = match header[4] {
                        0 => Tag::Ordinal { offset },
                        1 => Tag::NonNull {
                            offset,
                            nullary: u32::try_from(header[5]).map_err(|_| Error::Program)?,
                        },
                        _ => return Err(Error::Program),
                    };
                    Node::Variant { extent, tag, arms }
                }
                VALUE_ENTRIES => Node::Entries {
                    extent,
                    entries: field(&[VALUE_FIELD, header[3], header[4], u64::MAX, 0, 0, 0, 0])?,
                    live: field(&[VALUE_FIELD, header[5], header[6], u64::MAX, 0, 0, 0, 0])?,
                },
                VALUE_PATH if extent == 8 => Node::Path,
                VALUE_HANDLE if extent == 8 => {
                    Node::Handle(Kind::from_code(header[3]).ok_or(Error::Program)?)
                }
                VALUE_INDIRECT if extent == 8 => {
                    let descriptor = u32::try_from(header[3]).map_err(|_| Error::Program)?;
                    let node = field(&[VALUE_FIELD, 0, header[3], u64::MAX, 0, 0, 0, 0])?.node;
                    Node::Indirect { node, descriptor }
                }
                VALUE_SEQUENCE => {
                    let node = field(&[VALUE_FIELD, 0, header[5], u64::MAX, 0, 0, 0, 0])?.node;
                    Node::Sequence {
                        minimum: extent,
                        offset: usize::try_from(header[4]).map_err(|_| Error::Program)?,
                        stride: usize::try_from(header[3]).map_err(|_| Error::Program)?,
                        node,
                    }
                }
                VALUE_SLICE if extent == 24 => {
                    let byte = field(&[VALUE_FIELD, 0, header[3], u64::MAX, 0, 0, 0, 0])?.node;
                    Node::Slice { byte }
                }
                VALUE_UNKNOWN => Node::Unsupported { extent },
                _ => return Err(Error::Program),
            };
            nodes.push(node);
        }
        let needs_owners = nodes.iter().any(|node| matches!(node, Node::Slice { .. }));
        let mut program = Self::new(nodes, 0)?;
        if needs_owners {
            if count as usize > WORK_MAX {
                return Err(Error::Limit);
            }
            program.owners = (0..count)
                .map(|id| {
                    let layout = unsafe { &*descriptor(id) };
                    OwnerBounds {
                        size: layout.size,
                        flags: layout.flags,
                        #[cfg(feature = "system-allocator")]
                        align: layout.align,
                    }
                })
                .collect();
        }
        Ok(program)
    }

    pub(super) fn new(nodes: Vec<Node>, root: usize) -> Result<Self, Error> {
        if nodes.len() > WORK_MAX || root >= nodes.len() {
            return Err(Error::Program);
        }
        for node in &nodes {
            let extent = node.extent();
            let check_fields = |fields: &[Field]| {
                if fields.len() > WORK_MAX {
                    return Err(Error::Program);
                }
                for field in fields {
                    let child = nodes.get(field.node).ok_or(Error::Program)?;
                    if matches!(child, Node::Sequence { .. }) {
                        return Err(Error::Program);
                    }
                    if field
                        .offset
                        .checked_add(child.extent())
                        .is_none_or(|end| end > extent)
                    {
                        return Err(Error::Program);
                    }
                }
                Ok(())
            };
            match node {
                Node::Bits(bytes) if !matches!(bytes, 1 | 2 | 4 | 8 | 16) => {
                    return Err(Error::Program);
                }
                Node::Indirect { node, .. } if *node >= nodes.len() => return Err(Error::Program),
                Node::Slice { byte } if !matches!(nodes.get(*byte), Some(Node::Bits(1))) => {
                    return Err(Error::Program);
                }
                Node::Sequence {
                    minimum,
                    offset,
                    stride,
                    node,
                } => {
                    let child = nodes.get(*node).ok_or(Error::Program)?;
                    if *minimum < 16
                        || *offset < *minimum
                        || child.extent() > *stride
                        || matches!(child, Node::Sequence { .. })
                    {
                        return Err(Error::Program);
                    }
                }
                Node::Entries { entries, live, .. } => {
                    check_fields(&[entries.clone(), live.clone()])?;
                    if !matches!(nodes.get(live.node), Some(Node::Bits(8))) {
                        return Err(Error::Program);
                    }
                    entry_walk(&nodes, entries.node)?;
                }
                Node::Fields { fields, .. } => check_fields(fields)?,
                Node::Variant { tag, arms, .. } => {
                    if arms.is_empty() || arms.len() > WORK_MAX {
                        return Err(Error::Program);
                    }
                    let (offset, size) = match tag {
                        Tag::Ordinal { offset } => (*offset, 4),
                        Tag::NonNull { offset, nullary } => {
                            if arms.len() != 2 || *nullary > 1 {
                                return Err(Error::Program);
                            }
                            (*offset, 8)
                        }
                    };
                    if offset.checked_add(size).is_none_or(|end| end > extent) {
                        return Err(Error::Program);
                    }
                    for arm in arms {
                        check_fields(arm)?;
                    }
                }
                _ => {}
            }
        }
        Ok(Self {
            nodes,
            root,
            owners: Vec::new(),
            names: None,
        })
    }

    pub(super) fn extent(&self) -> usize {
        self.nodes[self.root].extent()
    }

    /// # Safety
    /// `value` holds the program's live native value. The program identifies
    /// its actual initialized fields and tags; inactive fields and padding may
    /// be uninitialized. Resolve is read-only, bounded, and cannot yield.
    pub(super) unsafe fn encode(
        &self,
        value: *const u8,
        mut resolve: impl FnMut(Kind, u64) -> Option<ObjectId>,
    ) -> Result<Vec<u8>, Error> {
        if self.extent() > 0 && value.is_null() {
            return Err(Error::Value);
        }
        let mut work = vec![(self.root, value, None)];
        let mut out = Vec::new();
        let mut visited = 0;
        while let Some((index, at, capacity)) = work.pop() {
            visited += 1;
            if visited > WORK_MAX {
                return Err(Error::Limit);
            }
            let fields = match &self.nodes[index] {
                Node::Bits(bytes) => {
                    // Read exactly the scalar, never its containing carrier.
                    let bits = unsafe {
                        match bytes {
                            1 => u128::from(at.read()),
                            2 => u128::from(at.cast::<u16>().read_unaligned()),
                            4 => u128::from(at.cast::<u32>().read_unaligned()),
                            8 => u128::from(at.cast::<u64>().read_unaligned()),
                            16 => at.cast::<u128>().read_unaligned(),
                            _ => unreachable!("validated scalar width"),
                        }
                    };
                    out.extend_from_slice(&bits.to_le_bytes()[..*bytes]);
                    continue;
                }
                Node::Bool => {
                    let value = unsafe { at.read() };
                    if value > 1 {
                        return Err(Error::Value);
                    }
                    out.push(value);
                    continue;
                }
                Node::Path => {
                    // SER-2's codec.rs Node::Path normalization: text is
                    // semantic, while insertion order and the intern word vary.
                    let word = unsafe { at.cast::<u64>().read_unaligned() };
                    let names = self.names.as_ref().ok_or(Error::Unsupported)?;
                    let text = names.paths.text(word).ok_or(Error::Value)?;
                    if out
                        .len()
                        .checked_add(8)
                        .and_then(|n| n.checked_add(text.len()))
                        .is_none_or(|n| n > BYTES_MAX)
                    {
                        return Err(Error::Limit);
                    }
                    out.extend_from_slice(&(text.len() as u64).to_le_bytes());
                    out.extend_from_slice(text.as_bytes());
                    continue;
                }
                Node::Handle(kind) => {
                    let word = unsafe { at.cast::<u64>().read_unaligned() };
                    let object = resolve(*kind, word).ok_or(Error::UnresolvedHandle)?;
                    if object.creator.len() > WORK_MAX {
                        return Err(Error::Limit);
                    }
                    out.extend_from_slice(&kind.code().to_le_bytes());
                    out.extend_from_slice(&(object.creator.len() as u64).to_le_bytes());
                    for part in object.creator {
                        out.extend_from_slice(&part.to_le_bytes());
                    }
                    out.extend_from_slice(&object.ordinal.to_le_bytes());
                    if out.len() > BYTES_MAX {
                        return Err(Error::Limit);
                    }
                    continue;
                }
                Node::Entries { entries, live, .. } => {
                    let shape = entry_walk(&self.nodes, entries.node)?;
                    let owner = unsafe {
                        at.add(entries.offset)
                            .cast::<*const crate::Header>()
                            .read_unaligned()
                    };
                    if owner.is_null() || unsafe { (*owner).index() } != shape.descriptor {
                        return Err(Error::Value);
                    }
                    let capacity = usize::try_from(unsafe { (*owner).capacity() })
                        .map_err(|_| Error::Value)?;
                    let payload = unsafe { owner.add(1).cast::<u8>() };
                    let count = unsafe {
                        sequence_count(
                            payload,
                            capacity,
                            shape.minimum,
                            shape.offset,
                            shape.stride,
                            WORK_MAX.saturating_sub(visited + work.len()),
                        )?
                    };
                    visited += count;
                    let live = unsafe { at.add(live.offset).cast::<u64>().read_unaligned() };
                    if live > count as u64 {
                        return Err(Error::Value);
                    }
                    out.extend_from_slice(&live.to_le_bytes());
                    let mut observed = 0u64;
                    // Codec container walk, preserving insertion order for both
                    // Map and Set (core-library); skip holes without payload reads.
                    for i in (0..count).rev() {
                        let entry = unsafe { payload.add(shape.offset + i * shape.stride) };
                        match unsafe { shape.tag.ordinal(entry) } {
                            0 => (),
                            1 => {
                                observed += 1;
                                for field in shape.fields.iter().rev() {
                                    work.push((
                                        field.node,
                                        unsafe { entry.add(field.offset) },
                                        None,
                                    ));
                                }
                            }
                            _ => return Err(Error::Value),
                        }
                    }
                    if live != observed {
                        return Err(Error::Value);
                    }
                    if visited + work.len() > WORK_MAX {
                        return Err(Error::Limit);
                    }
                    continue;
                }
                Node::Slice { .. } => {
                    // Same byte range as codec.rs::text_bytes; the physical
                    // owner and start never enter the canonical value.
                    let owner = unsafe { at.cast::<*const crate::Header>().read_unaligned() };
                    if owner.is_null() {
                        return Err(Error::Value);
                    }
                    let bounds = self
                        .owners
                        .get(unsafe { (*owner).index() } as usize)
                        .ok_or(Error::Value)?;
                    let capacity = unsafe { bounds.capacity(owner)? };
                    let start = unsafe { at.add(8).cast::<u64>().read_unaligned() };
                    let length = unsafe { at.add(16).cast::<u64>().read_unaligned() };
                    let start = usize::try_from(start)
                        .ok()
                        .and_then(|n| n.checked_add(8))
                        .ok_or(Error::Value)?;
                    let length = usize::try_from(length).map_err(|_| Error::Value)?;
                    let end = start.checked_add(length).ok_or(Error::Value)?;
                    if end > capacity || end > isize::MAX as usize {
                        return Err(Error::Value);
                    }
                    if bounds.flags & crate::LAYOUT_SEQUENCE != 0 {
                        if capacity < 16 || start < 16 {
                            return Err(Error::Value);
                        }
                        let live = unsafe { owner.add(1).cast::<u64>().read_unaligned() };
                        let live_end = usize::try_from(live)
                            .ok()
                            .and_then(|n| n.checked_add(16))
                            .ok_or(Error::Value)?;
                        if live_end > capacity || end > live_end {
                            return Err(Error::Value);
                        }
                    }
                    if out
                        .len()
                        .checked_add(8)
                        .and_then(|n| n.checked_add(length))
                        .is_none_or(|n| n > BYTES_MAX)
                    {
                        return Err(Error::Limit);
                    }
                    out.extend_from_slice(&(length as u64).to_le_bytes());
                    out.extend_from_slice(unsafe {
                        std::slice::from_raw_parts(owner.add(1).cast::<u8>().add(start), length)
                    });
                    continue;
                }
                Node::Indirect { node, descriptor } => {
                    let block = unsafe { at.cast::<*const crate::Header>().read_unaligned() };
                    if block.is_null() || unsafe { (*block).index() } != *descriptor {
                        return Err(Error::Value);
                    }
                    // The typed live carrier owns this counted payload; only
                    // initialized fields of its linked layout are traversed.
                    let capacity = if matches!(self.nodes[*node], Node::Sequence { .. }) {
                        Some(
                            usize::try_from(unsafe { (*block).capacity() })
                                .map_err(|_| Error::Value)?,
                        )
                    } else {
                        None
                    };
                    work.push((*node, unsafe { block.add(1).cast() }, capacity));
                    continue;
                }
                Node::Sequence {
                    minimum,
                    offset,
                    stride,
                    node,
                } => {
                    let capacity = capacity.ok_or(Error::Value)?;
                    let count = unsafe {
                        sequence_count(
                            at,
                            capacity,
                            *minimum,
                            *offset,
                            *stride,
                            WORK_MAX.saturating_sub(visited + work.len()),
                        )?
                    };
                    out.extend_from_slice(&(count as u64).to_le_bytes());
                    for i in (0..count).rev() {
                        work.push((*node, unsafe { at.add(offset + i * stride) }, None));
                    }
                    continue;
                }
                Node::Fields { fields, .. } => fields,
                Node::Variant { tag, arms, .. } => {
                    let ordinal = unsafe { tag.ordinal(at) };
                    let arm = arms.get(ordinal as usize).ok_or(Error::Value)?;
                    out.extend_from_slice(&ordinal.to_le_bytes());
                    arm
                }
                Node::Unsupported { .. } => return Err(Error::Unsupported),
            };
            if work.len() + fields.len() > WORK_MAX {
                return Err(Error::Limit);
            }
            for field in fields.iter().rev() {
                work.push((field.node, unsafe { at.add(field.offset) }, None));
            }
        }
        if out.len() > BYTES_MAX {
            return Err(Error::Limit);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::MaybeUninit;

    #[test]
    fn derived_layout_metadata_decodes_active_fields_independently_of_link_indices() {
        use hirsel_layout::layout::{LayoutBuilder, Owners};
        use hirsel_lir::layout_types::{SizeSpec, TargetSpec};
        use hirsel_types::{Primitive, testing::Types};
        for target in TargetSpec::executed() {
            let mut types = Types::default();
            let boolean = types.p(Primitive::Bool);
            let number = types.p(Primitive::U64);
            let record = types.record(&[boolean, number]);
            let option = types.variant(&[&[], &[record]]);
            let owners = Owners::default();
            let mut builder = LayoutBuilder::new(&target, &owners);
            let root = builder.layout(&types.store, option).unwrap();
            let table = builder.finish();
            hirsel_lir::layout_validate::validate_table(&table, &target).unwrap();
            let count = table.layouts.len();
            let mut reference = None;
            for rotation in [0, 1] {
                let linked = |id: hirsel_lir::LayoutId| ((id.0 as usize + rotation) % count) as u32;
                let words: Vec<_> = table
                    .layouts
                    .iter()
                    .map(|record| {
                        hirsel_lir::handle_scan::Program::lower(record)
                            .linked(|id| Some(linked(id)))
                            .unwrap()
                    })
                    .collect();
                let layouts: Vec<_> = table
                    .layouts
                    .iter()
                    .enumerate()
                    .map(|(i, record)| crate::Layout {
                        magic: crate::LAYOUT_MAGIC,
                        version: crate::LAYOUT_VERSION,
                        size: match record.size {
                            SizeSpec::Fixed(size) => size,
                            _ => panic!("fixed fixture"),
                        },
                        align: u64::from(record.align),
                        finalizer: None,
                        steps: std::ptr::null(),
                        step_count: 0,
                        field_count: 0,
                        flags: 0,
                        logical: 0,
                        logical_count: std::ptr::null(),
                        scan: words[i].as_ptr(),
                        scan_words: words[i].len() as u64,
                    })
                    .collect();
                let program = unsafe {
                    Program::from_layouts(linked(root), count as u32, |id| {
                        &layouts[(id as usize + count - rotation) % count]
                    })
                }
                .unwrap();
                assert_eq!(program.extent(), 24);
                let mut value = MaybeUninit::<[u64; 3]>::uninit();
                let at = value.as_mut_ptr().cast::<u8>();
                unsafe {
                    at.cast::<u32>().write(1);
                    at.add(8).write(1);
                    at.add(16).cast::<u64>().write(42);
                    let bytes = program.encode(at, |_, _| panic!("no handles")).unwrap();
                    assert_eq!(
                        bytes,
                        [1u32.to_le_bytes().as_slice(), &[1], &42u64.to_le_bytes()].concat()
                    );
                    if let Some(reference) = &reference {
                        assert_eq!(&bytes, reference);
                    } else {
                        reference = Some(bytes);
                    }
                    at.cast::<u32>().write(0);
                    assert_eq!(
                        program.encode(at, |_, _| None),
                        Ok(0u32.to_le_bytes().to_vec())
                    );
                }
            }
        }
    }

    fn native_layouts(
        table: &hirsel_lir::layout_types::LayoutTable,
        rotation: usize,
    ) -> (Vec<Vec<u64>>, Vec<crate::Layout>) {
        let count = table.layouts.len();
        let linked = |id: hirsel_lir::LayoutId| ((id.0 as usize + rotation) % count + 2) as u32;
        let words: Vec<_> = table
            .layouts
            .iter()
            .map(|record| {
                hirsel_lir::handle_scan::Program::lower(record)
                    .linked(|id| Some(linked(id)))
                    .unwrap()
            })
            .collect();
        let layouts = table
            .layouts
            .iter()
            .enumerate()
            .map(|(i, record)| crate::Layout {
                magic: crate::LAYOUT_MAGIC,
                version: crate::LAYOUT_VERSION,
                size: hirsel_lir::rt_program::descriptor(table, hirsel_lir::LayoutId(i as u32))
                    .unwrap()
                    .size,
                align: u64::from(record.align),
                finalizer: None,
                steps: std::ptr::null(),
                step_count: 0,
                field_count: 0,
                flags: hirsel_lir::rt_program::descriptor(table, hirsel_lir::LayoutId(i as u32))
                    .unwrap()
                    .flags,
                logical: 0,
                logical_count: std::ptr::null(),
                scan: words[i].as_ptr(),
                scan_words: words[i].len() as u64,
            })
            .collect();
        (words, layouts)
    }

    #[test]
    fn linked_indirect_values_ignore_addresses_counts_padding_and_layout_index_order() {
        use hirsel_layout::layout::{LayoutBuilder, Owners};
        use hirsel_lir::layout_types::{LayoutKind, TargetSpec};
        use hirsel_types::{BuiltinType, GenericArgument, Primitive, testing::Types};
        for target in TargetSpec::executed() {
            let mut types = Types::default();
            let boolean = types.p(Primitive::Bool);
            let number = types.p(Primitive::U64);
            let record = types.record(&[boolean, number]);
            let boxed = types.boxed(record);
            let shared = types.builtin(BuiltinType::Shared, vec![GenericArgument::Type(boxed)]);
            let pair = types.record(&[boxed, shared]);
            let owners = Owners::default();
            let mut builder = LayoutBuilder::new(&target, &owners);
            let root = builder.layout(&types.store, pair).unwrap();
            let table = builder.finish();
            let pair = &table.layouts[root.0 as usize];
            let LayoutKind::Box { payload } = table.layouts[pair.fields[0].layout.0 as usize].kind
            else {
                panic!("box")
            };
            let LayoutKind::Shared { payload: frozen } =
                table.layouts[pair.fields[1].layout.0 as usize].kind
            else {
                panic!("shared")
            };
            let mut reference = None;
            for rotation in [0, 1] {
                let count = table.layouts.len();
                let linked =
                    |id: hirsel_lir::LayoutId| ((id.0 as usize + rotation) % count + 2) as u32;
                let (_words, layouts) = native_layouts(&table, rotation);
                let program = unsafe {
                    Program::from_layouts(linked(root), count as u32 + 2, |id| {
                        &layouts[((id as usize - 2) + count - rotation) % count]
                    })
                }
                .unwrap();
                let mut first = MaybeUninit::<[u64; 3]>::uninit();
                let mut second = MaybeUninit::<[u64; 3]>::uninit();
                let mut frozen_value = MaybeUninit::<[u64; 2]>::uninit();
                let mut frozen_other = MaybeUninit::<[u64; 2]>::uninit();
                unsafe {
                    for (block, count) in [(first.as_mut_ptr(), 1), (second.as_mut_ptr(), 79)] {
                        let block = block.cast::<crate::Header>();
                        block.write(crate::Header {
                            count,
                            descriptor: linked(payload),
                        });
                        block.add(1).cast::<u8>().write(1);
                        block.add(1).cast::<u8>().add(8).cast::<u64>().write(42);
                    }
                    let a = first.as_ptr().cast::<crate::Header>();
                    let b = second.as_ptr().cast::<crate::Header>();
                    for (frozen_block, boxed) in [
                        (frozen_value.as_mut_ptr(), a),
                        (frozen_other.as_mut_ptr(), b),
                    ] {
                        let block = frozen_block.cast::<crate::Header>();
                        block.write(crate::Header {
                            count: 9,
                            descriptor: linked(frozen),
                        });
                        block.add(1).cast::<*const crate::Header>().write(boxed);
                    }
                    let one = [a as u64, frozen_value.as_ptr() as u64];
                    let other = [b as u64, frozen_other.as_ptr() as u64];
                    let bytes = program.encode(one.as_ptr().cast(), |_, _| None).unwrap();
                    assert_eq!(
                        bytes,
                        [&[1][..], &42u64.to_le_bytes(), &[1], &42u64.to_le_bytes()].concat()
                    );
                    assert_eq!(
                        program.encode(other.as_ptr().cast(), |_, _| None).unwrap(),
                        bytes
                    );
                    if let Some(reference) = &reference {
                        assert_eq!(&bytes, reference);
                    } else {
                        reference = Some(bytes);
                    }
                }
            }
        }
    }

    #[test]
    fn linked_sequence_values_only_read_live_initialized_elements() {
        use hirsel_layout::layout::{LayoutBuilder, Owners};
        use hirsel_lir::layout_types::{LayoutKind, TargetSpec};
        use hirsel_types::{Primitive, testing::Types};
        for target in TargetSpec::executed() {
            let mut types = Types::default();
            let boolean = types.p(Primitive::Bool);
            let number = types.p(Primitive::U64);
            let record = types.record(&[boolean, number]);
            let list = types.list(record);
            let owners = Owners::default();
            let mut builder = LayoutBuilder::new(&target, &owners);
            let root = builder.layout(&types.store, list).unwrap();
            let table = builder.finish();
            let LayoutKind::Block { payload } = table.layouts[root.0 as usize].kind else {
                panic!("list block")
            };
            for rotation in [0, 1] {
                let count = table.layouts.len();
                let linked =
                    |id: hirsel_lir::LayoutId| ((id.0 as usize + rotation) % count + 2) as u32;
                let (_words, layouts) = native_layouts(&table, rotation);
                let descriptor = |id: u32| -> *const crate::Layout {
                    &layouts[((id as usize - 2) + count - rotation) % count]
                };
                let program =
                    unsafe { Program::from_layouts(linked(root), count as u32 + 2, descriptor) }
                        .unwrap();
                let pointers = [
                    vec![&layouts[0] as *const crate::Layout; 2],
                    (2..count as u32 + 2).map(descriptor).collect(),
                ]
                .concat();
                let mut heap = crate::alloc::Heap::new(pointers.into_boxed_slice(), false, 0);
                let mut view = crate::alloc::View::new();
                unsafe { heap.bind(&mut view) };
                for live in [0u64, 2] {
                    let expected = if live == 0 {
                        0u64.to_le_bytes().to_vec()
                    } else {
                        [
                            2u64.to_le_bytes().as_slice(),
                            &[1],
                            &42u64.to_le_bytes(),
                            &[0],
                            &99u64.to_le_bytes(),
                        ]
                        .concat()
                    };
                    for spare in [0u64, 3] {
                        unsafe {
                            let (block, _) =
                                heap.allocate(linked(payload), 16 + (live + spare) * 16, 8);
                            let at = crate::alloc::payload(block);
                            at.cast::<u64>().write(live);
                            at.add(8).cast::<u64>().write(live + spare);
                            // Neither padding nor unused elements are initialized.
                            if live != 0 {
                                at.add(16).write(1);
                                at.add(24).cast::<u64>().write(42);
                                at.add(32).write(0);
                                at.add(40).cast::<u64>().write(99);
                            }
                            let bytes = program
                                .encode((&block as *const *mut crate::Header).cast(), |_, _| None)
                                .unwrap();
                            assert_eq!(bytes, expected);
                            if live != 0 {
                                at.add(24).cast::<u64>().write(1000);
                            }
                            heap.free(block);
                            assert_eq!(bytes, expected, "snapshot outlives payload");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn sequence_values_reject_invalid_lengths_capacities_and_work() {
        let nodes = vec![
            Node::Indirect {
                node: 1,
                descriptor: 0,
            },
            Node::Sequence {
                minimum: 16,
                offset: 16,
                stride: 8,
                node: 2,
            },
            Node::Bits(8),
        ];
        let program = Program::new(nodes.clone(), 0).unwrap();
        let raw = Program::new(nodes.clone(), 1).unwrap();
        let layout = &crate::text::hirsel_rt_v2_bytes_descriptor.0 as *const crate::Layout;
        let mut heap = crate::alloc::Heap::new(Box::new([layout]), false, 0);
        let mut view = crate::alloc::View::new();
        unsafe {
            heap.bind(&mut view);
            let (block, capacity) = heap.allocate(0, 32, 8);
            let at = crate::alloc::payload(block);
            let encode =
                || program.encode((&block as *const *mut crate::Header).cast(), |_, _| None);
            for (length, reserved, error) in [
                (2, 1, Error::Value),
                (capacity / 8 + 1, capacity / 8 + 1, Error::Value),
                (0, u64::MAX, Error::Value),
                (WORK_MAX as u64, WORK_MAX as u64, Error::Limit),
            ] {
                at.cast::<u64>().write(length);
                at.add(8).cast::<u64>().write(reserved);
                assert_eq!(encode(), Err(error));
            }
            at.cast::<u64>().write(0);
            at.add(8).cast::<u64>().write(0);
            assert_eq!(
                raw.encode(at, |_, _| None),
                Err(Error::Value),
                "no backing extent"
            );
            heap.free(block);
            let (small, capacity) = heap.allocate(0, 4097, 8);
            // A synthetic minimum larger than this real allocation must fail
            // before any length or element read.
            let too_large = Program::new(
                vec![
                    Node::Indirect {
                        node: 1,
                        descriptor: 0,
                    },
                    Node::Sequence {
                        minimum: capacity as usize + 16,
                        offset: capacity as usize + 16,
                        stride: 8,
                        node: 2,
                    },
                    Node::Bits(8),
                ],
                0,
            )
            .unwrap();
            assert_eq!(
                too_large.encode((&small as *const *mut crate::Header).cast(), |_, _| None),
                Err(Error::Value)
            );
            heap.free(small);
        }
        for (minimum, offset, stride, node) in [
            (8, 8, 8, 2),
            (16, 8, 8, 2),
            (16, 16, 4, 2),
            (16, 16, 8, 1),
            (16, 16, 8, 3),
        ] {
            let mut bad = nodes.clone();
            bad[1] = Node::Sequence {
                minimum,
                offset,
                stride,
                node,
            };
            assert!(Program::new(bad, 0).is_err());
        }
    }

    #[test]
    fn native_boxed_and_sequence_message_events_own_payloads_before_caller_mutation() {
        use crate::tasks::*;
        use hirsel_layout::layout::{LayoutBuilder, Owners};
        use hirsel_lir::layout_types::{LayoutKind, SizeSpec, TagForm, TargetSpec};
        use hirsel_types::{Primitive, testing::Types};
        use std::sync::{Arc, Mutex};
        for sequence in [false, true] {
            let mut types = Types::default();
            let boolean = types.p(Primitive::Bool);
            let number = types.p(Primitive::U64);
            let record = types.record(&[boolean, number]);
            let boxed = if sequence {
                types.list(record)
            } else {
                types.boxed(record)
            };
            let option = types.variant(&[&[], &[boxed]]);
            let owners = Owners::default();
            let mut builder = LayoutBuilder::new(&TargetSpec::x86_64_unknown_linux_gnu(), &owners);
            let root = builder.layout(&types.store, option).unwrap();
            let table = builder.finish();
            let option = &table.layouts[root.0 as usize];
            let tag = option.tag.as_ref().unwrap();
            let pointer_offset = tag.variants[1].fields[0].offset as usize;
            let tag_offset = tag.offset as usize;
            let explicit_tag = matches!(tag.form, TagForm::Ordinal);
            let (LayoutKind::Box { payload } | LayoutKind::Block { payload }) =
                table.layouts[tag.variants[1].fields[0].layout.0 as usize].kind
            else {
                panic!("box")
            };
            let SizeSpec::Fixed(extent) = option.size else {
                panic!("fixed option")
            };
            let mut reference = None;
            for rotation in [0, 1] {
                let count = table.layouts.len();
                let linked =
                    |id: hirsel_lir::LayoutId| ((id.0 as usize + rotation) % count + 2) as u32;
                let (_words, layouts) = native_layouts(&table, rotation);
                let pointers: Vec<_> = (0..count)
                    .map(|id| &layouts[(id + count - rotation) % count] as *const crate::Layout)
                    .collect();
                for width in [1, 2, 4, 8] {
                    for reorder in [
                        hirsel_run_config::Reorder::Off,
                        hirsel_run_config::Reorder::Auto,
                    ] {
                        let output = Arc::new(Mutex::new(Vec::new()));
                        let seen = output.clone();
                        let mut run = crate::host::BodyRun::new(width, true);
                        run.config.layouts = pointers.as_ptr();
                        run.config.layout_count = pointers.len() as u32;
                        unsafe { run.use_physical_dispatch() };
                        let descriptor = linked(root);
                        let payload = linked(payload);
                        let outcome = run.run(
                            |ctx| unsafe {
                                let mut policy =
                                    crate::ordered::Policy::admit(&Default::default(), None);
                                policy.reorder = reorder;
                                super::super::engine::enable(ctx, policy);
                                super::super::events::enable(ctx);
                            },
                            move |ctx| unsafe {
                                let mut endpoints = [0u64; 2];
                                let none = vec![0u64; extent.div_ceil(8) as usize];
                                assert_eq!(
                                    hirsel_rt_v2_channel_open(
                                        ctx,
                                        std::ptr::null_mut(),
                                        endpoints.as_mut_ptr(),
                                        &0,
                                        none.as_ptr().cast(),
                                        descriptor
                                    ),
                                    crate::STATUS_OK
                                );
                                let block = crate::alloc::hirsel_rt_v2_alloc(
                                    ctx,
                                    payload,
                                    if sequence { 64 } else { 16 },
                                    8,
                                    crate::alloc::DOMAIN_HEAP,
                                    0,
                                );
                                let record = if sequence {
                                    crate::alloc::payload(block).cast::<u64>().write(1);
                                    crate::alloc::payload(block).add(8).cast::<u64>().write(3);
                                    crate::alloc::payload(block).add(16)
                                } else {
                                    crate::alloc::payload(block)
                                };
                                record.write(1);
                                record.add(8).cast::<u64>().write(42);
                                let mut image = none.clone();
                                if explicit_tag {
                                    image
                                        .as_mut_ptr()
                                        .cast::<u8>()
                                        .add(tag_offset)
                                        .cast::<u32>()
                                        .write(1);
                                }
                                image
                                    .as_mut_ptr()
                                    .cast::<u8>()
                                    .add(pointer_offset)
                                    .cast::<*mut crate::Header>()
                                    .write(block);
                                let mut status = u32::MAX;
                                assert_eq!(
                                    hirsel_rt_v2_channel_send(
                                        ctx,
                                        &mut status,
                                        &mut endpoints[0],
                                        image.as_mut_ptr().cast()
                                    ),
                                    crate::STATUS_OK
                                );
                                assert_eq!(status, crate::channels::DONE);
                                assert_eq!(image, none);
                                let mut received = none.clone();
                                assert_eq!(
                                    hirsel_rt_v2_channel_receive(
                                        ctx,
                                        0,
                                        0,
                                        &mut status,
                                        &mut endpoints[1],
                                        received.as_mut_ptr().cast()
                                    ),
                                    crate::STATUS_OK
                                );
                                assert_eq!(status, crate::channels::DONE);
                                assert_eq!(
                                    received
                                        .as_ptr()
                                        .cast::<u8>()
                                        .add(pointer_offset)
                                        .cast::<*mut crate::Header>()
                                        .read(),
                                    block
                                );
                                record.add(8).cast::<u64>().write(99);
                                received.fill(0);
                                assert_eq!(
                                    crate::release::hirsel_rt_v2_release(
                                        ctx,
                                        block,
                                        payload,
                                        std::ptr::null(),
                                        0,
                                        0
                                    ),
                                    crate::STATUS_OK
                                );
                                assert_eq!(
                                    close_endpoint(ctx, endpoints[0], 0, 0),
                                    crate::STATUS_OK
                                );
                                assert_eq!(
                                    close_endpoint(ctx, endpoints[1], 0, 0),
                                    crate::STATUS_OK
                                );
                                let _role = role::Guard::new(ctx);
                                super::super::engine::at_head(ctx).unwrap();
                                let log =
                                    super::super::engine::driver(ctx).events.as_ref().unwrap();
                                let rows = log.canonical().unwrap();
                                let sequence_count = 1u64.to_le_bytes();
                                let expected = [
                                    1u32.to_le_bytes().as_slice(),
                                    if sequence { &sequence_count[..] } else { &[] },
                                    &[1],
                                    &42u64.to_le_bytes(),
                                ]
                                .concat();
                                assert!(rows.iter().any(|row| row.outcome
                                    == super::super::events::CanonicalOutcome::Value {
                                        status: 0,
                                        payload: expected.clone()
                                    }));
                                assert!(rows.iter().any(|row| row.outcome
                                    == super::super::events::CanonicalOutcome::Received {
                                        status: crate::channels::DONE,
                                        payload: Some(expected.clone())
                                    }));
                                *seen.lock().unwrap() = rows.to_vec();
                                crate::STATUS_OK
                            },
                        );
                        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
                        let rows = output.lock().unwrap().clone();
                        if let Some(reference) = &reference {
                            assert_eq!(&rows, reference);
                        } else {
                            reference = Some(rows);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn linked_slice_values_ignore_backing_storage_and_view_offsets() {
        use hirsel_layout::layout::{LayoutBuilder, Owners};
        use hirsel_lir::layout_types::TargetSpec;
        use hirsel_types::{Primitive, testing::Types};
        for target in TargetSpec::executed() {
            for primitive in [Primitive::String, Primitive::Bytes] {
                let mut types = Types::default();
                let slice = types.p(primitive);
                let byte = types.p(Primitive::U8);
                let inline = types.inline(byte, 40);
                let owners = Owners::default();
                let mut builder = LayoutBuilder::new(&target, &owners);
                let root = builder.layout(&types.store, slice).unwrap();
                let flat = builder.flat(&types.store, slice).unwrap();
                let fixed = builder.flat(&types.store, inline).unwrap();
                let table = builder.finish();
                for rotation in [0, 1] {
                    let count = table.layouts.len();
                    let linked =
                        |id: hirsel_lir::LayoutId| ((id.0 as usize + rotation) % count + 2) as u32;
                    let (_words, layouts) = native_layouts(&table, rotation);
                    let mut pointers = vec![
                        &crate::text::hirsel_rt_v2_bytes_descriptor.0 as *const crate::Layout,
                        &crate::io::hirsel_rt_v2_file_walk_descriptor.0,
                    ];
                    pointers.extend((0..count).map(|id| {
                        &layouts[(id + count - rotation) % count] as *const crate::Layout
                    }));
                    let program = unsafe {
                        Program::from_layouts(linked(root), pointers.len() as u32, |id| {
                            pointers[id as usize]
                        })
                    }
                    .unwrap();
                    let mut heap = crate::alloc::Heap::new(pointers.into_boxed_slice(), false, 0);
                    let mut view = crate::alloc::View::new();
                    unsafe { heap.bind(&mut view) };
                    for bytes in [b"".as_slice(), "h\u{e9}llo".as_bytes()] {
                        let expected =
                            [(bytes.len() as u64).to_le_bytes().as_slice(), bytes].concat();
                        for (descriptor, offset) in
                            [(0, 16usize), (linked(flat), 9), (linked(fixed), 13)]
                        {
                            unsafe {
                                let (owner, _) = heap.allocate(descriptor, 40, 8);
                                let data = crate::alloc::payload(owner);
                                if descriptor == 0 {
                                    data.cast::<u64>().write((offset - 16 + bytes.len()) as u64);
                                    data.add(8).cast::<u64>().write(24);
                                } else {
                                    // Fixed flat storage is an initialized inline byte array.
                                    if descriptor == linked(fixed) {
                                        data.write_bytes(0xee, 40);
                                    }
                                    data.cast::<u64>().write(bytes.len() as u64);
                                }
                                std::ptr::copy_nonoverlapping(
                                    bytes.as_ptr(),
                                    data.add(offset),
                                    bytes.len(),
                                );
                                let slice = crate::views::Slice {
                                    owner,
                                    start: (offset - 8) as u64,
                                    length: bytes.len() as u64,
                                };
                                let observed = program
                                    .encode(
                                        (&slice as *const crate::views::Slice).cast(),
                                        |_, _| None,
                                    )
                                    .unwrap();
                                assert_eq!(observed, expected);
                                data.add(offset).write_bytes(0xff, bytes.len());
                                heap.free(owner);
                                assert_eq!(observed, expected);
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn slice_values_reject_invalid_ranges_and_bounded_output_before_reading_bytes() {
        let mut program = Program::new(vec![Node::Slice { byte: 1 }, Node::Bits(1)], 0).unwrap();
        program.owners = vec![OwnerBounds {
            size: 16,
            flags: crate::LAYOUT_DYNAMIC | crate::LAYOUT_SEQUENCE,
            #[cfg(feature = "system-allocator")]
            align: 8,
        }];
        let layout = &crate::text::hirsel_rt_v2_bytes_descriptor.0 as *const crate::Layout;
        let mut heap = crate::alloc::Heap::new(Box::new([layout]), false, 0);
        let mut view = crate::alloc::View::new();
        unsafe {
            heap.bind(&mut view);
            let (owner, capacity) = heap.allocate(0, 32, 8);
            crate::alloc::payload(owner).cast::<u64>().write(2);
            crate::alloc::payload(owner)
                .add(8)
                .cast::<u64>()
                .write(capacity - 16);
            for (start, length) in [(u64::MAX, 0), (8, u64::MAX), (0, 1), (8, 3), (capacity, 0)] {
                let slice = crate::views::Slice {
                    owner,
                    start,
                    length,
                };
                assert_eq!(
                    program.encode((&slice as *const crate::views::Slice).cast(), |_, _| None),
                    Err(Error::Value)
                );
            }
            let null = crate::views::Slice {
                owner: std::ptr::null_mut(),
                start: 8,
                length: 0,
            };
            assert_eq!(
                program.encode((&null as *const crate::views::Slice).cast(), |_, _| None),
                Err(Error::Value)
            );
            // Out-of-registry provenance fails before looking for block metadata.
            (*owner).descriptor = 1;
            let bad = crate::views::Slice {
                owner,
                start: 8,
                length: 0,
            };
            assert_eq!(
                program.encode((&bad as *const crate::views::Slice).cast(), |_, _| None),
                Err(Error::Value)
            );
            (*owner).descriptor = 0;
            heap.free(owner);
            let (large, _) = heap.allocate(0, BYTES_MAX as u64 + 16, 8);
            crate::alloc::payload(large)
                .cast::<u64>()
                .write(BYTES_MAX as u64);
            let slice = crate::views::Slice {
                owner: large,
                start: 8,
                length: BYTES_MAX as u64,
            };
            // No element needs to be initialized: the output limit wins first.
            assert_eq!(
                program.encode((&slice as *const crate::views::Slice).cast(), |_, _| None),
                Err(Error::Limit)
            );
            heap.free(large);
        }
        for byte in [0, 2] {
            assert!(Program::new(vec![Node::Slice { byte }, Node::Bits(1)], 0).is_err());
        }
        assert!(Program::new(vec![Node::Slice { byte: 1 }, Node::Bits(8)], 0).is_err());
    }

    #[test]
    fn native_slice_message_events_own_visible_bytes_before_mutation_and_release() {
        use crate::tasks::*;
        use hirsel_layout::layout::{LayoutBuilder, Owners};
        use hirsel_lir::layout_types::{SizeSpec, TagForm, TargetSpec};
        use hirsel_types::{Primitive, testing::Types};
        use std::sync::{Arc, Mutex};
        for primitive in [Primitive::String, Primitive::Bytes] {
            let mut types = Types::default();
            let text = types.p(primitive);
            let option = types.variant(&[&[], &[text]]);
            let owners = Owners::default();
            let mut builder = LayoutBuilder::new(&TargetSpec::x86_64_unknown_linux_gnu(), &owners);
            let root = builder.layout(&types.store, option).unwrap();
            let table = builder.finish();
            let option = &table.layouts[root.0 as usize];
            let tag = option.tag.as_ref().unwrap();
            let slice_offset = tag.variants[1].fields[0].offset as usize;
            let tag_offset = tag.offset as usize;
            let explicit_tag = matches!(tag.form, TagForm::Ordinal);
            let SizeSpec::Fixed(extent) = option.size else {
                panic!("fixed slice option")
            };
            let mut reference = None;
            for rotation in [0, 1] {
                let count = table.layouts.len();
                let linked =
                    |id: hirsel_lir::LayoutId| ((id.0 as usize + rotation) % count + 2) as u32;
                let (_words, layouts) = native_layouts(&table, rotation);
                let pointers: Vec<_> = (0..count)
                    .map(|id| &layouts[(id + count - rotation) % count] as *const crate::Layout)
                    .collect();
                for width in [1, 2, 4, 8] {
                    for reorder in [
                        hirsel_run_config::Reorder::Off,
                        hirsel_run_config::Reorder::Auto,
                    ] {
                        let output = Arc::new(Mutex::new(Vec::new()));
                        let seen = output.clone();
                        let mut run = crate::host::BodyRun::new(width, true);
                        run.config.layouts = pointers.as_ptr();
                        run.config.layout_count = pointers.len() as u32;
                        unsafe { run.use_physical_dispatch() };
                        let descriptor = linked(root);
                        let outcome = run.run(
                            |ctx| unsafe {
                                let mut policy =
                                    crate::ordered::Policy::admit(&Default::default(), None);
                                policy.reorder = reorder;
                                super::super::engine::enable(ctx, policy);
                                super::super::events::enable(ctx);
                            },
                            move |ctx| unsafe {
                                let mut endpoints = [0u64; 2];
                                let none = vec![0u64; extent.div_ceil(8) as usize];
                                assert_eq!(
                                    hirsel_rt_v2_channel_open(
                                        ctx,
                                        std::ptr::null_mut(),
                                        endpoints.as_mut_ptr(),
                                        &0,
                                        none.as_ptr().cast(),
                                        descriptor
                                    ),
                                    crate::STATUS_OK
                                );
                                let mut slice =
                                    crate::text::allocate_bytes(ctx, b"left-visible-right");
                                slice.start += 5;
                                slice.length = 7;
                                let mut image = none.clone();
                                if explicit_tag {
                                    image
                                        .as_mut_ptr()
                                        .cast::<u8>()
                                        .add(tag_offset)
                                        .cast::<u32>()
                                        .write(1);
                                }
                                image
                                    .as_mut_ptr()
                                    .cast::<u8>()
                                    .add(slice_offset)
                                    .cast::<crate::views::Slice>()
                                    .write(slice);
                                let mut status = u32::MAX;
                                assert_eq!(
                                    hirsel_rt_v2_channel_send(
                                        ctx,
                                        &mut status,
                                        &mut endpoints[0],
                                        image.as_mut_ptr().cast()
                                    ),
                                    crate::STATUS_OK
                                );
                                assert_eq!(status, crate::channels::DONE);
                                assert_eq!(image, none);
                                let mut received = none.clone();
                                assert_eq!(
                                    hirsel_rt_v2_channel_receive(
                                        ctx,
                                        0,
                                        0,
                                        &mut status,
                                        &mut endpoints[1],
                                        received.as_mut_ptr().cast()
                                    ),
                                    crate::STATUS_OK
                                );
                                assert_eq!(status, crate::channels::DONE);
                                let returned = received
                                    .as_ptr()
                                    .cast::<u8>()
                                    .add(slice_offset)
                                    .cast::<crate::views::Slice>()
                                    .read();
                                assert_eq!(returned.owner, slice.owner);
                                assert_eq!(returned.start, slice.start);
                                assert_eq!(returned.length, slice.length);
                                crate::alloc::payload(slice.owner)
                                    .add(8 + slice.start as usize)
                                    .write_bytes(0xff, 7);
                                received.fill(0);
                                assert_eq!(
                                    crate::release::hirsel_rt_v2_release(
                                        ctx,
                                        slice.owner,
                                        0,
                                        std::ptr::null(),
                                        0,
                                        0
                                    ),
                                    crate::STATUS_OK
                                );
                                assert_eq!(
                                    close_endpoint(ctx, endpoints[0], 0, 0),
                                    crate::STATUS_OK
                                );
                                assert_eq!(
                                    close_endpoint(ctx, endpoints[1], 0, 0),
                                    crate::STATUS_OK
                                );
                                let _role = role::Guard::new(ctx);
                                super::super::engine::at_head(ctx).unwrap();
                                let log =
                                    super::super::engine::driver(ctx).events.as_ref().unwrap();
                                let rows = log.canonical().unwrap();
                                let expected = [
                                    1u32.to_le_bytes().as_slice(),
                                    &7u64.to_le_bytes(),
                                    b"visible",
                                ]
                                .concat();
                                assert!(rows.iter().any(|row| row.outcome
                                    == super::super::events::CanonicalOutcome::Value {
                                        status: 0,
                                        payload: expected.clone()
                                    }));
                                assert!(rows.iter().any(|row| row.outcome
                                    == super::super::events::CanonicalOutcome::Received {
                                        status: crate::channels::DONE,
                                        payload: Some(expected.clone())
                                    }));
                                *seen.lock().unwrap() = rows.to_vec();
                                crate::STATUS_OK
                            },
                        );
                        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
                        let rows = output.lock().unwrap().clone();
                        if let Some(reference) = &reference {
                            assert_eq!(&rows, reference);
                        } else {
                            reference = Some(rows);
                        }
                    }
                }
            }
        }
    }

    fn path_layout(
        target: &hirsel_lir::layout_types::TargetSpec,
        option: bool,
    ) -> (hirsel_lir::layout_types::LayoutTable, hirsel_lir::LayoutId) {
        use hirsel_layout::layout::{LayoutBuilder, OwnerRepresentation, Owners};
        use hirsel_syntax::{ContentHash, DefPath, ItemPath, PackageId};
        use hirsel_types::testing::Types;
        let path = DefPath(ItemPath {
            package: PackageId {
                name: "core".into(),
                version: "1".into(),
            },
            modules: Vec::new(),
            name: "Path".into(),
        });
        let mut owners = Owners::default();
        owners.nominals.insert(
            path.clone().into(),
            OwnerRepresentation::Handle {
                revision: ContentHash::of_bytes(b"path trace fixture"),
                resource: false,
            },
        );
        let mut types = Types::default();
        let path = types.named(&path, Vec::new());
        let value = if option {
            types.variant(&[&[], &[path]])
        } else {
            path
        };
        let mut builder = LayoutBuilder::new(target, &owners);
        let root = builder.layout(&types.store, value).unwrap();
        (builder.finish(), root)
    }

    #[test]
    fn linked_path_values_resolve_text_and_reject_unknown_provenance() {
        use hirsel_lir::layout_types::TargetSpec;
        for target in TargetSpec::executed() {
            let (table, root) = path_layout(&target, false);
            for rotation in [0, 1] {
                let count = table.layouts.len();
                let linked =
                    |id: hirsel_lir::LayoutId| ((id.0 as usize + rotation) % count + 2) as u32;
                let (_words, layouts) = native_layouts(&table, rotation);
                let mut program = unsafe {
                    Program::from_layouts(linked(root), count as u32 + 2, |id| {
                        &layouts[((id as usize - 2) + count - rotation) % count]
                    })
                }
                .unwrap();
                assert_eq!(
                    unsafe { program.encode((&1u64 as *const u64).cast(), |_, _| None) },
                    Err(Error::Unsupported)
                );
                for noise in [0, 7] {
                    let names = std::sync::Arc::new(crate::paths::Names::default());
                    for i in 0..noise {
                        names.paths.intern(&format!("/noise/{i}"));
                    }
                    let word = names.paths.intern("/dir/h\u{e9}llo");
                    program.names = Some(names.clone());
                    let text = "/dir/h\u{e9}llo";
                    let expected = [
                        (text.len() as u64).to_le_bytes().as_slice(),
                        text.as_bytes(),
                    ]
                    .concat();
                    assert_eq!(
                        unsafe { program.encode((&word as *const u64).cast(), |_, _| None) }
                            .unwrap(),
                        expected
                    );
                    for unknown in [0, u64::MAX] {
                        assert_eq!(
                            unsafe { program.encode((&unknown as *const u64).cast(), |_, _| None) },
                            Err(Error::Value)
                        );
                    }
                    let huge = names.paths.intern(&"x".repeat(BYTES_MAX));
                    assert_eq!(
                        unsafe { program.encode((&huge as *const u64).cast(), |_, _| None) },
                        Err(Error::Limit)
                    );
                }
            }
        }
    }

    #[test]
    fn native_path_message_events_ignore_concurrent_intern_order() {
        use crate::tasks::*;
        use hirsel_lir::layout_types::{SizeSpec, TagForm, TargetSpec};
        use std::sync::{Arc, Mutex};
        let (table, root) = path_layout(&TargetSpec::x86_64_unknown_linux_gnu(), true);
        let option = &table.layouts[root.0 as usize];
        let tag = option.tag.as_ref().unwrap();
        let word_offset = tag.variants[1].fields[0].offset as usize;
        let tag_offset = tag.offset as usize;
        let explicit_tag = matches!(tag.form, TagForm::Ordinal);
        let SizeSpec::Fixed(extent) = option.size else {
            panic!("fixed path option")
        };
        let mut reference = None;
        let mut physical = Vec::new();
        for rotation in [0, 1] {
            let count = table.layouts.len();
            let linked = |id: hirsel_lir::LayoutId| ((id.0 as usize + rotation) % count + 2) as u32;
            let (_words, layouts) = native_layouts(&table, rotation);
            let pointers: Vec<_> = (0..count)
                .map(|id| &layouts[(id + count - rotation) % count] as *const crate::Layout)
                .collect();
            for width in [1, 2, 4, 8] {
                for reorder in [
                    hirsel_run_config::Reorder::Off,
                    hirsel_run_config::Reorder::Auto,
                ] {
                    let names = Arc::new(crate::paths::Names::default());
                    // Concurrent interning changes the physical word before the
                    // same semantic value is delivered through the native channel.
                    std::thread::scope(|scope| {
                        for i in 0..width {
                            let names = &names;
                            scope.spawn(move || {
                                names.paths.intern(&format!("/noise/{i}"));
                            });
                        }
                    });
                    let word = names.paths.intern("/same/path");
                    physical.push(word);
                    let output = Arc::new(Mutex::new(Vec::new()));
                    let seen = output.clone();
                    let mut run = crate::host::BodyRun::new(width, true);
                    run.config.layouts = pointers.as_ptr();
                    run.config.layout_count = pointers.len() as u32;
                    unsafe { run.use_physical_dispatch() };
                    let descriptor = linked(root);
                    let outcome = run.run(
                        move |ctx| unsafe {
                            (*ctx).run().names = Some(names);
                            let mut policy =
                                crate::ordered::Policy::admit(&Default::default(), None);
                            policy.reorder = reorder;
                            super::super::engine::enable(ctx, policy);
                            super::super::events::enable(ctx);
                        },
                        move |ctx| unsafe {
                            let mut endpoints = [0u64; 2];
                            let none = vec![0u64; extent.div_ceil(8) as usize];
                            assert_eq!(
                                hirsel_rt_v2_channel_open(
                                    ctx,
                                    std::ptr::null_mut(),
                                    endpoints.as_mut_ptr(),
                                    &0,
                                    none.as_ptr().cast(),
                                    descriptor
                                ),
                                crate::STATUS_OK
                            );
                            let mut image = none.clone();
                            if explicit_tag {
                                image
                                    .as_mut_ptr()
                                    .cast::<u8>()
                                    .add(tag_offset)
                                    .cast::<u32>()
                                    .write(1);
                            }
                            image
                                .as_mut_ptr()
                                .cast::<u8>()
                                .add(word_offset)
                                .cast::<u64>()
                                .write(word);
                            let mut status = u32::MAX;
                            assert_eq!(
                                hirsel_rt_v2_channel_send(
                                    ctx,
                                    &mut status,
                                    &mut endpoints[0],
                                    image.as_mut_ptr().cast()
                                ),
                                crate::STATUS_OK
                            );
                            assert_eq!(status, crate::channels::DONE);
                            assert_eq!(image, none);
                            let mut received = none.clone();
                            assert_eq!(
                                hirsel_rt_v2_channel_receive(
                                    ctx,
                                    0,
                                    0,
                                    &mut status,
                                    &mut endpoints[1],
                                    received.as_mut_ptr().cast()
                                ),
                                crate::STATUS_OK
                            );
                            assert_eq!(status, crate::channels::DONE);
                            assert_eq!(
                                received
                                    .as_ptr()
                                    .cast::<u8>()
                                    .add(word_offset)
                                    .cast::<u64>()
                                    .read(),
                                word
                            );
                            received.fill(0);
                            assert_eq!(close_endpoint(ctx, endpoints[0], 0, 0), crate::STATUS_OK);
                            assert_eq!(close_endpoint(ctx, endpoints[1], 0, 0), crate::STATUS_OK);
                            let _role = role::Guard::new(ctx);
                            super::super::engine::at_head(ctx).unwrap();
                            let log = super::super::engine::driver(ctx).events.as_ref().unwrap();
                            let rows = log.canonical().unwrap();
                            let expected = [
                                1u32.to_le_bytes().as_slice(),
                                &10u64.to_le_bytes(),
                                b"/same/path",
                            ]
                            .concat();
                            assert!(rows.iter().any(|row| row.outcome
                                == super::super::events::CanonicalOutcome::Value {
                                    status: 0,
                                    payload: expected.clone()
                                }));
                            assert!(rows.iter().any(|row| row.outcome
                                == super::super::events::CanonicalOutcome::Received {
                                    status: crate::channels::DONE,
                                    payload: Some(expected.clone())
                                }));
                            *seen.lock().unwrap() = rows.to_vec();
                            crate::STATUS_OK
                        },
                    );
                    assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
                    let rows = output.lock().unwrap().clone();
                    if let Some(reference) = &reference {
                        assert_eq!(&rows, reference);
                    } else {
                        reference = Some(rows);
                    }
                }
            }
        }
        assert!(physical.windows(2).any(|words| words[0] != words[1]));
    }

    #[test]
    fn linked_inline_and_simd_values_skip_element_and_vector_padding() {
        use hirsel_layout::layout::{LayoutBuilder, Owners};
        use hirsel_lir::layout_types::{SizeSpec, TargetSpec};
        use hirsel_types::{Primitive, testing::Types};
        for target in TargetSpec::executed() {
            let mut types = Types::default();
            let boolean = types.p(Primitive::Bool);
            let number = types.p(Primitive::U64);
            let byte = types.p(Primitive::U8);
            let float = types.p(Primitive::F32);
            let unit = types.unit();
            let record = types.record(&[boolean, number]);
            let pair = types.inline(record, 2);
            let nested = types.inline(pair, 3);
            let empty = types.inline(record, 0);
            let units = types.inline(unit, 4);
            let bytes = types.simd(byte, 3);
            let bools = types.simd(boolean, 3);
            let floats = types.simd(float, 3);
            let owners = Owners::default();
            let mut builder = LayoutBuilder::new(&target, &owners);
            let roots = [nested, empty, units, bytes, bools, floats]
                .map(|ty| builder.layout(&types.store, ty).unwrap());
            let table = builder.finish();
            hirsel_lir::layout_validate::validate_table(&table, &target).unwrap();
            for rotation in [0, 1] {
                let count = table.layouts.len();
                let linked =
                    |id: hirsel_lir::LayoutId| ((id.0 as usize + rotation) % count + 2) as u32;
                let (_words, layouts) = native_layouts(&table, rotation);
                for (case, root) in roots.into_iter().enumerate() {
                    let program = unsafe {
                        Program::from_layouts(linked(root), count as u32 + 2, |id| {
                            &layouts[((id as usize - 2) + count - rotation) % count]
                        })
                    }
                    .unwrap();
                    let SizeSpec::Fixed(size) = table.layouts[root.0 as usize].size else {
                        panic!("fixed")
                    };
                    let mut image = MaybeUninit::<[u64; 12]>::uninit();
                    let at = image.as_mut_ptr().cast::<u8>();
                    let mut expected = Vec::new();
                    unsafe {
                        match case {
                            0 => {
                                assert_eq!(size, 96);
                                for i in 0..6usize {
                                    at.add(i * 16).write((i % 2) as u8);
                                    at.add(i * 16 + 8).cast::<u64>().write(i as u64 + 42);
                                    expected.push((i % 2) as u8);
                                    expected.extend_from_slice(&(i as u64 + 42).to_le_bytes());
                                }
                            }
                            1 | 2 => assert_eq!(size, 0),
                            3 | 4 => {
                                assert_eq!(size, 4, "three byte lanes have tail padding");
                                let lanes = if case == 3 { [7, 8, 9] } else { [1, 0, 1] };
                                for (i, lane) in lanes.into_iter().enumerate() {
                                    at.add(i).write(lane);
                                }
                                expected.extend_from_slice(&lanes);
                            }
                            5 => {
                                assert_eq!(size, 16, "three f32 lanes have tail padding");
                                for (i, bits) in
                                    [0x7fc0_0001u32, 0x8000_0000, 0].into_iter().enumerate()
                                {
                                    at.add(i * 4).cast::<u32>().write(bits);
                                    expected.extend_from_slice(&bits.to_le_bytes());
                                }
                            }
                            _ => unreachable!(),
                        }
                        assert_eq!(program.encode(at, |_, _| None).unwrap(), expected);
                        if case == 4 {
                            at.add(1).write(2);
                            assert_eq!(program.encode(at, |_, _| None), Err(Error::Value));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn native_inline_and_simd_message_events_own_initialized_values() {
        use crate::tasks::*;
        use hirsel_layout::layout::{LayoutBuilder, Owners};
        use hirsel_lir::layout_types::{SizeSpec, TagForm, TargetSpec};
        use hirsel_types::{Primitive, testing::Types};
        use std::sync::{Arc, Mutex};
        for vector in [false, true] {
            let mut types = Types::default();
            let boolean = types.p(Primitive::Bool);
            let number = types.p(Primitive::U64);
            let byte = types.p(Primitive::U8);
            let record = types.record(&[boolean, number]);
            let aggregate = if vector {
                types.simd(byte, 3)
            } else {
                types.inline(record, 2)
            };
            let option = types.variant(&[&[], &[aggregate]]);
            let owners = Owners::default();
            let mut builder = LayoutBuilder::new(&TargetSpec::x86_64_unknown_linux_gnu(), &owners);
            let root = builder.layout(&types.store, option).unwrap();
            let table = builder.finish();
            let option = &table.layouts[root.0 as usize];
            let tag = option.tag.as_ref().unwrap();
            let value_offset = tag.variants[1].fields[0].offset as usize;
            let tag_offset = tag.offset as usize;
            let explicit_tag = matches!(tag.form, TagForm::Ordinal);
            let SizeSpec::Fixed(extent) = option.size else {
                panic!("fixed inline option")
            };
            let mut reference = None;
            for rotation in [0, 1] {
                let count = table.layouts.len();
                let linked =
                    |id: hirsel_lir::LayoutId| ((id.0 as usize + rotation) % count + 2) as u32;
                let (_words, layouts) = native_layouts(&table, rotation);
                let pointers: Vec<_> = (0..count)
                    .map(|id| &layouts[(id + count - rotation) % count] as *const crate::Layout)
                    .collect();
                for width in [1, 2, 4, 8] {
                    for reorder in [
                        hirsel_run_config::Reorder::Off,
                        hirsel_run_config::Reorder::Auto,
                    ] {
                        let output = Arc::new(Mutex::new(Vec::new()));
                        let seen = output.clone();
                        let mut run = crate::host::BodyRun::new(width, true);
                        run.config.layouts = pointers.as_ptr();
                        run.config.layout_count = pointers.len() as u32;
                        unsafe { run.use_physical_dispatch() };
                        let descriptor = linked(root);
                        let outcome = run.run(
                            |ctx| unsafe {
                                let mut policy =
                                    crate::ordered::Policy::admit(&Default::default(), None);
                                policy.reorder = reorder;
                                super::super::engine::enable(ctx, policy);
                                super::super::events::enable(ctx);
                            },
                            move |ctx| unsafe {
                                let mut endpoints = [0u64; 2];
                                let none = vec![0u64; extent.div_ceil(8) as usize];
                                assert_eq!(
                                    hirsel_rt_v2_channel_open(
                                        ctx,
                                        std::ptr::null_mut(),
                                        endpoints.as_mut_ptr(),
                                        &0,
                                        none.as_ptr().cast(),
                                        descriptor
                                    ),
                                    crate::STATUS_OK
                                );
                                let mut image = none.clone();
                                if explicit_tag {
                                    image
                                        .as_mut_ptr()
                                        .cast::<u8>()
                                        .add(tag_offset)
                                        .cast::<u32>()
                                        .write(1);
                                }
                                let at = image.as_mut_ptr().cast::<u8>().add(value_offset);
                                let mut expected = 1u32.to_le_bytes().to_vec();
                                if vector {
                                    std::ptr::copy_nonoverlapping([7u8, 8, 9].as_ptr(), at, 3);
                                    expected.extend_from_slice(&[7, 8, 9]);
                                } else {
                                    for (i, value) in [42u64, 99].into_iter().enumerate() {
                                        at.add(i * 16).write((i % 2) as u8);
                                        at.add(i * 16 + 8).cast::<u64>().write(value);
                                        expected.push((i % 2) as u8);
                                        expected.extend_from_slice(&value.to_le_bytes());
                                    }
                                }
                                let mut status = u32::MAX;
                                assert_eq!(
                                    hirsel_rt_v2_channel_send(
                                        ctx,
                                        &mut status,
                                        &mut endpoints[0],
                                        image.as_mut_ptr().cast()
                                    ),
                                    crate::STATUS_OK
                                );
                                assert_eq!(status, crate::channels::DONE);
                                assert_eq!(image, none);
                                let mut received = none.clone();
                                assert_eq!(
                                    hirsel_rt_v2_channel_receive(
                                        ctx,
                                        0,
                                        0,
                                        &mut status,
                                        &mut endpoints[1],
                                        received.as_mut_ptr().cast()
                                    ),
                                    crate::STATUS_OK
                                );
                                assert_eq!(status, crate::channels::DONE);
                                // Destroy the caller's copy before inspecting immutable event values.
                                received.fill(u64::MAX);
                                assert_eq!(
                                    close_endpoint(ctx, endpoints[0], 0, 0),
                                    crate::STATUS_OK
                                );
                                assert_eq!(
                                    close_endpoint(ctx, endpoints[1], 0, 0),
                                    crate::STATUS_OK
                                );
                                let _role = role::Guard::new(ctx);
                                super::super::engine::at_head(ctx).unwrap();
                                let log =
                                    super::super::engine::driver(ctx).events.as_ref().unwrap();
                                let rows = log.canonical().unwrap();
                                assert!(rows.iter().any(|row| row.outcome
                                    == super::super::events::CanonicalOutcome::Value {
                                        status: 0,
                                        payload: expected.clone()
                                    }));
                                assert!(rows.iter().any(|row| row.outcome
                                    == super::super::events::CanonicalOutcome::Received {
                                        status: crate::channels::DONE,
                                        payload: Some(expected.clone())
                                    }));
                                *seen.lock().unwrap() = rows.to_vec();
                                crate::STATUS_OK
                            },
                        );
                        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
                        let rows = output.lock().unwrap().clone();
                        if let Some(reference) = &reference {
                            assert_eq!(&rows, reference);
                        } else {
                            reference = Some(rows);
                        }
                    }
                }
            }
        }
    }

    #[derive(Clone, Copy)]
    struct ContainerShape {
        root: hirsel_lir::LayoutId,
        message: hirsel_lir::LayoutId,
        payload: hirsel_lir::LayoutId,
        entries: hirsel_lir::LayoutId,
        metadata: hirsel_lir::LayoutId,
        extent: u64,
        entry_offset: usize,
        stride: usize,
        pair_offset: usize,
        key_offset: usize,
        value_offset: usize,
        tag: Option<(usize, hirsel_lir::layout_types::TagForm)>,
        sorted: bool,
        set: bool,
    }

    fn container_layout(
        target: &hirsel_lir::layout_types::TargetSpec,
        family: u32,
    ) -> (hirsel_lir::layout_types::LayoutTable, ContainerShape) {
        use hirsel_layout::layout::{Container, LayoutBuilder, Owners};
        use hirsel_lir::layout_types::{LayoutKind, SizeSpec};
        use hirsel_syntax::{ContentHash, DefPath};
        use hirsel_tir::RepresentationRecord;
        use hirsel_types::{
            BuiltinType, GenericArgument, GenericParameter, Primitive, testing::Types,
        };
        let (name, kind, container) = match family {
            0 => ("Map", BuiltinType::Map, Container::Map),
            1 => ("Set", BuiltinType::Set, Container::Set),
            2 => ("SortedMap", BuiltinType::SortedMap, Container::SortedMap),
            _ => unreachable!(),
        };
        let mut types = Types::default();
        let key = types.param(0);
        let value = if family == 1 {
            types.unit()
        } else {
            types.param(1)
        };
        let pair = types.tuple(&[key, value]);
        let entry = if family == 2 {
            pair
        } else {
            types.variant(&[&[], &[pair]])
        };
        let entries = types.list(entry);
        let word = types.p(Primitive::U64);
        let metadata = types.list(word);
        // Port the live owner shape in hirsel-rc/src/library/owners.rs:248-315.
        let private_shape = if family == 2 {
            types.record(&[entries])
        } else {
            types.record(&[entries, metadata, metadata, word])
        };
        let mut owners = Owners::default();
        owners.containers.insert(
            container,
            RepresentationRecord {
                definition: DefPath::core(name),
                representation_revision: ContentHash::of_bytes(name.as_bytes()),
                generics: (0..if family == 1 { 1 } else { 2 })
                    .map(|i| GenericParameter::Type {
                        name: format!("T{i}").into(),
                        bounds: Vec::new().into(),
                    })
                    .collect(),
                private_shape,
                layout_promises: Default::default(),
                resource: false,
                finalizer: None,
                unsafe_ffi_package: false,
            },
        );
        let boolean = types.p(Primitive::Bool);
        let value = types.record(&[boolean, word]);
        let arguments = if family == 1 {
            vec![GenericArgument::Type(word)]
        } else {
            vec![GenericArgument::Type(word), GenericArgument::Type(value)]
        };
        let root = types.builtin(kind, arguments);
        let message = types.variant(&[&[], &[root]]);
        let mut builder = LayoutBuilder::new(target, &owners);
        let root = builder.layout(&types.store, root).unwrap();
        let message = builder.layout(&types.store, message).unwrap();
        let metadata = builder.layout(&types.store, metadata).unwrap();
        let table = builder.finish();
        hirsel_lir::layout_validate::validate_table(&table, target).unwrap();
        let payload = |id: hirsel_lir::LayoutId| match table.layouts[id.0 as usize].kind {
            LayoutKind::Block { payload } => payload,
            _ => panic!("counted block"),
        };
        let payload_id = payload(root);
        let container = &table.layouts[payload_id.0 as usize];
        let entries = payload(container.fields[0].layout);
        let LayoutKind::Sequence { element } = table.layouts[entries.0 as usize].kind else {
            panic!("sequence")
        };
        let entry = &table.layouts[element.0 as usize];
        let SizeSpec::Fixed(stride) = entry.size else {
            panic!("fixed entry")
        };
        let (pair, pair_offset) = if family == 2 {
            (element, 0)
        } else {
            let pair = &entry.tag.as_ref().unwrap().variants[1].fields[0];
            (pair.layout, pair.offset as usize)
        };
        let pair = &table.layouts[pair.0 as usize];
        let SizeSpec::Fixed(extent) = container.size else {
            panic!("fixed container")
        };
        let shape = ContainerShape {
            root,
            message,
            payload: payload_id,
            entries,
            metadata: payload(metadata),
            extent,
            entry_offset: hirsel_lir::layout::sequence_elements_offset(entry).unwrap() as usize,
            stride: stride as usize,
            pair_offset,
            key_offset: pair.fields[0].offset as usize,
            value_offset: pair.fields[1].offset as usize,
            tag: entry
                .tag
                .as_ref()
                .map(|tag| (tag.offset as usize, tag.form)),
            sorted: family == 2,
            set: family == 1,
        };
        (table, shape)
    }

    /// Write only the logical live data and the tags of holes. Other bytes may
    /// be uninitialized, including pair payloads of None slots.
    unsafe fn fill_container_entries(
        owner: *mut crate::Header,
        shape: ContainerShape,
        holes: bool,
    ) {
        use hirsel_lir::layout_types::TagForm;
        unsafe {
            let count = if holes && !shape.sorted { 3 } else { 2 };
            let at = crate::alloc::payload(owner);
            at.cast::<u64>().write(count);
            at.add(8).cast::<u64>().write(
                ((*owner).capacity() as usize - shape.entry_offset) as u64 / shape.stride as u64,
            );
            for i in 0..count as usize {
                let slot = at.add(shape.entry_offset + i * shape.stride);
                let hole = holes && !shape.sorted && i == 1;
                if let Some((offset, form)) = shape.tag {
                    match form {
                        TagForm::Ordinal => slot.add(offset).cast::<u32>().write(u32::from(!hole)),
                        TagForm::Niche { .. } if hole => slot.add(offset).cast::<u64>().write(0),
                        TagForm::Niche { .. } => (),
                        _ => panic!("optional entry tag"),
                    }
                }
                if !hole {
                    let logical = usize::from(i != 0);
                    let pair = slot.add(shape.pair_offset);
                    pair.add(shape.key_offset)
                        .cast::<u64>()
                        .write(if (logical == 0) == shape.sorted { 3 } else { 9 });
                    if !shape.set {
                        pair.add(shape.value_offset).write(logical as u8);
                        pair.add(shape.value_offset + 8)
                            .cast::<u64>()
                            .write(if logical == 0 { 42 } else { 99 });
                    }
                }
            }
        }
    }

    fn expected_container(shape: ContainerShape) -> Vec<u8> {
        let mut bytes = 2u64.to_le_bytes().to_vec();
        let keys = if shape.sorted { [3u64, 9] } else { [9u64, 3] };
        for (i, key) in keys.into_iter().enumerate() {
            bytes.extend_from_slice(&key.to_le_bytes());
            if !shape.set {
                bytes.push(i as u8);
                bytes.extend_from_slice(&(if i == 0 { 42u64 } else { 99 }).to_le_bytes());
            }
        }
        bytes
    }

    #[test]
    fn linked_container_values_skip_holes_hashes_indices_and_padding() {
        use hirsel_lir::layout_types::TargetSpec;
        for target in TargetSpec::executed() {
            for family in 0..3 {
                let (table, shape) = container_layout(&target, family);
                let expected = expected_container(shape);
                for rotation in [0, 1] {
                    let count = table.layouts.len();
                    let linked =
                        |id: hirsel_lir::LayoutId| ((id.0 as usize + rotation) % count + 2) as u32;
                    let (_words, layouts) = native_layouts(&table, rotation);
                    let mut pointers = vec![
                        &crate::text::hirsel_rt_v2_bytes_descriptor.0 as *const crate::Layout,
                        &crate::io::hirsel_rt_v2_file_walk_descriptor.0,
                    ];
                    pointers.extend((0..count).map(|id| {
                        &layouts[(id + count - rotation) % count] as *const crate::Layout
                    }));
                    let program = unsafe {
                        Program::from_layouts(linked(shape.root), pointers.len() as u32, |id| {
                            pointers[id as usize]
                        })
                    }
                    .unwrap();
                    let mut heap = crate::alloc::Heap::new(pointers.into_boxed_slice(), false, 0);
                    let mut view = crate::alloc::View::new();
                    unsafe { heap.bind(&mut view) };
                    for holes in [false, true] {
                        unsafe {
                            let (root, _) = heap.allocate(linked(shape.payload), shape.extent, 8);
                            let (entries, _) = heap.allocate(
                                linked(shape.entries),
                                (shape.entry_offset + 5 * shape.stride) as u64,
                                8,
                            );
                            fill_container_entries(entries, shape, holes);
                            let root_at = crate::alloc::payload(root);
                            root_at.cast::<*mut crate::Header>().write(entries);
                            let mut metadata = Vec::new();
                            if !shape.sorted {
                                // Private metadata words differ across representations.
                                for (field, seed) in [
                                    (1, if holes { 7 } else { 91 }),
                                    (2, if holes { 13 } else { 67 }),
                                ] {
                                    let (block, _) = heap.allocate(linked(shape.metadata), 48, 8);
                                    let at = crate::alloc::payload(block);
                                    at.cast::<u64>().write(4);
                                    at.add(8).cast::<u64>().write(4);
                                    for i in 0..4 {
                                        at.add(16 + i * 8).cast::<u64>().write(seed + i as u64);
                                    }
                                    root_at
                                        .add(field * 8)
                                        .cast::<*mut crate::Header>()
                                        .write(block);
                                    metadata.push(block);
                                }
                                root_at.add(24).cast::<u64>().write(2);
                            }
                            let observed = program
                                .encode((&root as *const *mut crate::Header).cast(), |_, _| None)
                                .unwrap();
                            assert_eq!(observed, expected);
                            for block in metadata {
                                heap.free(block);
                            }
                            heap.free(entries);
                            heap.free(root);
                            assert_eq!(
                                observed, expected,
                                "owned bytes outlive every backing allocation"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn container_values_reject_inconsistent_live_counts_tags_and_bounds() {
        use hirsel_lir::layout_types::{TagForm, TargetSpec};
        let (table, shape) = container_layout(&TargetSpec::x86_64_unknown_linux_gnu(), 0);
        let count = table.layouts.len();
        let linked = |id: hirsel_lir::LayoutId| id.0 + 2;
        let (_words, layouts) = native_layouts(&table, 0);
        let program = unsafe {
            Program::from_layouts(linked(shape.root), count as u32 + 2, |id| {
                &layouts[id as usize - 2]
            })
        }
        .unwrap();
        let mut pointers = vec![&layouts[0] as *const crate::Layout; 2];
        pointers.extend(layouts.iter().map(|layout| layout as *const crate::Layout));
        let mut heap = crate::alloc::Heap::new(pointers.into_boxed_slice(), false, 0);
        let mut view = crate::alloc::View::new();
        unsafe {
            heap.bind(&mut view);
            let (root, _) = heap.allocate(linked(shape.payload), shape.extent, 8);
            let (entries, _) = heap.allocate(
                linked(shape.entries),
                (shape.entry_offset + 5 * shape.stride) as u64,
                8,
            );
            let at = crate::alloc::payload(root);
            at.cast::<*mut crate::Header>().write(entries);
            fill_container_entries(entries, shape, true);
            let encode =
                || program.encode((&root as *const *mut crate::Header).cast(), |_, _| None);
            for live in [0u64, 1, 3, u64::MAX] {
                at.add(24).cast::<u64>().write(live);
                assert_eq!(encode(), Err(Error::Value));
            }
            at.add(24).cast::<u64>().write(2);
            let Some((offset, TagForm::Ordinal)) = shape.tag else {
                panic!("integer optional entry has ordinal tag")
            };
            crate::alloc::payload(entries)
                .add(shape.entry_offset + offset)
                .cast::<u32>()
                .write(7);
            assert_eq!(encode(), Err(Error::Value));
            fill_container_entries(entries, shape, true);
            crate::alloc::payload(entries)
                .cast::<u64>()
                .write(WORK_MAX as u64 + 1);
            assert_eq!(encode(), Err(Error::Limit));
            fill_container_entries(entries, shape, true);
            crate::alloc::payload(entries)
                .add(8)
                .cast::<u64>()
                .write(u64::MAX);
            assert_eq!(encode(), Err(Error::Value));
            fill_container_entries(entries, shape, true);
            let descriptor = (*entries).descriptor;
            (*entries).descriptor = 0;
            assert_eq!(encode(), Err(Error::Value));
            (*entries).descriptor = descriptor;
            at.cast::<*mut crate::Header>().write(std::ptr::null_mut());
            assert_eq!(encode(), Err(Error::Value));
            heap.free(entries);
            heap.free(root);
        }
    }

    #[test]
    fn native_container_message_events_own_live_values_before_backing_release() {
        use crate::tasks::*;
        use hirsel_lir::layout_types::{SizeSpec, TagForm, TargetSpec};
        use std::sync::{Arc, Mutex};
        for family in 0..3 {
            let (table, shape) = container_layout(&TargetSpec::x86_64_unknown_linux_gnu(), family);
            let option = &table.layouts[shape.message.0 as usize];
            let tag = option.tag.as_ref().unwrap();
            let value_offset = tag.variants[1].fields[0].offset as usize;
            let tag_offset = tag.offset as usize;
            let explicit_tag = matches!(tag.form, TagForm::Ordinal);
            let SizeSpec::Fixed(extent) = option.size else {
                panic!("fixed map option")
            };
            let mut reference = None;
            for rotation in [0, 1] {
                let count = table.layouts.len();
                let linked =
                    |id: hirsel_lir::LayoutId| ((id.0 as usize + rotation) % count + 2) as u32;
                let (_words, layouts) = native_layouts(&table, rotation);
                let pointers: Vec<_> = (0..count)
                    .map(|id| &layouts[(id + count - rotation) % count] as *const crate::Layout)
                    .collect();
                for width in [1, 2, 4, 8] {
                    for reorder in [
                        hirsel_run_config::Reorder::Off,
                        hirsel_run_config::Reorder::Auto,
                    ] {
                        let output = Arc::new(Mutex::new(Vec::new()));
                        let seen = output.clone();
                        let mut run = crate::host::BodyRun::new(width, true);
                        run.config.layouts = pointers.as_ptr();
                        run.config.layout_count = pointers.len() as u32;
                        unsafe { run.use_physical_dispatch() };
                        let descriptor = linked(shape.message);
                        let root_index = linked(shape.payload);
                        let entry_index = linked(shape.entries);
                        let metadata_index = linked(shape.metadata);
                        let outcome = run.run(
                            |ctx| unsafe {
                                let mut policy =
                                    crate::ordered::Policy::admit(&Default::default(), None);
                                policy.reorder = reorder;
                                super::super::engine::enable(ctx, policy);
                                super::super::events::enable(ctx);
                            },
                            move |ctx| unsafe {
                                let mut endpoints = [0u64; 2];
                                let none = vec![0u64; extent.div_ceil(8) as usize];
                                assert_eq!(
                                    hirsel_rt_v2_channel_open(
                                        ctx,
                                        std::ptr::null_mut(),
                                        endpoints.as_mut_ptr(),
                                        &0,
                                        none.as_ptr().cast(),
                                        descriptor
                                    ),
                                    crate::STATUS_OK
                                );
                                let root = crate::alloc::hirsel_rt_v2_alloc(
                                    ctx,
                                    root_index,
                                    shape.extent,
                                    8,
                                    crate::alloc::DOMAIN_HEAP,
                                    0,
                                );
                                let entries = crate::alloc::hirsel_rt_v2_alloc(
                                    ctx,
                                    entry_index,
                                    (shape.entry_offset + 5 * shape.stride) as u64,
                                    8,
                                    crate::alloc::DOMAIN_HEAP,
                                    0,
                                );
                                fill_container_entries(entries, shape, width > 1);
                                let at = crate::alloc::payload(root);
                                at.cast::<*mut crate::Header>().write(entries);
                                let mut metadata = Vec::new();
                                if !shape.sorted {
                                    for field in 1..3 {
                                        let block = crate::alloc::allocate_sequence(
                                            ctx,
                                            metadata_index,
                                            4,
                                            8,
                                            8,
                                        );
                                        for i in 0..4 {
                                            crate::alloc::payload(block)
                                                .add(16 + i * 8)
                                                .cast::<u64>()
                                                .write(width as u64 * 17 + i as u64 + field as u64);
                                        }
                                        at.add(field * 8).cast::<*mut crate::Header>().write(block);
                                        metadata.push(block);
                                    }
                                    at.add(24).cast::<u64>().write(2);
                                }
                                let mut image = none.clone();
                                if explicit_tag {
                                    image
                                        .as_mut_ptr()
                                        .cast::<u8>()
                                        .add(tag_offset)
                                        .cast::<u32>()
                                        .write(1);
                                }
                                image
                                    .as_mut_ptr()
                                    .cast::<u8>()
                                    .add(value_offset)
                                    .cast::<*mut crate::Header>()
                                    .write(root);
                                let mut status = u32::MAX;
                                assert_eq!(
                                    hirsel_rt_v2_channel_send(
                                        ctx,
                                        &mut status,
                                        &mut endpoints[0],
                                        image.as_mut_ptr().cast()
                                    ),
                                    crate::STATUS_OK
                                );
                                assert_eq!(status, crate::channels::DONE);
                                assert_eq!(image, none);
                                let mut received = none.clone();
                                assert_eq!(
                                    hirsel_rt_v2_channel_receive(
                                        ctx,
                                        0,
                                        0,
                                        &mut status,
                                        &mut endpoints[1],
                                        received.as_mut_ptr().cast()
                                    ),
                                    crate::STATUS_OK
                                );
                                assert_eq!(status, crate::channels::DONE);
                                assert_eq!(
                                    received
                                        .as_ptr()
                                        .cast::<u8>()
                                        .add(value_offset)
                                        .cast::<*mut crate::Header>()
                                        .read(),
                                    root
                                );
                                // The event must already own the original logical entry bytes.
                                crate::alloc::payload(entries)
                                    .add(shape.entry_offset + shape.pair_offset + shape.key_offset)
                                    .cast::<u64>()
                                    .write(1000);
                                received.fill(0);
                                for block in metadata {
                                    assert_eq!(
                                        crate::release::hirsel_rt_v2_release(
                                            ctx,
                                            block,
                                            metadata_index,
                                            std::ptr::null(),
                                            0,
                                            0
                                        ),
                                        crate::STATUS_OK
                                    );
                                }
                                assert_eq!(
                                    crate::release::hirsel_rt_v2_release(
                                        ctx,
                                        entries,
                                        entry_index,
                                        std::ptr::null(),
                                        0,
                                        0
                                    ),
                                    crate::STATUS_OK
                                );
                                assert_eq!(
                                    crate::release::hirsel_rt_v2_release(
                                        ctx,
                                        root,
                                        root_index,
                                        std::ptr::null(),
                                        0,
                                        0
                                    ),
                                    crate::STATUS_OK
                                );
                                assert_eq!(
                                    close_endpoint(ctx, endpoints[0], 0, 0),
                                    crate::STATUS_OK
                                );
                                assert_eq!(
                                    close_endpoint(ctx, endpoints[1], 0, 0),
                                    crate::STATUS_OK
                                );
                                let _role = role::Guard::new(ctx);
                                super::super::engine::at_head(ctx).unwrap();
                                let log =
                                    super::super::engine::driver(ctx).events.as_ref().unwrap();
                                let rows = log.canonical().unwrap();
                                let expected =
                                    [1u32.to_le_bytes().as_slice(), &expected_container(shape)]
                                        .concat();
                                assert!(rows.iter().any(|row| row.outcome
                                    == super::super::events::CanonicalOutcome::Value {
                                        status: 0,
                                        payload: expected.clone()
                                    }));
                                assert!(rows.iter().any(|row| row.outcome
                                    == super::super::events::CanonicalOutcome::Received {
                                        status: crate::channels::DONE,
                                        payload: Some(expected.clone())
                                    }));
                                *seen.lock().unwrap() = rows.to_vec();
                                crate::STATUS_OK
                            },
                        );
                        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
                        let rows = output.lock().unwrap().clone();
                        if let Some(reference) = &reference {
                            assert_eq!(&rows, reference);
                        } else {
                            reference = Some(rows);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn indirect_values_reject_null_mismatched_descriptors_and_bounded_cycles() {
        let program = Program::new(
            vec![
                Node::Indirect {
                    node: 1,
                    descriptor: 17,
                },
                Node::Bits(8),
            ],
            0,
        )
        .unwrap();
        assert_eq!(
            unsafe { program.encode((&0u64 as *const u64).cast(), |_, _| None) },
            Err(Error::Value)
        );
        let wrong = [crate::Header {
            count: 1,
            descriptor: 18,
        }];
        let pointer = wrong.as_ptr();
        assert_eq!(
            unsafe {
                program.encode((&pointer as *const *const crate::Header).cast(), |_, _| {
                    None
                })
            },
            Err(Error::Value)
        );
        let cycle = Program::new(
            vec![Node::Indirect {
                node: 0,
                descriptor: 17,
            }],
            0,
        )
        .unwrap();
        let mut block = [0u64; 2];
        let pointer = block.as_mut_ptr().cast::<crate::Header>();
        unsafe {
            pointer.write(crate::Header {
                count: 1,
                descriptor: 17,
            });
            pointer.add(1).cast::<*mut crate::Header>().write(pointer);
        }
        assert_eq!(
            unsafe { cycle.encode((&pointer as *const *mut crate::Header).cast(), |_, _| None) },
            Err(Error::Limit)
        );
        assert!(
            Program::new(
                vec![Node::Indirect {
                    node: 1,
                    descriptor: 17
                }],
                0
            )
            .is_err()
        );
    }

    #[test]
    fn active_fields_skip_uninitialized_record_padding_and_inactive_arms() {
        let program = Program::new(
            vec![
                Node::Variant {
                    extent: 24,
                    tag: Tag::Ordinal { offset: 0 },
                    arms: vec![vec![], vec![Field { offset: 8, node: 1 }]],
                },
                Node::Fields {
                    extent: 16,
                    fields: vec![Field { offset: 0, node: 2 }, Field { offset: 8, node: 3 }],
                },
                Node::Bool,
                Node::Bits(8),
            ],
            0,
        )
        .unwrap();
        for active in [false, true] {
            let mut value = MaybeUninit::<[u64; 3]>::uninit();
            let at = value.as_mut_ptr().cast::<u8>();
            unsafe {
                at.cast::<u32>().write(u32::from(active));
                if active {
                    at.add(8).write(1);
                    at.add(16).cast::<u64>().write(42);
                }
                let encoded = program.encode(at, |_, _| panic!("no handles")).unwrap();
                assert_eq!(
                    encoded,
                    if active {
                        [1u32.to_le_bytes().as_slice(), &[1], &42u64.to_le_bytes()].concat()
                    } else {
                        0u32.to_le_bytes().to_vec()
                    }
                );
            }
        }
    }

    #[test]
    fn handle_words_do_not_escape_and_inactive_niche_payloads_are_not_read() {
        let program = Program::new(
            vec![
                Node::Variant {
                    extent: 16,
                    tag: Tag::NonNull {
                        offset: 0,
                        nullary: 0,
                    },
                    arms: vec![
                        vec![],
                        vec![Field { offset: 0, node: 1 }, Field { offset: 8, node: 2 }],
                    ],
                },
                Node::Handle(Kind::Sender),
                Node::Bits(8),
            ],
            0,
        )
        .unwrap();
        let mut reference = None;
        for word in [7u64, 999] {
            let value = [word, 42];
            let bytes = unsafe {
                program.encode(value.as_ptr().cast(), |kind, seen| {
                    assert_eq!((kind, seen), (Kind::Sender, word));
                    Some(ObjectId {
                        creator: vec![1, 2],
                        ordinal: 5,
                    })
                })
            }
            .unwrap();
            if let Some(reference) = &reference {
                assert_eq!(&bytes, reference);
            } else {
                reference = Some(bytes);
            }
        }
        let mut none = MaybeUninit::<[u64; 2]>::uninit();
        unsafe {
            none.as_mut_ptr().cast::<u64>().write(0);
            assert_eq!(
                program.encode(none.as_ptr().cast(), |_, _| panic!("inactive handle")),
                Ok(0u32.to_le_bytes().to_vec())
            );
        }
        assert_eq!(
            unsafe { program.encode([7u64, 42].as_ptr().cast(), |_, _| None) },
            Err(Error::UnresolvedHandle)
        );
    }

    #[test]
    fn invalid_metadata_values_and_recursive_work_refuse_the_entire_observation() {
        assert!(Program::new(vec![Node::Bits(3)], 0).is_err());
        assert!(
            Program::new(
                vec![
                    Node::Fields {
                        extent: 8,
                        fields: vec![Field {
                            offset: usize::MAX,
                            node: 1
                        }]
                    },
                    Node::Bits(8)
                ],
                0
            )
            .is_err()
        );
        let unsupported = Program::new(vec![Node::Unsupported { extent: 8 }], 0).unwrap();
        assert_eq!(
            unsafe {
                unsupported.encode(MaybeUninit::<u64>::uninit().as_ptr().cast(), |_, _| None)
            },
            Err(Error::Unsupported)
        );
        let recursive = Program::new(
            vec![Node::Fields {
                extent: 0,
                fields: vec![Field { offset: 0, node: 0 }],
            }],
            0,
        )
        .unwrap();
        assert_eq!(
            unsafe { recursive.encode(std::ptr::NonNull::<u8>::dangling().as_ptr(), |_, _| None) },
            Err(Error::Limit)
        );
        let boolean = Program::new(vec![Node::Bool], 0).unwrap();
        assert_eq!(
            unsafe { boolean.encode(&2, |_, _| None) },
            Err(Error::Value)
        );
    }
}
