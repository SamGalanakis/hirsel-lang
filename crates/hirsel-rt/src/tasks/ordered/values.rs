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
    Indirect {
        node: usize,
        descriptor: u32,
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
            Self::Handle(_) | Self::Indirect { .. } => 8,
            Self::Fields { extent, .. }
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

#[derive(Clone, Debug)]
pub(super) struct Program {
    nodes: Vec<Node>,
    root: usize,
}

impl Program {
    /// Decode compiler-emitted initialized-field metadata already linked in
    /// the native holder-scan stream. No compiler table index enters values.
    /// # Safety
    /// Matching immutable descriptors and programs live for this run.
    pub(super) unsafe fn linked(ctx: *mut crate::TaskCtx, root: u32) -> Result<Self, Error> {
        unsafe {
            Self::from_layouts(root, (*ctx).heap_ref().layout_count(), |id| {
                (*ctx).descriptor(id)
            })
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
                VALUE_HANDLE if extent == 8 => {
                    Node::Handle(Kind::from_code(header[3]).ok_or(Error::Program)?)
                }
                VALUE_INDIRECT if extent == 8 => {
                    let descriptor = u32::try_from(header[3]).map_err(|_| Error::Program)?;
                    let node = field(&[VALUE_FIELD, 0, header[3], u64::MAX, 0, 0, 0, 0])?.node;
                    Node::Indirect { node, descriptor }
                }
                VALUE_UNKNOWN => Node::Unsupported { extent },
                _ => return Err(Error::Program),
            };
            nodes.push(node);
        }
        Self::new(nodes, 0)
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
        Ok(Self { nodes, root })
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
        let mut work = vec![(self.root, value)];
        let mut out = Vec::new();
        let mut visited = 0;
        while let Some((index, at)) = work.pop() {
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
                Node::Indirect { node, descriptor } => {
                    let block = unsafe { at.cast::<*const crate::Header>().read_unaligned() };
                    if block.is_null() || unsafe { (*block).index() } != *descriptor {
                        return Err(Error::Value);
                    }
                    // The typed live carrier owns this counted payload; only
                    // initialized fields of its linked layout are traversed.
                    work.push((*node, unsafe { block.add(1).cast() }));
                    continue;
                }
                Node::Fields { fields, .. } => fields,
                Node::Variant { tag, arms, .. } => {
                    let ordinal = unsafe {
                        match tag {
                            Tag::Ordinal { offset } => {
                                at.add(*offset).cast::<u32>().read_unaligned()
                            }
                            Tag::NonNull { offset, nullary } => {
                                if at.add(*offset).cast::<u64>().read_unaligned() == 0 {
                                    *nullary
                                } else {
                                    1 - *nullary
                                }
                            }
                        }
                    };
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
                work.push((field.node, unsafe { at.add(field.offset) }));
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
        use hirsel_lir::layout_types::SizeSpec;
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
                size: match record.size {
                    SizeSpec::Fixed(size) => size,
                    _ => panic!("fixed graph fixture"),
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
    fn native_boxed_message_events_own_linked_payloads_before_caller_mutation() {
        use crate::tasks::*;
        use hirsel_layout::layout::{LayoutBuilder, Owners};
        use hirsel_lir::layout_types::{LayoutKind, SizeSpec, TagForm, TargetSpec};
        use hirsel_types::{Primitive, testing::Types};
        use std::sync::{Arc, Mutex};
        let mut types = Types::default();
        let boolean = types.p(Primitive::Bool);
        let number = types.p(Primitive::U64);
        let record = types.record(&[boolean, number]);
        let boxed = types.boxed(record);
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
        let LayoutKind::Box { payload } =
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
                                16,
                                8,
                                crate::alloc::DOMAIN_HEAP,
                                0,
                            );
                            crate::alloc::payload(block).write(1);
                            crate::alloc::payload(block).add(8).cast::<u64>().write(42);
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
                            crate::alloc::payload(block).add(8).cast::<u64>().write(99);
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
                            assert_eq!(close_endpoint(ctx, endpoints[0], 0, 0), crate::STATUS_OK);
                            assert_eq!(close_endpoint(ctx, endpoints[1], 0, 0), crate::STATUS_OK);
                            let _role = role::Guard::new(ctx);
                            super::super::engine::at_head(ctx).unwrap();
                            let log = super::super::engine::driver(ctx).events.as_ref().unwrap();
                            let rows = log.canonical().unwrap();
                            let expected =
                                [1u32.to_le_bytes().as_slice(), &[1], &42u64.to_le_bytes()]
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
